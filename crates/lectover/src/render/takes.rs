//! Synthesise every chunk, re-roll takes the ASR check rejects, keep the best
//! (port of `read_qwen.synthesize_chunks`).

use std::collections::HashMap;
use std::io::{self, Write};

use anyhow::{bail, Result};

use crate::check::score;
use crate::ffi::{Synth, TakeRequest, Transcribe};
use crate::plan::{bucket_for, BUCKETS};
use crate::text::normalize::Lexicon;

use super::carrier::Ending;

pub const MIN_SCORE: f64 = 0.90;
pub const MAX_ROUNDS: usize = 4;
/// Most takes of one chunk per round. More cost more than they save: each is scored by the
/// ASR one after the other (~2.4 s per 40 s take), and of 10 candidates of a hard 40 s
/// paragraph 4 passed (a recorded voice).
pub const MAX_CANDIDATES: usize = 3;
/// Each re-roll samples this much cooler than the round before (narrower, steadier takes) ...
pub const RETRY_COOLING: f32 = 0.1;
/// ... but never below this.
pub const MIN_RETRY_TEMPERATURE: f32 = 0.5;

/// Talker temperature for round `rnd` (0-based): `temperature`, then [`RETRY_COOLING`] less
/// per round, floored at [`MIN_RETRY_TEMPERATURE`] (or `temperature` if that is lower).
pub fn round_temperature(temperature: f32, rnd: usize) -> f32 {
    (temperature - RETRY_COOLING * rnd as f32).max(MIN_RETRY_TEMPERATURE.min(temperature))
}

/// Python's `round(x, 2)` (correctly rounded, ties to even; see `timings::round3`).
fn round2(x: f64) -> f64 {
    format!("{x:.2}").parse().unwrap_or(x)
}

/// One checked take; `score` is already rounded to two decimals.
#[derive(Debug, Clone)]
pub struct Take {
    pub score: f64,
    pub wav: Vec<f32>,
    pub heard: String,
    /// The [`Ending`] found no clean cut (e.g. no pause before the carrier).
    pub clipped: bool,
}

impl Take {
    pub fn new(score: f64, wav: Vec<f32>, heard: String) -> Self {
        Take {
            score: round2(score),
            wav,
            heard,
            clipped: false,
        }
    }

    /// Good enough to keep without another round.
    pub fn accepted(&self) -> bool {
        self.score >= MIN_SCORE && !self.clipped
    }

    /// Ranking: passing the ASR check first, then a clean ending, then the
    /// Python order `(round(score, 2), -len(wav))`: higher score, then shorter; ties lose.
    pub fn beats(&self, other: &Take) -> bool {
        let key = |t: &Take| (t.score >= MIN_SCORE, !t.clipped);
        let (a, b) = (key(self), key(other));
        if a != b {
            return a > b;
        }
        self.score > other.score || (self.score == other.score && self.wav.len() < other.wav.len())
    }
}

/// Synthesise `texts`; `budgets[i]` is the token budget of `ending.tts_text(texts[i])`
/// (`plan::max_tokens_for`). Progress goes to stdout, warnings to stderr.
#[allow(clippy::too_many_arguments)]
pub fn synthesize_chunks(
    tts: &mut dyn Synth,
    asr: &mut dyn Transcribe,
    ending: &mut dyn Ending,
    texts: &[String],
    budgets: &[usize],
    seed: u64,
    temperature: f32,
    lex: &Lexicon,
) -> Result<Vec<Vec<f32>>> {
    synthesize_chunks_with(
        tts,
        asr,
        ending,
        texts,
        budgets,
        seed,
        temperature,
        lex,
        &mut io::stdout(),
        &mut io::stderr(),
    )
}

