//! ggml, qwentts.cpp and qwen3-asr.cpp link into one binary, and ggml's backend registry is
//! populated. No model files are needed.

use std::ffi::CStr;

use lectover_sys as sys;

fn registry_names() -> Vec<String> {
    // SAFETY: plain registry queries; the returned names are static C strings owned by ggml.
    unsafe {
        (0..sys::ggml_backend_reg_count())
            .map(|i| {
                let reg = sys::ggml_backend_reg_get(i);
                CStr::from_ptr(sys::ggml_backend_reg_name(reg)).to_string_lossy().into_owned()
            })
            .collect()
    }
}

#[test]
fn ggml_cpu_backend_initialises() {
    // SAFETY: ggml_backend_cpu_init has no preconditions; the handle is freed once below.
    unsafe {
        let backend = sys::ggml_backend_cpu_init();
        assert!(!backend.is_null(), "ggml_backend_cpu_init returned NULL");
        sys::ggml_backend_free(backend);
    }
}

#[test]
fn qwentts_symbols_link() {
    let init = sys::qt_init as *const ();
    let free = sys::qt_free as *const ();
    assert!(!init.is_null() && !free.is_null());
    // SAFETY: qt_version returns a static NUL-terminated string.
    let version = unsafe { CStr::from_ptr(sys::qt_version()) }.to_string_lossy();
    assert!(!version.is_empty(), "qt_version returned an empty string");
}

#[test]
fn aligner_symbols_link() {
    let fns = [
        sys::qwen3_asr_init as *const (),
        sys::qwen3_asr_transcribe as *const (),
        sys::qwen3_aligner_init as *const (),
        sys::qwen3_aligner_align as *const (),
    ];
    assert!(fns.iter().all(|f| !f.is_null()));
}

#[test]
fn aligner_init_reports_missing_model() {
    let path = c"/nonexistent/lectover-test.gguf";
    // SAFETY: valid NUL-terminated path; a NULL result is the expected failure, and
    // qwen3_asr_last_error returns a NUL-terminated thread-local string.
    unsafe {
        let ctx = sys::qwen3_aligner_init(path.as_ptr(), 1);
        assert!(ctx.is_null(), "init on a missing file must fail");
        let err = CStr::from_ptr(sys::qwen3_asr_last_error()).to_string_lossy();
        assert!(!err.is_empty(), "missing model must leave an error message");
    }
}

#[test]
fn reuse_symbols_link() {
    // SAFETY: reuse_last_error returns a NUL-terminated thread-local string; the path is a
    // valid NUL-terminated string and a NULL result is the expected failure.
    unsafe {
        let err = sys::reuse_last_error();
        assert!(!err.is_null());
        let _ = CStr::from_ptr(err).to_string_lossy();
        let ctx = sys::reuse_load(c"/nonexistent.gguf".as_ptr(), std::ptr::null(), 1);
        assert!(ctx.is_null(), "reuse_load on a missing file must fail");
    }
}

#[test]
fn cpu_backend_is_registered() {
    let names = registry_names();
    assert!(names.iter().any(|n| n == "CPU"), "registry: {names:?}");
}

#[cfg(feature = "metal")]
#[test]
fn metal_backend_registered() {
    // ggml names the Metal registry "MTL" (devices "MTL0", ...).
    let names = registry_names();
    assert!(names.iter().any(|n| n == "MTL" || n == "Metal"), "registry: {names:?}");
}
