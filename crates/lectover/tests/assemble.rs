//! Port of tests/test_longform.py (assembly) plus the DSP checks from the Task 11 brief.

use std::f64::consts::PI;

use lectover::audio::assemble::{apply_trim, trim_and_level_with};
use lectover::audio::{
    assemble_parts, resample, room_tone, stretch, trim_and_level, ROOM_TONE_DB, START_LEAD_SECS,
    TRIM_KEEP_SECS, TRIM_KEEP_TAIL_SECS,
};
use lectover::plan::{ChunkKind, TITLE_LEAD_SECS};

fn sine(sr: u32, secs: f64, freq: f64, amp: f64) -> Vec<f32> {
    (0..(sr as f64 * secs) as usize)
        .map(|i| (amp * (2.0 * PI * freq * i as f64 / sr as f64).sin()) as f32)
        .collect()
}

/// 0.2 s silence + tone + 0.2 s silence, like the Python `_take`.
fn take(sr: u32, secs: f64, kind: ChunkKind) -> (Vec<f32>, String, ChunkKind) {
    let pad = vec![0.0f32; (sr as f64 * 0.2) as usize];
    let mut wav = pad.clone();
    wav.extend(sine(sr, secs, 220.0, 0.3));
    wav.extend(pad);
    (wav, "ein Satz mit Silben".to_string(), kind)
}

fn rms_db(x: &[f32]) -> f64 {
    let ms = x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / x.len() as f64;
    20.0 * (ms.sqrt() + 1e-9).log10()
}

/// Frequency (150..300 Hz, 0.05 Hz steps) with the largest DFT magnitude.
fn dominant_freq(x: &[f32], sr: u32) -> f64 {
    let mut best = (0.0, 0.0);
    let mut f = 150.0;
    while f <= 300.0 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, &v) in x.iter().enumerate() {
            let ph = 2.0 * PI * f * i as f64 / sr as f64;
            re += v as f64 * ph.cos();
            im += v as f64 * ph.sin();
        }
        let mag = re * re + im * im;
        if mag > best.1 {
            best = (f, mag);
        }
        f += 0.05;
    }
    best.0
}

#[test]
fn assemble_parts_offsets_match_layout() {
    let sr = 24000;
    let takes = [
        take(sr, 1.0, ChunkKind::Sentence),
        take(sr, 0.5, ChunkKind::Paragraph),
        take(sr, 0.7, ChunkKind::Title),
    ];
    let (audio, parts) = assemble_parts(&takes, sr, false);
    let tol = 1.0 / sr as f64;
    // the file opens with a lead, so an output that wakes up first (Bluetooth) loses no speech
    assert!(
        (parts[0].0 - START_LEAD_SECS).abs() <= tol,
        "{}",
        parts[0].0
    );
    let lead = (START_LEAD_SECS * sr as f64) as usize;
    assert!(
        rms_db(&audio[..lead]) < ROOM_TONE_DB + 3.0,
        "lead is room tone only"
    );
    let p1 = parts[0].0 + parts[0].1.len() as f64 / sr as f64 + ChunkKind::Sentence.pause_secs();
    assert!((parts[1].0 - p1).abs() <= tol, "{} vs {p1}", parts[1].0);
    // a heading gets the longer lead-in pause, not the paragraph pause
    let p2 = parts[1].0 + parts[1].1.len() as f64 / sr as f64 + TITLE_LEAD_SECS;
    assert!((parts[2].0 - p2).abs() <= tol, "{} vs {p2}", parts[2].0);
    assert_eq!(
        audio.len(),
        (parts[2].0 * sr as f64).round() as usize + parts[2].1.len()
    );
}

#[test]
fn assemble_audio_is_deterministic() {
    // Python's `assemble` is `assemble_parts(...)[0]`; the Rust port has only assemble_parts,
    // so the equivalent guarantee is that the audio is reproducible bit for bit.
    let sr = 24000;
    let takes = [
        take(sr, 1.0, ChunkKind::Sentence),
        take(sr, 0.5, ChunkKind::Paragraph),
    ];
    assert_eq!(
        assemble_parts(&takes, sr, false).0,
        assemble_parts(&takes, sr, false).0
    );
}

#[test]
fn trim_and_level_trims_levels_and_fades() {
    let sr = 24000;
    let (wav, _, _) = take(sr, 1.0, ChunkKind::Sentence);
    let out = trim_and_level(&wav, sr);
    // 1 s tone, 0.1 s kept before and 0.2 s after (the whole pad; frame aligned)
    let expect = sr as usize + (sr as f64 * (TRIM_KEEP_SECS + TRIM_KEEP_TAIL_SECS)) as usize;
    assert!(
        (out.len() as i64 - expect as i64).abs() <= 240,
        "len {}",
        out.len()
    );
    // loud part levelled to -20 dB
    let loud: Vec<f32> = out.iter().copied().filter(|v| v.abs() > 0.01).collect();
    assert!((rms_db(&loud) - -20.0).abs() < 0.5, "{}", rms_db(&loud));
    assert_eq!(out[0], 0.0);
    assert!(out[out.len() - 1].abs() < 1e-6);
    assert!(out.iter().all(|v| v.abs() <= 1.0));
}

