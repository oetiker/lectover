//! Render loop: synthesise checked takes, then align the assembled segments.

mod cache;
mod carrier;
mod rate;
pub mod run;
mod segments;
mod takes;

pub use cache::{prune, CallStats, MemoAlign, MemoAsr, PruneReport, TakeCache};
pub use carrier::{
    pause_cut, with_carrier, CarrierEnding, Ending, ALIGN_SLACK_SECS, CARRIER, CUT_FADE_SECS,
    CUT_INTO_PAUSE_SECS, MIN_PAUSE_SECS, PAUSE_ABOVE_FLOOR_DB, PAUSE_BELOW_PEAK_DB,
    PAUSE_MAX_BELOW_PEAK_DB,
};
pub use rate::{
    chunk_rate, measure_take, pick_reroll, rate_outliers, ChunkRate, RateOutlier, RerollPick,
    RATE_CONFIRM, RATE_MIN_NEIGHBOURS, RATE_MIN_SYLLABLES, RATE_NEIGHBOURS, RATE_OUTLIER,
};
pub use segments::{align_segments, align_segments_with};
pub use takes::{
    reroll_seed, reroll_takes, synthesize_chunks, synthesize_chunks_with, take_seed, ChunkRequest,
    Take, MAX_CANDIDATES, MAX_ROUNDS, MIN_SCORE, REROLL_CANDIDATES, REROLL_SEED_OFFSET,
};
