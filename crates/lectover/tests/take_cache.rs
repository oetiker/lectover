//! Disk cache of TTS takes: a re-render regenerates only what changed.

use anyhow::Result;
use lectover::ffi::{Synth, TakeRequest};
use lectover::render::TakeCache;

/// Returns a take derived from (text, seed, temperature) and counts what it was asked for.
struct Counting {
    fingerprint: Option<String>,
    asked: Vec<String>,
}

impl Synth for Counting {
    fn sample_rate(&self) -> u32 {
        24000
    }
    fn generate(&mut self, reqs: &[TakeRequest], temperature: f32) -> Result<Vec<Vec<f32>>> {
        self.asked.extend(reqs.iter().map(|r| r.text.clone()));
        Ok(reqs
            .iter()
            .map(|r| vec![r.text.len() as f32, r.max_tokens as f32, r.seed as f32, temperature, -0.25])
            .collect())
    }
    fn tail_pad_samples(&self) -> usize {
        7
    }
    fn fingerprint(&self) -> Option<String> {
        self.fingerprint.clone()
    }
}

/// Requests for `texts`, all with the same budget and seed.
fn reqs(texts: &[&str], max_tokens: usize, seed: u64) -> Vec<TakeRequest> {
    texts.iter().map(|t| TakeRequest::new(*t, max_tokens, seed)).collect()
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn second_render_comes_from_the_cache_and_only_changed_texts_are_generated() {
    let dir = tempfile::tempdir().unwrap();
    let mut inner = Counting { fingerprint: Some("voice-a".into()), asked: Vec::new() };
    let first = TakeCache::new(&mut inner, dir.path()).generate(&reqs(&["Eins.", "Zwei."], 100, 42), 0.9).unwrap();
    assert_eq!(inner.asked, strings(&["Eins.", "Zwei."]));

    inner.asked.clear();
    let mut cache = TakeCache::new(&mut inner, dir.path());
    assert_eq!(cache.tail_pad_samples(), 7);
    assert_eq!(cache.sample_rate(), 24000);
    let second = cache.generate(&reqs(&["Eins.", "Drei.", "Zwei."], 100, 42), 0.9).unwrap();
    assert_eq!(inner.asked, strings(&["Drei."]), "only the new text is synthesised");
    assert_eq!(second[0], first[0], "cached take is returned exactly");
    assert_eq!(second[2], first[1]);
    assert_eq!(second[1], vec![5.0, 100.0, 42.0, 0.9, -0.25]);
}

#[test]
fn seed_temperature_budget_and_voice_are_part_of_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let mut inner = Counting { fingerprint: Some("voice-a".into()), asked: Vec::new() };
    let t = ["Eins."];
    TakeCache::new(&mut inner, dir.path()).generate(&reqs(&t, 100, 42), 0.9).unwrap();
    TakeCache::new(&mut inner, dir.path()).generate(&reqs(&t, 100, 43), 0.9).unwrap();
    TakeCache::new(&mut inner, dir.path()).generate(&reqs(&t, 100, 42), 0.8).unwrap();
    TakeCache::new(&mut inner, dir.path()).generate(&reqs(&t, 250, 42), 0.9).unwrap();
    assert_eq!(inner.asked.len(), 4);
    inner.fingerprint = Some("voice-b".into());
    TakeCache::new(&mut inner, dir.path()).generate(&reqs(&t, 100, 42), 0.9).unwrap();
    assert_eq!(inner.asked.len(), 5, "another voice misses");
    inner.fingerprint = Some("voice-a".into());
    TakeCache::new(&mut inner, dir.path()).generate(&reqs(&t, 100, 42), 0.9).unwrap();
    assert_eq!(inner.asked.len(), 5, "back to voice-a hits");
}

#[test]
fn engines_without_a_fingerprint_are_never_cached() {
    let dir = tempfile::tempdir().unwrap();
    let mut inner = Counting { fingerprint: None, asked: Vec::new() };
    let t = ["Eins."];
    TakeCache::new(&mut inner, dir.path()).generate(&reqs(&t, 100, 42), 0.9).unwrap();
    TakeCache::new(&mut inner, dir.path()).generate(&reqs(&t, 100, 42), 0.9).unwrap();
    assert_eq!(inner.asked.len(), 2);
    assert_eq!(std::fs::read_dir(dir.path()).map(|d| d.count()).unwrap_or(0), 0);
}

#[test]
fn a_damaged_cache_file_is_regenerated() {
    let dir = tempfile::tempdir().unwrap();
    let mut inner = Counting { fingerprint: Some("voice-a".into()), asked: Vec::new() };
    let t = ["Eins."];
    TakeCache::new(&mut inner, dir.path()).generate(&reqs(&t, 100, 42), 0.9).unwrap();
    for e in std::fs::read_dir(dir.path()).unwrap() {
        std::fs::write(e.unwrap().path(), b"xyz").unwrap(); // not a whole number of f32
    }
    let again = TakeCache::new(&mut inner, dir.path()).generate(&reqs(&t, 100, 42), 0.9).unwrap();
    assert_eq!(inner.asked.len(), 2);
    assert_eq!(again[0], vec![5.0, 100.0, 42.0, 0.9, -0.25]);
}

#[test]
fn generate_each_reports_cached_and_fresh_takes_under_the_callers_index() {
    let dir = tempfile::tempdir().unwrap();
    let mut inner = Counting { fingerprint: Some("voice-a".into()), asked: Vec::new() };
    TakeCache::new(&mut inner, dir.path()).generate(&reqs(&["Eins.", "Zwei."], 100, 42), 0.9).unwrap();

    let mut seen: Vec<(usize, Vec<f32>)> = Vec::new();
    let all = TakeCache::new(&mut inner, dir.path())
        .generate_each(&reqs(&["Drei!", "Eins.", "Vierzehn.", "Zwei."], 100, 42), 0.9, &mut |i, t| {
            seen.push((i, t.to_vec()));
            Ok(())
        })
        .unwrap();
    let mut order: Vec<usize> = seen.iter().map(|(i, _)| *i).collect();
    assert_eq!(order[..2], [1, 3], "cached takes come first");
    order.sort();
    assert_eq!(order, [0, 1, 2, 3], "every take reported once");
    for (i, t) in &seen {
        assert_eq!(t, &all[*i], "take {i} reported under its own index");
    }
}

#[test]
fn generate_each_error_from_the_callback_stops_the_call() {
    let dir = tempfile::tempdir().unwrap();
    let mut inner = Counting { fingerprint: Some("voice-a".into()), asked: Vec::new() };
    let err = TakeCache::new(&mut inner, dir.path())
        .generate_each(&reqs(&["Eins."], 100, 42), 0.9, &mut |_, _| anyhow::bail!("judge failed"))
        .unwrap_err();
    assert_eq!(err.to_string(), "judge failed");
}
