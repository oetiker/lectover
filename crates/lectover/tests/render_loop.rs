//! Port of tests/test_render_post.py (take loop and segment alignment) plus difflib parity.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use anyhow::Result;
use lectover::check::{key, ratio, score};
use lectover::ffi::{Align, FillSpanError, Synth, TakeRequest, Transcribe};
use lectover::plan::{Chunk, ChunkKind};
use lectover::render::{
    align_segments_with, synthesize_chunks_with, ChunkRequest, Ending, Take, MAX_CANDIDATES,
    MAX_ROUNDS, MIN_SCORE,
};
use lectover::text::normalize::Lexicon;
use lectover::text::spoken_words::spoken_words;

const SR: u32 = 1000;

/// What the fake ASR will "hear", one entry per generated take, in generation order.
type Heard = Rc<RefCell<VecDeque<String>>>;

/// Fake TTS: a take of `10 * (offset + 1)` ones, offset = seed - first seed seen
/// (the round, for a single candidate per chunk).
struct FakeSynth {
    /// Per `generate` call: (max_tokens, text, seed) of every request.
    calls: Vec<Vec<(usize, String, u64)>>,
    temperatures: Vec<f32>,
    first_seed: Option<u64>,
    heard: Heard,
    /// (text, seed offset) -> what the ASR hears for that take
    hear: fn(&str, u64) -> String,
    max_batch: usize,
}

impl FakeSynth {
    fn new(heard: Heard, hear: fn(&str, u64) -> String) -> Self {
        FakeSynth {
            calls: Vec::new(),
            temperatures: Vec::new(),
            first_seed: None,
            heard,
            hear,
            max_batch: 1,
        }
    }
}

impl Synth for FakeSynth {
    fn sample_rate(&self) -> u32 {
        SR
    }

    fn generate(&mut self, reqs: &[TakeRequest], temperature: f32) -> Result<Vec<Vec<f32>>> {
        self.temperatures.push(temperature);
        self.calls.push(
            reqs.iter()
                .map(|r| (r.max_tokens, r.text.clone(), r.seed))
                .collect(),
        );
        let first = *self.first_seed.get_or_insert(reqs[0].seed);
        Ok(reqs
            .iter()
            .map(|r| {
                let offset = r.seed - first;
                self.heard
                    .borrow_mut()
                    .push_back((self.hear)(&r.text, offset));
                vec![1.0; 10 * (offset as usize + 1)]
            })
            .collect())
    }

    fn max_batch(&self) -> usize {
        self.max_batch
    }
}

struct FakeAsr {
    heard: Heard,
    calls: usize,
}

