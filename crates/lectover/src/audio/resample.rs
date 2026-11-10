//! Band-limited sample-rate conversion (windowed sinc).

use std::f64::consts::PI;

/// Zero crossings of the sinc kernel on each side of the output sample.
const HALF_TAPS: f64 = 32.0;
/// Passband edge relative to the lower Nyquist frequency (leaves room for the transition band).
const ROLLOFF: f64 = 0.94;

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        (PI * x).sin() / (PI * x)
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Largest reduced output-phase count for which a kernel table per phase is precomputed.
const MAX_PHASES: u64 = 4096;

/// Kernel weight for a tap at signed distance `x` (input samples) from the output position.
fn weight(x: f64, cutoff: f64, half: f64) -> f64 {
    // Blackman window over [-half, half]
    let w = 0.42 + 0.5 * (PI * x / half).cos() + 0.08 * (2.0 * PI * x / half).cos();
    2.0 * cutoff * sinc(2.0 * cutoff * x) * w
}

/// Resample `wav` from `from` Hz to `to` Hz; the result has `round(len * to / from)` samples.
pub fn resample(wav: &[f32], from: u32, to: u32) -> Vec<f32> {
    resample_with(wav, from, to, MAX_PHASES)
}

fn resample_with(wav: &[f32], from: u32, to: u32, max_phases: u64) -> Vec<f32> {
    if from == to || wav.is_empty() {
        return wav.to_vec();
    }
    let ratio = to as f64 / from as f64;
    let out_len = (wav.len() as f64 * ratio).round() as usize;
    // cutoff in cycles per input sample
    let cutoff = ROLLOFF * 0.5 * ratio.min(1.0);
    // kernel half-width in input samples
    let half = HALF_TAPS / (2.0 * cutoff);

    let g = gcd(from as u64, to as u64);
    let (p, q) = (to as u64 / g, from as u64 / g); // output n sits at input n*q/p
    if p > max_phases {
        return resample_direct(wav, out_len, ratio, cutoff, half);
    }

    // Per output phase r: taps cover input offsets kmin.. relative to floor(n*q/p).
    struct Phase {
        kmin: i64,
        w: Vec<f64>,
    }
    let phases: Vec<Phase> = (0..p)
        .map(|r| {
            let frac = r as f64 / p as f64;
            let kmin = (frac - half).ceil() as i64;
            let kmax = (frac + half).floor() as i64;
            let w = (kmin..=kmax).map(|k| weight(k as f64 - frac, cutoff, half)).collect();
            Phase { kmin, w }
        })
        .collect();

    let last = wav.len() as i64 - 1;
    (0..out_len as u64)
        .map(|n| {
            let pos = n * q;
            let ph = &phases[(pos % p) as usize];
            let base = (pos / p) as i64 + ph.kmin;
            // clip the tap range to the signal
            let skip = (-base).max(0) as usize;
            let end = ((last - base + 1).max(0) as usize).min(ph.w.len());
            if skip >= end {
                return 0.0;
            }
            let src = &wav[(base + skip as i64) as usize..(base + end as i64) as usize];
            let acc: f64 = src.iter().zip(&ph.w[skip..end]).map(|(&v, &w)| v as f64 * w).sum();
            acc as f32
        })
        .collect()
}

