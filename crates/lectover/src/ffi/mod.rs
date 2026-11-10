//! Safe wrappers around the native model libraries (`lectover-sys`). This module tree is the
//! only place in the crate allowed to use `unsafe`.
//!
//! Every C handle is owned by exactly one Rust value whose `Drop` releases it. There is no
//! process-global state: a voice lives on its `QwenTts` handle (`QwenTts::set_voice`).

mod asr;
mod backend;
mod reuse;
mod tts;

use std::ffi::{c_char, CStr, CString};
use std::path::Path;

use anyhow::{anyhow, Result};

pub use asr::{QwenAligner, QwenAsr};
pub use backend::{asr_library_device, backend_banner, check_ggml_layout, init_ggml_logging, resolve_device, warn_unified_memory, Device};
pub(crate) use backend::{unified_memory_warning, UNIFIED_MEMORY_ENV};
pub use reuse::{convert_reuse, ReuseDenoiser};
pub use tts::{QwenTts, QwenVoiceDesign};

/// One take to synthesise: `text` in at most `max_tokens` codec frames, sampled with `seed`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TakeRequest {
    pub text: String,
    pub max_tokens: usize,
    pub seed: u64,
}

impl TakeRequest {
    pub fn new(text: impl Into<String>, max_tokens: usize, seed: u64) -> Self {
        TakeRequest { text: text.into(), max_tokens, seed }
    }
}

/// Text-to-speech engine: one take per request.
pub trait Synth {
    /// Sample rate of the takes `generate` returns.
    fn sample_rate(&self) -> u32;
    /// Synthesise every request at `temperature`. Returns one mono take per request, in
    /// order. An engine with [`Synth::max_batch`] > 1 may decode them together.
    fn generate(&mut self, reqs: &[TakeRequest], temperature: f32) -> Result<Vec<Vec<f32>>>;
    /// [`Synth::generate`], calling `done(i, take)` on the caller's thread for every take as
    /// soon as it is ready, in any order, while the engine may still be decoding the rest.
    /// An error from `done` ends the call with that error once running decodes are finished.
    /// The default generates everything first and then reports the takes in order.
    fn generate_each(
        &mut self,
        reqs: &[TakeRequest],
        temperature: f32,
        done: &mut dyn FnMut(usize, &[f32]) -> Result<()>,
    ) -> Result<Vec<Vec<f32>>> {
        let takes = self.generate(reqs, temperature)?;
        for (i, t) in takes.iter().enumerate() {
            done(i, t)?;
        }
        Ok(takes)
    }
    /// Takes the engine decodes at once; a round fills spare slots with extra candidates.
    fn max_batch(&self) -> usize {
        1
    }
    /// Samples of ring-out padding after the generated end of every take (room tone the
    /// decoder needs to let the last syllable decay). End-of-speech checks look before it.
    fn tail_pad_samples(&self) -> usize {
        0
    }
    /// Everything besides (text, budget, seed, temperature) a take depends on: model, device,
    /// voice, sampling setup and batch width. Same fingerprint and inputs give a take of the
    /// same quality (bit for bit at batch width 1; batched, the companions in a decode step
    /// shift the floats), which lets `render::TakeCache` reuse it. `None` (the default)
    /// disables caching.
    fn fingerprint(&self) -> Option<String> {
        None
    }
}

/// Speech recognition on 16 kHz mono audio (resample with `audio::resample` first).
pub trait Transcribe {
    fn transcribe(&mut self, pcm16k: &[f32]) -> Result<String>;
}

/// Forced alignment of a word list to 16 kHz mono audio. `words` must be punctuation-free
/// (`text::spoken_words::spoken_words`); raw text gives zero-length spans. Returns
/// `(word, start_s, end_s)` per word, in order.
pub trait Align {
    fn align(&mut self, pcm16k: &[f32], words: &[String]) -> Result<Vec<(String, f64, f64)>>;
}

/// The aligner got no timestamps for word `word` (1-based) of `total` and filled it with the
/// `(0, duration)` span. Recoverable: callers interpolate the chunk's word times
/// (`anyhow::Error::downcast_ref::<FillSpanError>()`).
#[derive(Debug, Clone, PartialEq)]
pub struct FillSpanError {
    pub word: usize,
    pub total: usize,
    pub text: String,
    pub duration: f32,
}

impl std::fmt::Display for FillSpanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "aligner returned too few timestamps: word {} of {} ({:?}) got the (0, {}) fill span",
            self.word, self.total, self.text, self.duration
        )
    }
}

impl std::error::Error for FillSpanError {}

/// Copy a NUL-terminated C string; NULL becomes "".
fn c_str(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: callers pass strings the C APIs document as NUL-terminated and valid for the
    // duration of this call (static, handle-owned or thread-local until the next failure).
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

fn c_string(s: &str, what: &str) -> Result<CString> {
    CString::new(s).map_err(|_| anyhow!("{what} contains a NUL byte: {s:?}"))
}

fn c_path(p: &Path) -> Result<CString> {
    let s = p
        .to_str()
        .ok_or_else(|| anyhow!("path is not UTF-8: {}", p.display()))?;
    c_string(s, "path")
}

/// Fail early with a readable message instead of letting the C side report a vague load error.
fn require_file(p: &Path, what: &str) -> Result<()> {
    if p.is_file() {
        Ok(())
    } else {
        Err(anyhow!("{what} not found: {}", p.display()))
    }
}

/// Lengths cross the C ABI as `int`.
fn c_len(n: usize, what: &str) -> Result<i32> {
    i32::try_from(n).map_err(|_| anyhow!("{what} too long for the C API: {n}"))
}
