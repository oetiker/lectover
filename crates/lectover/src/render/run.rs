//! `lectover render`: port of `render_post.main`.
//!
//! [`prepare`] does every check that needs no model (fetch, visibility, text plan, voices);
//! [`render`] loads the models only after it succeeded. A local Markdown or HTML file skips
//! Ghost: [`prepare_post`] takes the post as is.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use indexmap::IndexMap;

use crate::audio::{assemble_parts_with, encode_mp3, encode_opus, level_sting, place_take};
use crate::cli::RenderArgs;
use crate::config::SiteVoice;
use crate::ffi::{
    asr_library_device, backend_banner, resolve_device, Align, QwenAligner, QwenAsr, QwenTts,
    Synth, Transcribe,
};
use crate::ghost::{
    fetch_post, http_get, list_posts, require_public, slug_from_arg, DateRange, GhostConfig,
    HttpReply, Post,
};
use crate::models::ModelFiles;
use crate::plan::{max_tokens_for, plan_steps, Chunk};
use crate::render::{
    align_segments, chunk_rate, measure_take, pick_reroll, rate_outliers, reroll_seed,
    reroll_takes, synthesize_chunks, with_carrier, CallStats, CarrierEnding, ChunkRate,
    ChunkRequest, MemoAlign, MemoAsr, RateOutlier, TakeCache, RATE_NEIGHBOURS, RATE_OUTLIER,
};
use crate::script::{build_script, intro_text, SiteSounds, Sting};
use crate::settings::{Settings, SiteSettings};
use crate::site::{encode_and_publish, prune_stale, VoiceTarget};
use crate::text::normalize::{load_lexicon, spoken_groups, Group, Lexicon};
use crate::text::{extract_marks, Block};
use crate::timings::WordTime;
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
            label: site
                .iter()
                .find(|v| &v.id == id)
                .and_then(|v| v.label.clone()),
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
    prepare_post(post, voices, voices_dir, lexicon, max_chars)
}

/// [`prepare`] for a post already in hand (a local file): plan the chunks, check the voices.
pub fn prepare_post(
    post: Post,
    voices: &[SiteVoice],
    voices_dir: &Path,
    lexicon: &Lexicon,
    max_chars: usize,
) -> Result<Prepared> {
    let slug = post.slug.clone();
    let (page, marks) = extract_marks(&post.html);
    if page.is_empty() {
        anyhow::bail!("post {slug} has no readable text");
    }
    let script = build_script(page, &marks, intro_text(&post));
    let blocks = script.blocks;
    let groups: Vec<Vec<Group>> = blocks
        .iter()
        .map(|b| spoken_groups(&b.text, lexicon))
        .collect();
    let (chunks, stings) = plan_steps(&blocks, &groups, &script.steps, max_chars);
    if chunks.is_empty() {
        anyhow::bail!("post {slug} has nothing to speak");
    }

    let registry = load_registry(voices_dir)?;
    let wanted: Vec<SiteVoice> = if voices.is_empty() {
        registry
            .keys()
            .take(1)
            .map(|id| SiteVoice {
                id: id.clone(),
                label: None,
            })
            .collect()
    } else {
        voices.to_vec()
    };
    if wanted.is_empty() {
        anyhow::bail!("no voices in {}", voices_dir.join("voices.json").display());
    }
    let unknown: Vec<&str> = wanted
        .iter()
        .filter(|v| !registry.contains_key(&v.id))
        .map(|v| v.id.as_str())
        .collect();
    if !unknown.is_empty() {
        let known: Vec<&str> = registry.keys().map(String::as_str).collect();
        anyhow::bail!(
            "unknown voice(s): {}; known: {}",
            unknown.join(", "),
            known.join(", ")
        );
    }
    let labels: IndexMap<String, String> = wanted
        .iter()
        .map(|v| {
            (
                v.id.clone(),
                v.label.clone().unwrap_or_else(|| registry[&v.id].clone()),
            )
        })
        .collect();
    let voices = wanted.into_iter().map(|v| v.id).collect();
    Ok(Prepared {
        post,
        blocks,
        page_blocks: script.page_blocks,
        stings,
        groups,
        chunks,
        labels,
        voices,
    })
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
    match &args.post {
        Some(post) => render_one(args, settings, &files, post),
        None => render_range(args, settings, &files),
    }
}

/// What every post of a site shares in a render: its settings, lexicon, voices and sounds.
struct SiteContext {
    site: SiteSettings,
    lexicon: Lexicon,
    voices: Vec<SiteVoice>,
    sounds: SiteSounds,
}

