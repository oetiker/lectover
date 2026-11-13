//! Carrier ending: every chunk is synthesised with [`CARRIER`] after it and the take is cut in
//! the pause before the carrier. The model's end-of-speech then never falls on the chunk's last
//! sound: ending on it, Qwen3-TTS often stopped inside a final plosive's closure and dropped
//! the burst ("Der Entscheid" without its t), and no check on the take's last frames could
//! tell such a take from a clean one (listening tests with cloned voices).

use anyhow::Result;

use crate::audio::resample;
use crate::ffi::Align;
use crate::plan::ends_sentence;
use crate::text::spoken_words::spoken_words;

/// Spoken after every chunk and cut off again. The ellipsis plus the paragraph break makes the
/// model pause before it: measured 330..1120 ms over 18 takes (a cloned voice), against 50..730 ms
/// for ". Ja." and 110..670 ms for a paragraph break alone. "A new section starts" also keeps
/// the chunk's own ending intonation final.
pub const CARRIER: &str = " …\n\nNeues Kapitel.";
/// A 10 ms frame this far below the take's loudest frame is always pause (room noise of a
/// clean voice clip sits at about -55..-60 dB) ...
pub const PAUSE_BELOW_PEAK_DB: f64 = 50.0;
/// ... and in a noisier take, a frame up to [`PAUSE_ABOVE_FLOOR_DB`] above the search window's
/// floor (its 10th percentile frame) is too, but never one louder than this below the peak
/// (a decaying plosive release measured -35..-45 dB).
pub const PAUSE_MAX_BELOW_PEAK_DB: f64 = 40.0;
/// See [`PAUSE_MAX_BELOW_PEAK_DB`]. "Die Debatte." (a cloned voice, seed 42) paused 950 ms in noise
/// wobbling around -50 dB.
pub const PAUSE_ABOVE_FLOOR_DB: f64 = 6.0;
/// A shorter pause leaves no room to cut: a final plosive released late runs into the carrier.
pub const MIN_PAUSE_SECS: f64 = 0.15;
/// The cut lands this far into the pause at most (half the pause if that is shorter), well
/// after a late plosive release and before any breath ahead of the carrier.
pub const CUT_INTO_PAUSE_SECS: f64 = 0.15;
/// The forced aligner's timestamps come in 80 ms steps; the carrier's first sound may lie up
/// to one step after its aligned start.
pub const ALIGN_SLACK_SECS: f64 = 0.08;
/// Fade-out ending at the cut (it lies in the pause, so it only smooths room noise).
pub const CUT_FADE_SECS: f64 = 0.03;

const FRAME_SECS: f64 = 0.01;

/// `text` followed by [`CARRIER`]; a text without a sentence end gets a full stop first.
pub fn with_carrier(text: &str) -> String {
    let text = text.trim_end();
    if ends_sentence(text) {
        format!("{text}{CARRIER}")
    } else {
        format!("{text}.{CARRIER}")
    }
}

/// Runs of pause frames in `wav[from..to]` as `(start sample, length in samples)`, in order.
/// A frame is pause below a threshold between [`PAUSE_BELOW_PEAK_DB`] and
/// [`PAUSE_MAX_BELOW_PEAK_DB`] under the peak, following the window's noise floor.
fn pauses(wav: &[f32], sr: u32, from: usize, to: usize) -> Vec<(usize, usize)> {
    let frame = ((sr as f64 * FRAME_SECS) as usize).max(1);
    let energy = |k: usize| {
        wav[k..k + frame]
            .iter()
            .map(|v| (*v as f64).powi(2))
            .sum::<f64>()
    };
    let db = |e: f64| 10.0 * (e + 1e-20).log10();
    let peak = db((0..wav.len() / frame)
        .map(|i| energy(i * frame))
        .fold(0.0, f64::max));
    let to = to.min(wav.len());
    let mut window: Vec<f64> = (from..to.saturating_sub(frame - 1))
        .step_by(frame)
        .map(|k| db(energy(k)))
        .collect();
    if window.is_empty() {
        return Vec::new();
    }
    window.sort_by(f64::total_cmp);
    let room = window[window.len() / 10] + PAUSE_ABOVE_FLOOR_DB;
    let threshold = room.clamp(peak - PAUSE_BELOW_PEAK_DB, peak - PAUSE_MAX_BELOW_PEAK_DB);
    let floor = 10f64.powf(threshold / 10.0);
    let mut runs = Vec::new();
    let mut start = None;
    let mut k = from;
    while k + frame <= to {
        match (energy(k) < floor, start) {
            (true, None) => start = Some(k),
            (false, Some(s)) => {
                runs.push((s, k - s));
                start = None;
            }
            _ => {}
        }
        k += frame;
    }
    if let Some(s) = start {
        runs.push((s, k - s));
    }
    runs
}

