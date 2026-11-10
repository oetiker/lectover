//! Safe model wrappers (Task 12). The `#[ignore]`d tests load the GGUF models from the directory
//! named by `LECTOVER_MODELS` (e.g. ~/.cache/lectover/models):
//!
//! `LECTOVER_MODELS=~/.cache/lectover/models cargo test -p lectover --features metal --test models -- --ignored`

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use lectover::audio::{read_wav, resample, trim_and_level, TRIM_KEEP_SECS};
use lectover::ffi::{
    backend_banner, check_ggml_layout, resolve_device, Align, Device, QwenAligner, QwenAsr, QwenTts, Synth, TakeRequest,
    Transcribe,
};
use lectover::models::{
    ensure_reuse_gguf, fetch_all, fetch_sources, CleanFiles, Download, REUSE_GGUF, REUSE_SAFETENSORS, REUSE_SOURCE,
};
use lectover::text::spoken_words::spoken_words;

const TALKER: &str = "qwen-talker-1.7b-base-BF16.gguf";
const CODEC: &str = "qwen-tokenizer-12hz-F32.gguf";
const ASR: &str = "qwen3-asr-0.6b-f16.gguf";
const ALIGNER: &str = "qwen3-forced-aligner-0.6b-f16.gguf";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn voice(name: &str) -> (PathBuf, String) {
    let wav = root().join(format!("tests/fixtures/voices/{name}.wav"));
    let text = std::fs::read_to_string(wav.with_extension("txt")).unwrap();
    (wav, text.trim().to_string())
}

fn model(file: &str) -> PathBuf {
    let dir = std::env::var_os("LECTOVER_MODELS").unwrap_or_else(|| {
        panic!("LECTOVER_MODELS is unset: point it at the directory holding {file} (e.g. ~/.cache/lectover/models)")
    });
    let path = PathBuf::from(dir).join(file);
    assert!(path.is_file(), "model file missing: {}", path.display());
    path
}

fn device() -> Device {
    resolve_device(None).unwrap()
}

/// One TTS handle shared by the tests running at the same time: the talker is 3.9 GB and takes
/// ~11 s to load. Each test sets the voice it needs. The static holds only a `Weak`, so the
/// handle is dropped when the last test using it ends: a ggml Metal device still holding
/// buffers at process exit aborts in its destructor (`GGML_ASSERT([rsets->data count] == 0)`).
fn tts() -> Arc<Mutex<QwenTts>> {
    static TTS: Mutex<Weak<Mutex<QwenTts>>> = Mutex::new(Weak::new());
    let mut slot = TTS.lock().unwrap();
    if let Some(tts) = slot.upgrade() {
        return tts;
    }
    let (wav, text) = voice("erzaehler");
    let tts = QwenTts::load(&model(TALKER), &model(CODEC), &wav, &text, device(), 1).unwrap();
    let tts = Arc::new(Mutex::new(tts));
    *slot = Arc::downgrade(&tts);
    tts
}

const SHORT: &str = "Der Handlungsbedarf ist real.";

#[test]
#[ignore = "needs model files (LECTOVER_MODELS)"]
fn tts_is_deterministic_per_seed() {
    let tts = tts();
    let mut tts = tts.lock().unwrap();
    let (wav, text) = voice("erzaehler");
    tts.set_voice(&wav, &text).unwrap();
    assert_eq!(tts.sample_rate(), 24000);
    let texts = vec![TakeRequest::new(SHORT, 200, 42)];
    let a = tts.generate(&texts, 0.9).unwrap();
    let b = tts.generate(&texts, 0.9).unwrap();
    assert_eq!(a.len(), 1);
    assert!(a[0].len() > 24000 / 2, "take too short: {} samples", a[0].len());
    assert_eq!(
        a, b,
        "same text, same seed, same handle must give identical samples"
    );
}

