//! Disk cache of TTS takes. A take depends only on the engine's fingerprint (model, voice,
//! sampling setup) and on (text, token budget, seed, temperature), and the same inputs give
//! the same take bit for bit. So a re-render after an edit synthesises only the chunks whose
//! text changed and reads every other take back from disk.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use sha1::{Digest, Sha1};

use crate::ffi::{Synth, TakeRequest};

/// A [`Synth`] that answers from `dir` when it can and asks `inner` for the rest, in one
/// `generate` call (so batching still sees every missing request). An engine without a
/// [`Synth::fingerprint`] is passed through uncached.
pub struct TakeCache<'a> {
    inner: &'a mut dyn Synth,
    dir: PathBuf,
}

impl<'a> TakeCache<'a> {
    pub fn new(inner: &'a mut dyn Synth, dir: &Path) -> Self {
        TakeCache {
            inner,
            dir: dir.to_path_buf(),
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
    Some(words.iter().map(|b| f32::from_le_bytes(*b)).collect())
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
        if !missing.is_empty() {
            let todo: Vec<TakeRequest> = missing.iter().map(|&i| reqs[i].clone()).collect();
            let fresh = self
                .inner
                .generate_each(&todo, temperature, &mut |j, t| done(missing[j], t))?;
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
}