impl Transcribe for FakeAsr {
    fn transcribe(&mut self, pcm16k: &[f32]) -> Result<String> {
        assert!(!pcm16k.is_empty());
        self.calls += 1;
        Ok(self
            .heard
            .borrow_mut()
            .pop_front()
            .expect("one transcription per take"))
    }
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// Synthesises the chunk text as is and keeps every take whole and clean.
struct PlainEnding;

impl Ending for PlainEnding {
    fn tts_text(&self, text: &str) -> String {
        text.into()
    }
    fn finish(&mut self, wav: &[f32], _sr: u32, _text: &str) -> Result<(Vec<f32>, bool)> {
        Ok((wav.to_vec(), true))
    }
}

#[test]
fn synthesize_chunks_buckets_and_retries() {
    let heard: Heard = Rc::default();
    // "schlecht" is misheard in round 0 only; every other take is heard verbatim
    let mut tts = FakeSynth::new(heard.clone(), |t, round| {
        if t == "schlecht" && round == 0 {
            "Ganz?".into()
        } else {
            format!(" {t} ")
        }
    });
    let mut asr = FakeAsr { heard, calls: 0 };
    let mut log = Vec::new();
    let mut warn = Vec::new();
    let wavs = synthesize_chunks_with(
        &mut tts,
        &mut asr,
        &mut PlainEnding,
        &ChunkRequest {
            texts: &strings(&["kurz", "schlecht", "lang"]),
            budgets: &[60, 60, 700],
            seed: 42,
            temperature: 0.9,
            lex: &Lexicon::new(),
        },
        &mut log,
        &mut warn,
    )
    .unwrap();
    assert_eq!(
        tts.calls,
        vec![
            vec![
                (100, "kurz".into(), 42),
                (100, "schlecht".into(), 42),
                (1000, "lang".into(), 42)
            ],
            vec![(100, "schlecht".into(), 43)],
        ]
    );
    assert_eq!(
        wavs.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![10, 20, 10]
    );
    assert_eq!(asr.calls, 4);
    assert!(warn.is_empty());
    let log = String::from_utf8(log).unwrap();
    assert_eq!(
        log,
        "round 1: generating 3 chunk(s)\n\
         \x20 [1/3]   0.0s score 1.000\n\
         \x20 [2/3]   0.0s score 0.000\n\
         \x20 [3/3]   0.0s score 1.000\n\
         round 2: generating 1 chunk(s)\n\
         \x20 [2/3]   0.0s score 1.000\n"
    );
}

#[test]
fn synthesize_chunks_warns_on_persistent_failure() {
    let heard: Heard = Rc::default();
    // key "falsch" vs key "axyz": difflib ratio 0.2
    let mut tts = FakeSynth::new(heard.clone(), |_, _| "falsch".into());
    let mut asr = FakeAsr { heard, calls: 0 };
    let mut log = Vec::new();
    let mut warn = Vec::new();
    let wavs = synthesize_chunks_with(
        &mut tts,
        &mut asr,
        &mut PlainEnding,
        &ChunkRequest {
            texts: &strings(&["axyz"]),
            budgets: &[60],
            seed: 1,
            temperature: 0.9,
            lex: &Lexicon::new(),
        },
        &mut log,
        &mut warn,
    )
    .unwrap();
    assert_eq!(tts.calls.len(), MAX_ROUNDS);
    assert_eq!(
        tts.calls.iter().map(|c| c[0].2).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    // each re-roll samples cooler: 0.9, 0.8, 0.7, 0.6
    let temps: Vec<f32> = tts
        .temperatures
        .iter()
        .map(|t| (t * 100.0).round() / 100.0)
        .collect();
    assert_eq!(temps, vec![0.9, 0.8, 0.7, 0.6]);
    assert_eq!(wavs.len(), 1);
    // equal scores keep the first (shortest) take
    assert_eq!(wavs[0].len(), 10);
    assert_eq!(
        String::from_utf8(warn).unwrap(),
        "WARNING: chunk 1 best score 0.20, heard: falsch\n"
    );
}

#[test]
fn best_take_prefers_score_then_shorter() {
    let take = |score: f64, n: usize| Take::new(score, vec![0.0; n], String::new());
    // higher rounded score wins even when longer
    assert!(take(0.95, 100).beats(&take(0.90, 10)));
    assert!(!take(0.90, 10).beats(&take(0.95, 100)));
    // equal rounded score: the shorter take wins (0.904 and 0.896 both round to 0.90)
    assert!(take(0.904, 20).beats(&take(0.896, 30)));
    assert!(take(0.896, 20).beats(&take(0.904, 30)));
    assert!(!take(0.904, 30).beats(&take(0.896, 20)));
    // a full tie does not replace the earlier take
    assert!(!take(0.5, 10).beats(&take(0.5, 10)));
    assert_eq!(take(0.896, 1).score, 0.90);
    assert!(take(0.896, 1).score >= MIN_SCORE);
}

const LONG_A: &str = "indieserkolumnegehtesumdiezukunftderstädtischenbetriebeoltenundumdiefragewaseinefusionfürunserestadtbedeutetdiegemeindemussentscheidenobsieihreanteileverkauftoderbehältvielebürgerinnenundbürgerfragensichweramendediekostenträgtundobdieversorgungmitstromwasserundgassicherbleibtgrüßeausolten";
const LONG_B: &str = "indiserkolumegehtesumdiezukunftderstädtischenbetriebevonoltenundumdiefragewaseinefusionfürunserestadtbedeutenwürdediegemeindemussentscheidenobsiedieanteileverkauftoderbehältvielebürgerfragensichweramschlussdiekostenträgtundobdieversorgungmitstromundwasserundgassicherbleibtgrüsseausolten";

#[test]
fn ratio_matches_difflib() {
    // expected values from `.venv/bin/python -I -c` with difflib.SequenceMatcher(None, a, b, autojunk=...).ratio()
    for autojunk in [true, false] {
        assert_eq!(ratio("abc", "abc", autojunk), 1.0);
        assert_eq!(ratio("abcd", "bcde", autojunk), 0.75);
        assert_eq!(ratio("", "", autojunk), 1.0);
        assert_eq!(ratio("abc", "", autojunk), 0.0);
        assert_eq!(ratio("falsch", "axyz", autojunk), 0.2);
    }
    assert_eq!((LONG_A.chars().count(), LONG_B.chars().count()), (289, 287));
    // autojunk drops popular characters of b (len >= 200)
    assert_eq!(ratio(LONG_A, LONG_B, true), 0.4236111111111111);
    assert_eq!(ratio(LONG_B, LONG_A, true), 0.16666666666666666);
}

#[test]
fn ratio_without_autojunk_matches_difflib() {
    // the path `score` uses: SequenceMatcher(None, a, b, autojunk=False)
    assert_eq!(ratio(LONG_A, LONG_B, false), 0.9201388888888888);
    assert_eq!(ratio(LONG_B, LONG_A, false), 0.9201388888888888);
}

#[test]
fn score_disables_autojunk() {
    // heard text is the long pair's first half, target the second: autojunk would score 0.42
    struct Hear;
    impl Transcribe for Hear {
        fn transcribe(&mut self, _: &[f32]) -> Result<String> {
            Ok(LONG_A.into())
        }
    }
    let (s, heard) = score(&mut Hear, &[0.0; 100], SR, LONG_B, &Lexicon::new()).unwrap();
    assert_eq!(s, 0.9201388888888888);
    assert_eq!(heard, LONG_A);
}

#[test]
fn key_keeps_lowercase_german_letters_only() {
    assert_eq!(
        key("Grüße, Herr Müller!", &Lexicon::new()),
        "grüßeherrmüller"
    );
}

struct FakeAligner {
    drop_last: bool,
}

impl Align for FakeAligner {
    fn align(&mut self, pcm16k: &[f32], words: &[String]) -> Result<Vec<(String, f64, f64)>> {
        assert_eq!(pcm16k.len() % 16000, 0, "aligner gets 16 kHz audio");
        let mut words = words.to_vec();
        if self.drop_last {
            words.pop();
        }
        Ok(words
            .into_iter()
            .enumerate()
            .map(|(i, w)| (w, 0.1 * i as f64, 0.1 * i as f64 + 0.1))
            .collect())
    }
}

fn chunk(text: &str, block: usize) -> Chunk {
    Chunk {
        block,
        g0: 0,
        g1: text.split(' ').count(),
        text: text.into(),
        kind: ChunkKind::Paragraph,
    }
}

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn align_segments_applies_offsets() {
    let chunks = vec![chunk("eins zwei drei", 0), chunk("vier fünf", 1)];
    let parts = vec![(0.0, vec![0.0; SR as usize]), (2.0, vec![0.0; SR as usize])];
    let mut warn = Vec::new();
    let (times, interp) = align_segments_with(
        &mut FakeAligner { drop_last: false },
        &parts,
        SR,
        &chunks,
        &mut warn,
    )
    .unwrap();
    let flat: Vec<f64> = times[0].iter().flat_map(|&(s, e)| [s, e]).collect();
    let want = [0.0, 0.1, 0.1, 0.2, 0.2, 0.3];
    assert!(
        flat.iter().zip(want).all(|(&a, b)| approx(a, b)),
        "{flat:?}"
    );
    assert!(approx(times[1][0].0, 2.0) && approx(times[1][0].1, 2.1));
    assert_eq!(interp, vec![vec![false; 3], vec![false; 2]]);
    assert!(warn.is_empty());
}

#[test]
fn align_segments_word_count_mismatch_interpolates() {
    let chunks = vec![chunk("eins zwei drei vier", 0)];
    assert_eq!(spoken_words(&chunks[0].text).len(), 4);
    let parts = vec![(3.0, vec![0.0; 2 * SR as usize])];
    let mut warn = Vec::new();
    let (times, interp) = align_segments_with(
        &mut FakeAligner { drop_last: true },
        &parts,
        SR,
        &chunks,
        &mut warn,
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(warn).unwrap(),
        "WARNING: chunk 1: aligner returned 3 words, expected 4; interpolating its word times\n"
    );
    assert_eq!(interp, vec![vec![true; 4]]);
    assert_eq!(times[0].len(), 4);
    let flat: Vec<f64> = times[0].iter().flat_map(|&(s, e)| [s, e]).collect();
    assert!(flat.windows(2).all(|w| w[0] <= w[1]));
    assert!(flat[0] >= 3.0 && flat[flat.len() - 1] <= 5.0);
}

/// Fails every call with the given error.
struct FailingAligner(fn() -> anyhow::Error);

impl Align for FailingAligner {
    fn align(&mut self, _: &[f32], _: &[String]) -> Result<Vec<(String, f64, f64)>> {
        Err((self.0)())
    }
}

#[test]
fn align_segments_fill_span_error_interpolates() {
    let chunks = vec![chunk("eins zwei drei vier", 0)];
    let parts = vec![(3.0, vec![0.0; 2 * SR as usize])];
    let mut warn = Vec::new();
    let mut aligner = FailingAligner(|| {
        FillSpanError {
            word: 2,
            total: 4,
            text: "zwei".into(),
            duration: 2.0,
        }
        .into()
    });
    let (times, interp) =
        align_segments_with(&mut aligner, &parts, SR, &chunks, &mut warn).unwrap();
    assert_eq!(
        String::from_utf8(warn).unwrap(),
        "WARNING: chunk 1: aligner returned too few timestamps: word 2 of 4 (\"zwei\") got the (0, 2) fill span; \
         interpolating its word times\n"
    );
    assert_eq!(interp, vec![vec![true; 4]]);
    assert_eq!(times[0].len(), 4);
    let flat: Vec<f64> = times[0].iter().flat_map(|&(s, e)| [s, e]).collect();
    assert!(flat.windows(2).all(|w| w[0] <= w[1]));
    assert!(approx(flat[0], 3.0) && approx(flat[flat.len() - 1], 5.0));
}

#[test]
fn align_segments_other_aligner_error_propagates() {
    let chunks = vec![chunk("eins zwei", 0)];
    let parts = vec![(0.0, vec![0.0; SR as usize])];
    let mut warn = Vec::new();
    let mut aligner = FailingAligner(|| anyhow::anyhow!("qwen3_aligner_align failed (-1): boom"));
    let err = align_segments_with(&mut aligner, &parts, SR, &chunks, &mut warn).unwrap_err();
    assert_eq!(err.to_string(), "qwen3_aligner_align failed (-1): boom");
    assert!(warn.is_empty());
}

/// Round 1 returns a 300 ms take, later rounds a 400 ms one.
struct ShortThenLong {
    rounds: u64,
}

impl Synth for ShortThenLong {
    fn sample_rate(&self) -> u32 {
        SR
    }
    fn generate(&mut self, reqs: &[TakeRequest], _temperature: f32) -> Result<Vec<Vec<f32>>> {
        self.rounds += 1;
        let n = if self.rounds > 1 { 400 } else { 300 };
        Ok(reqs.iter().map(|_| vec![0.5f32; n]).collect())
    }
}

/// Synthesises `text + " Neues Kapitel."`, cuts the last 100 ms off every take and calls it
/// clean when it is longer than 300 ms; records the texts it was asked to finish.
#[derive(Default)]
struct SuffixEnding {
    finished: Vec<String>,
}

impl Ending for SuffixEnding {
    fn tts_text(&self, text: &str) -> String {
        format!("{text} Neues Kapitel.")
    }
    fn finish(&mut self, wav: &[f32], _sr: u32, text: &str) -> Result<(Vec<f32>, bool)> {
        self.finished.push(text.into());
        Ok((wav[..wav.len() - 100].to_vec(), wav.len() > 300))
    }
}

#[test]
fn unclean_ending_is_regenerated_and_the_clean_one_kept() {
    let heard: Heard = Rc::new(RefCell::new(VecDeque::from(strings(&[
        "Wie es weitergeht",
        "Wie es weitergeht",
    ]))));
    let mut tts = ShortThenLong { rounds: 0 };
    let mut asr = FakeAsr { heard, calls: 0 };
    let mut ending = SuffixEnding::default();
    let (mut log, mut warn) = (Vec::new(), Vec::new());
    let wavs = synthesize_chunks_with(
        &mut tts,
        &mut asr,
        &mut ending,
        &ChunkRequest {
            texts: &strings(&["Wie es weitergeht."]),
            budgets: &[80],
            seed: 42,
            temperature: 0.9,
            lex: &Lexicon::new(),
        },
        &mut log,
        &mut warn,
    )
    .unwrap();
    assert_eq!(tts.rounds, 2, "the unclean take must be re-rolled");
    assert_eq!(
        wavs[0].len(),
        300,
        "the finished (cut) clean take wins although it is longer"
    );
    assert_eq!(
        ending.finished,
        strings(&["Wie es weitergeht.", "Wie es weitergeht."]),
        "finished with the chunk text"
    );
    let log = String::from_utf8(log).unwrap();
    assert!(log.contains("(no clean cut)"), "{log}");
    assert!(warn.is_empty());
}

/// Synthesises `text + " Neues Kapitel."` and keeps every take whole and clean.
struct Suffix;

impl Ending for Suffix {
    fn tts_text(&self, text: &str) -> String {
        format!("{text} Neues Kapitel.")
    }
    fn finish(&mut self, wav: &[f32], _sr: u32, _text: &str) -> Result<(Vec<f32>, bool)> {
        Ok((wav.to_vec(), true))
    }
}

#[test]
fn the_ending_chooses_the_synthesised_text_and_scoring_uses_the_chunk() {
    let heard: Heard = Rc::default();
    // the ASR hears the chunk only (the carrier was cut): a pass means scoring used the chunk text
    let mut tts = FakeSynth::new(heard.clone(), |t, _| {
        t.trim_end_matches(" Neues Kapitel.").into()
    });
    let mut asr = FakeAsr { heard, calls: 0 };
    let (mut log, mut warn) = (Vec::new(), Vec::new());
    synthesize_chunks_with(
        &mut tts,
        &mut asr,
        &mut Suffix,
        &ChunkRequest {
            texts: &strings(&["kurz"]),
            budgets: &[60],
            seed: 42,
            temperature: 0.9,
            lex: &Lexicon::new(),
        },
        &mut log,
        &mut warn,
    )
    .unwrap();
    assert_eq!(
        tts.calls,
        vec![vec![(100, "kurz Neues Kapitel.".into(), 42)]],
        "one round: the take passed"
    );
    assert!(warn.is_empty(), "{}", String::from_utf8_lossy(&warn));
}

#[test]
fn clipped_beats_ranking_prefers_clean_endings_among_passing_takes() {
    let clean = Take::new(0.95, vec![0.0; 20], String::new());
    let clipped = Take {
        clipped: true,
        ..Take::new(1.0, vec![0.0; 10], String::new())
    };
    assert!(clean.beats(&clipped));
    assert!(!clipped.beats(&clean));
    // a take failing the ASR check never beats a passing one, clipped or not
    let failing = Take::new(0.5, vec![0.0; 10], String::new());
    assert!(clipped.beats(&failing));
}

#[test]
fn spare_batch_slots_become_extra_candidates() {
    let heard: Heard = Rc::default();
    // "zäh" is heard right only from the candidate seeded 42 + MAX_ROUNDS * 2 + 1 (round 2, j = 2)
    let mut tts = FakeSynth::new(heard.clone(), |t, off| {
        if t == "zäh" && off != (MAX_ROUNDS * 2 + 1) as u64 {
            "Ganz?".into()
        } else {
            format!(" {t} ")
        }
    });
    tts.max_batch = 4;
    let mut asr = FakeAsr { heard, calls: 0 };
    let (mut log, mut warn) = (Vec::new(), Vec::new());
    let wavs = synthesize_chunks_with(
        &mut tts,
        &mut asr,
        &mut PlainEnding,
        &ChunkRequest {
            texts: &strings(&["zäh", "gut"]),
            budgets: &[60, 60],
            seed: 42,
            temperature: 0.9,
            lex: &Lexicon::new(),
        },
        &mut log,
        &mut warn,
    )
    .unwrap();
    let m = MAX_ROUNDS as u64;
    let seeds =
        |c: &Vec<(usize, String, u64)>| c.iter().map(|r| (r.1.clone(), r.2)).collect::<Vec<_>>();
    assert_eq!(tts.calls.len(), 2, "round 2 succeeds");
    assert_eq!(
        seeds(&tts.calls[0]),
        vec![
            ("zäh".into(), 42),
            ("zäh".into(), 42 + m),
            ("gut".into(), 42),
            ("gut".into(), 42 + m)
        ],
        "4 slots, 2 chunks: 2 candidates each"
    );
    assert_eq!(
        seeds(&tts.calls[1]),
        (0..MAX_CANDIDATES as u64)
            .map(|j| ("zäh".to_string(), 43 + m * j))
            .collect::<Vec<_>>(),
        "1 chunk left: 4 slots, capped at MAX_CANDIDATES"
    );
    assert_eq!(
        wavs[0].len(),
        10 * (MAX_ROUNDS * 2 + 2),
        "the candidate heard right wins"
    );
    let log = String::from_utf8(log).unwrap();
    assert!(
        log.starts_with("round 1: generating 2 chunk(s), 2 candidates each\n"),
        "{log}"
    );
    assert!(
        log.contains("round 2: generating 1 chunk(s), 3 candidates each\n"),
        "{log}"
    );
    assert!(warn.is_empty());
}

#[test]
fn a_repeated_heading_is_generated_and_scored_once() {
    let heard: Heard = Rc::default();
    let mut tts = FakeSynth::new(heard.clone(), |t, _| format!(" {t} "));
    let mut asr = FakeAsr { heard, calls: 0 };
    let (mut log, mut warn) = (Vec::new(), Vec::new());
    let wavs = synthesize_chunks_with(
        &mut tts,
        &mut asr,
        &mut PlainEnding,
        &ChunkRequest {
            texts: &strings(&["Titel", "Text", "Titel"]),
            budgets: &[60, 60, 60],
            seed: 42,
            temperature: 0.9,
            lex: &Lexicon::new(),
        },
        &mut log,
        &mut warn,
    )
    .unwrap();
    assert_eq!(
        tts.calls,
        vec![vec![(100, "Titel".into(), 42), (100, "Text".into(), 42)]]
    );
    assert_eq!(asr.calls, 2);
    assert_eq!(wavs[0], wavs[2]);
    assert_eq!(
        String::from_utf8(log).unwrap().lines().count(),
        4,
        "every chunk is still logged"
    );
}
