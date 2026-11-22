//! Tempo re-roll: new takes for the chunks whose tempo stands out, the closest kept.

use anyhow::Result;
use lectover::audio::place_take;
use lectover::ffi::{Align, Synth, TakeRequest, Transcribe};
use lectover::plan::{Chunk, ChunkKind};
use lectover::render::{
    measure_take, pick_reroll, reroll_seed, reroll_takes, take_seed, ChunkRequest, Ending,
    MemoAlign, MemoAsr, RerollPick, TakeCache, MAX_CANDIDATES, MAX_ROUNDS, RATE_OUTLIER,
    REROLL_CANDIDATES, REROLL_SEED_OFFSET,
};
use lectover::text::normalize::Lexicon;

#[test]
fn closest_candidate_within_the_threshold_replaces() {
    // reference 5.0: +8 %, -4 %, +6 %
    let pick = pick_reroll(5.0, &[Some(5.4), Some(4.8), Some(5.3)]).unwrap();
    assert_eq!(pick.candidate, 1);
    assert!((pick.dev + 0.04).abs() < 1e-9);
    assert!(pick.replace);
}

#[test]
fn no_candidate_within_the_threshold_keeps_the_original() {
    // +14 %, -14 %, +12 %
    let pick = pick_reroll(5.0, &[Some(5.7), Some(4.3), Some(5.6)]).unwrap();
    // the closest is still reported, but not used
    assert_eq!(pick.candidate, 2);
    assert!((pick.dev - 0.12).abs() < 1e-9);
    assert!(!pick.replace);
    // just inside the threshold replaces
    let edge = pick_reroll(4.0, &[Some(4.0 * (1.0 + RATE_OUTLIER - 1e-9))]).unwrap();
    assert!(edge.replace);
}

#[test]
fn failed_and_unmeasurable_candidates_are_ignored() {
    // the perfect candidate failed the check, the next best has no aligner rate
    let pick = pick_reroll(
        5.0,
        &[None, Some(f64::NAN), Some(5.25), Some(f64::INFINITY)],
    )
    .unwrap();
    assert_eq!((pick.candidate, pick.replace), (2, true));
    assert!((pick.dev - 0.05).abs() < 1e-9);
    assert_eq!(pick_reroll(5.0, &[None, Some(f64::NAN), Some(0.0)]), None);
    assert_eq!(pick_reroll(5.0, &[]), None);
    assert_eq!(pick_reroll(f64::NAN, &[Some(5.0)]), None);
}

#[test]
fn ties_keep_the_earlier_candidate() {
    let pick = pick_reroll(5.0, &[Some(5.25), Some(4.75)]).unwrap();
    assert_eq!(pick.candidate, 0);
}

#[test]
fn reroll_seeds_never_collide_with_render_seeds() {
    assert_eq!(REROLL_CANDIDATES, 4);
    for seed in [0u64, 1, 42, 1_000_000, 123_456_789] {
        let render: Vec<u64> = (0..MAX_ROUNDS)
            .flat_map(|r| (0..MAX_CANDIDATES).map(move |j| take_seed(seed, r, j)))
            .collect();
        let reroll: Vec<u64> = (0..REROLL_CANDIDATES)
            .map(|k| reroll_seed(seed, k))
            .collect();
        for s in &reroll {
            assert!(
                !render.contains(s),
                "seed {seed}: re-roll seed {s} collides"
            );
        }
        // distinct among themselves
        let mut sorted = reroll.clone();
        sorted.dedup();
        assert_eq!(sorted.len(), REROLL_CANDIDATES);
        assert!(render.iter().all(|s| *s < seed + REROLL_SEED_OFFSET));
    }
}

const SR: u32 = 16000;
const SEED: u64 = 42;
/// 4 syllables ("eins zwei drei vier", one vowel run each).
const TEXT: &str = "eins zwei drei vier";

/// Takes of constant level whose length depends on the re-roll candidate: candidate `k`
/// lasts `SECS[k]` seconds. Counts the takes it synthesises.
struct FakeSynth {
    calls: usize,
}

const SECS: [f64; REROLL_CANDIDATES] = [0.75, 1.0, 0.95, 1.05];

impl Synth for FakeSynth {
    fn sample_rate(&self) -> u32 {
        SR
    }
    fn generate(&mut self, reqs: &[TakeRequest], _temperature: f32) -> Result<Vec<Vec<f32>>> {
        self.calls += reqs.len();
        Ok(reqs
            .iter()
            .map(|r| {
                let k = (r.seed - reroll_seed(SEED, 0)) as usize;
                vec![0.3; (SECS[k] * SR as f64) as usize]
            })
            .collect())
    }
    fn fingerprint(&self) -> Option<String> {
        Some("tts".into())
    }
}