impl SiteContext {
    fn load(args: &RenderArgs, site: SiteSettings) -> Result<SiteContext> {
        let lexicon_path = args.lexicon.as_ref().unwrap_or(&site.lexicon);
        let lexicon = load_lexicon(lexicon_path)
            .with_context(|| format!("reading {}", lexicon_path.display()))?;
        let voices = select_voices(&args.voices, &site.voices);
        let sounds_dir = args.sounds.as_ref().unwrap_or(&site.sounds_dir);
        let sounds = SiteSounds::load(sounds_dir)
            .with_context(|| format!("reading {}", sounds_dir.display()))?;
        Ok(SiteContext {
            site,
            lexicon,
            voices,
            sounds,
        })
    }
}

/// Render the post `post` names: a Ghost post URL or slug, or a local file.
fn render_one(
    args: &RenderArgs,
    settings: &Settings,
    files: &ModelFiles,
    post: &str,
) -> Result<()> {
    let site = settings.site_for_post(post, args.site.as_deref())?;
    let local = crate::cli::local_post(post, &args.local)?;
    let is_local = local.is_some();
    let ctx = SiteContext::load(args, site)?;
    let voices_dir = settings.voices.as_path();
    let prep = match local {
        Some(post) => prepare_post(post, &ctx.voices, voices_dir, &ctx.lexicon, args.max_chars)?,
        None => {
            let cfg = GhostConfig::for_site(&ctx.site, &|k| std::env::var(k).ok())?;
            let slug = slug_from_arg(post)?;
            prepare(
                &cfg,
                &slug,
                &http_get,
                &ctx.voices,
                voices_dir,
                &ctx.lexicon,
                args.max_chars,
            )?
        }
    };
    let mut models = load_models(args, settings, files, &prep)?;
    let result = render_prepared(args, settings, &ctx, &prep, &mut models);
    drop(models);
    result?;
    if is_local {
        // A local file has no Ghost page: write its dev page from the very text the audio was
        // made from, so an edit to the file during the render cannot desynchronise the two.
        let page = crate::dev::write_page(&prep.post, &ctx.site.domain, &settings.site_root)?;
        println!("page: {}", page.display());
    }
    Ok(())
}

/// Render every public post of the site published in `--since`..`--until` (the site's `since`
/// when `--since` is not given), oldest first, loading the models once. A post that fails is
/// reported and skipped; the render fails at the end if any did.
fn render_range(args: &RenderArgs, settings: &Settings, files: &ModelFiles) -> Result<()> {
    let site = settings.site_for_post("", args.site.as_deref())?;
    let range = DateRange {
        since: args.since.clone().or_else(|| site.since.clone()),
        until: args.until.clone(),
    };
    let cfg = GhostConfig::for_site(&site, &|k| std::env::var(k).ok())?;
    let listing = list_posts(&cfg, &range, &http_get)?;
    let ctx = SiteContext::load(args, site)?;
    for p in &listing.not_public {
        println!(
            "skipping {} {}: not public (visibility={})",
            p.date(),
            p.slug,
            p.visibility
        );
    }
    let total = listing.public.len();
    if total == 0 {
        println!(
            "{}: no public posts published {}",
            ctx.site.domain,
            range.describe()
        );
        return Ok(());
    }
    println!(
        "{}: {total} public post(s) published {}",
        ctx.site.domain,
        range.describe()
    );
    for p in &listing.public {
        println!("  {} {}", p.date(), p.slug);
    }
    let started = Instant::now();
    let mut models: Option<Models> = None;
    let mut failed: Vec<String> = Vec::new();
    for (n, p) in listing.public.iter().enumerate() {
        println!("\n=== [{}/{total}] {} {}", n + 1, p.date(), p.slug);
        if let Err(e) = render_listed(args, settings, files, &cfg, &ctx, &p.slug, &mut models) {
            eprintln!("{}: {e:#}", p.slug);
            failed.push(p.slug.clone());
        }
    }
    drop(models);
    println!(
        "\nrendered {} of {total} post(s) in {:.0}s",
        total - failed.len(),
        started.elapsed().as_secs_f64()
    );
    if !failed.is_empty() {
        anyhow::bail!("{} post(s) failed: {}", failed.len(), failed.join(", "));
    }
    Ok(())
}

