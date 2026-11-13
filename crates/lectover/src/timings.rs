//! Map forced-aligner word times onto display tokens and build the timings document.

use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::plan::Chunk;
use crate::text::normalize::Group;
use crate::text::spoken_words::spoken_words;
use crate::text::Block;

pub type WordTime = (f64, f64);

/// Per-block token times and per-block interpolation flags.
pub type TokenTimes = (Vec<Vec<WordTime>>, Vec<Vec<bool>>);

/// Python's `round(x, 3)`: correctly rounded from the exact binary value,
/// ties to even. Rust's `{:.3}` formatting does exactly this (the naive
/// `(x * 1000).round_ties_even() / 1000` does not: `0.0005 * 1000` is exactly
/// 0.5 after multiplication although the binary value of 0.0005 is above it).
pub fn round3(x: f64) -> f64 {
    format!("{x:.3}").parse().unwrap_or(x)
}

/// Split `[left, right]` proportionally to word length.
fn spread(words: &[&str], left: f64, right: f64) -> Vec<WordTime> {
    let weights: Vec<usize> = words.iter().map(|w| w.chars().count().max(1)).collect();
    let total = weights.iter().sum::<usize>() as f64;
    let mut pos = 0usize;
    weights
        .iter()
        .map(|&w| {
            let a = left + (right - left) * pos as f64 / total;
            pos += w;
            (a, left + (right - left) * pos as f64 / total)
        })
        .collect()
}

pub fn place_words(
    items: &[WordTime],
    words: &[String],
    duration: f64,
) -> (Vec<WordTime>, Vec<bool>) {
    let mut placed = Vec::with_capacity(items.len());
    let mut last_end = 0.0_f64;
    for &(s, e) in items {
        let ok = s >= last_end - 0.02 && e > s.max(last_end) && e <= duration + 0.05;
        placed.push(ok);
        if ok {
            last_end = e;
        }
    }
    let mut times: Vec<WordTime> = vec![(0.0, 0.0); items.len()];
    let mut left = 0.0_f64;
    let mut i = 0;
    while i < items.len() {
        if placed[i] {
            // start never precedes the previous word's end (absorbs the 0.02 s overlap tolerance)
            let start = items[i].0.max(left).max(0.0).min(duration);
            times[i] = (start, start.max(items[i].1.max(0.0).min(duration)));
            left = times[i].1;
            i += 1;
            continue;
        }
        let mut j = i;
        while j < items.len() && !placed[j] {
            j += 1;
        }
        let right = if j < items.len() {
            items[j].0.max(0.0).min(duration)
        } else {
            duration
        };
        let right = right.max(left);
        let ws: Vec<&str> = words[i..j].iter().map(String::as_str).collect();
        for (k, t) in spread(&ws, left, right).into_iter().enumerate() {
            times[i + k] = t;
        }
        i = j;
    }
    (times, placed.into_iter().map(|p| !p).collect())
}

pub fn token_times<S: AsRef<str>>(
    tokens: &[S],
    groups: &[Group],
    word_times: &[WordTime],
    word_interp: &[bool],
    carry: f64,
) -> (Vec<WordTime>, Vec<bool>) {
    let mut slots: Vec<Option<WordTime>> = vec![None; tokens.len()];
    let mut interp = vec![false; tokens.len()];
    let mut w = 0;
    for g in groups {
        let k = spoken_words(&g.spoken).len();
        if k == 0 {
            continue;
        }
        let gt = &word_times[w..w + k];
        let gi = &word_interp[w..w + k];
        w += k;
        let n = g.last - g.first;
        if k == n {
            for (t, (&time, &flag)) in gt.iter().zip(gi).enumerate() {
                slots[g.first + t] = Some(time);
                interp[g.first + t] = flag;
            }
            continue;
        }
        let toks: Vec<&str> = (g.first..g.last).map(|i| tokens[i].as_ref()).collect();
        let parts = spread(&toks, gt[0].0, gt[k - 1].1);
        let any = gi.iter().any(|&b| b);
        for (t, part) in parts.into_iter().enumerate() {
            slots[g.first + t] = Some(part);
            interp[g.first + t] = any;
        }
    }
    let mut out = Vec::with_capacity(slots.len());
    let mut prev: Option<f64> = None;
    for i in 0..slots.len() {
        let mut s = match slots[i] {
            Some(s) => s,
            None => match prev {
                Some(p) => (p, p),
                None => match slots[i..].iter().flatten().next() {
                    Some(n) => (n.0, n.0),
                    None => (carry, carry),
                },
            },
        };
        if let Some(p) = prev {
            if s.0 < p {
                // keep starts monotonic
                s = (p, p.max(s.1));
            }
        }
        out.push(s);
        prev = Some(s.1);
    }
    (out, interp)
}

