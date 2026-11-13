//! `lectover say`: speak a few lines with the render pipeline, to try pronunciations by ear.
//!
//! Each line goes through the lexicon, the TTS with its ASR check and retries, and the same
//! trim and levelling as a rendered chunk; it is written to `<out>/<voice>-<n>.wav` and,
//! unless `--no-play`, played right away with the system's command-line player.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::audio::{level_segments, trim_and_level};
use crate::cli::SayArgs;
use crate::design::write_wav16;
use crate::ffi::{asr_library_device, resolve_device, QwenAligner, QwenAsr, QwenTts, Synth};
use crate::models::ModelFiles;
use crate::plan::max_tokens_for;
use crate::render::run::voice_ref;
use crate::render::{synthesize_chunks, with_carrier, CarrierEnding, ChunkRequest, TakeCache};
use crate::settings::Settings;
use crate::text::normalize::{load_lexicon, spoken_groups, Lexicon};
use crate::voices::load_registry;

/// The command that plays a WAV file here: `afplay` on macOS, else `aplay`.
fn player() -> &'static str {
    if cfg!(target_os = "macos") {
        "afplay"
    } else {
        "aplay"
    }
}

/// Silence played before each take. An output that has to wake up first (Bluetooth
/// headphones) swallows the start of playback: on AirPods the 0.1 s lead and the "D" of
/// "Die" were lost, while the same take with 1 s of silence in front played complete.
const PLAYBACK_LEAD_SECS: f64 = 1.0;

/// `take` with [`PLAYBACK_LEAD_SECS`] of silence in front, for playback only.
fn for_playback(take: &[f32], sr: u32) -> Vec<f32> {
    let mut out = vec![0.0; (sr as f64 * PLAYBACK_LEAD_SECS) as usize];
    out.extend_from_slice(take);
    out
}

/// Play `take` (the content of `path`) from a temporary copy that leads with silence.
fn play(take: &[f32], sr: u32, path: &Path) -> Result<()> {
    let tmp = tempfile::Builder::new()
        .prefix("lectover-say-")
        .suffix(".wav")
        .tempfile()?;
    write_wav16(tmp.path(), &for_playback(take, sr), sr)?;
    let status = Command::new(player())
        .arg(tmp.path())
        .status()
        .with_context(|| format!("running {} (use --no-play to skip playback)", player()))?;
    if !status.success() {
        bail!("{} failed on {}", player(), path.display());
    }
    Ok(())
}

/// The lexicon `say` applies: `--lexicon`, else that of `--site`, else that of the only
/// configured site; none with several sites (or none) and neither flag.
pub fn lexicon_path(
    flag: Option<&Path>,
    site: Option<&str>,
    settings: &Settings,
) -> Result<Option<PathBuf>> {
    if let Some(p) = flag {
        return Ok(Some(p.to_path_buf()));
    }
    if let Some(domain) = site {
        return Ok(Some(settings.site(domain)?.lexicon));
    }
    match settings.config.as_ref().map(|l| &l.config.site) {
        Some(sites) if sites.len() == 1 => {
            let domain = sites.keys().next().expect("one site");
            Ok(Some(settings.site(domain)?.lexicon))
        }
        _ => Ok(None),
    }
}

pub fn run(args: &SayArgs, settings: &Settings) -> Result<()> {
    if args.lines.is_empty() {
        bail!("nothing to say: give one or more lines of text");
    }
    let files = ModelFiles::check(&settings.models)?;
    let lexicon = match lexicon_path(args.lexicon.as_deref(), args.site.as_deref(), settings)? {
        Some(p) => load_lexicon(&p).with_context(|| format!("reading {}", p.display()))?,
        None => Lexicon::new(),
    };
    let voices_dir = settings.voices.as_path();
    let out_dir = args.out.as_ref().unwrap_or(&settings.say_out);
    let registry = load_registry(voices_dir)?;
    let voices: Vec<String> = if args.voices.is_empty() {
        registry.keys().take(1).cloned().collect()
    } else {
        args.voices.clone()
    };
    if let Some(v) = voices.iter().find(|v| !registry.contains_key(*v)) {
        let known: Vec<&str> = registry.keys().map(String::as_str).collect();
        bail!("unknown voice {v}; known: {}", known.join(", "));
    }
    // spoken form after the lexicon, exactly as render sends it
    let texts: Vec<String> = args
        .lines
        .iter()
        .map(|l| {
            spoken_groups(l, &lexicon)
                .into_iter()
                .map(|g| g.spoken)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    let budgets: Vec<usize> = texts
        .iter()
        .map(|t| max_tokens_for(&with_carrier(t)))
        .collect();
    fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    let device = resolve_device(args.device.or(settings.device))?;
    let (wav, text) = voice_ref(voices_dir, &voices[0])?;
    let mut tts = QwenTts::load(&files.talker, &files.codec, &wav, &text, device, 6)?;
    tts.set_subtalker_temperature(0.6);
    tts.set_non_streaming(true);
    let mut asr = QwenAsr::load(&files.asr, asr_library_device())?;
    let mut aligner = QwenAligner::load(&files.aligner, asr_library_device())?;
    let result = (|| -> Result<()> {
        for voice in &voices {
            let (wav, text) = voice_ref(voices_dir, voice)?;
            tts.set_voice(&wav, &text)?;
            let sr = tts.sample_rate();
            let mut cached = TakeCache::new(&mut tts, &settings.takes);
            let mut ending = CarrierEnding::new(&mut aligner);
            let takes = synthesize_chunks(
                &mut cached,
                &mut asr,
                &mut ending,
                &ChunkRequest {
                    texts: &texts,
                    budgets: &budgets,
                    seed: args.seed,
                    temperature: 0.9,
                    lex: &lexicon,
                },
            )?;
            for (n, ((take, line), spoken)) in takes.iter().zip(&args.lines).zip(&texts).enumerate()
            {
                let path = out_dir.join(format!("{voice}-{}.wav", n + 1));
                let audio = level_segments(&trim_and_level(take, sr), sr);
                write_wav16(&path, &audio, sr)?;
                println!("{voice} #{}: {line}  ->  {}", n + 1, path.display());
                if spoken != line {
                    println!("    spoken as: {spoken}");
                }
                if !args.no_play {
                    play(&audio, sr, &path)?;
                }
            }
        }
        Ok(())
    })();
    // every model handle must be gone before main returns (Metal aborts otherwise)
    drop(aligner);
    drop(asr);
    drop(tts);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_copy_leads_with_silence_and_keeps_the_take() {
        let take = vec![0.5f32; 100];
        let out = for_playback(&take, 1000);
        let lead = (1000.0 * PLAYBACK_LEAD_SECS) as usize;
        assert_eq!(out.len(), lead + take.len());
        assert!(out[..lead].iter().all(|&v| v == 0.0));
        assert_eq!(&out[lead..], &take[..]);
    }
}
