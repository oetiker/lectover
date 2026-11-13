//! `lectover voice design`: a new reference voice from a description (port of the flow of
//! `design_voices.py`, design plus adopt, on qwentts.cpp's VoiceDesign talker).
//!
//! The VoiceDesign model speaks `REF_TEXT` in the described voice; the take is trimmed and
//! levelled like a design_voices.py candidate, written as `<voices>/<id>.wav` (16-bit mono)
//! with `REF_TEXT` as its `.txt` transcript, and registered in `voices.json`. An existing id
//! needs `force`; its old clip and transcript are kept with a timestamp, as `--adopt` did.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

use crate::audio::trim_and_level;
use crate::cli::DesignArgs;
use crate::ffi::{backend_banner, resolve_device, Device, QwenVoiceDesign};
use crate::models::DesignFiles;
use crate::settings::Settings;
use crate::voices::{add_voice, is_valid_id};

/// What every new reference clip says; its tone carries over to the renders. Neutral, so a
/// voice suits any site.
pub const REF_TEXT: &str = "Guten Abend. In dieser Kolumne geht es um die Zukunft unserer öffentlichen \
                            Betriebe, und um die Frage, was eine Fusion für alle Beteiligten bedeutet.";
/// Frame budget of the reference take (design_voices.py's `max_tokens=400`, 32 s).
pub const MAX_TOKENS: usize = 400;

/// Speaks a text in a voice described in words.
pub trait VoiceDesigner {
    fn sample_rate(&self) -> u32;
    fn design(
        &mut self,
        text: &str,
        instruct: &str,
        max_tokens: usize,
        seed: u64,
        temperature: f32,
    ) -> Result<Vec<f32>>;
}

impl VoiceDesigner for QwenVoiceDesign {
    fn sample_rate(&self) -> u32 {
        QwenVoiceDesign::sample_rate(self)
    }

    fn design(
        &mut self,
        text: &str,
        instruct: &str,
        max_tokens: usize,
        seed: u64,
        temperature: f32,
    ) -> Result<Vec<f32>> {
        QwenVoiceDesign::design(self, text, instruct, max_tokens, seed, temperature)
    }
}

/// Load the VoiceDesign talker. The caller drops the box before the process exits (a ggml
/// Metal device still holding buffers at exit aborts).
pub fn load_designer(files: &DesignFiles, device: Device) -> Result<Box<dyn VoiceDesigner>> {
    Ok(Box::new(QwenVoiceDesign::load(
        &files.talker,
        &files.codec,
        device,
    )?))
}

#[derive(Debug, Clone, PartialEq)]
pub struct DesignRequest {
    pub id: String,
    pub label: String,
    /// Voice description for the VoiceDesign model.
    pub instruct: String,
    pub seed: u64,
    pub temperature: f32,
    /// Replace an existing voice (its files are kept with a timestamp).
    pub force: bool,
}

pub(crate) fn clip_paths(voices_dir: &Path, id: &str) -> (PathBuf, PathBuf) {
    (
        voices_dir.join(format!("{id}.wav")),
        voices_dir.join(format!("{id}.txt")),
    )
}

/// Refuse an invalid id, and an existing one (registered, or with a clip or transcript on
/// disk) unless `force`. Needs no model: `run` calls it before loading one.
pub fn check_target(voices_dir: &Path, id: &str, force: bool) -> Result<()> {
    if !is_valid_id(id) || id.ends_with('\n') {
        bail!(
            "invalid voice id {id:?} (allowed: {})",
            crate::voices::VOICE_ID_RE
        );
    }
    if force {
        return Ok(());
    }
    let (wav, txt) = clip_paths(voices_dir, id);
    // Read the ids directly: load_registry would fail on another voice's missing file.
    let registry = voices_dir.join("voices.json");
    let registered = match fs::read_to_string(&registry) {
        Ok(text) => serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text)
            .with_context(|| format!("parsing {}", registry.display()))?
            .contains_key(id),
        Err(_) => false,
    };
    if registered || wav.exists() || txt.exists() {
        bail!(
            "voice {id:?} already exists in {}; pass --force to replace it (the old files are kept with a timestamp)",
            voices_dir.display()
        );
    }
    Ok(())
}

