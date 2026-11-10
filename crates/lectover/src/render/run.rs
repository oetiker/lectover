//! `lectover render`: port of `render_post.main`.
//!
//! [`prepare`] does every check that needs no model (fetch, visibility, text plan, voices);
//! [`render`] loads the models only after it succeeded.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use indexmap::IndexMap;

use crate::audio::{assemble_parts_with, encode_mp3, encode_opus, level_sting};
use crate::cli::RenderArgs;
use crate::config::SiteVoice;
use crate::ffi::{asr_library_device, backend_banner, resolve_device, QwenAligner, QwenAsr, QwenTts, Synth};
use crate::ghost::{fetch_post, http_get, require_public, slug_from_arg, GhostConfig, HttpReply, Post};
use crate::models::ModelFiles;
use crate::plan::{max_tokens_for, plan_steps, Chunk};
use crate::script::{build_script, intro_text, SiteSounds, Sting};
use crate::render::{align_segments, synthesize_chunks, with_carrier, CarrierEnding, TakeCache};
use crate::settings::Settings;
use crate::site::{encode_and_publish, prune_stale};
use crate::text::normalize::{load_lexicon, spoken_groups, Group, Lexicon};
use crate::text::{extract_marks, Block};
use crate::timings::{alignment_report, assemble_token_times, build_timings, round3};
use crate::voices::load_registry;

/// A post that passed every check and is planned for speech; no model was touched.
#[derive(Debug)]
pub struct Prepared {
    pub post: Post,
    /// The page blocks, then the audio-only blocks (intro, `AUDIO ONLY` comments).
    pub blocks: Vec<Block>,
    /// How many of `blocks` are on the page (and in the timings).
    pub page_blocks: usize,
    /// Each sting with the chunk it precedes.
    pub stings: Vec<(usize, Sting)>,
    pub groups: Vec<Vec<Group>>,
    pub chunks: Vec<Chunk>,
    /// Voice id -> label in the manifest, for the voices to render, in their order.
    pub labels: IndexMap<String, String>,
    /// The voices to render: the requested ones, else the registry's first.
    pub voices: Vec<String>,
}

/// The voices to render: those of `--voice`, in flag order, with the site's label where the
/// site lists the voice; without the flag, the site's voices in config order.
pub fn select_voices(flag: &[String], site: &[SiteVoice]) -> Vec<SiteVoice> {
    if flag.is_empty() {
        return site.to_vec();
    }
    flag.iter()
        .map(|id| SiteVoice {
            id: id.clone(),
            label: site.iter().find(|v| &v.id == id).and_then(|v| v.label.clone()),
        })
        .collect()
}

/// Fetch the post, refuse non-public or empty posts, plan the chunks and check the voices.
/// `voices` empty means the registry's first voice; a voice without a label takes the
/// registry's.
pub fn prepare(
    cfg: &GhostConfig,
    slug: &str,
    get: &dyn Fn(&str) -> Result<HttpReply>,
    voices: &[SiteVoice],
    voices_dir: &Path,
    lexicon: &Lexicon,
    max_chars: usize,
) -> Result<Prepared> {
    let post = fetch_post(cfg, slug, get)?;
    require_public(&post)?; // before any synthesis

    let (page, marks) = extract_marks(&post.html);
    if page.is_empty() {
        anyhow::bail!("post {slug} has no readable text");
    }
    let script = build_script(page, &marks, intro_text(&post));
    let blocks = script.blocks;
    let groups: Vec<Vec<Group>> = blocks.iter().map(|b| spoken_groups(&b.text, lexicon)).collect();
    let (chunks, stings) = plan_steps(&blocks, &groups, &script.steps, max_chars);
    if chunks.is_empty() {
        anyhow::bail!("post {slug} has nothing to speak");
    }

    let registry = load_registry(voices_dir)?;
    let wanted: Vec<SiteVoice> = if voices.is_empty() {
        registry.keys().take(1).map(|id| SiteVoice { id: id.clone(), label: None }).collect()
    } else {
        voices.to_vec()
    };
    if wanted.is_empty() {
        anyhow::bail!("no voices in {}", voices_dir.join("voices.json").display());
    }
    let unknown: Vec<&str> =
        wanted.iter().filter(|v| !registry.contains_key(&v.id)).map(|v| v.id.as_str()).collect();
    if !unknown.is_empty() {
        let known: Vec<&str> = registry.keys().map(String::as_str).collect();
        anyhow::bail!("unknown voice(s): {}; known: {}", unknown.join(", "), known.join(", "));
    }
    let labels: IndexMap<String, String> = wanted
        .iter()
        .map(|v| (v.id.clone(), v.label.clone().unwrap_or_else(|| registry[&v.id].clone())))
        .collect();
    let voices = wanted.into_iter().map(|v| v.id).collect();
    Ok(Prepared { post, blocks, page_blocks: script.page_blocks, stings, groups, chunks, labels, voices })
}

