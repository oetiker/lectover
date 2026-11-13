//! RE-USE speech cleanup via reuse.cpp (`reuse.h`).

use std::ffi::CString;
use std::path::Path;

use anyhow::{bail, Result};
use lectover_sys as sys;

use super::backend::ggml_device_name;
use super::{c_path, c_str, require_file, Device};
use crate::audio::denoise::{check_clean_len, Denoise};

fn last_error() -> String {
    // SAFETY: reuse_last_error never returns NULL; the thread-local string is copied at once.
    c_str(unsafe { sys::reuse_last_error() })
}

/// Convert the RE-USE `model.safetensors` to an f32 GGUF at `gguf`.
pub fn convert_reuse(safetensors: &Path, gguf: &Path) -> Result<()> {
    require_file(safetensors, "RE-USE weights")?;
    let src = c_path(safetensors)?;
    let dst = c_path(gguf)?;
    // SAFETY: both paths are valid NUL-terminated strings for the duration of the call.
    let rc = unsafe { sys::reuse_convert(src.as_ptr(), dst.as_ptr()) };
    if rc != 0 {
        bail!(
            "reuse_convert failed ({}): {}",
            safetensors.display(),
            last_error()
        );
    }
    Ok(())
}

/// The progress hook of a denoise run, called with the fraction done.
type ProgressFn = Box<dyn FnMut(f32) + Send>;

/// RE-USE handle. The C side serialises nothing: calls on one handle must not overlap.
pub struct ReuseDenoiser {
    ctx: *mut sys::reuse_ctx,
    /// Progress closure the C side points at; boxed twice so its address stays stable.
    progress: Option<Box<ProgressFn>>,
}

unsafe extern "C" fn progress_trampoline(fraction: f32, user: *mut std::ffi::c_void) {
    // SAFETY: `user` is the `Box<Box<dyn FnMut>>` heap address kept alive in `progress`.
    let f = unsafe { &mut *(user as *mut ProgressFn) };
    // A panic must not unwind into C; a panicking hook only loses that update.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(fraction)));
}

// SAFETY: the handle may move between threads; `&mut self` serialises every call.
unsafe impl Send for ReuseDenoiser {}

impl Drop for ReuseDenoiser {
    fn drop(&mut self) {
        // SAFETY: ctx came from reuse_load and is freed exactly once.
        unsafe { sys::reuse_free(self.ctx) };
    }
}

impl ReuseDenoiser {
    /// Report progress (fraction in [0, 1], non-decreasing, ends at 1.0) on the calling
    /// thread during `denoise`. `None` removes the hook.
    pub fn set_progress(&mut self, f: Option<ProgressFn>) {
        match f {
            Some(f) => {
                let mut boxed = Box::new(f);
                let user = &mut *boxed as *mut ProgressFn as *mut std::ffi::c_void;
                // SAFETY: ctx is live; `boxed` is stored below and outlives the registration.
                unsafe { sys::reuse_set_progress(self.ctx, Some(progress_trampoline), user) };
                self.progress = Some(boxed);
            }
            None => {
                // SAFETY: ctx is live; NULL fn clears the hook.
                unsafe { sys::reuse_set_progress(self.ctx, None, std::ptr::null_mut()) };
                self.progress = None;
            }
        }
    }

    pub fn load(model: &Path, device: Device) -> Result<Self> {
        require_file(model, "RE-USE model")?;
        let path = c_path(model)?;
        let dev = CString::new(ggml_device_name(device)?)?;
        // reuse.cpp refuses n_threads < 1.
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .min(i32::MAX as usize) as i32;
        // SAFETY: both strings are valid NUL-terminated strings for the duration of the call.
        let ctx = unsafe { sys::reuse_load(path.as_ptr(), dev.as_ptr(), threads) };
        if ctx.is_null() {
            bail!("reuse_load failed ({}): {}", model.display(), last_error());
        }
        Ok(ReuseDenoiser {
            ctx,
            progress: None,
        })
    }
}

impl Denoise for ReuseDenoiser {
    fn denoise(&mut self, pcm: &[f32], sr: u32) -> Result<Vec<f32>> {
        check_clean_len(pcm.len(), sr)?;
        let rate =
            i32::try_from(sr).map_err(|_| anyhow::anyhow!("sample rate {sr} out of range"))?;
        let mut out = vec![0f32; pcm.len()];
        // SAFETY: ctx is live; `pcm` and `out` both hold pcm.len() floats and do not alias.
        let rc = unsafe {
            sys::reuse_enhance(self.ctx, pcm.as_ptr(), pcm.len(), rate, out.as_mut_ptr())
        };
        if rc != 0 {
            bail!("reuse_enhance failed (code {rc}): {}", last_error());
        }
        finish_output(out)
    }
}

/// Reject non-finite samples (NaN would silently become 0 as i16), then clamp to [-1, 1].
fn finish_output(mut out: Vec<f32>) -> Result<Vec<f32>> {
    if out.iter().any(|s| !s.is_finite()) {
        bail!("reuse_enhance returned non-finite output");
    }
    for s in &mut out {
        *s = s.clamp(-1.0, 1.0);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::finish_output;

    #[test]
    fn finish_output_clamps_finite_samples() {
        assert_eq!(
            finish_output(vec![0.5, 2.0, -3.0]).unwrap(),
            vec![0.5, 1.0, -1.0]
        );
    }

    #[test]
    fn finish_output_rejects_non_finite() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let err = finish_output(vec![0.0, bad]).unwrap_err().to_string();
            assert!(err.contains("non-finite output"), "{err}");
        }
    }
}
