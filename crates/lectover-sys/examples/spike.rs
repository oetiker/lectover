//! Phase 0 spike (throwaway): proves qwentts.cpp and qwen3-asr.cpp run on our one ggml.
//!
//! Raw FFI only. Synthesises the first three chunks of the frohburg golden with ICL cloning
//! from tests/fixtures/voices/erzaehler.wav, re-synthesises chunk 1 to check seed determinism, transcribes
//! every take with Qwen3-ASR-0.6B and force-aligns it with Qwen3-ForcedAligner-0.6B.
//!
//! Run: cargo run -p lectover-sys --release --features metal --example spike
//! Env: LECTOVER_MODELS (default ~/.cache/lectover/models), SPIKE_OUT (default out/spike).

use std::ffi::{c_char, CStr, CString};
use std::path::{Path, PathBuf};
use std::time::Instant;

use lectover_sys as sys;

const TALKER: &str = "qwen-talker-1.7b-base-BF16.gguf";
const CODEC: &str = "qwen-tokenizer-12hz-F32.gguf";
const ASR: &str = "qwen3-asr-0.6b-f16.gguf";
const ALIGNER: &str = "qwen3-forced-aligner-0.6b-f16.gguf";
const SEED: i64 = 42;
const TEMPERATURE: f32 = 0.9;
const LANG: &str = "german";
/// read_qwen.py's max_tokens buckets: the renderer passes the bucket, not the raw budget.
const BUCKETS: [i32; 4] = [100, 250, 500, 1000];

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let models = std::env::var("LECTOVER_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap()).join(".cache/lectover/models")
        });
    let out = std::env::var("SPIKE_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join("out/spike"));
    std::fs::create_dir_all(&out).unwrap();

    let golden: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("tests/fixtures/goldens/frohburg-spiel-satz-patt.json"))
            .unwrap(),
    )
    .unwrap();
    struct Chunk {
        text: String,
        max_tokens: i32,
        words: Vec<String>,
    }
    let chunks: Vec<Chunk> = golden["chunks"].as_array().unwrap()[..3]
        .iter()
        .map(|c| Chunk {
            text: c["text"].as_str().unwrap().to_owned(),
            max_tokens: c["max_tokens"].as_i64().unwrap() as i32,
            words: c["spoken_words"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| w.as_str().unwrap().to_owned())
                .collect(),
        })
        .collect();

    let (ref_audio, ref_sr) = read_wav_mono(&root.join("tests/fixtures/voices/erzaehler.wav"));
    assert_eq!(ref_sr, 24000, "reference must be 24 kHz");
    let ref_text = std::fs::read_to_string(root.join("tests/fixtures/voices/erzaehler.txt"))
        .unwrap()
        .trim()
        .to_owned();
    println!(
        "ref: {} samples ({:.2} s), text: {ref_text:?}",
        ref_audio.len(),
        ref_audio.len() as f32 / 24000.0
    );

    // ---------------------------------------------------------------- TTS
    let mut takes: Vec<(String, Vec<f32>)> = Vec::new(); // (label, 24 kHz samples)
    unsafe {
        println!("qt_version: {}", cstr(sys::qt_version()));
        let talker = cpath(&models.join(TALKER));
        let codec = cpath(&models.join(CODEC));
        let mut ip: sys::qt_init_params = std::mem::zeroed();
        sys::qt_init_default_params(&mut ip);
        ip.talker_path = talker.as_ptr();
        ip.codec_path = codec.as_ptr();
        println!(
            "qt_init_params defaults: use_fa={} clamp_fp16={} max_batch={} codec_chunk_sec={} device=NULL",
            ip.use_fa, ip.clamp_fp16, ip.max_batch, ip.codec_chunk_sec
        );
        let t = Instant::now();
        let q = sys::qt_init(&ip);
        assert!(
            !q.is_null(),
            "qt_init failed: {}",
            cstr(sys::qt_last_error())
        );
        println!(
            "qt_init: {:.2} s, model_type={}",
            t.elapsed().as_secs_f32(),
            cstr(sys::qt_model_type(q))
        );
        let langs: Vec<String> = (0..sys::qt_n_languages(q))
            .map(|i| cstr(sys::qt_language_name(q, i)))
            .collect();
        println!("languages: {langs:?}");

        let lang = CString::new(LANG).unwrap();
        let rtext = CString::new(ref_text.clone()).unwrap();
        let synth = |label: &str, text: &str, max_tokens: i32| -> Vec<f32> {
            let ctext = CString::new(text).unwrap();
            let mut p: sys::qt_tts_params = std::mem::zeroed();
            sys::qt_tts_default_params(&mut p);
            if label == "chunk1" {
                println!(
                    "qt_tts_params defaults: seed={} max_new_tokens={} temperature={} top_k={} top_p={} \
                     repetition_penalty={} subtalker_temperature={} subtalker_top_k={} subtalker_top_p={}",
                    p.seed,
                    p.max_new_tokens,
                    p.temperature,
                    p.top_k,
                    p.top_p,
                    p.repetition_penalty,
                    p.subtalker_temperature,
                    p.subtalker_top_k,
                    p.subtalker_top_p
                );
            }
            p.text = ctext.as_ptr();
            p.lang = lang.as_ptr();
            p.ref_audio_24k = ref_audio.as_ptr();
            p.ref_n_samples = ref_audio.len() as i32;
            p.ref_text = rtext.as_ptr();
            p.seed = SEED;
            p.temperature = TEMPERATURE;
            p.max_new_tokens = BUCKETS
                .iter()
                .copied()
                .find(|&b| b >= max_tokens)
                .unwrap_or(1000);
            let mut a: sys::qt_audio = std::mem::zeroed();
            let t = Instant::now();
            let st = sys::qt_synthesize(q, &p, &mut a);
            let dt = t.elapsed().as_secs_f32();
            assert_eq!(
                st,
                sys::QT_STATUS_OK,
                "qt_synthesize({label}) = {st}: {}",
                cstr(sys::qt_last_error())
            );
            assert_eq!((a.sample_rate, a.channels), (24000, 1));
            let s = std::slice::from_raw_parts(a.samples, a.n_samples as usize).to_vec();
            sys::qt_audio_free(&mut a);
            let dur = s.len() as f32 / 24000.0;
            println!(
                "{label}: max_new_tokens={} frames={} audio={dur:.2} s synth={dt:.2} s RTF={:.3} peak={:.3}",
                p.max_new_tokens,
                s.len() / 1920,
                dt / dur,
                s.iter().fold(0f32, |m, x| m.max(x.abs()))
            );
            s
        };
        for (i, c) in chunks.iter().enumerate() {
            let label = format!("chunk{}", i + 1);
            let s = synth(&label, &c.text, c.max_tokens);
            write_wav16(&out.join(format!("{label}.wav")), &s, 24000);
            takes.push((label, s));
        }
        // Determinism: chunk 1 again, same seed, after two other synth calls on the same handle.
        let again = synth("chunk1-repeat", &chunks[0].text, chunks[0].max_tokens);
        write_wav16(&out.join("chunk1-repeat.wav"), &again, 24000);
        let first = &takes[0].1;
        let same_len = first.len() == again.len();
        let n = first.len().min(again.len());
        let diffs = (0..n)
            .filter(|&i| first[i].to_bits() != again[i].to_bits())
            .count();
        let maxd = (0..n)
            .map(|i| (first[i] - again[i]).abs())
            .fold(0f32, f32::max);
        println!(
            "determinism: len {} vs {} (same={same_len}), differing samples={diffs}/{n}, max |diff|={maxd:e} => identical={}",
            first.len(),
            again.len(),
            same_len && diffs == 0
        );
        takes.push(("chunk1-repeat".into(), again));
        sys::qt_free(q);
    }

    let takes16: Vec<(String, Vec<f32>)> = takes
        .iter()
        .map(|(l, s)| (l.clone(), resample_24k_to_16k(s)))
        .collect();

    // ---------------------------------------------------------------- ASR
    unsafe {
        let p = cpath(&models.join(ASR));
        let t = Instant::now();
        let ctx = sys::qwen3_asr_init(p.as_ptr(), 0);
        assert!(
            !ctx.is_null(),
            "qwen3_asr_init: {}",
            cstr(sys::qwen3_asr_last_error())
        );
        println!("asr init: {:.2} s", t.elapsed().as_secs_f32());
        for (label, s) in &takes16 {
            let ref_text = &chunks[if label.starts_with("chunk1") {
                0
            } else {
                label[5..6].parse::<usize>().unwrap() - 1
            }]
            .text;
            let t = Instant::now();
            let r = sys::qwen3_asr_transcribe(ctx, s.as_ptr(), s.len() as i32, 0);
            let dt = t.elapsed().as_secs_f32();
            assert!(
                !r.is_null(),
                "transcribe({label}): {}",
                cstr(sys::qwen3_asr_last_error())
            );
            let text = cstr(r);
            sys::qwen3_asr_string_free(r);
            let (errs, nref) = word_errors(ref_text, &text);
            println!(
                "asr {label} ({dt:.2} s): WER {errs}/{nref} = {:.1}%",
                100.0 * errs as f32 / nref as f32
            );
            println!("  chunk: {ref_text}");
            println!("  asr:   {text}");
        }
        sys::qwen3_asr_free(ctx);
    }

    // ---------------------------------------------------------------- aligner
    unsafe {
        let p = cpath(&models.join(ALIGNER));
        let t = Instant::now();
        let ctx = sys::qwen3_aligner_init(p.as_ptr(), 0);
        assert!(
            !ctx.is_null(),
            "qwen3_aligner_init: {}",
            cstr(sys::qwen3_asr_last_error())
        );
        println!("aligner init: {:.2} s", t.elapsed().as_secs_f32());
        let lang = CString::new("German").unwrap();
        let align = |label: &str, s: &[f32], words: &[String]| -> Vec<sys::qwen3_word_span> {
            let cw: Vec<CString> = words
                .iter()
                .map(|w| CString::new(w.as_str()).unwrap())
                .collect();
            let ptrs: Vec<*const c_char> = cw.iter().map(|c| c.as_ptr()).collect();
            let mut spans = vec![
                sys::qwen3_word_span {
                    start_s: -1.0,
                    end_s: -1.0
                };
                words.len()
            ];
            let t = Instant::now();
            let rc = sys::qwen3_aligner_align(
                ctx,
                s.as_ptr(),
                s.len() as i32,
                ptrs.as_ptr(),
                words.len() as i32,
                lang.as_ptr(),
                spans.as_mut_ptr(),
            );
            assert_eq!(
                rc,
                0,
                "align({label}) = {rc}: {}",
                cstr(sys::qwen3_asr_last_error())
            );
            let dur = s.len() as f32 / 16000.0;
            // The library fills (0, duration) when it got fewer than 2*words timestamps.
            let filled = spans
                .iter()
                .skip(1)
                .filter(|sp| sp.start_s == 0.0 && (sp.end_s - dur).abs() < 1e-3)
                .count();
            let nonmono = spans
                .windows(2)
                .filter(|w| w[1].start_s < w[0].start_s)
                .count();
            let inverted = spans.iter().filter(|sp| sp.end_s < sp.start_s).count();
            println!(
                "align {label} ({:.2} s): words={} spans={} audio={dur:.2} s last_end={:.2} \
                 fill-fallback spans={filled} non-monotonic starts={nonmono} end<start={inverted}",
                t.elapsed().as_secs_f32(),
                words.len(),
                spans.len(),
                spans.last().map(|s| s.end_s).unwrap_or(0.0)
            );
            spans
        };
        for (label, s) in &takes16 {
            let ci = if label.starts_with("chunk1") {
                0
            } else {
                label[5..6].parse::<usize>().unwrap() - 1
            };
            let spans = align(label, s, &chunks[ci].words);
            if label == "chunk1" {
                for (w, sp) in chunks[0].words.iter().zip(&spans) {
                    println!("    {:>8.3} {:>8.3}  {w}", sp.start_s, sp.end_s);
                }
            }
        }
        // Raw text: the library's own split (ForcedAligner::split_words for non-CJK languages)
        // is ASCII whitespace only, punctuation kept. Emulated here, as the C API takes a list.
        let raw: Vec<String> = chunks[0]
            .text
            .split([' ', '\t', '\n', '\r'])
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect();
        println!(
            "raw-text split of chunk 1: {} words vs golden {}",
            raw.len(),
            chunks[0].words.len()
        );
        let spans = align("chunk1-rawsplit", &takes16[0].1, &raw);
        for (w, sp) in raw.iter().zip(&spans) {
            println!("    {:>8.3} {:>8.3}  {w}", sp.start_s, sp.end_s);
        }
        sys::qwen3_aligner_free(ctx);
    }
}

fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        return "<NULL>".into();
    }
    // SAFETY: the libraries return NUL-terminated strings.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

fn cpath(p: &Path) -> CString {
    assert!(p.exists(), "missing model file {}", p.display());
    CString::new(p.to_str().unwrap()).unwrap()
}

/// Minimal RIFF reader: 16-bit PCM, mixed down to mono.
fn read_wav_mono(path: &Path) -> (Vec<f32>, u32) {
    let b = std::fs::read(path).unwrap();
    assert_eq!(&b[0..4], b"RIFF");
    let (mut pos, mut ch, mut sr, mut bits, mut data) = (12usize, 0u16, 0u32, 0u16, None);
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &b[pos + 8..(pos + 8 + len).min(b.len())];
        if id == b"fmt " {
            ch = u16::from_le_bytes(body[2..4].try_into().unwrap());
            sr = u32::from_le_bytes(body[4..8].try_into().unwrap());
            bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
        } else if id == b"data" {
            data = Some(body);
        }
        pos += 8 + len + (len & 1);
    }
    assert_eq!(bits, 16, "only 16-bit PCM");
    let data = data.unwrap();
    let s: Vec<f32> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
        .collect();
    let ch = ch as usize;
    (
        s.chunks_exact(ch)
            .map(|f| f.iter().sum::<f32>() / ch as f32)
            .collect(),
        sr,
    )
}