/// Reference clip and transcript of a voice (`read_qwen.ref_text_for`, minus the fallback text:
/// the registry guarantees the `.txt`).
pub(crate) fn voice_ref(voices_dir: &Path, voice: &str) -> Result<(PathBuf, String)> {
    let wav = voices_dir.join(format!("{voice}.wav"));
    let txt = wav.with_extension("txt");
    let text = fs::read_to_string(&txt).with_context(|| format!("reading {}", txt.display()))?;
    Ok((wav, text.trim().to_string()))
}

/// The loaded models. Owned by [`render`] and dropped before it returns: a ggml Metal device
/// still holding buffers at process exit aborts.
struct Models {
    tts: QwenTts,
    asr: QwenAsr,
    aligner: QwenAligner,
}

pub fn render(args: &RenderArgs, settings: &Settings) -> Result<()> {
    let files = ModelFiles::check(&settings.models)?; // before any network or model work
    let site = settings.site_for_post(&args.post, args.site.as_deref())?;
    let cfg = GhostConfig::for_site(&site, &|k| std::env::var(k).ok())?;
    let slug = slug_from_arg(&args.post)?;
    let lexicon_path = args.lexicon.as_ref().unwrap_or(&site.lexicon);
    let lexicon = load_lexicon(lexicon_path).with_context(|| format!("reading {}", lexicon_path.display()))?;
    let voices_dir = settings.voices.as_path();
    let voices = select_voices(&args.voices, &site.voices);
    let prep = prepare(&cfg, &slug, &http_get, &voices, voices_dir, &lexicon, args.max_chars)?;
    let sounds_dir = args.sounds.as_ref().unwrap_or(&site.sounds_dir);
    let sounds = SiteSounds::load(sounds_dir).with_context(|| format!("reading {}", sounds_dir.display()))?;

    let device = resolve_device(args.device.or(settings.device))?;
    println!("{}", backend_banner(device));
    // qwen3-asr.cpp has no device parameter; load it where it will run.
    let asr_device = asr_library_device();
    if asr_device != device {
        println!("ASR and aligner: {asr_device} (qwen3-asr.cpp uses the first GPU)");
    }
    let (wav, text) = voice_ref(voices_dir, &prep.voices[0])?;
    let mut models = Models {
        tts: QwenTts::load(&files.talker, &files.codec, &wav, &text, device, args.max_batch)?,
        asr: QwenAsr::load(&files.asr, asr_device)?,
        aligner: QwenAligner::load(&files.aligner, asr_device)?,
    };
    models.tts.set_subtalker_temperature(args.subtalker_temperature);
    models.tts.set_non_streaming(!args.streaming);
    let slug_dir = settings.site_root.join(&site.domain).join(&prep.post.slug);
    let result =
        render_voices(args, &slug_dir, &settings.takes, &slug, &prep, &sounds, voices_dir, &lexicon, &mut models);
    drop(models);
    result
}

