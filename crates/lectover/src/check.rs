//! Score a take: transcribe it and compare with the intended text (port of `longform.Checker`).

use std::collections::HashMap;

use anyhow::Result;

use crate::audio::resample::resample;
use crate::ffi::Transcribe;
use crate::text::normalize::{normalize, Lexicon};

/// Sample rate the ASR expects.
const ASR_SR: u32 = 16000;

/// Python `str.isspace()`, for `str.strip()`.
fn py_is_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// `re.sub(r"[^a-zäöüß]", "", normalize(text).lower())`
pub fn key(text: &str, lex: &Lexicon) -> String {
    normalize(text, lex)
        .to_lowercase()
        .chars()
        .filter(|&c| c.is_ascii_lowercase() || matches!(c, 'ä' | 'ö' | 'ü' | 'ß'))
        .collect()
}

/// Transcribe `wav` (at `sr`) and return `(ratio(key(heard), key(target)), heard)`.
///
/// Deviates from longform.py:96 on purpose: the ratio runs with autojunk disabled. With
/// autojunk, keys of 200+ letters lose their common letters, so a take with one error per
/// 25 letters scores 0.49-0.85 instead of 0.96 and every long chunk burns all retry rounds.
pub fn score(
    asr: &mut dyn Transcribe,
    wav: &[f32],
    sr: u32,
    target: &str,
    lex: &Lexicon,
) -> Result<(f64, String)> {
    let heard = asr.transcribe(&resample(wav, sr, ASR_SR))?;
    let heard = heard.trim_matches(py_is_space).to_string();
    Ok((ratio(&key(&heard, lex), &key(target, lex), false), heard))
}

/// `difflib.SequenceMatcher(None, a, b, autojunk=autojunk).ratio()` over code points.
pub fn ratio(a: &str, b: &str, autojunk: bool) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    let matches = Matcher::new(&a, &b, autojunk).matching_chars();
    2.0 * matches as f64 / total as f64
}

struct Matcher<'a> {
    a: &'a [char],
    b: &'a [char],
    /// positions of every non-popular element of b
    b2j: HashMap<char, Vec<usize>>,
}

impl<'a> Matcher<'a> {
    /// `__chain_b`: no junk function; with `autojunk`, popular elements are dropped when
    /// `len(b) >= 200`.
    fn new(a: &'a [char], b: &'a [char], autojunk: bool) -> Self {
        let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
        for (i, &c) in b.iter().enumerate() {
            b2j.entry(c).or_default().push(i);
        }
        let n = b.len();
        if autojunk && n >= 200 {
            let ntest = n / 100 + 1;
            b2j.retain(|_, idxs| idxs.len() <= ntest);
        }
        Matcher { a, b, b2j }
    }

    /// `find_longest_match`; the junk set is empty, so only the non-junk extension loops apply.
    fn find_longest_match(
        &self,
        alo: usize,
        ahi: usize,
        blo: usize,
        bhi: usize,
    ) -> (usize, usize, usize) {
        let (a, b) = (self.a, self.b);
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for (i, c) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut newj2len = HashMap::new();
            if let Some(js) = self.b2j.get(c) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j
                        .checked_sub(1)
                        .and_then(|p| j2len.get(&p))
                        .copied()
                        .unwrap_or(0)
                        + 1;
                    newj2len.insert(j, k);
                    if k > bestsize {
                        (besti, bestj, bestsize) = (i + 1 - k, j + 1 - k, k);
                    }
                }
            }
            j2len = newj2len;
        }
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            (besti, bestj, bestsize) = (besti - 1, bestj - 1, bestsize + 1);
        }
        while besti + bestsize < ahi
            && bestj + bestsize < bhi
            && a[besti + bestsize] == b[bestj + bestsize]
        {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    /// Sum of the sizes of `get_matching_blocks()` (order does not matter for the sum).
    fn matching_chars(&self) -> usize {
        let mut queue = vec![(0, self.a.len(), 0, self.b.len())];
        let mut total = 0;
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.find_longest_match(alo, ahi, blo, bhi);
            if k == 0 {
                continue;
            }
            total += k;
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
        total
    }
}