fn write_wav16(path: &Path, s: &[f32], sr: u32) {
    let n = s.len() as u32 * 2;
    let mut b = Vec::with_capacity(44 + n as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + n).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&sr.to_le_bytes());
    b.extend_from_slice(&(sr * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&n.to_le_bytes());
    for x in s {
        b.extend_from_slice(&((x.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

/// 24 kHz -> 16 kHz with a Blackman-windowed sinc (cutoff 0.95 x 8 kHz, 64 taps).
fn resample_24k_to_16k(x: &[f32]) -> Vec<f32> {
    let fc = 0.95 * 8000.0 / 24000.0; // cycles per input sample
    let half = 32isize;
    let n_out = x.len() * 2 / 3;
    (0..n_out)
        .map(|n| {
            let t = n as f64 * 1.5;
            let c = t.floor() as isize;
            let mut acc = 0.0f64;
            for k in (c - half + 1)..=(c + half) {
                if k < 0 || k as usize >= x.len() {
                    continue;
                }
                let u = t - k as f64;
                let sinc = if u == 0.0 {
                    1.0
                } else {
                    (2.0 * std::f64::consts::PI * fc * u).sin()
                        / (std::f64::consts::PI * u)
                        / (2.0 * fc)
                };
                let wpos = (u / half as f64 + 1.0) / 2.0; // 0..1 across the window
                let w = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * wpos).cos()
                    + 0.08 * (4.0 * std::f64::consts::PI * wpos).cos();
                acc += x[k as usize] as f64 * 2.0 * fc * sinc * w;
            }
            acc as f32
        })
        .collect()
}

/// Word-level Levenshtein distance after lowercasing and stripping punctuation.
fn word_errors(reference: &str, hyp: &str) -> (usize, usize) {
    let norm = |s: &str| -> Vec<String> {
        s.split_whitespace()
            .map(|w| {
                w.chars()
                    .filter(|c| c.is_alphanumeric())
                    .collect::<String>()
                    .to_lowercase()
            })
            .filter(|w| !w.is_empty())
            .collect()
    };
    let (r, h) = (norm(reference), norm(hyp));
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    for i in 1..=r.len() {
        let mut cur = vec![i; h.len() + 1];
        for j in 1..=h.len() {
            cur[j] = (prev[j] + 1)
                .min(cur[j - 1] + 1)
                .min(prev[j - 1] + usize::from(r[i - 1] != h[j - 1]));
        }
        prev = cur;
    }
    (prev[h.len()], r.len())
}