#[test]
#[ignore = "needs model files (LECTOVER_MODELS)"]
fn tts_two_voices_differ() {
    let tts = tts();
    let mut tts = tts.lock().unwrap();
    let texts = vec![TakeRequest::new(SHORT, 200, 42)];
    let (wav, text) = voice("erzaehler");
    tts.set_voice(&wav, &text).unwrap();
    let a = tts.generate(&texts, 0.9).unwrap();
    let (wav, text) = voice("tief");
    tts.set_voice(&wav, &text).unwrap();
    let b = tts.generate(&texts, 0.9).unwrap();
    assert_ne!(a, b, "erzaehler and tief must not give the same samples");
}

fn alnum_words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

#[test]
#[ignore = "needs model files (LECTOVER_MODELS)"]
fn asr_hears_reference_clip() {
    let (wav, text) = voice("erzaehler");
    let (pcm, sr) = read_wav(&wav).unwrap();
    let pcm16k = resample(&pcm, sr, 16000);
    let mut asr = QwenAsr::load(&model(ASR), device()).unwrap();
    let heard = asr.transcribe(&pcm16k).unwrap();
    let heard_words = alnum_words(&heard);
    let want = alnum_words(&text);
    let found = want.iter().filter(|w| heard_words.contains(w)).count();
    assert!(
        found * 100 >= want.len() * 80,
        "only {found}/{} reference words heard; transcript: {heard:?}",
        want.len()
    );
}

/// Port of tests/test_aligner_slow.py.
#[test]
#[ignore = "needs model files (LECTOVER_MODELS)"]
fn aligner_onset_and_order() {
    let text = "Die Städtischen Betriebe Olten sollen mit der Zofinger S-T-W-Z fusionieren.";
    let (wav, sr) = {
        let tts = tts();
        let mut tts = tts.lock().unwrap();
        let (ref_wav, ref_text) = voice("qwen-designed");
        tts.set_voice(&ref_wav, &ref_text).unwrap();
        let mut takes = tts.generate(&[TakeRequest::new(text, 200, 42)], 0.9).unwrap();
        (takes.remove(0), tts.sample_rate())
    };
    let wav = trim_and_level(&wav, sr);
    let pcm16k = resample(&wav, sr, 16000);
    let words = spoken_words(text);
    let mut aligner = QwenAligner::load(&model(ALIGNER), device()).unwrap();
    let items = aligner.align(&pcm16k, &words).unwrap();
    let got: Vec<&String> = items.iter().map(|(w, _, _)| w).collect();
    assert_eq!(got, words.iter().collect::<Vec<_>>());
    let starts: Vec<f64> = items.iter().map(|&(_, s, _)| s).collect();
    let mut sorted = starts.clone();
    sorted.sort_by(f64::total_cmp);
    assert_eq!(starts, sorted, "starts must be in order");
    assert!(
        (starts[0] - TRIM_KEEP_SECS).abs() < 0.15,
        "speech onset {} not near TRIM_KEEP_SECS",
        starts[0]
    );
    let dur = wav.len() as f64 / sr as f64;
    assert!(items.last().unwrap().2 <= dur + 0.05);
}

#[cfg(not(feature = "metal"))]
#[test]
fn metal_request_without_feature_fails() {
    let err = resolve_device(Some(Device::Metal)).unwrap_err().to_string();
    assert!(err.contains("--features metal"), "{err}");
}

#[test]
fn banner_names_device_and_metal_flag() {
    let banner = backend_banner(Device::Cpu);
    assert!(banner.contains("CPU"), "{banner}");
    let flag = if cfg!(feature = "metal") {
        "metal=on"
    } else {
        "metal=off"
    };
    assert!(banner.contains(flag), "{banner}");
}

#[test]
fn cpu_device_always_resolves() {
    assert_eq!(resolve_device(Some(Device::Cpu)).unwrap(), Device::Cpu);
}

#[cfg(feature = "metal")]
#[test]
fn default_device_is_metal_with_feature() {
    assert_eq!(resolve_device(None).unwrap(), Device::Metal);
    assert!(backend_banner(Device::Metal).contains("MTL0"));
}

/// Task 2 finding: the bindings are generated with GGML_MAX_NAME=128; the C side must agree.
#[test]
fn ggml_tensor_layout_matches_c_side() {
    check_ggml_layout().unwrap();
}