/// Fallback for huge phase counts: weights per output sample, trig advanced by rotation
/// instead of a fresh cos/sin per tap.
fn resample_direct(wav: &[f32], out_len: usize, ratio: f64, cutoff: f64, half: f64) -> Vec<f32> {
    let mut w: Vec<f64> = Vec::new();
    (0..out_len)
        .map(|n| {
            let t = n as f64 / ratio;
            let lo = ((t - half).ceil().max(0.0)) as usize;
            let hi = ((t + half).floor() as usize).min(wav.len() - 1);
            if lo > hi {
                return 0.0;
            }
            w.clear();
            // angles for window cos(a), cos(2a) with a = pi*x/half, and sinc argument b = 2*pi*cutoff*x
            let x0 = lo as f64 - t;
            let (da, db) = (PI / half, 2.0 * PI * cutoff);
            let (sa, ca) = (da.sin(), da.cos());
            let (sb, cb) = (db.sin(), db.cos());
            let (mut cur_ca, mut cur_sa) = ((da * x0).cos(), (da * x0).sin());
            let (mut cur_cb, mut cur_sb) = ((db * x0).cos(), (db * x0).sin());
            for i in lo..=hi {
                let x = i as f64 - t;
                let win = 0.42 + 0.5 * cur_ca + 0.08 * (2.0 * cur_ca * cur_ca - 1.0);
                let sc = if (2.0 * cutoff * x).abs() < 1e-12 { 1.0 } else { cur_sb / (db * x) };
                w.push(2.0 * cutoff * sc * win);
                (cur_ca, cur_sa) = (cur_ca * ca - cur_sa * sa, cur_sa * ca + cur_ca * sa);
                (cur_cb, cur_sb) = (cur_cb * cb - cur_sb * sb, cur_sb * cb + cur_cb * sb);
            }
            let acc: f64 = wav[lo..=hi].iter().zip(&w).map(|(&v, &w)| v as f64 * w).sum();
            acc as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Original per-tap implementation, kept as the oracle for the fast version.
    fn resample_reference(wav: &[f32], from: u32, to: u32) -> Vec<f32> {
        if from == to || wav.is_empty() {
            return wav.to_vec();
        }
        let ratio = to as f64 / from as f64;
        let out_len = (wav.len() as f64 * ratio).round() as usize;
        // cutoff in cycles per input sample
        let cutoff = ROLLOFF * 0.5 * ratio.min(1.0);
        // kernel half-width in input samples
        let half = HALF_TAPS / (2.0 * cutoff);
        (0..out_len)
            .map(|n| {
                let t = n as f64 / ratio; // position in input samples
                let lo = ((t - half).ceil().max(0.0)) as usize;
                let hi = ((t + half).floor() as usize).min(wav.len() - 1);
                let mut acc = 0.0f64;
                for (i, &v) in wav.iter().enumerate().take(hi + 1).skip(lo) {
                    let x = i as f64 - t;
                    // Blackman window over [-half, half]
                    let w = 0.42 + 0.5 * (PI * x / half).cos() + 0.08 * (2.0 * PI * x / half).cos();
                    acc += v as f64 * 2.0 * cutoff * sinc(2.0 * cutoff * x) * w;
                }
                acc as f32
            })
            .collect()
    }

    fn sine(n: usize, rate: u32) -> Vec<f32> {
        (0..n).map(|i| (2.0 * PI * 440.0 * i as f64 / rate as f64).sin() as f32 * 0.8).collect()
    }

    fn noise(n: usize) -> Vec<f32> {
        let mut s: u64 = 0x1234_5678_9abc_def0;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((s >> 40) as f64 / (1u64 << 24) as f64 * 2.0 - 1.0) as f32
            })
            .collect()
    }

    fn max_err(a: &[f32], b: &[f32]) -> f32 {
        assert_eq!(a.len(), b.len());
        a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
    }

    const CASES: [(u32, u32); 5] = [(24000, 16000), (16000, 24000), (24000, 48000), (44100, 24000), (22050, 24000)];

    #[test]
    fn matches_reference_sine_and_noise() {
        for (from, to) in CASES {
            for wav in [sine(5000, from), noise(5000)] {
                let (new, old) = (resample(&wav, from, to), resample_reference(&wav, from, to));
                let e = max_err(&new, &old);
                eprintln!("{from}->{to}: max abs err {e:e}");
                assert!(e < 1e-5, "{from}->{to}: {e}");
            }
        }
    }

    #[test]
    fn matches_reference_short_inputs() {
        for (from, to) in CASES {
            for n in [0usize, 1, 7] {
                let wav = noise(n);
                let (new, old) = (resample(&wav, from, to), resample_reference(&wav, from, to));
                assert!(max_err(&new, &old) < 1e-5, "{from}->{to} n={n}");
            }
        }
    }

    #[test]
    fn length_rule_unchanged() {
        for (from, to) in CASES {
            for n in [0usize, 1, 7, 100, 4999, 24001] {
                let wav = noise(n);
                let expect = if n == 0 { 0 } else { (n as f64 * to as f64 / from as f64).round() as usize };
                assert_eq!(resample(&wav, from, to).len(), expect, "{from}->{to} n={n}");
            }
        }
    }

    #[test]
    fn edge_samples_match() {
        for (from, to) in CASES {
            let wav = noise(3000);
            let (new, old) = (resample(&wav, from, to), resample_reference(&wav, from, to));
            let k = 64.min(new.len());
            assert!(max_err(&new[..k], &old[..k]) < 1e-5, "{from}->{to} head");
            assert!(max_err(&new[new.len() - k..], &old[old.len() - k..]) < 1e-5, "{from}->{to} tail");
        }
    }

    #[test]
    fn direct_fallback_matches_reference() {
        // large coprime rates force the per-sample path with the default phase limit
        for (from, to) in [(7001u32, 5003u32), (5003, 7001)] {
            let wav = noise(3000);
            let (new, old) = (resample(&wav, from, to), resample_reference(&wav, from, to));
            let e = max_err(&new, &old);
            eprintln!("{from}->{to} (direct): max abs err {e:e}");
            assert!(e < 1e-5, "{from}->{to}: {e}");
        }
    }

    #[test]
    #[ignore = "timing; run with --release -- --ignored --nocapture"]
    fn timing_60s_24k_to_16k() {
        let wav = noise(24000 * 60);
        let t = Instant::now();
        let old = resample_reference(&wav, 24000, 16000);
        let t_old = t.elapsed();
        let t = Instant::now();
        let new = resample(&wav, 24000, 16000);
        let t_new = t.elapsed();
        eprintln!("old {t_old:?}  new {t_new:?}  speedup {:.1}x  err {:e}", t_old.as_secs_f64() / t_new.as_secs_f64(), max_err(&new, &old));
    }
}
