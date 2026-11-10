//! Port of the assembly half of `longform.py`: trim, level, pace-match, join with pauses.

use std::f64::consts::FRAC_PI_2;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use rand_chacha::ChaCha8Rng;
use rand_core::{Rng, SeedableRng};

use super::stretch::stretch;
use crate::plan::{syllables, ChunkKind, TITLE_LEAD_SECS};

/// Edge trimming: frames quieter than speech level minus this are silence.
pub const TRIM_BELOW_SPEECH_DB: f64 = 35.0;
pub const TRIM_KEEP_SECS: f64 = 0.10;
/// Kept after the last voiced frame: more than at the start, because word
/// endings decay below the voiced threshold and 0.1 s still sounded clipped.
pub const TRIM_KEEP_TAIL_SECS: f64 = 0.20;
pub const FADE_IN_SECS: f64 = 0.01;
pub const FADE_OUT_SECS: f64 = 0.06;
pub const SPEECH_TARGET_DB: f64 = -20.0;
/// Quiet noise bed under everything so pauses are never digital silence (heard as dropouts).
pub const ROOM_TONE_DB: f64 = -65.0;
/// Pace normalisation: stretch chunks towards the median syllable rate, within limits.
pub const PACE_MIN: f64 = 0.88;
pub const PACE_MAX: f64 = 1.12;

fn to_db(x: impl ExactSizeIterator<Item = f32>) -> f64 {
    let n = x.len();
    let ms = x.map(|v| (v as f64) * (v as f64)).sum::<f64>() / n as f64;
    20.0 * (ms.sqrt() + 1e-9).log10()
}

/// dB level of consecutive 10 ms frames (a trailing partial frame is ignored) and the frame size.
fn frame_db(wav: &[f32], sr: u32) -> (Vec<f64>, usize) {
    let frame = ((sr as f64 * 0.01) as usize).max(1);
    let db = wav
        .chunks_exact(frame)
        .map(|f| to_db(f.iter().copied()))
        .collect();
    (db, frame)
}

/// numpy's default (linear) percentile.
fn percentile(values: &[f64], q: f64) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let pos = q / 100.0 * (v.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    v[lo] + (v[hi] - v[lo]) * (pos - lo as f64)
}

/// Indices of the frames louder than the 80th-percentile level minus `TRIM_BELOW_SPEECH_DB`.
fn voiced_frames(db: &[f64]) -> Vec<usize> {
    if db.is_empty() {
        return Vec::new();
    }
    let threshold = percentile(db, 80.0) - TRIM_BELOW_SPEECH_DB;
    (0..db.len()).filter(|&i| db[i] > threshold).collect()
}

/// Length of a voice clip's ending that must be quiet: the TTS ring-out pads
/// each take with the clip's last codec frame (80 ms), so that frame must be room tone.
pub const VOICE_QUIET_TAIL_SECS: f64 = 0.16;
/// The quiet ending must sit at least this far below the clip's loudest 10 ms frame.
pub const VOICE_TAIL_BELOW_PEAK_DB: f64 = 35.0;

/// True when a voice clip ends in room tone ([`VOICE_QUIET_TAIL_SECS`] at least
/// [`VOICE_TAIL_BELOW_PEAK_DB`] below its peak), as the tail padding requires.
pub fn quiet_ending(wav: &[f32], sr: u32) -> bool {
    let (db, _) = frame_db(wav, sr);
    let n = (sr as f64 * VOICE_QUIET_TAIL_SECS) as usize;
    if db.is_empty() || wav.len() < n {
        return false;
    }
    let peak = db.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    to_db(wav[wav.len() - n..].iter().copied()) < peak - VOICE_TAIL_BELOW_PEAK_DB
}

/// `np.linspace(0, pi/2, n)`.
fn quarter_wave(n: usize) -> impl Iterator<Item = f64> {
    let step = if n > 1 {
        FRAC_PI_2 / (n - 1) as f64
    } else {
        0.0
    };
    (0..n).map(move |k| k as f64 * step)
}

/// The cut and gain [`trim_and_level_with`] chose, reusable on a sample-aligned signal via [`apply_trim`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trim {
    /// First kept sample.
    pub start: usize,
    /// One past the last kept sample.
    pub end: usize,
    /// Linear gain applied to the kept slice (1.0 when no sample was loud enough to measure).
    pub gain: f32,
}

/// Trim edge silence (keeping `TRIM_KEEP_SECS` before, `TRIM_KEEP_TAIL_SECS` after), level speech to `SPEECH_TARGET_DB`, fade the edges.
pub fn trim_and_level(wav: &[f32], sr: u32) -> Vec<f32> {
    trim_and_level_with(wav, sr).0
}