pub fn assemble_token_times(
    blocks: &[Block],
    groups: &[Vec<Group>],
    chunks: &[Chunk],
    chunk_times: &[Vec<WordTime>],
    chunk_interp: &[Vec<bool>],
) -> Result<TokenTimes> {
    let mut result = Vec::new();
    let mut flags = Vec::new();
    let mut carry = 0.0;
    for (bi, block) in blocks.iter().enumerate() {
        let mut covered = vec![false; groups[bi].len()];
        let mut wt: Vec<WordTime> = Vec::new();
        let mut wi: Vec<bool> = Vec::new();
        for ((c, ct), ci) in chunks.iter().zip(chunk_times).zip(chunk_interp) {
            if c.block != bi {
                continue;
            }
            let need: usize = groups[bi][c.g0..c.g1]
                .iter()
                .map(|g| spoken_words(&g.spoken).len())
                .sum();
            if need != ct.len() {
                bail!(
                    "block {bi}: chunk has {} word times, groups need {need}",
                    ct.len()
                );
            }
            covered[c.g0..c.g1].fill(true);
            wt.extend_from_slice(ct);
            wi.extend_from_slice(ci);
        }
        let gs: Vec<Group> = groups[bi]
            .iter()
            .enumerate()
            .map(|(i, g)| {
                if covered[i] {
                    g.clone()
                } else {
                    Group {
                        first: g.first,
                        last: g.last,
                        spoken: String::new(),
                    }
                }
            })
            .collect();
        let (t, f) = token_times(&block.tokens, &gs, &wt, &wi, carry);
        if let Some(l) = t.last() {
            carry = l.1;
        }
        result.push(t);
        flags.push(f);
    }
    Ok((result, flags))
}

pub fn build_timings(
    voice: &str,
    duration: f64,
    blocks: &[Block],
    times: &[Vec<WordTime>],
) -> Result<Value> {
    let mut last_start = 0.0;
    let mut out = Vec::new();
    for (b, ts) in blocks.iter().zip(times) {
        let mut words = Vec::new();
        for &(s, e) in ts {
            if s < last_start || s > e || e > duration + 0.01 {
                bail!("invalid word time ({s}, {e}) in block {}", b.hash);
            }
            last_start = s;
            words.push(json!([round3(s), round3(e)]));
        }
        out.push(json!({"tag": b.tag, "hash": b.hash, "words": words}));
    }
    Ok(json!({"version": 1, "voice": voice, "duration": round3(duration), "blocks": out}))
}

pub fn alignment_report(blocks: &[Block], interp: &[Vec<bool>]) -> String {
    let n: usize = interp.iter().map(Vec::len).sum();
    let bad: usize = interp
        .iter()
        .map(|f| f.iter().filter(|&&b| b).count())
        .sum();
    let pct = |x: usize| {
        if n > 0 {
            100.0 * x as f64 / n as f64
        } else {
            0.0
        }
    };
    let mut lines = vec![format!(
        "aligned directly: {:.1} % of {n} tokens, interpolated: {:.1} %",
        pct(n - bad),
        pct(bad)
    )];
    for (i, (b, f)) in blocks.iter().zip(interp).enumerate() {
        let k = f.iter().filter(|&&x| x).count();
        if k >= 3 && k as f64 / f.len() as f64 >= 0.15 {
            let head: String = b.text.chars().take(30).collect();
            lines.push(format!(
                "block {i} ({}, \"{head}…\"): {k}/{} interpolated",
                b.tag,
                f.len()
            ));
        }
    }
    lines.join("\n")
}
