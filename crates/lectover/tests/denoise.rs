//! Speech cleanup (Task 13). The `#[ignore]`d tests need the RE-USE GGUF in `LECTOVER_MODELS` and
//! `noisy.wav` (a noisy voice recording) in the directory `LECTOVER_TEST_VOICES`:
//!
//! `LECTOVER_MODELS=~/.cache/lectover/models LECTOVER_TEST_VOICES=<dir with noisy.wav> cargo test --release --features metal -p lectover --test denoise -- --ignored --test-threads 1`

use std::f64::consts::PI;
use std::path::PathBuf;

use lectover::audio::denoise::{check_clean_len, noise_floor_db, Denoise, MAX_CLEAN_SECS};
use lectover::audio::read_wav;
use lectover::ffi::{resolve_device, ReuseDenoiser};
use lectover::models::REUSE_GGUF;

#[test]
fn noise_floor_of_tone_plus_silence() {
    let sr = 24000u32;
    let mut wav: Vec<f32> = (0..sr)
        .map(|i| (0.5 * (2.0 * PI * 440.0 * i as f64 / sr as f64).sin()) as f32)
        .collect();
    // Uniform noise with an RMS of 1.4e-3 (-48 dB below the tone's RMS of 0.354) (deterministic LCG).
    let mut state = 12345u64;
    let peak = 3f64.sqrt() * 1.4e-3;
    for _ in 0..sr {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let u = (state >> 11) as f64 / (1u64 << 53) as f64;
        wav.push(((2.0 * u - 1.0) * peak) as f32);
    }
    let floor = noise_floor_db(&wav, sr);
    assert!((floor - -48.0).abs() <= 3.0, "floor {floor} dB");
}

#[test]
fn overlong_clip_is_refused() {
    let sr = 24000u32;
    let n = (MAX_CLEAN_SECS * sr as f64) as usize + 1;
    let err = check_clean_len(n, sr).unwrap_err().to_string();
    assert!(err.contains("cleaning is limited to 90 s"), "{err}");
    assert!(err.contains("clip is 90.0 s long"), "{err}");
    check_clean_len(n - 1, sr).unwrap();
}

fn voices() -> Option<PathBuf> {
    let dir = std::env::var_os("LECTOVER_TEST_VOICES").map(PathBuf::from);
    if dir.is_none() {
        eprintln!("skipped: LECTOVER_TEST_VOICES is unset (the directory must hold noisy.wav)");
    }
    dir
}

fn denoiser() -> ReuseDenoiser {
    let dir = std::env::var_os("LECTOVER_MODELS").unwrap_or_else(|| {
        panic!("LECTOVER_MODELS is unset: point it at the directory holding {REUSE_GGUF}")
    });
    ReuseDenoiser::load(
        &PathBuf::from(dir).join(REUSE_GGUF),
        resolve_device(None).unwrap(),
    )
    .unwrap()
}

#[test]
#[ignore = "needs the RE-USE GGUF (LECTOVER_MODELS) and noisy.wav (a noisy voice recording) in LECTOVER_TEST_VOICES"]
fn cleaned_output_is_clamped() {
    let Some(dir) = voices() else { return };
    let (wav, sr) = read_wav(&dir.join("noisy.wav")).unwrap();
    let loud: Vec<f32> = wav.iter().map(|s| s * 4.0).collect();
    let mut d = denoiser();
    let out = d.denoise(&loud, sr).unwrap();
    drop(d);
    assert_eq!(out.len(), loud.len());
    assert!(out.iter().all(|s| (-1.0..=1.0).contains(s)));
}

#[test]
#[ignore = "needs the RE-USE GGUF (LECTOVER_MODELS) and noisy.wav (a noisy voice recording) in LECTOVER_TEST_VOICES"]
fn reuse_cleans_twice() {
    let Some(dir) = voices() else { return };
    let mut d = denoiser();
    let (clip, sr) = read_wav(&dir.join("noisy.wav")).unwrap();
    let before = noise_floor_db(&clip, sr);
    let out = d.denoise(&clip, sr).unwrap();
    let after = noise_floor_db(&out, sr);
    eprintln!("noisy voice noise floor: {before:.1} dB -> {after:.1} dB (sr {sr})");
    assert_eq!(out.len(), clip.len());
    assert!(before - after >= 10.0, "floor {before} -> {after} dB");
    // second pass on the same loaded denoiser: the model state must not leak between calls
    let out2 = d.denoise(&clip, sr).unwrap();
    let after2 = noise_floor_db(&out2, sr);
    eprintln!("second pass noise floor: {before:.1} dB -> {after2:.1} dB (sr {sr})");
    assert_eq!(out2.len(), clip.len());
    assert!(
        before - after2 >= 10.0,
        "second pass floor {before} -> {after2} dB"
    );
    drop(d);
}
