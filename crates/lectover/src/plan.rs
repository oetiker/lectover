//! Plan which groups of a Ghost block are synthesised together (one TTS call per chunk).

use crate::script::{Step, Sting};
use crate::text::normalize::Group;
use crate::text::spoken_words::spoken_words;
use crate::text::{Block, HEADING_TAGS};

/// Pause classes after a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkKind {
    Sentence,
    Paragraph,
    Title,
}

impl ChunkKind {
    pub fn pause_secs(self) -> f64 {
        match self {
            ChunkKind::Sentence => 0.30,
            ChunkKind::Paragraph => 0.70,
            ChunkKind::Title => 0.90,
        }
    }

    /// The key used in the goldens (`longform.PAUSES`).
    pub fn name(self) -> &'static str {
        match self {
            ChunkKind::Sentence => "sentence",
            ChunkKind::Paragraph => "paragraph",
            ChunkKind::Title => "title",
        }
    }
}

/// Groups `[g0, g1)` of block `block`, synthesised in one TTS call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub block: usize,
    pub g0: usize,
    pub g1: usize,
    pub text: String,
    pub kind: ChunkKind,
}

impl Chunk {
    /// Text sent to the TTS: headings get a full stop so they are spoken as a
    /// finished phrase. (" …" helped before takes were padded after
    /// end-of-speech; with padding and the non-streaming prompt "." ends
    /// cleanly as often or more: "Wie es weitergeht" 6/8 vs 3/8 over 8 seeds.)
    /// Punctuation is not a spoken word, so alignment and display are unchanged.
    pub fn tts_text(&self) -> String {
        if self.kind == ChunkKind::Title && !ends_sentence(&self.text) {
            format!("{}.", self.text.trim_end_matches([':', ',', ';', '.']))
        } else {
            self.text.clone()
        }
    }
}

/// Pause before a heading: longer than any pause after a chunk, so a heading
/// is heard as the start of a new section.
pub const TITLE_LEAD_SECS: f64 = 1.3;

/// max_tokens buckets: each round runs one batch per bucket.
pub const BUCKETS: [usize; 4] = [100, 250, 500, 1000];

/// German vowel groups (ei, au, eu, ie count once) approximate syllables.
pub fn syllables(text: &str) -> usize {
    let mut count = 0;
    let mut in_run = false;
    for c in text.chars().flat_map(char::to_lowercase) {
        let vowel = matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'ä' | 'ö' | 'ü' | 'y');
        if vowel && !in_run {
            count += 1;
        }
        in_run = vowel;
    }
    count
}

pub fn max_tokens_for(text: &str) -> usize {
    // same float expression as Python, so rounding matches bit for bit
    let tokens = (syllables(text).max(1) as f64 / 2.5 * 12.5 * 2.0).ceil() as usize;
    1000.min(40 + tokens)
}

/// Smallest bucket >= `max_tokens` (the largest bucket caps it).
pub fn bucket_for(max_tokens: usize) -> usize {
    BUCKETS
        .iter()
        .copied()
        .find(|&b| b >= max_tokens)
        .unwrap_or(BUCKETS[3])
}

/// Matches `[.!?][»«"”’)]*$`.
pub(crate) fn ends_sentence(spoken: &str) -> bool {
    spoken
        .trim_end_matches(['»', '«', '"', '”', '’', ')'])
        .ends_with(['.', '!', '?'])
}

/// (g0, g1, text) per sentence; ends are judged on the spoken text, so "25." or "ca." never end one.
fn sentences(groups: &[Group]) -> Vec<(usize, usize, String)> {
    let join = |gs: &[Group]| {
        gs.iter()
            .map(|g| g.spoken.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut out = Vec::new();
    let mut start = 0;
    for (i, g) in groups.iter().enumerate() {
        if ends_sentence(&g.spoken) || i == groups.len() - 1 {
            out.push((start, i + 1, join(&groups[start..=i])));
            start = i + 1;
        }
    }
    out
}

fn block_chunks(index: usize, block: &Block, groups: &[Group], max_chars: usize) -> Vec<Chunk> {
    if groups.is_empty() {
        return Vec::new();
    }
    if HEADING_TAGS.contains(&block.tag.as_str()) {
        let text = groups
            .iter()
            .map(|g| g.spoken.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        return vec![Chunk {
            block: index,
            g0: 0,
            g1: groups.len(),
            text,
            kind: ChunkKind::Title,
        }];
    }
    let mut merged: Vec<(usize, usize, String)> = Vec::new();
    for (g0, g1, text) in sentences(groups) {
        if let Some(last) = merged.last_mut() {
            let joined = last.2.chars().count() + 1 + text.chars().count();
            if joined <= max_chars {
                last.1 = g1;
                last.2 = format!("{} {}", last.2, text);
                continue;
            }
        }
        merged.push((g0, g1, text));
    }
    let last = merged.len() - 1;
    merged
        .into_iter()
        .enumerate()
        .map(|(i, (g0, g1, text))| Chunk {
            block: index,
            g0,
            g1,
            text,
            kind: if i == last {
                ChunkKind::Paragraph
            } else {
                ChunkKind::Sentence
            },
        })
        .collect()
}

pub fn plan_chunks(blocks: &[Block], groups: &[Vec<Group>], max_chars: usize) -> Vec<Chunk> {
    let steps: Vec<Step> = (0..blocks.len()).map(Step::Block).collect();
    plan_steps(blocks, groups, &steps, max_chars).0
}

/// Chunks in spoken order, and each sting with the index of the chunk it precedes
/// (`chunks.len()` for a sting after the last chunk).
pub fn plan_steps(
    blocks: &[Block],
    groups: &[Vec<Group>],
    steps: &[Step],
    max_chars: usize,
) -> (Vec<Chunk>, Vec<(usize, Sting)>) {
    let mut chunks = Vec::new();
    let mut stings = Vec::new();
    for step in steps {
        let i = match *step {
            Step::Block(i) => i,
            Step::Sting(s) => {
                stings.push((chunks.len(), s));
                continue;
            }
        };
        let (block, gs) = (&blocks[i], &groups[i]);
        let all = gs
            .iter()
            .map(|g| g.spoken.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        if spoken_words(&all).is_empty() {
            continue;
        }
        chunks.extend(
            block_chunks(i, block, gs, max_chars)
                .into_iter()
                .filter(|c| !spoken_words(&c.text).is_empty()),
        );
    }
    (chunks, stings)
}
