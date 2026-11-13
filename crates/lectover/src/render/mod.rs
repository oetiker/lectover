//! Render loop: synthesise checked takes, then align the assembled segments.

mod cache;
mod carrier;
pub mod run;
mod segments;
mod takes;

pub use cache::TakeCache;
pub use carrier::{
    pause_cut, with_carrier, CarrierEnding, Ending, ALIGN_SLACK_SECS, CARRIER, CUT_FADE_SECS,
    CUT_INTO_PAUSE_SECS, MIN_PAUSE_SECS, PAUSE_ABOVE_FLOOR_DB, PAUSE_BELOW_PEAK_DB,
    PAUSE_MAX_BELOW_PEAK_DB,
};
pub use segments::{align_segments, align_segments_with};
pub use takes::{
    synthesize_chunks, synthesize_chunks_with, Take, MAX_CANDIDATES, MAX_ROUNDS, MIN_SCORE,
};