#[test]
fn apply_trim_reproduces_trim_and_level() {
    let sr = 24000;
    let (x, _, _) = take(sr, 1.0, ChunkKind::Sentence);
    let (out, t) = trim_and_level_with(&x, sr);
    assert_eq!(out, trim_and_level(&x, sr));
    assert_eq!(apply_trim(&x, sr, &t), out);
}

#[test]
fn apply_trim_to_other_signal_keeps_length() {
    let sr = 24000;
    let (x, _, _) = take(sr, 1.0, ChunkKind::Sentence);
    let y: Vec<f32> = x.iter().map(|v| v * 0.5 + 0.01).collect();
    let (_, t) = trim_and_level_with(&x, sr);
    assert_eq!(apply_trim(&y, sr, &t).len(), t.end - t.start);
}

#[test]
fn stretch_changes_duration_by_tempo() {
    let sr = 24000;
    let wav = sine(sr, 1.0, 220.0, 0.3);
    let out = stretch(&wav, sr, 1.10);
    let want = 24000.0 / 1.10;
    assert!(
        (out.len() as f64 - want).abs() <= want * 0.01,
        "len {}",
        out.len()
    );
    let f = dominant_freq(&out, sr);
    assert!((f - 220.0).abs() <= 2.0, "dominant {f}");
}

#[test]
fn stretch_near_unity_returns_input() {
    let wav = sine(24000, 0.5, 220.0, 0.3);
    assert_eq!(stretch(&wav, 24000, 1.01), wav);
}

#[test]
fn room_tone_level() {
    let tone = room_tone(48000, 24000);
    assert_eq!(tone.len(), 48000);
    let db = rms_db(&tone);
    assert!((db - ROOM_TONE_DB).abs() <= 0.5, "{db}");
    assert_eq!(tone, room_tone(48000, 24000), "seeded, reproducible");
}

#[test]
fn resample_keeps_tone_and_length() {
    let wav = sine(24000, 1.0, 440.0, 0.5);
    let out = resample(&wav, 24000, 16000);
    assert_eq!(out.len(), 16000);
    let ideal = sine(16000, 1.0, 440.0, 0.5);
    // away from the edges the band-limited result matches the ideal tone closely
    let err = out[200..15800]
        .iter()
        .zip(&ideal[200..15800])
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(err < 0.01, "max err {err}");
    assert_eq!(resample(&wav, 24000, 24000), wav);
}

#[test]
fn quiet_ending_requires_room_tone_after_the_speech() {
    let sr = 24000;
    let tone = |i: usize| (2.0 * PI * 200.0 * i as f64 / sr as f64).sin() as f32 * 0.5;
    let speech: Vec<f32> = (0..sr as usize).map(tone).collect();
    // speech to the last sample: rejected
    assert!(!lectover::audio::quiet_ending(&speech, sr));
    // 0.2 s of -66 dB noise-like tone after it: accepted
    let mut ok = speech.clone();
    ok.extend((0..sr as usize / 5).map(|i| tone(i * 7) * 0.001));
    assert!(lectover::audio::quiet_ending(&ok, sr));
    // only 0.1 s quiet: the last 0.16 s still holds speech
    let mut short = speech.clone();
    short.extend(vec![0.0; sr as usize / 10]);
    assert!(!lectover::audio::quiet_ending(&short, sr));
    assert!(!lectover::audio::quiet_ending(&[], sr));
}

/// Sessionsbericht listening test: a long take sagged ~8 dB towards its end and
/// the next take started loud again. Sentences inside a take get levelled
/// individually; the gain changes only inside the pauses between them.
#[test]
fn level_segments_evens_out_sentences_inside_a_take() {
    let sr = 24000;
    let tone = |i: usize, amp: f32| (2.0 * PI * 200.0 * i as f64 / sr as f64).sin() as f32 * amp;
    let sec = |s: f64| (s * sr as f64) as usize;
    // 3 s loud sentence, 0.5 s pause, 3 s sentence 10 dB quieter
    let mut wav: Vec<f32> = (0..sec(3.0)).map(|i| tone(i, 0.3)).collect();
    wav.extend(vec![0.0; sec(0.5)]);
    wav.extend((0..sec(3.0)).map(|i| tone(i, 0.3 / 10f32.powf(0.5))));
    let out = lectover::audio::level_segments(&wav, sr);
    assert_eq!(out.len(), wav.len());
    let db = |x: &[f32]| {
        10.0 * (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).log10()
    };
    let (a, b) = (db(&out[sec(0.5)..sec(2.5)]), db(&out[sec(4.0)..sec(6.0)]));
    assert!((a - b).abs() < 1.0, "sentences at {a:.1} and {b:.1} dB");
    // the overall level stays where chunk levelling put it (between the two)
    let before = db(&wav[..]);
    let after = db(&out[..]);
    assert!(
        (before - after).abs() < 3.0,
        "overall {before:.1} -> {after:.1} dB"
    );
    // a short gap (< 0.3 s) inside a sentence is not a segment boundary
    let mut word_gap: Vec<f32> = (0..sec(2.0)).map(|i| tone(i, 0.3)).collect();
    word_gap.extend(vec![0.0; sec(0.15)]);
    word_gap.extend((0..sec(2.0)).map(|i| tone(i, 0.1)));
    let out = lectover::audio::level_segments(&word_gap, sr);
    assert_eq!(out, word_gap);
}
