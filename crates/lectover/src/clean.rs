//! `lectover voice clean`: write a RE-USE-cleaned copy of a registered voice beside the original.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};

use crate::audio::denoise::{check_clean_len, noise_floor_db, Denoise};
use crate::audio::read_wav;
use crate::cli::CleanArgs;
use crate::settings::Settings;
use crate::design::{check_target, clip_paths, keep_old, write_wav16};
use crate::voices::add_voice;

/// Checks that need no model: the source is registered, `dst` differs from it and is free
/// (or `force`). Returns the source's label.
fn preflight(voices_dir: &Path, src: &str, dst: &str, force: bool) -> Result<String> {
    if src == dst {
        bail!("the new voice id must not be the same as the source id {src:?}");
    }
    // Read the source's label directly: load_registry would fail on another voice's missing file.
    let registry = voices_dir.join("voices.json");
    let map = match std::fs::read_to_string(&registry) {
        Ok(text) => serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text)
            .with_context(|| format!("parsing {}", registry.display()))?,
        Err(_) => Default::default(),
    };
    let Some(label) = map.get(src).and_then(|v| v.get("label")).and_then(|l| l.as_str()) else {
        bail!("unknown voice {src:?} in {}", voices_dir.display());
    };
    let (wav, txt) = clip_paths(voices_dir, src);
    for f in [&wav, &txt] {
        if !f.exists() {
            bail!("voice {src:?}: missing {}", f.display());
        }
    }
    check_target(voices_dir, dst, force)?;
    Ok(label.to_string())
}

/// Clean voice `src` and register the result as `dst`; the source stays untouched.
pub fn clean_voice(
    denoiser: &mut dyn Denoise,
    voices_dir: &Path,
    src: &str,
    dst: &str,
    label: &str,
    force: bool,
    log: &mut dyn Write,
) -> Result<PathBuf> {
    preflight(voices_dir, src, dst, force)?;
    let (src_wav, src_txt) = clip_paths(voices_dir, src);
    let (wav, txt) = clip_paths(voices_dir, dst);
    let (pcm, sr) = read_wav(&src_wav)?;
    check_clean_len(pcm.len(), sr)?;
    let start = Instant::now();
    let cleaned = denoiser.denoise(&pcm, sr)?;
    let secs = start.elapsed().as_secs_f64();
    writeln!(
        log,
        "noise floor {:.1} dB -> {:.1} dB ({secs:.1} s)",
        noise_floor_db(&pcm, sr),
        noise_floor_db(&cleaned, sr)
    )?;

    keep_old(&[&wav, &txt], log)?;
    write_wav16(&wav, &cleaned, sr)?;
    fs::copy(&src_txt, &txt).with_context(|| format!("copying {} to {}", src_txt.display(), txt.display()))?;
    add_voice(voices_dir, dst, label)?;
    writeln!(log, "cleaned voice {src} -> {dst} ({})", wav.display())?;
    Ok(wav)
}

/// `lectover voice clean`.
pub fn run(args: &CleanArgs, settings: &Settings) -> Result<()> {
    let voices_dir = &settings.voices;
    let dst = args.as_id.clone().unwrap_or_else(|| format!("{}reuse", args.id));
    let src_label = preflight(voices_dir, &args.id, &dst, args.force)?; // before any model work
    let label = args.label.clone().unwrap_or_else(|| format!("{src_label} (RE-USE)"));
    let mut out = std::io::stdout();
    let gguf = crate::models::ensure_reuse_gguf(&settings.models, &crate::ffi::convert_reuse, &mut out)?;
    let mut denoiser = crate::ffi::ReuseDenoiser::load(&gguf, crate::ffi::resolve_device(args.device.or(settings.device))?)?;
    denoiser.set_progress(crate::audio::denoise::stderr_progress_hook());
    let result = clean_voice(&mut denoiser, voices_dir, &args.id, &dst, &label, args.force, &mut out);
    drop(denoiser); // before exit: a Metal device still holding buffers aborts
    result.map(|_| ())
}