/// Design a voice and adopt it as `<voices_dir>/<id>.wav|txt`; returns the clip's path.
pub fn design_voice(
    designer: &mut dyn VoiceDesigner,
    voices_dir: &Path,
    req: &DesignRequest,
    log: &mut dyn Write,
) -> Result<PathBuf> {
    check_target(voices_dir, &req.id, req.force)?;
    let sr = designer.sample_rate();
    let raw = designer.design(
        REF_TEXT,
        &req.instruct,
        MAX_TOKENS,
        req.seed,
        req.temperature,
    )?;
    let audio = trim_and_level(&raw, sr);
    if audio.is_empty() {
        bail!("the VoiceDesign model returned no audio");
    }

    fs::create_dir_all(voices_dir).with_context(|| format!("creating {}", voices_dir.display()))?;
    let (wav, txt) = clip_paths(voices_dir, &req.id);
    keep_old(&[&wav, &txt], log)?;
    write_wav16(&wav, &audio, sr)?;
    fs::write(&txt, format!("{REF_TEXT}\n"))
        .with_context(|| format!("writing {}", txt.display()))?;
    add_voice(voices_dir, &req.id, &req.label)?;
    writeln!(
        log,
        "designed voice {} ({:.1}s) -> {}",
        req.id,
        audio.len() as f64 / sr as f64,
        wav.display()
    )?;
    Ok(wav)
}

/// design_voices.adopt: move an existing clip and transcript aside as `<id>.<stamp>.wav|txt`.
pub(crate) fn keep_old(files: &[&Path], log: &mut dyn Write) -> Result<()> {
    let stamp = timestamp(SystemTime::now());
    for &old in files {
        if old.exists() {
            let stem = old.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            let ext = old.extension().and_then(|s| s.to_str()).unwrap_or_default();
            let backup = old.with_file_name(format!("{stem}.{stamp}.{ext}"));
            if backup.exists() {
                bail!("{} already exists; not overwriting it", backup.display());
            }
            fs::rename(old, &backup)
                .with_context(|| format!("renaming {} to {}", old.display(), backup.display()))?;
            writeln!(log, "kept old voice as {}", backup.display())?;
        }
    }
    Ok(())
}

/// `YYYYmmdd-HHMMSS` in UTC (Python used local time; UTC needs no time zone data).
fn timestamp(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Howard Hinnant's civil_from_days, for days since 1970-01-01 (non-negative here).
    let z = days as i64 + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// 16-bit PCM mono, as soundfile wrote the existing voices.
pub(crate) fn write_wav16(path: &Path, audio: &[f32], sr: u32) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sr,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)
        .with_context(|| format!("creating {}", path.display()))?;
    for &s in audio {
        // libsndfile's float -> PCM_16 (f2s): scale by 0x7FFF, round to nearest; clamp first.
        let v = (f64::from(s.clamp(-1.0, 1.0)) * 32767.0).round() as i16;
        w.write_sample(v)?;
    }
    w.finalize()
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// `lectover voice design`.
pub fn run(args: &DesignArgs, settings: &Settings) -> Result<()> {
    let voices_dir = &settings.voices;
    let req = DesignRequest {
        id: args.id.clone(),
        label: args.label.clone(),
        instruct: args.instruct.clone(),
        seed: args.seed,
        temperature: args.temperature,
        force: args.force,
    };
    check_target(voices_dir, &req.id, req.force)?; // before any model work
    let files = DesignFiles::check(&settings.models)?;
    let device = resolve_device(args.device.or(settings.device))?;
    println!("{}", backend_banner(device));
    let mut designer = load_designer(&files, device)?;
    let result = design_voice(&mut *designer, voices_dir, &req, &mut std::io::stdout());
    drop(designer); // before exit: see load_designer
    result.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamp_is_utc_civil_time() {
        assert_eq!(timestamp(UNIX_EPOCH), "19700101-000000");
        // 2026-11-01T13:05:09Z
        assert_eq!(
            timestamp(UNIX_EPOCH + Duration::from_secs(1_793_538_309)),
            "20261101-130509"
        );
        // 2024-02-29T23:59:59Z
        assert_eq!(
            timestamp(UNIX_EPOCH + Duration::from_secs(1_709_251_199)),
            "20240229-235959"
        );
    }

    #[test]
    fn ref_text_is_neutral() {
        assert_eq!(
            REF_TEXT,
            "Guten Abend. In dieser Kolumne geht es um die Zukunft unserer öffentlichen Betriebe, \
             und um die Frage, was eine Fusion für alle Beteiligten bedeutet."
        );
    }
}
