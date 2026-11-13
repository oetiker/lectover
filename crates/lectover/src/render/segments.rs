//! Word times of the assembled segments (port of `render_post.align_segments`).

use std::io::{self, Write};

use anyhow::Result;

use crate::audio::resample::resample;
use crate::ffi::{Align, FillSpanError};
use crate::plan::Chunk;
use crate::text::spoken_words::spoken_words;
use crate::timings::{place_words, TokenTimes, WordTime};

/// Sample rate the aligner expects.
const ALIGNER_SR: u32 = 16000;

/// Absolute `(start, end)` per spoken word of every chunk, plus interpolation flags.
///
/// Always returns exactly `spoken_words(chunk.text).len()` pairs per chunk; if the aligner's
/// words differ, the whole chunk is interpolated over its duration. Warnings go to stderr.
pub fn align_segments(
    aligner: &mut dyn Align,
    parts: &[(f64, Vec<f32>)],
    sr: u32,
    chunks: &[Chunk],
) -> Result<TokenTimes> {
    align_segments_with(aligner, parts, sr, chunks, &mut io::stderr())
}

/// [`align_segments`] with warnings written to `warn`. Like Python's `zip`, stops at the
/// shorter of `parts` and `chunks`. A [`FillSpanError`] (the aligner could not time every word)
/// interpolates the chunk like a word mismatch; any other aligner error is returned.
pub fn align_segments_with(
    aligner: &mut dyn Align,
    parts: &[(f64, Vec<f32>)],
    sr: u32,
    chunks: &[Chunk],
    warn: &mut dyn Write,
) -> Result<TokenTimes> {
    let mut times = Vec::with_capacity(chunks.len());
    let mut flags = Vec::with_capacity(chunks.len());
    for (i, ((offset, wav), chunk)) in parts.iter().zip(chunks).enumerate() {
        let words = spoken_words(&chunk.text);
        let aligned = match aligner.align(&resample(wav, sr, ALIGNER_SR), &words) {
            Ok(result) if result.iter().map(|(w, _, _)| w).eq(words.iter()) => {
                Some(result.iter().map(|&(_, s, e)| (s, e)).collect())
            }
            Ok(result) => {
                writeln!(
                    warn,
                    "WARNING: chunk {}: aligner returned {} words, expected {}; interpolating its word times",
                    i + 1,
                    result.len(),
                    words.len()
                )?;
                None
            }
            Err(err) => match err.downcast_ref::<FillSpanError>() {
                Some(fill) => {
                    writeln!(
                        warn,
                        "WARNING: chunk {}: {fill}; interpolating its word times",
                        i + 1
                    )?;
                    None
                }
                None => return Err(err),
            },
        };
        // never placeable -> spread over the chunk
        let items: Vec<WordTime> = aligned.unwrap_or_else(|| vec![(0.0, 0.0); words.len()]);
        let (local, interp) = place_words(&items, &words, wav.len() as f64 / sr as f64);
        times.push(
            local
                .into_iter()
                .map(|(s, e)| (offset + s, offset + e))
                .collect(),
        );
        flags.push(interp);
    }
    Ok((times, flags))
}
