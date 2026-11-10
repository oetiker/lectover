//! Pitch-preserving time stretch (WSOLA), replacing Python's `ffmpeg -af atempo`.

/// Analysis/synthesis window length.
const WINDOW_SECS: f64 = 0.030;
/// Largest shift of an analysis frame away from its nominal position.
const SEARCH_SECS: f64 = 0.010;

/// Change the tempo of `wav` (tempo > 1 is faster/shorter) without changing its pitch.
///
/// WSOLA with a 30 ms Hann window, 50 % overlap and a ±10 ms similarity search. Tempos within
/// 2 % of 1 return the input unchanged, as in `longform.stretch`.
pub fn stretch(wav: &[f32], sr: u32, tempo: f64) -> Vec<f32> {
    if (tempo - 1.0).abs() < 0.02 || wav.is_empty() {
        return wav.to_vec();
    }
    let win = ((sr as f64 * WINDOW_SECS) as usize).max(4) & !1;
    let hop = win / 2; // synthesis hop: 50 % overlap
    let search = (sr as f64 * SEARCH_SECS) as isize;
    let out_len = (wav.len() as f64 / tempo).round() as usize;
    // periodic Hann: overlapped at `hop` it sums to a constant
    let window: Vec<f32> = (0..win)
        .map(|n| (0.5 - 0.5 * (std::f64::consts::TAU * n as f64 / win as f64).cos()) as f32)
        .collect();
    let at = |i: isize| -> f32 {
        if i >= 0 && (i as usize) < wav.len() {
            wav[i as usize]
        } else {
            0.0
        }
    };

    let mut out = vec![0.0f32; out_len + win];
    let mut norm = vec![0.0f32; out_len + win];
    // input position of the previously placed frame
    let mut prev: Option<isize> = None;
    let mut k = 0usize;
    while k * hop < out_len {
        let nominal = (k as f64 * hop as f64 * tempo).round() as isize;
        let pos = match prev {
            None => nominal,
            Some(p) => {
                // the natural continuation of the previous frame, i.e. what would follow it
                let natural: Vec<f32> = (0..win as isize)
                    .map(|j| at(p + hop as isize + j))
                    .collect();
                let mut best = (nominal, f32::NEG_INFINITY);
                for d in -search..=search {
                    let cand = nominal + d;
                    let score: f32 = natural
                        .iter()
                        .enumerate()
                        .map(|(j, &n)| n * at(cand + j as isize))
                        .sum();
                    if score > best.1 {
                        best = (cand, score);
                    }
                }
                best.0
            }
        };
        let base = k * hop;
        for j in 0..win {
            out[base + j] += window[j] * at(pos + j as isize);
            norm[base + j] += window[j];
        }
        prev = Some(pos);
        k += 1;
    }
    out.truncate(out_len);
    for (v, w) in out.iter_mut().zip(&norm) {
        if *w > 1e-3 {
            *v /= w;
        }
    }
    out
}