fn render_voices(
    args: &RenderArgs,
    slug_dir: &Path,
    cache_dir: &Path,
    slug: &str,
    prep: &Prepared,
    sounds: &SiteSounds,
    voices_dir: &Path,
    lexicon: &Lexicon,
    models: &mut Models,
) -> Result<()> {
    let sr = models.tts.sample_rate();
    fs::create_dir_all(slug_dir).with_context(|| format!("creating {}", slug_dir.display()))?;
    let texts: Vec<String> = prep.chunks.iter().map(|c| c.tts_text()).collect();
    // the budget covers what is synthesised: the chunk plus the carrier
    let budgets: Vec<usize> = texts.iter().map(|t| max_tokens_for(&with_carrier(t))).collect();
    let page = &prep.blocks[..prep.page_blocks];
    let page_groups = &prep.groups[..prep.page_blocks];
    let hashes: Vec<String> = page.iter().map(|b| b.hash.clone()).collect();
    // an intro without a sound plays nothing; a break without one is a pause
    let levelled: Vec<(usize, Option<Vec<f32>>)> = prep
        .stings
        .iter()
        .filter_map(|&(i, s)| match (s, sounds.get(s)) {
            (_, Some((wav, from))) => Some((i, Some(level_sting(wav, *from, sr)))),
            (Sting::Intro, None) => None,
            (Sting::Break, None) => Some((i, None)),
        })
        .collect();
    let stings: Vec<(usize, Option<&[f32]>)> = levelled.iter().map(|(i, w)| (*i, w.as_deref())).collect();
    println!(
        "{slug}: {} blocks (+{} audio only), {} chunks, {} stings, voices {}",
        prep.page_blocks,
        prep.blocks.len() - prep.page_blocks,
        prep.chunks.len(),
        stings.len(),
        prep.voices.join(", ")
    );

    for voice in &prep.voices {
        let t0 = Instant::now();
        let (wav, text) = voice_ref(voices_dir, voice)?;
        models.tts.set_voice(&wav, &text)?;
        let mut cached;
        let tts: &mut dyn Synth = if args.no_cache {
            &mut models.tts
        } else {
            cached = TakeCache::new(&mut models.tts, cache_dir);
            &mut cached
        };
        let mut ending = CarrierEnding::new(&mut models.aligner);
        let wavs = synthesize_chunks(
            tts,
            &mut models.asr,
            &mut ending,
            &texts,
            &budgets,
            args.seed,
            args.temperature,
            lexicon,
        )?;
        let takes: Vec<_> = wavs
            .into_iter()
            .zip(&prep.chunks)
            .map(|(w, c)| (w, c.text.clone(), c.kind))
            .collect();
        let (audio, parts) = assemble_parts_with(&takes, sr, !args.no_pace, &stings);
        let (chunk_times, chunk_interp) = align_segments(&mut models.aligner, &parts, sr, &prep.chunks)?;
        let (times, interp) =
            assemble_token_times(page, page_groups, &prep.chunks, &chunk_times, &chunk_interp)?;
        let timings = build_timings(voice, round3(audio.len() as f64 / sr as f64), page, &times)?;
        let label = &prep.labels[voice];
        let mut manifest = encode_and_publish(
            &audio,
            sr,
            slug_dir,
            voice,
            label,
            &timings,
            &prep.post,
            &prep.voices[0],
            encode_opus,
            encode_mp3,
        )?;
        prune_stale(slug_dir, &mut manifest, &hashes, voice)?;
        let entry = manifest["voices"]
            .as_array()
            .and_then(|vs| vs.iter().find(|v| v["id"] == voice.as_str()))
            .ok_or_else(|| anyhow!("manifest lost voice {voice}"))?;
        let file = |key: &str| slug_dir.join(entry[key].as_str().unwrap_or_default());
        println!("{}", alignment_report(page, &interp));
        println!(
            "voice {voice}: {:.1}s audio in {:.1}s",
            timings["duration"].as_f64().unwrap_or_default(),
            t0.elapsed().as_secs_f64()
        );
        println!(
            "  {}\n  {}\n  {}\n  {}",
            file("audio").display(),
            file("mp3").display(),
            file("timings").display(),
            slug_dir.join("manifest.json").display()
        );
    }
    Ok(())
}
