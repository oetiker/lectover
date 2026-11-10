//! Qwen3-ASR transcription and Qwen3-ForcedAligner word alignment via qwen3-asr.cpp
//! (`qwen3_asr_c.h`). Both take 16 kHz mono f32.

use std::ffi::{c_char, CString};
use std::path::Path;

use anyhow::{bail, Result};
use lectover_sys as sys;

use super::backend::{asr_library_device, check_ggml_layout};
use super::{c_len, c_path, c_str, c_string, require_file, Align, Device, FillSpanError, Transcribe};

/// The rate both models expect.
const ASR_RATE: f32 = 16000.0;
/// `n_threads` for the C API: `<= 0` selects its default (4 host threads; graphs run on the GPU).
const DEFAULT_THREADS: i32 = 0;
/// `max_tokens` for transcription: `<= 0` selects the library default (1024).
const DEFAULT_MAX_TOKENS: i32 = 0;
/// Unused by the library (it only splits text by language; we pass words).
const LANGUAGE: &std::ffi::CStr = c"German";

fn last_error() -> String {
    // SAFETY: qwen3_asr_last_error never returns NULL; the thread-local string is copied at once.
    c_str(unsafe { sys::qwen3_asr_last_error() })
}

/// Common load checks. qwen3-asr.cpp has no device parameter: it always runs on the first GPU
/// when there is one. Refuse a request it would silently ignore.
fn check_load(model: &Path, what: &str, device: Device) -> Result<CString> {
    check_ggml_layout()?;
    require_file(model, what)?;
    let actual = asr_library_device();
    if actual != device {
        bail!("{what} cannot run on {device}: qwen3-asr.cpp always uses the first GPU, here {actual}");
    }
    c_path(model)
}

/// Qwen3-ASR handle. Not thread safe on the C side: one handle per thread.
pub struct QwenAsr {
    ctx: *mut sys::qwen3_asr_context,
}

// SAFETY: the handle may move between threads; `&mut self` serialises every call.
unsafe impl Send for QwenAsr {}

impl Drop for QwenAsr {
    fn drop(&mut self) {
        // SAFETY: ctx came from qwen3_asr_init and is freed exactly once (NULL-safe).
        unsafe { sys::qwen3_asr_free(self.ctx) };
    }
}

impl QwenAsr {
    pub fn load(model: &Path, device: Device) -> Result<Self> {
        let path = check_load(model, "ASR model", device)?;
        // SAFETY: path is a valid NUL-terminated string for the duration of the call.
        let ctx = unsafe { sys::qwen3_asr_init(path.as_ptr(), DEFAULT_THREADS) };
        if ctx.is_null() {
            bail!("qwen3_asr_init failed ({}): {}", model.display(), last_error());
        }
        Ok(QwenAsr { ctx })
    }
}

/// Owns a string returned by `qwen3_asr_transcribe`.
struct AsrString(*mut c_char);

impl Drop for AsrString {
    fn drop(&mut self) {
        // SAFETY: the pointer came from qwen3_asr_transcribe; qwen3_asr_string_free is NULL-safe.
        unsafe { sys::qwen3_asr_string_free(self.0) };
    }
}

impl Transcribe for QwenAsr {
    fn transcribe(&mut self, pcm16k: &[f32]) -> Result<String> {
        if pcm16k.is_empty() {
            bail!("transcribe: empty audio");
        }
        let n = c_len(pcm16k.len(), "audio")?;
        // SAFETY: ctx is live; pcm16k holds n floats and is only read during the call.
        let s =
            AsrString(unsafe { sys::qwen3_asr_transcribe(self.ctx, pcm16k.as_ptr(), n, DEFAULT_MAX_TOKENS) });
        if s.0.is_null() {
            bail!("qwen3_asr_transcribe failed: {}", last_error());
        }
        Ok(c_str(s.0))
    }
}

/// Qwen3-ForcedAligner handle. Not thread safe on the C side: one handle per thread.
pub struct QwenAligner {
    ctx: *mut sys::qwen3_aligner_context,
}

// SAFETY: as for QwenAsr.
unsafe impl Send for QwenAligner {}

