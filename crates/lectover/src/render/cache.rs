//! Disk cache of TTS takes. A take depends only on the engine's fingerprint (model, voice,
//! sampling setup) and on (text, token budget, seed, temperature), and the same inputs give
//! the same take bit for bit. So a re-render after an edit synthesises only the chunks whose
//! text changed and reads every other take back from disk.
//!
//! A cached take comes back bit for bit, so the ASR check and the forced aligner then see the
//! same audio as before. [`MemoAsr`] and [`MemoAlign`] keep their answers on disk too (keyed by
//! the model fingerprint, the audio and the words), so an unchanged chunk costs no model call.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{bail, Context, Result};
use sha1::{Digest, Sha1};

use crate::ffi::{Align, Synth, TakeRequest, Transcribe};

/// A [`Synth`] that answers from `dir` when it can and asks `inner` for the rest, in one
/// `generate` call (so batching still sees every missing request). An engine without a
/// [`Synth::fingerprint`] is passed through uncached.
pub struct TakeCache<'a> {
    inner: &'a mut dyn Synth,
    dir: PathBuf,
    /// Takes read from disk (`hits`) and synthesised (`runs`, `secs` in the engine).
    pub stats: CallStats,
}

/// Model calls answered from disk and made, with the time the made calls took.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct CallStats {
    pub hits: usize,
    pub runs: usize,
    pub secs: f64,
}

impl std::ops::Sub for CallStats {
    type Output = CallStats;

    /// The calls made between two snapshots of the same counter.
    fn sub(self, before: CallStats) -> CallStats {
        CallStats {
            hits: self.hits - before.hits,
            runs: self.runs - before.runs,
            secs: self.secs - before.secs,
        }
    }
}

impl CallStats {
    /// "7 run, 46 cached, 3.1s"
    pub fn summary(&self, ran: &str) -> String {
        format!(
            "{} {ran}, {} cached, {:.1}s",
            self.runs, self.hits, self.secs
        )
    }
}

impl<'a> TakeCache<'a> {
    /// The wrapped model, bypassing the cache (`--no-cache`).
    pub fn inner_mut(&mut self) -> &mut dyn Synth {
        &mut *self.inner
    }

    pub fn new(inner: &'a mut dyn Synth, dir: &Path) -> Self {
        TakeCache {
            inner,
            dir: dir.to_path_buf(),
            stats: CallStats::default(),
        }
    }

    fn path(
        &self,
        fingerprint: &str,
        text: &str,
        max_tokens: usize,
        seed: u64,
        temperature: f32,
    ) -> PathBuf {
        let mut h = Sha1::new();
        for part in [
            fingerprint.as_bytes(),
            text.as_bytes(),
            &(max_tokens as u64).to_le_bytes(),
            &seed.to_le_bytes(),
            &temperature.to_bits().to_le_bytes(),
        ] {
            h.update((part.len() as u64).to_le_bytes());
            h.update(part);
        }
        let key: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        self.dir.join(format!("{key}.f32"))
    }
}

/// A cached take, or `None` when the file is missing or damaged.
fn read_take(path: &Path) -> Option<Vec<f32>> {
    let bytes = fs::read(path).ok()?;
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        return None;
    }
    let (words, _) = bytes.as_chunks::<4>();
    touch(path);
    Some(words.iter().map(|b| f32::from_le_bytes(*b)).collect())
}

/// Mark a cache file as used now (its mtime), so [`prune`] keeps it. Best effort.
fn touch(path: &Path) {
    let _ = fs::File::options()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(SystemTime::now()));
}

/// What [`prune`] removed (or, dry, would remove) and kept, in files and bytes.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct PruneReport {
    pub removed: usize,
    pub removed_bytes: u64,
    pub kept: usize,
    pub kept_bytes: u64,
}

/// Remove the cache entries in `dir` (takes `*.f32`, `asr/*.txt`, `align/*.tsv`) not used for
/// `unused` (by mtime: a render that reads an entry refreshes it). `dry` only counts. Other
/// files are left alone; a missing `dir` is an empty cache.
pub fn prune(dir: &Path, unused: Duration, dry: bool) -> Result<PruneReport> {
    let cutoff = SystemTime::now()
        .checked_sub(unused)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut report = PruneReport::default();
    for (sub, ext) in [("", "f32"), ("asr", "txt"), ("align", "tsv")] {
        let Ok(entries) = fs::read_dir(dir.join(sub)) else {
            continue;
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some(ext) {
                continue;
            }
            let meta = fs::metadata(&path)?;
            if !meta.is_file() {
                continue;
            }
            if meta.modified()? < cutoff {
                if !dry {
                    fs::remove_file(&path)
                        .with_context(|| format!("removing {}", path.display()))?;
                }
                report.removed += 1;
                report.removed_bytes += meta.len();
            } else {
                report.kept += 1;
                report.kept_bytes += meta.len();
            }
        }
    }
    Ok(report)
}