/// [`trim_and_level`], also returning the [`Trim`] it applied.
pub fn trim_and_level_with(wav: &[f32], sr: u32) -> (Vec<f32>, Trim) {
    let (db, frame) = frame_db(wav, sr);
    let voiced = voiced_frames(&db);
    let keep = (sr as f64 * TRIM_KEEP_SECS) as usize;
    let keep_tail = (sr as f64 * TRIM_KEEP_TAIL_SECS) as usize;
    let (start, end) = match (voiced.first(), voiced.last()) {
        (Some(&first), Some(&last)) => (
            (first * frame).saturating_sub(keep),
            ((last + 1) * frame + keep_tail).min(wav.len()),
        ),
        _ => (0, wav.len()), // shorter than one frame: nothing to judge
    };
    let loud: Vec<f32> = wav[start..end].iter().copied().filter(|v| v.abs() > 0.01).collect();
    let gain = if loud.is_empty() {
        1.0
    } else {
        10f64.powf((SPEECH_TARGET_DB - to_db(loud.into_iter())) / 20.0) as f32
    };
    let trim = Trim { start, end, gain };
    (apply_trim(wav, sr, &trim), trim)
}

/// Cut `wav` to `t`, apply its gain, fade the edges and clamp: the single implementation
/// behind [`trim_and_level_with`], also usable on any signal of the same length.
pub fn apply_trim(wav: &[f32], sr: u32, t: &Trim) -> Vec<f32> {
    let mut out = wav[t.start..t.end].to_vec();
    if t.gain != 1.0 {
        out.iter_mut().for_each(|v| *v *= t.gain);
    }
    let len = out.len();
    let n_in = ((sr as f64 * FADE_IN_SECS) as usize).min(len);
    for (v, k) in out[..n_in].iter_mut().zip(quarter_wave(n_in)) {
        *v *= k.sin().powi(2) as f32;
    }
    let n_out = ((sr as f64 * FADE_OUT_SECS) as usize).min(len);
    for (v, k) in out[len - n_out..].iter_mut().zip(quarter_wave(n_out)) {
        *v *= k.cos().powi(2) as f32;
    }
    out.iter_mut().for_each(|v| *v = v.clamp(-1.0, 1.0));
    out
}

/// A run of unvoiced frames at least this long separates two segments (sentences) of a take.
pub const SEGMENT_PAUSE_SECS: f64 = 0.3;
/// Segments with less voiced audio than this borrow a neighbour's gain (too short to measure).
pub const SEGMENT_MIN_SECS: f64 = 1.0;
/// Per-segment correction limit relative to the take's overall level.
pub const SEGMENT_MAX_GAIN_DB: f64 = 9.0;

/// Level every segment of a take (voiced stretches between pauses of at least
/// [`SEGMENT_PAUSE_SECS`]) to the take's overall speech level. Long takes drift in
/// loudness (measured: ~8 dB sag towards the end), which one gain per take cannot
/// fix. The gain is constant within a segment and ramps only across the pauses.
pub fn level_segments(wav: &[f32], sr: u32) -> Vec<f32> {
    let (db, frame) = frame_db(wav, sr);
    let voiced = voiced_frames(&db);
    let min_pause = (SEGMENT_PAUSE_SECS / 0.01).round() as usize;
    // segments as (first, last) voiced frame
    let mut segs: Vec<(usize, usize)> = Vec::new();
    for &f in &voiced {
        match segs.last_mut() {
            Some((_, last)) if f - *last <= min_pause => *last = f,
            _ => segs.push((f, f)),
        }
    }
    if segs.len() < 2 {
        return wav.to_vec();
    }
    let power = |frames: &mut dyn Iterator<Item = usize>| {
        let (sum, n) = frames.fold((0.0, 0usize), |(s, n), f| (s + 10f64.powf(db[f] / 10.0), n + 1));
        (10.0 * (sum / n.max(1) as f64 + 1e-18).log10(), n)
    };
    let (take_db, _) = power(&mut voiced.iter().copied());
    let min_frames = (SEGMENT_MIN_SECS / 0.01).round() as usize;
    let mut gains: Vec<Option<f64>> = segs
        .iter()
        .map(|&(a, b)| {
            let (seg_db, n) = power(&mut voiced.iter().copied().filter(|&f| f >= a && f <= b));
            (n >= min_frames).then(|| (take_db - seg_db).clamp(-SEGMENT_MAX_GAIN_DB, SEGMENT_MAX_GAIN_DB))
        })
        .collect();
    // short segments borrow the previous measurable gain, else the next, else none
    for i in 0..gains.len() {
        if gains[i].is_none() {
            gains[i] = gains[..i].iter().rev().flatten().next().copied().or_else(|| gains[i..].iter().flatten().next().copied());
        }
    }
    let gains: Vec<f64> = gains.into_iter().map(|g| g.unwrap_or(0.0)).collect();
    let lin = |g: f64| 10f64.powf(g / 20.0);
    let mut out = wav.to_vec();
    for (i, v) in out.iter_mut().enumerate() {
        let f = i as f64 / frame as f64;
        // segment k: the last one starting at or before this frame
        let k = segs.iter().rposition(|&(a, _)| a as f64 <= f).unwrap_or(0);
        let g = match segs.get(k + 1) {
            // inside the pause after segment k: ramp to the next segment's gain
            Some(&(next_a, _)) if f > (segs[k].1 + 1) as f64 => {
                let (p0, p1) = ((segs[k].1 + 1) as f64, next_a as f64);
                let t = ((f - p0) / (p1 - p0)).clamp(0.0, 1.0);
                lin(gains[k]) * (1.0 - t) + lin(gains[k + 1]) * t
            }
            _ => lin(gains[k]),
        };
        *v = (*v as f64 * g).clamp(-1.0, 1.0) as f32;
    }
    out
}

