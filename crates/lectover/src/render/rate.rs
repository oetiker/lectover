//! Speaking-rate outliers: chunks whose tempo stands out against their neighbours.
//!
//! Every chunk is its own TTS take, and takes differ in tempo by about as much as a reader
//! does between paragraphs (±6–12 %). Listeners judge tempo against what they just heard
//! (just noticeable difference ≈ 5 %), so a slow region or a gradual drift goes unnoticed,
//! while a single take clearly faster or slower than the ones around it is heard as a jump.
//! The reference is therefore the chunk's neighbours, not the post-wide median.
//!
//! Neither rate measure is reliable on its own (they agree only at r ≈ 0.85): the aligner
//! rate (syllables over the time inside aligned words) is the primary one, the voiced rate
//! (syllables over the loud frames of the take, see [`crate::audio::voiced_rate`]) has to
//! confirm it. Outliers are reported; `render --reroll-tempo` replaces them with a new take
//! closer to the neighbours when one of a few tries is ([`pick_reroll`]).

use std::io;

use anyhow::Result;

use crate::audio::voiced_rate;
use crate::ffi::Align;
use crate::plan::{syllables, Chunk, ChunkKind};
use crate::timings::WordTime;

use super::segments::align_segments_with;

/// Eligible neighbours taken on each side of a chunk for its reference rate.
pub const RATE_NEIGHBOURS: usize = 2;
/// Fewest syllables for a chunk to be measured or used as a neighbour: shorter chunks give
/// rates too noisy to compare.
pub const RATE_MIN_SYLLABLES: usize = 40;
/// Fewest eligible neighbours (both sides together) for a reference rate.
pub const RATE_MIN_NEIGHBOURS: usize = 2;
/// Aligner-rate deviation from the neighbours above which a chunk is an outlier: twice the
/// JND, beyond the natural spread between paragraphs.
pub const RATE_OUTLIER: f64 = 0.10;
/// Voiced-rate deviation (same sign) that has to confirm [`RATE_OUTLIER`]: one JND. The two
/// measures disagree too often for either to flag a chunk alone.
pub const RATE_CONFIRM: f64 = 0.05;

/// One chunk's rates, in chunk order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChunkRate {
    /// Chunk index (0-based).
    pub index: usize,
    pub kind: ChunkKind,
    pub syllables: usize,
    /// Syllables per second inside aligned words. Not finite: not measurable (the chunk is
    /// then neither flagged nor used as a neighbour).
    pub aligner: f64,
    /// Syllables per second of voiced audio. Not finite: as for `aligner`.
    pub voiced: f64,
}

impl ChunkRate {
    fn eligible(&self) -> bool {
        self.kind != ChunkKind::Title
            && self.syllables >= RATE_MIN_SYLLABLES
            && self.aligner.is_finite()
            && self.aligner > 0.0
            && self.voiced.is_finite()
            && self.voiced > 0.0
    }
}

/// A chunk whose tempo stands out against its neighbours.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateOutlier {
    /// Chunk index (0-based).
    pub index: usize,
    /// Aligner rate relative to the neighbours' median, minus 1 (+0.11: 11 % faster).
    pub aligner_dev: f64,
    /// The neighbours' median aligner rate (syllables per second): the reference.
    pub aligner_ref: f64,
    /// Voiced rate relative to the neighbours' median, minus 1.
    pub voiced_dev: f64,
    /// Eligible neighbours the reference was taken from.
    pub neighbours: usize,
}

/// The rates of chunk `index` from its take as placed and its word times: syllables over the
/// summed word durations (not finite when a word was interpolated) and over the voiced audio.
pub fn chunk_rate(
    index: usize,
    chunk: &Chunk,
    placed: &[f32],
    sr: u32,
    times: &[WordTime],
    interp: &[bool],
) -> ChunkRate {
    let syl = syllables(&chunk.text);
    let inside: f64 = times.iter().map(|(s, e)| e - s).sum();
    let aligner = if interp.iter().any(|&f| f) {
        f64::NAN
    } else {
        syl as f64 / inside
    };
    ChunkRate {
        index,
        kind: chunk.kind,
        syllables: syl,
        aligner,
        voiced: voiced_rate(placed, sr, &chunk.text),
    }
}