/// Write via a temporary file and rename, so a crash never leaves a half take behind.
fn write_take(path: &Path, wav: &[f32]) -> Result<()> {
    let dir = path.parent().expect("cache paths have a parent");
    fs::create_dir_all(dir)?;
    let bytes: Vec<u8> = wav.iter().flat_map(|v| v.to_le_bytes()).collect();
    let tmp = tempfile::NamedTempFile::new_in(dir)?;
    fs::write(tmp.path(), bytes)?;
    tmp.persist(path)?;
    Ok(())
}

impl Synth for TakeCache<'_> {
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn generate(&mut self, reqs: &[TakeRequest], temperature: f32) -> Result<Vec<Vec<f32>>> {
        self.generate_each(reqs, temperature, &mut |_, _| Ok(()))
    }

    /// Cached takes are reported first, then the fresh ones as the engine finishes them.
    fn generate_each(
        &mut self,
        reqs: &[TakeRequest],
        temperature: f32,
        done: &mut dyn FnMut(usize, &[f32]) -> Result<()>,
    ) -> Result<Vec<Vec<f32>>> {
        let Some(fp) = self.inner.fingerprint() else {
            return self.inner.generate_each(reqs, temperature, done);
        };
        let paths: Vec<PathBuf> = reqs
            .iter()
            .map(|r| self.path(&fp, &r.text, r.max_tokens, r.seed, temperature))
            .collect();
        let mut takes: Vec<Option<Vec<f32>>> = paths.iter().map(|p| read_take(p)).collect();
        for (i, t) in takes.iter().enumerate() {
            if let Some(t) = t {
                done(i, t)?;
            }
        }
        let missing: Vec<usize> = (0..reqs.len()).filter(|&i| takes[i].is_none()).collect();
        self.stats.hits += reqs.len() - missing.len();
        if !missing.is_empty() {
            let todo: Vec<TakeRequest> = missing.iter().map(|&i| reqs[i].clone()).collect();
            let t0 = Instant::now();
            let fresh = self
                .inner
                .generate_each(&todo, temperature, &mut |j, t| done(missing[j], t))?;
            self.stats.runs += todo.len();
            self.stats.secs += t0.elapsed().as_secs_f64();
            if fresh.len() != todo.len() {
                bail!(
                    "synthesis returned {} takes for {} requests",
                    fresh.len(),
                    todo.len()
                );
            }
            for (&i, wav) in missing.iter().zip(fresh) {
                // A cache that cannot be written only costs time on the next render.
                if let Err(e) = write_take(&paths[i], &wav) {
                    eprintln!(
                        "warning: cannot cache take in {}: {e:#}",
                        self.dir.display()
                    );
                }
                takes[i] = Some(wav);
            }
        }
        Ok(takes
            .into_iter()
            .map(|t| t.expect("every take was read or generated"))
            .collect())
    }

    fn tail_pad_samples(&self) -> usize {
        self.inner.tail_pad_samples()
    }

    fn max_batch(&self) -> usize {
        self.inner.max_batch()
    }

    fn fingerprint(&self) -> Option<String> {
        self.inner.fingerprint()
    }

    fn cached(&self, reqs: &[TakeRequest], temperature: f32) -> usize {
        let Some(fp) = self.inner.fingerprint() else {
            return 0;
        };
        reqs.iter()
            .filter(|r| {
                self.path(&fp, &r.text, r.max_tokens, r.seed, temperature)
                    .is_file()
            })
            .count()
    }
}

