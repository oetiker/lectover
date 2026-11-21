//! Memoized ASR and aligner answers: an unchanged take costs no model call on a re-render.

use anyhow::{bail, Result};
use lectover::ffi::{Align, Synth, TakeRequest, Transcribe};
use lectover::render::{prune, CallStats, MemoAlign, MemoAsr, PruneReport, TakeCache};

/// Transcribes audio as its length and counts the calls.
struct FakeAsr {
    fingerprint: Option<String>,
    calls: usize,
}

impl Transcribe for FakeAsr {
    fn transcribe(&mut self, pcm16k: &[f32]) -> Result<String> {
        self.calls += 1;
        Ok(format!("heard {} samples", pcm16k.len()))
    }
    fn fingerprint(&self) -> Option<String> {
        self.fingerprint.clone()
    }
}

/// Spreads the words evenly over the audio (awkward fractions, to test the exact round
/// trip) and counts the calls; fails for the word "fail".
struct FakeAligner {
    fingerprint: Option<String>,
    calls: usize,
}

impl Align for FakeAligner {
    fn align(&mut self, pcm16k: &[f32], words: &[String]) -> Result<Vec<(String, f64, f64)>> {
        self.calls += 1;
        if words.iter().any(|w| w == "fail") {
            bail!("cannot align");
        }
        let step = pcm16k.len() as f64 / 16000.0 / 3.0 / words.len() as f64;
        Ok(words
            .iter()
            .enumerate()
            .map(|(i, w)| (w.clone(), i as f64 * step, (i + 1) as f64 * step))
            .collect())
    }
    fn fingerprint(&self) -> Option<String> {
        self.fingerprint.clone()
    }
}

fn words(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn asr_answers_repeated_audio_from_disk_and_counts() {
    let dir = tempfile::tempdir().unwrap();
    let mut inner = FakeAsr {
        fingerprint: Some("asr-a".into()),
        calls: 0,
    };
    let a = vec![0.25f32; 10];
    let b = vec![0.5f32; 10];
    let first = MemoAsr::new(&mut inner, dir.path()).transcribe(&a).unwrap();
    assert_eq!(inner.calls, 1);

    let mut memo = MemoAsr::new(&mut inner, dir.path());
    assert_eq!(memo.transcribe(&a).unwrap(), first);
    memo.transcribe(&b).unwrap(); // same length, other samples: a new call
    assert_eq!((memo.stats.hits, memo.stats.runs), (1, 1));
    assert_eq!(inner.calls, 2);

    // another model never sees the first model's answers
    let mut other = FakeAsr {
        fingerprint: Some("asr-b".into()),
        calls: 0,
    };
    MemoAsr::new(&mut other, dir.path()).transcribe(&a).unwrap();
    assert_eq!(other.calls, 1);
}

#[test]
fn asr_without_a_fingerprint_is_never_memoized() {
    let dir = tempfile::tempdir().unwrap();
    let mut inner = FakeAsr {
        fingerprint: None,
        calls: 0,
    };
    let a = vec![0.25f32; 10];
    MemoAsr::new(&mut inner, dir.path()).transcribe(&a).unwrap();
    MemoAsr::new(&mut inner, dir.path()).transcribe(&a).unwrap();
    assert_eq!(inner.calls, 2);
}

#[test]
fn aligner_answers_are_exact_and_keyed_by_audio_and_words() {
    let dir = tempfile::tempdir().unwrap();
    let mut inner = FakeAligner {
        fingerprint: Some("al-a".into()),
        calls: 0,
    };
    let a = vec![0.1f32; 16000 * 7];
    let w = words(&["Die", "Debatte"]);
    let first = MemoAlign::new(&mut inner, dir.path())
        .align(&a, &w)
        .unwrap();

    let mut memo = MemoAlign::new(&mut inner, dir.path());
    let again = memo.align(&a, &w).unwrap();
    assert_eq!(again, first, "bit for bit, times included");
    memo.align(&a, &words(&["Die", "Debatten"])).unwrap();
    assert_eq!(memo.stats.hits, 1);
    assert_eq!(memo.stats.runs, 1);
    assert_eq!(inner.calls, 2);
}

#[test]
fn aligner_errors_are_not_memoized() {
    let dir = tempfile::tempdir().unwrap();
    let mut inner = FakeAligner {
        fingerprint: Some("al-a".into()),
        calls: 0,
    };
    let a = vec![0.1f32; 160];
    let w = words(&["fail"]);
    assert!(MemoAlign::new(&mut inner, dir.path())
        .align(&a, &w)
        .is_err());
    assert!(MemoAlign::new(&mut inner, dir.path())
        .align(&a, &w)
        .is_err());
    assert_eq!(inner.calls, 2);
}

#[test]
fn a_damaged_alignment_file_runs_the_aligner_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut inner = FakeAligner {
        fingerprint: Some("al-a".into()),
        calls: 0,
    };
    let a = vec![0.1f32; 1600];
    let w = words(&["eins", "zwei"]);
    let first = MemoAlign::new(&mut inner, dir.path())
        .align(&a, &w)
        .unwrap();
    for entry in std::fs::read_dir(dir.path().join("align")).unwrap() {
        std::fs::write(entry.unwrap().path(), "eins\tzz\n").unwrap();
    }
    let again = MemoAlign::new(&mut inner, dir.path())
        .align(&a, &w)
        .unwrap();
    assert_eq!(again, first);
    assert_eq!(inner.calls, 2);
}

