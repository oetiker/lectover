//! Speaking-rate outliers against the neighbouring chunks.

use lectover::plan::ChunkKind;
use lectover::render::{
    rate_outliers, ChunkRate, RATE_CONFIRM, RATE_MIN_SYLLABLES, RATE_NEIGHBOURS, RATE_OUTLIER,
};

/// Paragraph chunks with the given (aligner, voiced) rates, 60 syllables each.
fn chunks(rates: &[(f64, f64)]) -> Vec<ChunkRate> {
    rates
        .iter()
        .enumerate()
        .map(|(index, &(aligner, voiced))| ChunkRate {
            index,
            kind: ChunkKind::Paragraph,
            syllables: 60,
            aligner,
            voiced,
        })
        .collect()
}

/// `n` chunks at 5 syl/s (aligner) and 4 syl/s (voiced), with `(i, factor_aligner,
/// factor_voiced)` applied.
fn even(n: usize, changes: &[(usize, f64, f64)]) -> Vec<ChunkRate> {
    let mut rates = vec![(5.0, 4.0); n];
    for &(i, fa, fv) in changes {
        rates[i] = (5.0 * fa, 4.0 * fv);
    }
    chunks(&rates)
}

fn flagged(chunks: &[ChunkRate]) -> Vec<usize> {
    rate_outliers(chunks).iter().map(|o| o.index).collect()
}

#[test]
fn consts_match_the_documented_choice() {
    assert_eq!(RATE_NEIGHBOURS, 2);
    assert_eq!(RATE_MIN_SYLLABLES, 40);
    assert_eq!(RATE_OUTLIER, 0.10);
    assert_eq!(RATE_CONFIRM, 0.05);
}

#[test]
fn even_post_has_no_outliers() {
    assert!(rate_outliers(&even(9, &[])).is_empty());
}

#[test]
fn isolated_fast_spike_is_flagged() {
    let out = rate_outliers(&even(9, &[(4, 1.12, 1.08)]));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].index, 4);
    assert!((out[0].aligner_dev - 0.12).abs() < 1e-9);
    assert!((out[0].voiced_dev - 0.08).abs() < 1e-9);
    assert_eq!(out[0].neighbours, 4);
}

#[test]
fn isolated_slow_spike_is_flagged() {
    let out = rate_outliers(&even(9, &[(5, 0.86, 0.90)]));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].index, 5);
    assert!(out[0].aligner_dev < -RATE_OUTLIER);
    assert!(out[0].voiced_dev < -RATE_CONFIRM);
}

#[test]
fn spike_just_inside_the_threshold_is_not_flagged() {
    assert!(flagged(&even(9, &[(4, 1.09, 1.09)])).is_empty());
}

#[test]
fn slower_region_is_not_flagged() {
    // three adjacent chunks all 6 % slow: each is near its slow neighbours
    let slow = [(3, 0.94, 0.94), (4, 0.94, 0.94), (5, 0.94, 0.94)];
    assert!(flagged(&even(10, &slow)).is_empty());
    let two = [(3, 0.94, 0.94), (4, 0.94, 0.94)];
    assert!(flagged(&even(10, &two)).is_empty());
}

#[test]
fn gradual_drift_is_not_flagged() {
    // 3 % slower per chunk: 24 % over the post, never a jump
    let rates: Vec<(f64, f64)> = (0..9)
        .map(|i| {
            let f = 0.97f64.powi(i);
            (5.0 * f, 4.0 * f)
        })
        .collect();
    assert!(flagged(&chunks(&rates)).is_empty());
}

#[test]
fn opposite_signs_are_not_flagged() {
    assert!(flagged(&even(9, &[(4, 1.15, 0.92)])).is_empty());
    assert!(flagged(&even(9, &[(4, 0.85, 1.08)])).is_empty());
}

#[test]
fn weak_voiced_confirmation_is_not_flagged() {
    assert!(flagged(&even(9, &[(4, 1.15, 1.04)])).is_empty());
    // the voiced rate alone never flags
    assert!(flagged(&even(9, &[(4, 1.05, 1.20)])).is_empty());
}

#[test]
fn titles_and_short_chunks_are_skipped_and_not_neighbours() {
    let mut c = even(9, &[]);
    // a fast title and a fast short chunk: not flagged themselves
    c[1].kind = ChunkKind::Title;
    c[1].aligner = 8.0;
    c[1].voiced = 7.0;
    c[6].syllables = RATE_MIN_SYLLABLES - 1;
    c[6].aligner = 8.0;
    c[6].voiced = 7.0;
    assert!(flagged(&c).is_empty());

    // as neighbours they would drag the reference up and hide a fast spike: they do not
    let mut c = even(9, &[(4, 1.12, 1.08)]);
    for i in [3, 5] {
        c[i].syllables = 10;
        c[i].aligner = 10.0;
        c[i].voiced = 10.0;
    }
    c[2].kind = ChunkKind::Title;
    c[2].aligner = 10.0;
    c[2].voiced = 10.0;
    let out = rate_outliers(&c);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].index, 4);
    // the next eligible ones on each side: 0, 1 before; 6, 7 after
    assert_eq!(out[0].neighbours, 4);
}

#[test]
fn chunk_at_exactly_the_minimum_counts() {
    let mut c = even(5, &[(2, 1.2, 1.2)]);
    c[2].syllables = RATE_MIN_SYLLABLES;
    assert_eq!(flagged(&c), vec![2]);
}

#[test]
fn unmeasurable_rates_are_skipped() {
    let mut c = even(9, &[(4, 1.2, 1.2)]);
    c[4].aligner = f64::NAN;
    assert!(flagged(&c).is_empty());
    let mut c = even(9, &[(4, 1.2, 1.2)]);
    c[4].voiced = f64::INFINITY;
    assert!(flagged(&c).is_empty());
}

#[test]
fn edges_use_the_neighbours_they_have() {
    // first chunk: only 2 after; second: 1 before + 2 after
    let out = rate_outliers(&even(6, &[(0, 1.15, 1.10)]));
    assert_eq!(out.len(), 1);
    assert_eq!((out[0].index, out[0].neighbours), (0, 2));
    let out = rate_outliers(&even(6, &[(1, 0.85, 0.90)]));
    assert_eq!((out[0].index, out[0].neighbours), (1, 3));
    let out = rate_outliers(&even(6, &[(5, 1.15, 1.10)]));
    assert_eq!((out[0].index, out[0].neighbours), (5, 2));
}

#[test]
fn too_few_neighbours_are_skipped() {
    // two eligible chunks: each has only one neighbour
    assert!(flagged(&even(2, &[(0, 1.5, 1.5)])).is_empty());
    // three: each has two (with two neighbours the median is their mean, so the spike also
    // pulls its neighbours' reference away from them)
    assert!(flagged(&even(3, &[(1, 1.5, 1.5)])).contains(&1));
}

#[test]
fn spike_in_the_reference_does_not_flag_its_neighbours() {
    // the median keeps one fast neighbour from making the others look slow
    assert_eq!(flagged(&even(9, &[(4, 1.25, 1.25)])), vec![4]);
}