/// Where to cut a pause found at `(start, len)`: [`CUT_INTO_PAUSE_SECS`] in, or half of it.
fn cut_in(sr: u32, (start, len): (usize, usize)) -> usize {
    start + (len / 2).min((sr as f64 * CUT_INTO_PAUSE_SECS) as usize)
}

fn long_enough(sr: u32, (_, len): &(usize, usize)) -> bool {
    *len >= (sr as f64 * MIN_PAUSE_SECS).round() as usize
}

/// Cut point in the longest pause within `wav[from..to]`, or `None` when it is shorter than
/// [`MIN_PAUSE_SECS`].
pub fn pause_cut(wav: &[f32], sr: u32, from: usize, to: usize) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for run in pauses(wav, sr, from, to) {
        if best.is_none_or(|b| run.1 > b.1) {
            best = Some(run);
        }
    }
    best.filter(|r| long_enough(sr, r)).map(|r| cut_in(sr, r))
}

/// How a take's synthesised end becomes the chunk's end.
pub trait Ending {
    /// The text to synthesise for chunk `text`.
    fn tts_text(&self, text: &str) -> String;
    /// The take for chunk `text` from the raw synthesis `wav` (tail padding removed), and
    /// whether its ending is clean. An unclean take is re-rolled; if every take is unclean the
    /// best is used anyway.
    fn finish(&mut self, wav: &[f32], sr: u32, text: &str) -> Result<(Vec<f32>, bool)>;
}

/// The [`CARRIER`] ending: the forced aligner finds the last word of the chunk and the
/// carrier's start; the take is cut in the longest pause between them ([`pause_cut`]).
pub struct CarrierEnding<'a> {
    aligner: &'a mut dyn Align,
}

impl<'a> CarrierEnding<'a> {
    pub fn new(aligner: &'a mut dyn Align) -> Self {
        CarrierEnding { aligner }
    }

    /// (start of the chunk's last word, start of the carrier) in samples, if the aligner timed
    /// every word. The last word's start, not its end: the aligner may stretch a word ending
    /// in a weak vowel over the pause after it ("Debatte", 1.3 s).
    fn boundary(&mut self, wav: &[f32], sr: u32, text: &str) -> Option<(usize, usize)> {
        let own = spoken_words(text);
        let mut words = own.clone();
        words.extend(spoken_words(CARRIER));
        let spans = self.aligner.align(&resample(wav, sr, 16000), &words).ok()?;
        if own.is_empty() || !spans.iter().map(|(w, _, _)| w).eq(words.iter()) {
            return None;
        }
        let at = |s: f64| ((s * sr as f64) as usize).min(wav.len());
        Some((at(spans[own.len() - 1].1), at(spans[own.len()].1)))
    }
}

impl Ending for CarrierEnding<'_> {
    fn tts_text(&self, text: &str) -> String {
        with_carrier(text)
    }

    fn finish(&mut self, wav: &[f32], sr: u32, text: &str) -> Result<(Vec<f32>, bool)> {
        let (cut, clean) = match self.boundary(wav, sr, text) {
            Some((last_word, carrier)) => {
                let slack = (sr as f64 * ALIGN_SLACK_SECS) as usize;
                match pause_cut(wav, sr, last_word, carrier + slack) {
                    Some(cut) => (cut, true),
                    None => (carrier, false),
                }
            }
            // Unaligned: the carrier still follows the last long pause of the take.
            None => {
                let last = pauses(wav, sr, 0, wav.len())
                    .into_iter()
                    .rev()
                    .find(|r| long_enough(sr, r));
                (last.map_or(wav.len(), |r| cut_in(sr, r)), false)
            }
        };
        let mut out = wav[..cut].to_vec();
        let n = ((sr as f64 * CUT_FADE_SECS) as usize).min(out.len());
        let len = out.len();
        for (i, v) in out[len - n..].iter_mut().enumerate() {
            let t = (i + 1) as f64 / n as f64 * std::f64::consts::FRAC_PI_2;
            *v *= t.cos().powi(2) as f32;
        }
        Ok((out, clean))
    }
}