/// One sample per request; no batching.
struct OneSample;

impl Synth for OneSample {
    fn sample_rate(&self) -> u32 {
        24000
    }
    fn generate(&mut self, reqs: &[TakeRequest], _t: f32) -> Result<Vec<Vec<f32>>> {
        Ok(reqs.iter().map(|r| vec![r.seed as f32]).collect())
    }
    fn fingerprint(&self) -> Option<String> {
        Some("tts".into())
    }
}

#[test]
fn take_cache_counts_hits_and_runs_and_predicts_hits() {
    let dir = tempfile::tempdir().unwrap();
    let mut inner = OneSample;
    let r = |seed| TakeRequest::new("Eins.", 100, seed);
    let mut cache = TakeCache::new(&mut inner, dir.path());
    assert_eq!(cache.cached(&[r(1), r(2)], 0.9), 0);
    cache.generate(&[r(1)], 0.9).unwrap();
    assert_eq!(cache.cached(&[r(1), r(2)], 0.9), 1);
    assert_eq!(
        cache.cached(&[r(1)], 0.8),
        0,
        "temperature is part of the key"
    );
    cache.generate(&[r(1), r(2)], 0.9).unwrap();
    assert_eq!((cache.stats.hits, cache.stats.runs), (1, 2));
}

#[test]
fn call_stats_summary() {
    let s = CallStats {
        hits: 46,
        runs: 7,
        secs: 3.06,
    };
    assert_eq!(s.summary("run"), "7 run, 46 cached, 3.1s");
}

/// Set `path`'s mtime `days` into the past.
fn age(path: &std::path::Path, days: u64) {
    let then = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(then)
        .unwrap();
}

fn mtime(path: &std::path::Path) -> std::time::SystemTime {
    std::fs::metadata(path).unwrap().modified().unwrap()
}

#[test]
fn a_cache_hit_marks_the_entry_as_used() {
    let dir = tempfile::tempdir().unwrap();
    let mut inner = FakeAsr {
        fingerprint: Some("asr-a".into()),
        calls: 0,
    };
    let mut asr = MemoAsr::new(&mut inner, dir.path());
    asr.transcribe(&[0.25; 10]).unwrap();
    let file = std::fs::read_dir(dir.path().join("asr"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    age(&file, 200);
    let old = mtime(&file);
    asr.transcribe(&[0.25; 10]).unwrap();
    assert_eq!(asr.stats.hits, 1);
    assert!(mtime(&file) > old, "hit did not refresh the mtime");
}

#[test]
fn prune_removes_only_unused_cache_entries() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join("asr")).unwrap();
    std::fs::create_dir_all(d.join("align")).unwrap();
    let files = [
        ("old.f32", 100, true),
        ("new.f32", 1, false),
        ("asr/old.txt", 100, true),
        ("align/old.tsv", 100, true),
        ("align/new.tsv", 10, false),
        ("notes.txt", 100, false), // not a cache entry
    ];
    for (name, days, _) in files {
        std::fs::write(d.join(name), b"1234").unwrap();
        age(&d.join(name), days);
    }
    let unused = std::time::Duration::from_secs(90 * 86_400);
    let dry = prune(d, unused, true).unwrap();
    let want = PruneReport {
        removed: 3,
        removed_bytes: 12,
        kept: 2,
        kept_bytes: 8,
    };
    assert_eq!(dry, want);
    assert!(
        files.iter().all(|(n, _, _)| d.join(n).exists()),
        "dry run removed"
    );
    assert_eq!(prune(d, unused, false).unwrap(), want);
    for (name, _, gone) in files {
        assert_eq!(!d.join(name).exists(), gone, "{name}");
    }
    assert_eq!(
        prune(&d.join("missing"), unused, false).unwrap(),
        PruneReport::default()
    );
}