/// One post of [`render_range`]: fetch and plan it, load the models on first use, render.
fn render_listed(
    args: &RenderArgs,
    settings: &Settings,
    files: &ModelFiles,
    cfg: &GhostConfig,
    ctx: &SiteContext,
    slug: &str,
    models: &mut Option<Models>,
) -> Result<()> {
    let prep = prepare(
        cfg,
        slug,
        &http_get,
        &ctx.voices,
        settings.voices.as_path(),
        &ctx.lexicon,
        args.max_chars,
    )?;
    if models.is_none() {
        *models = Some(load_models(args, settings, files, &prep)?);
    }
    render_prepared(args, settings, ctx, &prep, models.as_mut().unwrap())
}

/// Load the models on the configured device, the TTS with the first voice of `prep`.
fn load_models(
    args: &RenderArgs,
    settings: &Settings,
    files: &ModelFiles,
    prep: &Prepared,
) -> Result<Models> {
    let device = resolve_device(args.device.or(settings.device))?;
    println!("{}", backend_banner(device));
    // qwen3-asr.cpp has no device parameter; load it where it will run.
    let asr_device = asr_library_device();
    if asr_device != device {
        println!("ASR and aligner: {asr_device} (qwen3-asr.cpp uses the first GPU)");
    }
    let (wav, text) = voice_ref(settings.voices.as_path(), &prep.voices[0])?;
    let mut models = Models {
        tts: QwenTts::load(
            &files.talker,
            &files.codec,
            &wav,
            &text,
            device,
            args.max_batch,
        )?,
        asr: QwenAsr::load(&files.asr, asr_device)?,
        aligner: QwenAligner::load(&files.aligner, asr_device)?,
    };
    models
        .tts
        .set_subtalker_temperature(args.subtalker_temperature);
    models.tts.set_non_streaming(!args.streaming);
    Ok(models)
}

/// Render every voice of a planned post into the deploy tree.
fn render_prepared(
    args: &RenderArgs,
    settings: &Settings,
    ctx: &SiteContext,
    prep: &Prepared,
    models: &mut Models,
) -> Result<()> {
    let slug_dir = settings
        .site_root
        .join(&ctx.site.domain)
        .join(&prep.post.slug);
    render_voices(
        args,
        &slug_dir,
        &settings.takes,
        &prep.post.slug,
        prep,
        &SiteInputs {
            sounds: &ctx.sounds,
            voices_dir: settings.voices.as_path(),
            lexicon: &ctx.lexicon,
        },
        models,
    )
}

/// Each chunk's rates from the placed parts and the final word times (see [`chunk_rate`]).
fn chunk_rates(
    prep: &Prepared,
    parts: &[(f64, Vec<f32>)],
    sr: u32,
    chunk_times: &[Vec<WordTime>],
    chunk_interp: &[Vec<bool>],
) -> Vec<ChunkRate> {
    prep.chunks
        .iter()
        .zip(parts)
        .zip(chunk_times.iter().zip(chunk_interp))
        .enumerate()
        .map(|(index, ((chunk, (_, wav)), (times, interp)))| {
            chunk_rate(index, chunk, wav, sr, times, interp)
        })
        .collect()
}

/// The `tempo:` lines of the per-voice summary: chunks clearly faster or slower than their
/// neighbours (see [`rate_outliers`]). A chunk with an interpolated word has no aligner rate
/// and is left out.
fn tempo_report(prep: &Prepared, outliers: &[RateOutlier]) -> String {
    if outliers.is_empty() {
        return format!(
            "  tempo: no outliers (> {:.0} % vs {RATE_NEIGHBOURS}+{RATE_NEIGHBOURS} neighbours, both measures)",
            RATE_OUTLIER * 100.0
        );
    }
    let mut out = format!("  tempo: {} outlier(s):", outliers.len());
    for o in outliers {
        let chunk = &prep.chunks[o.index];
        let block = if chunk.block < prep.page_blocks {
            format!("block {}", chunk.block + 1)
        } else {
            "audio only".to_string()
        };
        let mut text: String = chunk.text.chars().take(50).collect();
        if chunk.text.chars().count() > 50 {
            text.push_str(" …");
        }
        out.push_str(&format!(
            "\n    chunk {} ({block}): {:+.0} % vs neighbours (voiced {:+.0} %) «{text}»",
            o.index + 1,
            o.aligner_dev * 100.0,
            o.voiced_dev * 100.0
        ));
    }
    out
}