/// Standard normal deviate (Box-Muller) from a uniform source.
fn standard_normal(rng: &mut impl Rng) -> f64 {
    let mut unit = || (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64; // [0, 1)
    let u1 = 1.0 - unit(); // (0, 1], safe for ln
    let u2 = unit();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

/// Gently low-passed noise at `ROOM_TONE_DB`: sounds like a quiet room rather than hiss.
pub fn room_tone(n: usize, _sr: u32) -> Vec<f32> {
    let mut rng = ChaCha8Rng::seed_from_u64(0);
    let noise: Vec<f64> = (0..n).map(|_| standard_normal(&mut rng)).collect();
    // np.convolve(noise, ones(8)/8, mode="same"): out[i] = mean(noise[i-4 ..= i+3]), zero padded
    let mut prefix = vec![0.0f64; n + 1];
    for (i, v) in noise.iter().enumerate() {
        prefix[i + 1] = prefix[i] + v;
    }
    let smooth: Vec<f64> = (0..n)
        .map(|i| (prefix[(i + 4).min(n)] - prefix[i.saturating_sub(4)]) / 8.0)
        .collect();
    let mean = smooth.iter().sum::<f64>() / n.max(1) as f64;
    let std = (smooth.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n.max(1) as f64).sqrt();
    let scale = 10f64.powf(ROOM_TONE_DB / 20.0) / (std + 1e-12);
    smooth.iter().map(|v| (v * scale) as f32).collect()
}

fn voiced_secs(wav: &[f32], sr: u32) -> f64 {
    let (db, _) = frame_db(wav, sr);
    voiced_frames(&db).len() as f64 * 0.01
}

fn median(values: &[f64]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let mid = v.len() / 2;
    if v.len() % 2 == 1 {
        v[mid]
    } else {
        (v[mid - 1] + v[mid]) / 2.0
    }
}

/// `f` over `items` on all cores, results in input order. Every item is computed on its own,
/// so the result is the same as a sequential map.
fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).min(items.len());
    if workers <= 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(i) else { break };
                let r = f(item);
                out.lock().unwrap()[i] = Some(r);
            });
        }
    });
    out.into_inner().unwrap().into_iter().map(|r| r.expect("every item was mapped")).collect()
}

/// Room tone at the very start of the file, before the first take or sting. An output that
/// has to wake up first (Bluetooth headphones) swallows the start of playback: on AirPods the
/// first sound of a take was lost, while the same take behind 1 s of silence played complete.
pub const START_LEAD_SECS: f64 = 1.0;
/// Pause between the end of speech and a sting.
pub const STING_LEAD_SECS: f64 = 0.8;
/// Pause after a sting (and the whole pause of a break without a sound).
pub const STING_TAIL_SECS: f64 = 0.8;
/// Highest sample peak a levelled sting may reach.
pub const STING_MAX_PEAK: f32 = 0.95;

/// A site sound resampled to `to` Hz and levelled like speech ([`SPEECH_TARGET_DB`] over its
/// audible samples), but never above [`STING_MAX_PEAK`]; edges are left as they are.
pub fn level_sting(wav: &[f32], from: u32, to: u32) -> Vec<f32> {
    let mut out = super::resample::resample(wav, from, to);
    let loud: Vec<f32> = out.iter().copied().filter(|v| v.abs() > 0.01).collect();
    if loud.is_empty() {
        return out;
    }
    let gain = 10f64.powf((SPEECH_TARGET_DB - to_db(loud.into_iter())) / 20.0) as f32;
    let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let gain = gain.min(STING_MAX_PEAK / peak);
    out.iter_mut().for_each(|v| *v *= gain);
    out
}