/// [`chunk_rate`] of a take on its own: `placed` (see [`crate::audio::place_take`]) aligned
/// with the chunk's words, as the assembled audio would be. Aligner warnings are dropped: an
/// unaligned take just has no aligner rate.
pub fn measure_take(
    aligner: &mut dyn Align,
    index: usize,
    chunk: &Chunk,
    placed: &[f32],
    sr: u32,
) -> Result<ChunkRate> {
    let parts = [(0.0, placed.to_vec())];
    let (times, interp) = align_segments_with(
        aligner,
        &parts,
        sr,
        std::slice::from_ref(chunk),
        &mut io::sink(),
    )?;
    Ok(chunk_rate(index, chunk, placed, sr, &times[0], &interp[0]))
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        values[mid]
    } else {
        (values[mid - 1] + values[mid]) / 2.0
    }
}

/// The outliers among `chunks`, in chunk order. A chunk is one when its aligner rate differs
/// by more than [`RATE_OUTLIER`] from the median of up to [`RATE_NEIGHBOURS`] eligible
/// chunks on each side (at least [`RATE_MIN_NEIGHBOURS`] in all) and its voiced rate,
/// measured the same way, differs in the same direction by more than [`RATE_CONFIRM`].
/// Titles and chunks under [`RATE_MIN_SYLLABLES`] are skipped, also as neighbours.
pub fn rate_outliers(chunks: &[ChunkRate]) -> Vec<RateOutlier> {
    let eligible: Vec<&ChunkRate> = chunks.iter().filter(|c| c.eligible()).collect();
    let mut out = Vec::new();
    for (k, c) in eligible.iter().enumerate() {
        let before = &eligible[k.saturating_sub(RATE_NEIGHBOURS)..k];
        let after = &eligible[k + 1..(k + 1 + RATE_NEIGHBOURS).min(eligible.len())];
        let near: Vec<&&ChunkRate> = before.iter().chain(after).collect();
        if near.len() < RATE_MIN_NEIGHBOURS {
            continue;
        }
        let mut aligner: Vec<f64> = near.iter().map(|n| n.aligner).collect();
        let mut voiced: Vec<f64> = near.iter().map(|n| n.voiced).collect();
        let aligner_ref = median(&mut aligner);
        let aligner_dev = c.aligner / aligner_ref - 1.0;
        let voiced_dev = c.voiced / median(&mut voiced) - 1.0;
        if aligner_dev.abs() > RATE_OUTLIER
            && voiced_dev.abs() > RATE_CONFIRM
            && aligner_dev.signum() == voiced_dev.signum()
        {
            out.push(RateOutlier {
                index: c.index,
                aligner_dev,
                aligner_ref,
                voiced_dev,
                neighbours: near.len(),
            });
        }
    }
    out
}

/// The re-roll candidate [`pick_reroll`] chose.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RerollPick {
    /// Index into the candidates.
    pub candidate: usize,
    /// Its aligner rate relative to the reference, minus 1.
    pub dev: f64,
    /// Within [`RATE_OUTLIER`] of the reference: it replaces the original take.
    pub replace: bool,
}

/// Among a tempo outlier's re-roll candidates (aligner rates; `None`: failed the ASR check;
/// not finite: not measurable, both skipped), the one closest to the neighbours' `reference`
/// rate, if any. It replaces the original only when it deviates by at most [`RATE_OUTLIER`]:
/// the neighbour reference is kept from the first pass, so a candidate cannot pass by moving
/// the goalposts. On a tie the earlier candidate wins.
pub fn pick_reroll(reference: f64, candidates: &[Option<f64>]) -> Option<RerollPick> {
    if !(reference.is_finite() && reference > 0.0) {
        return None;
    }
    candidates
        .iter()
        .enumerate()
        .filter_map(|(k, r)| r.filter(|r| r.is_finite() && *r > 0.0).map(|r| (k, r)))
        .map(|(candidate, r)| (candidate, r / reference - 1.0))
        .fold(None, |best: Option<(usize, f64)>, (k, dev)| match best {
            Some((_, b)) if b.abs() <= dev.abs() => best,
            _ => Some((k, dev)),
        })
        .map(|(candidate, dev)| RerollPick {
            candidate,
            dev,
            replace: dev.abs() <= RATE_OUTLIER,
        })
}