/// The per-site inputs of a render besides the post: its sounds, the voice registry and the
/// pronunciation lexicon.
struct SiteInputs<'a> {
    sounds: &'a SiteSounds,
    voices_dir: &'a Path,
    lexicon: &'a Lexicon,
}

fn render_voices(
    args: &RenderArgs,
    slug_dir: &Path,
    cache_dir: &Path,
    slug: &str,
    prep: &Prepared,
    site: &SiteInputs,
    models: &mut Models,
) -> Result<()> {
    let sr = models.tts.sample_rate();
    fs::create_dir_all(slug_dir).with_context(|| format!("creating {}", slug_dir.display()))?;
    let texts: Vec<String> = prep.chunks.iter().map(|c| c.tts_text()).collect();
    // the budget covers what is synthesised: the chunk plus the carrier
    let budgets: Vec<usize> = texts
        .iter()
        .map(|t| max_tokens_for(&with_carrier(t)))
        .collect();
    let page = &prep.blocks[..prep.page_blocks];
    let page_groups = &prep.groups[..prep.page_blocks];
    let hashes: Vec<String> = page.iter().map(|b| b.hash.clone()).collect();
    // an intro without a sound plays nothing; a break without one is a pause
    let levelled: Vec<(usize, Option<Vec<f32>>)> = prep
        .stings
        .iter()
        .filter_map(|&(i, s)| match (s, site.sounds.get(s)) {
            (_, Some((wav, from))) => Some((i, Some(level_sting(wav, *from, sr)))),
            (Sting::Intro, None) => None,
            (Sting::Break, None) => Some((i, None)),
        })
        .collect();
    let stings: Vec<(usize, Option<&[f32]>)> =
        levelled.iter().map(|(i, w)| (*i, w.as_deref())).collect();
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
        let (wav, text) = voice_ref(site.voices_dir, voice)?;
        models.tts.set_voice(&wav, &text)?;
        // --no-cache: every model call runs (the memos then answer nothing and keep nothing).
        let mut tts = TakeCache::new(&mut models.tts, cache_dir);
        let mut asr = MemoAsr::new(&mut models.asr, cache_dir);
        let mut aligner = MemoAlign::new(&mut models.aligner, cache_dir);
        let use_cache = !args.no_cache;
        macro_rules! tts {
            () => {
                if use_cache {
                    &mut tts as &mut dyn Synth
                } else {
                    tts.inner_mut()
                }
            };
        }
        macro_rules! asr {
            () => {
                if use_cache {
                    &mut asr as &mut dyn Transcribe
                } else {
                    asr.inner_mut()
                }
            };
        }
        macro_rules! al {
            () => {
                if use_cache {
                    &mut aligner as &mut dyn Align
                } else {
                    aligner.inner_mut()
                }
            };
        }
        let req = ChunkRequest {
            texts: &texts,
            budgets: &budgets,
            seed: args.seed,
            temperature: args.temperature,
            lex: site.lexicon,
        };
        let t_takes = Instant::now();
        let mut wavs = {
            let mut ending = CarrierEnding::new(al!());
            synthesize_chunks(tts!(), asr!(), &mut ending, &req)?
        };
        let takes_secs = t_takes.elapsed().as_secs_f64();
        let (tts_takes, asr_takes, aligner_takes) = (tts.stats, asr.stats, aligner.stats);
        let assemble = |wavs: &[Vec<f32>]| {
            let takes: Vec<_> = wavs
                .iter()
                .zip(&prep.chunks)
                .map(|(w, c)| (w.clone(), c.text.clone(), c.kind))
                .collect();
            assemble_parts_with(&takes, sr, args.pace, &stings)
        };
        let (mut audio, mut parts) = assemble(&wavs);
        let t_align = Instant::now();
        let (mut chunk_times, mut chunk_interp) = align_segments(al!(), &parts, sr, &prep.chunks)?;
        let align_secs = t_align.elapsed().as_secs_f64();
        let aligner_words = aligner.stats - aligner_takes;

        let outliers = rate_outliers(&chunk_rates(prep, &parts, sr, &chunk_times, &chunk_interp));
        let mut tempo = vec![tempo_report(prep, &outliers)];
        // (seconds, TTS, ASR, aligner) of the tempo re-roll, if it ran.
        let mut reroll: Option<(f64, CallStats, CallStats, CallStats)> = None;
        if args.reroll_outliers() && !outliers.is_empty() {
            let t_reroll = Instant::now();
            let before = (tts.stats, asr.stats, aligner.stats);
            let flagged: Vec<usize> = outliers.iter().map(|o| o.index).collect();
            let candidates = {
                let mut ending = CarrierEnding::new(al!());
                reroll_takes(
                    tts!(),
                    asr!(),
                    &mut ending,
                    &req,
                    &flagged,
                    &mut io::stdout(),
                )?
            };
            let mut items = Vec::new();
            let mut replaced = false;
            for (o, mut cands) in outliers.iter().zip(candidates) {
                let chunk = &prep.chunks[o.index];
                // Measured as placed, against the reference of the first pass.
                let rates: Vec<Option<f64>> = cands
                    .iter()
                    .map(|c| {
                        c.as_ref()
                            .map(|t| {
                                measure_take(al!(), o.index, chunk, &place_take(&t.wav, sr), sr)
                                    .map(|r| r.aligner)
                            })
                            .transpose()
                    })
                    .collect::<Result<_>>()?;
                let from = format!("chunk {} {:+.0} %", o.index + 1, o.aligner_dev * 100.0);
                items.push(match pick_reroll(o.aligner_ref, &rates) {
                    Some(p) if p.replace => {
                        let take = cands[p.candidate].take().expect("picked a passing take");
                        wavs[o.index] = take.wav;
                        replaced = true;
                        format!(
                            "{from} -> {:+.0} % (seed {})",
                            p.dev * 100.0,
                            reroll_seed(args.seed, p.candidate)
                        )
                    }
                    Some(p) => format!("{from} -> still {:+.0} % (kept)", p.dev * 100.0),
                    None => format!("{from} -> no usable take (kept)"),
                });
            }
            tempo.push(format!("  tempo re-roll: {}", items.join(", ")));
            if replaced {
                (audio, parts) = assemble(&wavs);
                (chunk_times, chunk_interp) = align_segments(al!(), &parts, sr, &prep.chunks)?;
                let rates = chunk_rates(prep, &parts, sr, &chunk_times, &chunk_interp);
                tempo.push(tempo_report(prep, &rate_outliers(&rates)));
            }
            reroll = Some((
                t_reroll.elapsed().as_secs_f64(),
                tts.stats - before.0,
                asr.stats - before.1,
                aligner.stats - before.2,
            ));
        }
        let t_encode = Instant::now();
        let (times, interp) =
            assemble_token_times(page, page_groups, &prep.chunks, &chunk_times, &chunk_interp)?;
        let timings = build_timings(voice, round3(audio.len() as f64 / sr as f64), page, &times)?;
        let label = &prep.labels[voice];
        let mut manifest = encode_and_publish(
            &audio,
            sr,
            &VoiceTarget {
                slug_dir,
                voice,
                label,
                default: &prep.voices[0],
                post: &prep.post,
            },
            &timings,
            encode_opus,
            encode_mp3,
        )?;
        prune_stale(slug_dir, &mut manifest, &hashes, voice)?;
        let encode_secs = t_encode.elapsed().as_secs_f64();
        let entry = manifest["voices"]
            .as_array()
            .and_then(|vs| vs.iter().find(|v| v["id"] == voice.as_str()))
            .ok_or_else(|| anyhow!("manifest lost voice {voice}"))?;
        let file = |key: &str| slug_dir.join(entry[key].as_str().unwrap_or_default());
        println!("{}", alignment_report(page, &interp));
        let reroll_time =
            reroll.map_or(String::new(), |(secs, ..)| format!(", re-roll {secs:.1}s"));
        println!(
            "voice {voice}: {:.1}s audio in {:.1}s (takes {takes_secs:.1}s, word times {align_secs:.1}s{reroll_time}, encoding {encode_secs:.1}s)",
            timings["duration"].as_f64().unwrap_or_default(),
            t0.elapsed().as_secs_f64()
        );
        if use_cache {
            println!(
                "  takes: TTS {}; ASR check {}; carrier cut {}",
                tts_takes.summary("synthesised"),
                asr_takes.summary("run"),
                aligner_takes.summary("run")
            );
            println!("  word times: aligner {}", aligner_words.summary("run"));
            if let Some((_, tts, asr, al)) = reroll {
                println!(
                    "  re-roll: TTS {}; ASR check {}; aligner {}",
                    tts.summary("synthesised"),
                    asr.summary("run"),
                    al.summary("run")
                );
            }
        } else {
            println!("  cache off (--no-cache): every take, ASR check and alignment ran afresh");
        }
        println!("{}", tempo.join("\n"));
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