/// takes: (raw wav, chunk text, kind). Trim, level, pace-match and join with pauses.
///
/// Returns the final audio and, per chunk, (offset_seconds, processed_wav) exactly as placed.
pub fn assemble_parts(
    takes: &[(Vec<f32>, String, ChunkKind)],
    sr: u32,
    pace: bool,
) -> (Vec<f32>, Vec<(f64, Vec<f32>)>) {
    assemble_parts_with(takes, sr, pace, &[])
}

/// [`assemble_parts`] with stings: `(i, sound)` plays `sound` (already at `sr`, see
/// [`level_sting`]) before take `i` (`takes.len()`: after the last take), with
/// [`STING_LEAD_SECS`] before and [`STING_TAIL_SECS`] after it. `None` is a silent break.
/// `stings` must be sorted by `i`.
pub fn assemble_parts_with(
    takes: &[(Vec<f32>, String, ChunkKind)],
    sr: u32,
    pace: bool,
    stings: &[(usize, Option<&[f32]>)],
) -> (Vec<f32>, Vec<(f64, Vec<f32>)>) {
    let trimmed: Vec<Vec<f32>> = par_map(takes, |(w, _, _)| level_segments(&trim_and_level(w, sr), sr));
    let rates: Vec<f64> = trimmed
        .iter()
        .zip(takes)
        .map(|(w, (_, text, _))| syllables(text) as f64 / voiced_secs(w, sr))
        .collect();
    // the title is too short for a meaningful rate
    let body: Vec<f64> = rates
        .iter()
        .zip(takes)
        .filter(|(_, (_, _, kind))| *kind != ChunkKind::Title)
        .map(|(r, _)| *r)
        .collect();
    let target = if !body.is_empty() {
        median(&body)
    } else if !rates.is_empty() {
        median(&rates)
    } else {
        f64::NAN
    };

    let tempos: Vec<f64> = takes
        .iter()
        .zip(&rates)
        .enumerate()
        .map(|(i, ((_, _, kind), rate))| {
            let tempo = if !pace || *kind == ChunkKind::Title {
                1.0
            } else {
                let t = (target / rate).clamp(PACE_MIN, PACE_MAX);
                if t.is_finite() {
                    t
                } else {
                    1.0
                }
            };
            println!("  chunk {}: {rate:.2} syl/s -> tempo {tempo:.3}", i + 1);
            tempo
        })
        .collect();
    let jobs: Vec<(&Vec<f32>, f64)> = trimmed.iter().zip(tempos).collect();
    let stretched = par_map(&jobs, |&(wav, tempo)| stretch(wav, sr, tempo));

    let mut audio = Vec::new();
    let mut placed = Vec::with_capacity(takes.len());
    let pad = |audio: &mut Vec<f32>, secs: f64| audio.resize(audio.len() + (sr as f64 * secs) as usize, 0.0);
    let mut pending = stings.iter().peekable();
    let mut play_stings = |audio: &mut Vec<f32>, at: usize| {
        while let Some((_, sound)) = pending.next_if(|(i, _)| *i == at) {
            if let Some(sound) = sound {
                audio.extend_from_slice(sound);
            }
            pad(audio, STING_TAIL_SECS);
        }
    };
    pad(&mut audio, START_LEAD_SECS);
    let sting_at = |i: usize| stings.iter().any(|(j, _)| *j == i);
    for (i, ((_, _, kind), out)) in takes.iter().zip(stretched).enumerate() {
        play_stings(&mut audio, i);
        placed.push((audio.len() as f64 / sr as f64, out.clone()));
        audio.extend_from_slice(&out);
        if sting_at(i + 1) {
            pad(&mut audio, STING_LEAD_SECS);
        } else if let Some((_, _, next)) = takes.get(i + 1) {
            let secs = if *next == ChunkKind::Title {
                kind.pause_secs().max(TITLE_LEAD_SECS)
            } else {
                kind.pause_secs()
            };
            let gap = (sr as f64 * secs) as usize;
            audio.resize(audio.len() + gap, 0.0);
        }
    }
    play_stings(&mut audio, takes.len());
    let tone = room_tone(audio.len(), sr);
    for (a, t) in audio.iter_mut().zip(tone) {
        *a = (*a + t).clamp(-1.0, 1.0);
    }
    (audio, placed)
}