/// Hears the text, except in the one-second take (candidate 1), which it mishears.
struct FakeAsr {
    calls: usize,
}

impl Transcribe for FakeAsr {
    fn transcribe(&mut self, pcm16k: &[f32]) -> Result<String> {
        self.calls += 1;
        Ok(if pcm16k.len() == SR as usize {
            "falsch".into()
        } else {
            TEXT.into()
        })
    }
    fn fingerprint(&self) -> Option<String> {
        Some("asr".into())
    }
}

/// Spreads the words over the whole audio: the aligner rate is syllables over its length.
struct FakeAligner {
    calls: usize,
}

impl Align for FakeAligner {
    fn align(&mut self, pcm16k: &[f32], words: &[String]) -> Result<Vec<(String, f64, f64)>> {
        self.calls += 1;
        let step = pcm16k.len() as f64 / 16000.0 / words.len() as f64;
        Ok(words
            .iter()
            .enumerate()
            .map(|(i, w)| (w.clone(), i as f64 * step, (i + 1) as f64 * step))
            .collect())
    }
    fn fingerprint(&self) -> Option<String> {
        Some("align".into())
    }
}

struct PlainEnding;

impl Ending for PlainEnding {
    fn tts_text(&self, text: &str) -> String {
        text.into()
    }
    fn finish(&mut self, wav: &[f32], _sr: u32, _text: &str) -> Result<(Vec<f32>, bool)> {
        Ok((wav.to_vec(), true))
    }
}

/// One re-roll of chunk 0 against a reference of 4 syl/s: (picked candidate, its seed, log).
fn reroll_once(
    tts: &mut dyn Synth,
    asr: &mut dyn Transcribe,
    al: &mut dyn Align,
) -> (Option<RerollPick>, String) {
    let texts = vec![TEXT.to_string()];
    let chunk = Chunk {
        block: 0,
        g0: 0,
        g1: 4,
        text: TEXT.into(),
        kind: ChunkKind::Paragraph,
    };
    let lex = Lexicon::new();
    let req = ChunkRequest {
        texts: &texts,
        budgets: &[60],
        seed: SEED,
        temperature: 0.9,
        lex: &lex,
    };
    let mut log = Vec::new();
    let cands = reroll_takes(tts, asr, &mut PlainEnding, &req, &[0], &mut log).unwrap();
    assert_eq!(cands.len(), 1);
    assert_eq!(cands[0].len(), REROLL_CANDIDATES);
    let rates: Vec<Option<f64>> = cands[0]
        .iter()
        .map(|c| {
            c.as_ref().map(|t| {
                measure_take(al, 0, &chunk, &place_take(&t.wav, SR), SR)
                    .unwrap()
                    .aligner
            })
        })
        .collect();
    (pick_reroll(4.0, &rates), String::from_utf8(log).unwrap())
}

#[test]
fn reroll_keeps_the_closest_passing_take_and_repeats_from_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let (mut tts, mut asr, mut al) = (
        FakeSynth { calls: 0 },
        FakeAsr { calls: 0 },
        FakeAligner { calls: 0 },
    );
    let (pick, log) = reroll_once(
        &mut TakeCache::new(&mut tts, dir.path()),
        &mut MemoAsr::new(&mut asr, dir.path()),
        &mut MemoAlign::new(&mut al, dir.path()),
    );
    // candidate 1 (exactly 4 syl/s) failed the check; 0.95 s is +5 %, 1.05 s is -5 % (closer)
    let pick = pick.unwrap();
    assert_eq!(pick.candidate, 3);
    assert!(pick.replace);
    assert!((pick.dev - (1.0 / 1.05 - 1.0)).abs() < 1e-3, "{pick:?}");
    assert!(log.starts_with(
        "tempo re-roll: 1 chunk(s), 4 candidates each: 4 take(s) to synthesise, 0 cached\n"
    ));
    assert!(log.contains(&format!("seed {} ", reroll_seed(SEED, 3))));
    // the failed candidate is never aligned
    assert_eq!((tts.calls, asr.calls, al.calls), (4, 4, 3));

    // the same re-roll again: every answer comes from the cache
    let (again, log) = reroll_once(
        &mut TakeCache::new(&mut tts, dir.path()),
        &mut MemoAsr::new(&mut asr, dir.path()),
        &mut MemoAlign::new(&mut al, dir.path()),
    );
    assert_eq!(again, Some(pick));
    assert!(log.contains("0 take(s) to synthesise, 4 cached"));
    assert_eq!((tts.calls, asr.calls, al.calls), (4, 4, 3));
}