#[test]
fn fetch_includes_reuse_weights() {
    assert!(fetch_sources(false).contains(&REUSE_SOURCE));
    let dir = tempfile::tempdir().unwrap();
    let urls = std::cell::RefCell::new(Vec::new());
    let get = |url: &str, _offset: u64| -> anyhow::Result<Download> {
        urls.borrow_mut().push(url.to_string());
        let n = REUSE_SOURCE.size;
        Ok(Download { status: 200, content_length: Some(n), body: Box::new(std::io::repeat(0).take(n)) })
    };
    fetch_all(dir.path(), &[REUSE_SOURCE], &get, &mut Vec::new()).unwrap();
    assert_eq!(
        *urls.borrow(),
        ["https://huggingface.co/nvidia/NVIDIA-NemotronLabs-RE-USE/resolve/31f82f2efad1095034492f236e8dac9239ba713b/model.safetensors"]
    );
    assert!(dir.path().join(REUSE_SAFETENSORS).is_file());
}

#[test]
fn gguf_is_converted_once() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(REUSE_SAFETENSORS), b"w").unwrap();
    let calls = std::cell::Cell::new(0);
    let convert = |_: &Path, out: &Path| -> anyhow::Result<()> {
        calls.set(calls.get() + 1);
        std::fs::write(out, b"gguf")?;
        Ok(())
    };
    let a = ensure_reuse_gguf(dir.path(), &convert, &mut Vec::new()).unwrap();
    let b = ensure_reuse_gguf(dir.path(), &convert, &mut Vec::new()).unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(a, b);
    assert_eq!(a, dir.path().join(REUSE_GGUF));
    assert!(a.is_file());
}

#[test]
fn failed_conversion_leaves_no_gguf() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(REUSE_SAFETENSORS), b"w").unwrap();
    let convert = |_: &Path, out: &Path| -> anyhow::Result<()> {
        std::fs::write(out, b"half")?;
        anyhow::bail!("boom")
    };
    let err = ensure_reuse_gguf(dir.path(), &convert, &mut Vec::new()).unwrap_err();
    assert!(format!("{err:#}").contains("boom"));
    assert!(!dir.path().join(REUSE_GGUF).exists());
}

#[test]
fn clean_files_missing_names_fetch() {
    let dir = tempfile::tempdir().unwrap();
    let err = CleanFiles::check(dir.path()).unwrap_err().to_string();
    assert!(err.contains("lectover models fetch"), "{err}");
}

/// Aligner timing on the committed reference clips; with `LECTOVER_ALIGN_DUMP=<file>` it also
/// writes every word span, so two builds can be compared span by span.
/// `cargo test --release --features metal -p lectover --test models aligner_bench -- --ignored --nocapture`
#[test]
#[ignore = "timing; needs model files (LECTOVER_MODELS)"]
fn aligner_bench() {
    // qwen-designed.wav is byte-identical to erzaehler.wav, so it is left out.
    let clips = ["erzaehler", "lebhaft", "tief", "trocken"];
    let inputs: Vec<(&str, Vec<f32>, Vec<String>)> = clips
        .iter()
        .map(|&name| {
            let (wav, text) = voice(name);
            let (pcm, sr) = read_wav(&wav).unwrap();
            (name, resample(&pcm, sr, 16000), spoken_words(&text))
        })
        .collect();
    let audio_secs: f64 = inputs.iter().map(|(_, p, _)| p.len() as f64 / 16000.0).sum();
    let mut aligner = QwenAligner::load(&model(ALIGNER), device()).unwrap();
    let mut dump = String::new();
    for round in 0..3 {
        let t = std::time::Instant::now();
        for (name, pcm, words) in &inputs {
            let items = aligner.align(pcm, words).unwrap();
            if round == 0 {
                for (w, s, e) in items {
                    dump += &format!("{name}\t{w}\t{s:.3}\t{e:.3}\n");
                }
            }
        }
        let secs = t.elapsed().as_secs_f64();
        eprintln!("round {round}: {secs:.2} s for {audio_secs:.1} s audio ({:.1}x real time)", audio_secs / secs);
    }
    if let Some(path) = std::env::var_os("LECTOVER_ALIGN_DUMP") {
        std::fs::write(path, dump).unwrap();
    }
}