/// Cache key of a model call: SHA-1 over the fingerprint, the 16 kHz audio and the words.
fn memo_key(fingerprint: &str, pcm16k: &[f32], words: &[String]) -> String {
    let mut h = Sha1::new();
    h.update((fingerprint.len() as u64).to_le_bytes());
    h.update(fingerprint.as_bytes());
    h.update((pcm16k.len() as u64).to_le_bytes());
    for v in pcm16k {
        h.update(v.to_le_bytes());
    }
    h.update((words.len() as u64).to_le_bytes());
    for w in words {
        h.update((w.len() as u64).to_le_bytes());
        h.update(w.as_bytes());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Write `contents` via a temporary file and rename; a failure only costs time later.
fn write_memo(path: &Path, contents: &str) {
    let write = || -> Result<()> {
        let dir = path.parent().expect("cache paths have a parent");
        fs::create_dir_all(dir)?;
        let tmp = tempfile::NamedTempFile::new_in(dir)?;
        fs::write(tmp.path(), contents)?;
        tmp.persist(path)?;
        Ok(())
    };
    if let Err(e) = write() {
        eprintln!("warning: cannot cache in {}: {e:#}", path.display());
    }
}

/// A [`Transcribe`] that answers from `dir` when it has heard the audio before. A model
/// without a fingerprint is passed through uncached.
pub struct MemoAsr<'a> {
    inner: &'a mut dyn Transcribe,
    dir: PathBuf,
    pub stats: CallStats,
}

impl<'a> MemoAsr<'a> {
    /// The wrapped model, bypassing the cache (`--no-cache`).
    pub fn inner_mut(&mut self) -> &mut dyn Transcribe {
        &mut *self.inner
    }

    /// Keeps its answers in `dir/asr`.
    pub fn new(inner: &'a mut dyn Transcribe, dir: &Path) -> Self {
        MemoAsr {
            inner,
            dir: dir.join("asr"),
            stats: CallStats::default(),
        }
    }
}

impl Transcribe for MemoAsr<'_> {
    fn transcribe(&mut self, pcm16k: &[f32]) -> Result<String> {
        let path = self
            .inner
            .fingerprint()
            .map(|fp| self.dir.join(format!("{}.txt", memo_key(&fp, pcm16k, &[]))));
        if let Some(p) = &path {
            if let Ok(text) = fs::read_to_string(p) {
                touch(p);
                self.stats.hits += 1;
                return Ok(text);
            }
        }
        let t0 = Instant::now();
        let text = self.inner.transcribe(pcm16k)?;
        self.stats.runs += 1;
        self.stats.secs += t0.elapsed().as_secs_f64();
        if let Some(p) = path {
            write_memo(&p, &text);
        }
        Ok(text)
    }

    fn fingerprint(&self) -> Option<String> {
        self.inner.fingerprint()
    }
}

/// An [`Align`] that answers from `dir` when it has aligned the audio and words before.
/// Errors are not kept: the model runs again next time.
pub struct MemoAlign<'a> {
    inner: &'a mut dyn Align,
    dir: PathBuf,
    pub stats: CallStats,
}

impl<'a> MemoAlign<'a> {
    /// The wrapped model, bypassing the cache (`--no-cache`).
    pub fn inner_mut(&mut self) -> &mut dyn Align {
        &mut *self.inner
    }

    /// Keeps its answers in `dir/align`.
    pub fn new(inner: &'a mut dyn Align, dir: &Path) -> Self {
        MemoAlign {
            inner,
            dir: dir.join("align"),
            stats: CallStats::default(),
        }
    }
}

/// One `word<TAB>start<TAB>end` line per word, times as f64 bit patterns (exact round trip).
fn format_spans(spans: &[(String, f64, f64)]) -> String {
    spans
        .iter()
        .map(|(w, s, e)| format!("{w}\t{:016x}\t{:016x}\n", s.to_bits(), e.to_bits()))
        .collect()
}

fn parse_spans(text: &str, words: &[String]) -> Option<Vec<(String, f64, f64)>> {
    let spans: Vec<(String, f64, f64)> = text
        .lines()
        .map(|line| {
            let mut f = line.split('\t');
            let w = f.next()?.to_string();
            let s = u64::from_str_radix(f.next()?, 16).ok()?;
            let e = u64::from_str_radix(f.next()?, 16).ok()?;
            Some((w, f64::from_bits(s), f64::from_bits(e)))
        })
        .collect::<Option<_>>()?;
    // A damaged file never passes for an answer.
    (spans.len() == words.len()).then_some(spans)
}

impl Align for MemoAlign<'_> {
    fn align(&mut self, pcm16k: &[f32], words: &[String]) -> Result<Vec<(String, f64, f64)>> {
        let path = self.inner.fingerprint().map(|fp| {
            self.dir
                .join(format!("{}.tsv", memo_key(&fp, pcm16k, words)))
        });
        if let Some(p) = &path {
            if let Some(spans) = fs::read_to_string(p)
                .ok()
                .and_then(|t| parse_spans(&t, words))
            {
                touch(p);
                self.stats.hits += 1;
                return Ok(spans);
            }
        }
        let t0 = Instant::now();
        let result = self.inner.align(pcm16k, words);
        self.stats.runs += 1;
        self.stats.secs += t0.elapsed().as_secs_f64();
        let spans = result?;
        if let Some(p) = path {
            write_memo(&p, &format_spans(&spans));
        }
        Ok(spans)
    }

    fn fingerprint(&self) -> Option<String> {
        self.inner.fingerprint()
    }
}
