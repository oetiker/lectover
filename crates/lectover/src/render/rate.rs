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
//! confirm it. This only reports; nothing is corrected.

use crate::plan::ChunkKind;

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
    /// Voiced rate relative to the neighbours' median, minus 1.
    pub voiced_dev: f64,
    /// Eligible neighbours the reference was taken from.
    pub neighbours: usize,
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
        let aligner_dev = c.aligner / median(&mut aligner) - 1.0;
        let voiced_dev = c.voiced / median(&mut voiced) - 1.0;
        if aligner_dev.abs() > RATE_OUTLIER
            && voiced_dev.abs() > RATE_CONFIRM
            && aligner_dev.signum() == voiced_dev.signum()
        {
            out.push(RateOutlier {
                index: c.index,
                aligner_dev,
                voiced_dev,
                neighbours: near.len(),
            });
        }
    }
    out
}
