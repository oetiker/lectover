//! Audio post-processing: trim/level/pace the TTS takes, join them, encode Ogg/Opus.

pub mod assemble;
pub mod denoise;
pub mod mp3;
pub mod opus;
pub mod resample;
pub mod stretch;
pub mod wav;

pub use assemble::{
    apply_trim, assemble_parts, assemble_parts_with, level_segments, level_sting, quiet_ending, room_tone, trim_and_level, trim_and_level_with, Trim, FADE_IN_SECS, FADE_OUT_SECS, PACE_MAX, PACE_MIN,
    ROOM_TONE_DB, SPEECH_TARGET_DB, START_LEAD_SECS, TRIM_BELOW_SPEECH_DB, TRIM_KEEP_SECS, TRIM_KEEP_TAIL_SECS,
};
pub use mp3::encode_mp3;
pub use opus::encode_opus;
pub use resample::resample;
pub use stretch::stretch;
pub use wav::read_wav;