/// [`synthesize_chunks`] with progress written to `log` and warnings to `warn`.
///
/// Every chunk is synthesised as `ending.tts_text(text)`; each take, its tail padding removed,
/// goes through [`Ending::finish`], and the finished take is scored against the chunk text.
///
/// Each round re-generates the chunks not yet [`Take::accepted`] in one `generate` call,
/// each with its bucket's token budget, sampled at [`round_temperature`]. Spare batch slots
/// ([`Synth::max_batch`]) go to extra candidates: every chunk of the round gets
/// `max_batch / chunks` takes, at least 1 and at most [`MAX_CANDIDATES`], candidate `j` seeded `seed + round + MAX_ROUNDS * j`
/// (so candidate 0 keeps the `seed + round` of an unbatched render). Identical requests
/// (a heading repeated in the post) are generated and scored once.
#[allow(clippy::too_many_arguments)]
pub fn synthesize_chunks_with(
    tts: &mut dyn Synth,
    asr: &mut dyn Transcribe,
    ending: &mut dyn Ending,
    texts: &[String],
    budgets: &[usize],
    seed: u64,
    temperature: f32,
    lex: &Lexicon,
    log: &mut dyn Write,
    warn: &mut dyn Write,
) -> Result<Vec<Vec<f32>>> {
    if budgets.len() != texts.len() {
        bail!("{} token budgets for {} texts", budgets.len(), texts.len());
    }
    let sr = tts.sample_rate();
    let mut best: Vec<Option<Take>> = vec![None; texts.len()];
    let mut todo: Vec<usize> = (0..texts.len()).collect();
    for rnd in 0..MAX_ROUNDS {
        let per_chunk = (tts.max_batch() / todo.len()).clamp(1, MAX_CANDIDATES);
        if per_chunk == 1 {
            writeln!(log, "round {}: generating {} chunk(s)", rnd + 1, todo.len())?;
        } else {
            writeln!(
                log,
                "round {}: generating {} chunk(s), {per_chunk} candidates each",
                rnd + 1,
                todo.len()
            )?;
        }
        // (chunk, index into `reqs`), in bucket order as the log always listed them.
        let mut cands: Vec<(usize, usize)> = Vec::new();
        let mut reqs: Vec<TakeRequest> = Vec::new();
        let mut seen: HashMap<TakeRequest, usize> = HashMap::new();
        for bucket in BUCKETS {
            for &i in todo.iter().filter(|&&i| bucket_for(budgets[i]) == bucket) {
                for j in 0..per_chunk {
                    let req = TakeRequest::new(
                        ending.tts_text(&texts[i]),
                        bucket,
                        seed + (rnd + MAX_ROUNDS * j) as u64,
                    );
                    let slot = *seen.entry(req.clone()).or_insert_with(|| {
                        reqs.push(req);
                        reqs.len() - 1
                    });
                    cands.push((i, slot));
                }
            }
        }
        // The chunk whose text judges each request (the first that asked for it).
        let mut slot_chunk: Vec<Option<usize>> = vec![None; reqs.len()];
        for &(idx, slot) in &cands {
            slot_chunk[slot].get_or_insert(idx);
        }
        // Each take with its unrounded score (the log shows three decimals). Takes are judged
        // as the engine finishes them, so the ASR check runs while the TTS still decodes.
        let mut judged: Vec<Option<(Take, f64)>> = vec![None; reqs.len()];
        let pad = tts.tail_pad_samples();
        let wavs = tts.generate_each(
            &reqs,
            round_temperature(temperature, rnd),
            &mut |slot, raw| {
                let text = &texts[slot_chunk[slot].expect("every request belongs to a chunk")];
                let body = &raw[..raw.len().saturating_sub(pad)];
                let (wav, clean) = ending.finish(body, sr, text)?;
                let (s, heard) = score(asr, &wav, sr, text, lex)?;
                judged[slot] = Some((
                    Take {
                        clipped: !clean,
                        ..Take::new(s, wav, heard)
                    },
                    s,
                ));
                Ok(())
            },
        )?;
        if wavs.len() != reqs.len() {
            bail!(
                "synthesis returned {} takes for {} requests",
                wavs.len(),
                reqs.len()
            );
        }
        // Pick and log in request order, independent of the order the takes finished in.
        for (idx, slot) in cands {
            let (cand, raw) = judged[slot].clone().expect("every take was judged");
            writeln!(
                log,
                "  [{}/{}] {:5.1}s score {raw:.3}{}",
                idx + 1,
                texts.len(),
                cand.wav.len() as f64 / sr as f64,
                if cand.clipped { " (no clean cut)" } else { "" }
            )?;
            if best[idx].as_ref().is_none_or(|b| cand.beats(b)) {
                best[idx] = Some(cand);
            }
        }
        todo.retain(|&i| best[i].as_ref().is_some_and(|b| !b.accepted()));
        if todo.is_empty() {
            break;
        }
    }
    let best: Vec<Take> = best
        .into_iter()
        .map(|b| b.expect("every text was generated"))
        .collect();
    for &i in &todo {
        if best[i].score < MIN_SCORE {
            writeln!(
                warn,
                "WARNING: chunk {} best score {:.2}, heard: {}",
                i + 1,
                best[i].score,
                best[i].heard
            )?;
        }
        if best[i].clipped {
            writeln!(
                warn,
                "WARNING: chunk {} no clean cut before the carrier in any take: {}",
                i + 1,
                texts[i]
            )?;
        }
    }
    Ok(best.into_iter().map(|b| b.wav).collect())
}