impl Drop for QwenAligner {
    fn drop(&mut self) {
        // SAFETY: ctx came from qwen3_aligner_init and is freed exactly once (NULL-safe).
        unsafe { sys::qwen3_aligner_free(self.ctx) };
    }
}

impl QwenAligner {
    pub fn load(model: &Path, device: Device) -> Result<Self> {
        let path = check_load(model, "aligner model", device)?;
        // SAFETY: path is a valid NUL-terminated string for the duration of the call.
        let ctx = unsafe { sys::qwen3_aligner_init(path.as_ptr(), DEFAULT_THREADS) };
        if ctx.is_null() {
            bail!(
                "qwen3_aligner_init failed ({}): {}",
                model.display(),
                last_error()
            );
        }
        Ok(QwenAligner { ctx })
    }
}

/// The library fills words it got no timestamps for with `(0, duration)` (forced_aligner.cpp,
/// `align_words`) and reports success. Such a span is never a real alignment of a word of a
/// multi-word take, and for a single word it would need a timestamp of exactly 0 and one clamped
/// to the exact duration; treat it as a failure instead of handing out silent zero spans.
fn check_no_fill_spans(spans: &[sys::qwen3_word_span], words: &[String], n_samples: i32) -> Result<()> {
    // Same f32 expression as the C side, so the comparison is exact.
    let duration = n_samples as f32 / ASR_RATE;
    if let Some(i) = spans.iter().position(|s| s.start_s == 0.0 && s.end_s == duration) {
        return Err(FillSpanError { word: i + 1, total: words.len(), text: words[i].clone(), duration }.into());
    }
    Ok(())
}

impl Align for QwenAligner {
    fn align(&mut self, pcm16k: &[f32], words: &[String]) -> Result<Vec<(String, f64, f64)>> {
        if words.is_empty() {
            return Ok(Vec::new());
        }
        if pcm16k.is_empty() {
            bail!("align: empty audio for {} words", words.len());
        }
        let n = c_len(pcm16k.len(), "audio")?;
        let n_words = c_len(words.len(), "word list")?;
        let words_c = words
            .iter()
            .map(|w| c_string(w, "word"))
            .collect::<Result<Vec<_>>>()?;
        let ptrs: Vec<*const c_char> = words_c.iter().map(|w| w.as_ptr()).collect();
        let mut spans = vec![
            sys::qwen3_word_span {
                start_s: 0.0,
                end_s: 0.0
            };
            words.len()
        ];
        // SAFETY: ctx is live; pcm16k holds n floats; ptrs holds n_words valid NUL-terminated
        // strings (kept alive by words_c); spans has room for n_words entries.
        let rc = unsafe {
            sys::qwen3_aligner_align(
                self.ctx,
                pcm16k.as_ptr(),
                n,
                ptrs.as_ptr(),
                n_words,
                LANGUAGE.as_ptr(),
                spans.as_mut_ptr(),
            )
        };
        if rc != 0 {
            bail!("qwen3_aligner_align failed ({rc}): {}", last_error());
        }
        check_no_fill_spans(&spans, words, n)?;
        Ok(words
            .iter()
            .zip(&spans)
            .map(|(w, s)| (w.clone(), s.start_s as f64, s.end_s as f64))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start_s: f32, end_s: f32) -> sys::qwen3_word_span {
        sys::qwen3_word_span { start_s, end_s }
    }

    #[test]
    fn fill_span_is_an_error() {
        let words: Vec<String> = ["Der", "Bedarf", "ist"].map(String::from).to_vec();
        let n = 40000; // 2.5 s
        let ok = [span(0.48, 0.56), span(0.56, 1.44), span(1.44, 2.5)];
        check_no_fill_spans(&ok, &words, n).unwrap();
        let filled = [span(0.48, 0.56), span(0.0, 2.5), span(0.0, 2.5)];
        let err = check_no_fill_spans(&filled, &words, n).unwrap_err();
        let fill = err.downcast_ref::<FillSpanError>().expect("typed fill-span error");
        assert_eq!(fill, &FillSpanError { word: 2, total: 3, text: "Bedarf".into(), duration: 2.5 });
        let msg = err.to_string();
        assert_eq!(msg, "aligner returned too few timestamps: word 2 of 3 (\"Bedarf\") got the (0, 2.5) fill span");
    }
}
