//! The spoken order of a post: the page blocks plus what only the audio has (the intro,
//! `<!-- AUDIO ONLY … -->` comments) and the site's sounds (intro sting, break sting).
//!
//! The page blocks stay exactly what the player cuts from the page; audio-only text becomes
//! extra blocks after them, so the timings (which walk the page blocks only) skip it.

use std::path::Path;

use anyhow::Result;

use crate::audio::read_wav;
use crate::ghost::Post;
use crate::text::{block_hash, collapse, tokens, Block, Mark};

/// Which site sound a step plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sting {
    /// After the spoken intro.
    Intro,
    /// At an `<hr>` or a `---` inside an audio-only comment.
    Break,
}

/// One step of the spoken order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Index into [`Script::blocks`].
    Block(usize),
    Sting(Sting),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    /// The page blocks (`0..page_blocks`) followed by the audio-only blocks.
    pub blocks: Vec<Block>,
    pub page_blocks: usize,
    pub steps: Vec<Step>,
}

/// Marker that opens an audio-only comment (case-insensitive, after leading whitespace).
pub const AUDIO_ONLY: &str = "AUDIO ONLY";

/// The parts of an audio-only comment: `Some(paragraph)` to speak, `None` for a break
/// (a run of three or more `-`). `None` if the comment is not audio-only.
pub fn audio_only_parts(comment: &str) -> Option<Vec<Option<String>>> {
    let body = comment.trim_start();
    let head = body.get(..AUDIO_ONLY.len())?;
    if !head.eq_ignore_ascii_case(AUDIO_ONLY) {
        return None;
    }
    let body = &body[AUDIO_ONLY.len()..];
    let mut parts = Vec::new();
    for (i, section) in split_dashes(body).into_iter().enumerate() {
        if i > 0 {
            parts.push(None);
        }
        for para in split_paragraphs(section) {
            let text = collapse(&para);
            if !text.is_empty() {
                parts.push(Some(text));
            }
        }
    }
    Some(parts)
}

/// Split at every run of three or more `-`.
fn split_dashes(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let (mut start, mut i) = (0, 0);
    while i < bytes.len() {
        if bytes[i] == b'-' {
            let mut j = i;
            while j < bytes.len() && bytes[j] == b'-' {
                j += 1;
            }
            if j - i >= 3 {
                out.push(&s[start..i]);
                start = j;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out.push(&s[start..]);
    out
}

/// Split at blank lines (a line holding only whitespace counts as blank).
fn split_paragraphs(s: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    for line in s.lines() {
        if collapse(line).is_empty() {
            if !out.last().is_some_and(String::is_empty) {
                out.push(String::new());
            }
        } else {
            let last = out.last_mut().expect("never empty");
            last.push(' ');
            last.push_str(line);
        }
    }
    out
}

const MONTHS: [&str; 12] = [
    "Januar",
    "Februar",
    "März",
    "April",
    "Mai",
    "Juni",
    "Juli",
    "August",
    "September",
    "Oktober",
    "November",
    "Dezember",
];

/// "16. Oktober 2026" from a Ghost timestamp (`2026-10-16T10:27:51.000+02:00`). Ghost reports
/// the site's local time, so the date part is the local date.
fn written_date(ts: &str) -> Option<String> {
    let (y, rest) = ts.split_once('-')?;
    let (m, rest) = rest.split_once('-')?;
    let d = rest.get(..2)?;
    let (y, m, d): (u32, usize, u32) = (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    let month = MONTHS.get(m.checked_sub(1)?)?;
    (1..=31).contains(&d).then(|| format!("{d}. {month} {y}"))
}

/// "A", "A und B", "A, B und C".
fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} und {last}", rest.join(", ")),
    }
}

/// "Sie hören: «Titel». Verfasst von A und B. Publiziert am 16. Oktober 2026." The colon
/// gives a pause before the title; a title that already ends a sentence ("Patt?") gets no
/// extra full stop. Parts Ghost does not report are left out; `None` without a title.
pub fn intro_text(post: &Post) -> Option<String> {
    let title = collapse(&post.title);
    if title.is_empty() {
        return None;
    }
    let names: Vec<String> = post
        .authors
        .iter()
        .map(|a| collapse(&a.name))
        .filter(|n| !n.is_empty())
        .collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let stop = if title.ends_with(['.', '!', '?']) {
        ""
    } else {
        "."
    };
    let mut text = format!("Sie hören: «{title}»{stop}");
    if !names.is_empty() {
        text.push_str(&format!(" Verfasst von {}.", join_names(&names)));
    }
    if let Some(date) = written_date(&post.published_at) {
        text.push_str(&format!(" Publiziert am {date}."));
    }
    Some(text)
}

fn spoken_block(text: String) -> Block {
    Block {
        tag: "p".into(),
        hash: block_hash(&text),
        tokens: tokens(&text),
        text,
    }
}

/// The spoken order: optional intro + intro sting, then the page in document order with
/// `<hr>` as a break and audio-only comments spoken where they stand.
pub fn build_script(page: Vec<Block>, marks: &[Mark], intro: Option<String>) -> Script {
    let page_blocks = page.len();
    let mut blocks = page;
    let mut steps = Vec::new();
    let extra = |text: String, blocks: &mut Vec<Block>, steps: &mut Vec<Step>| {
        steps.push(Step::Block(blocks.len()));
        blocks.push(spoken_block(text));
    };
    if let Some(text) = intro {
        extra(text, &mut blocks, &mut steps);
        steps.push(Step::Sting(Sting::Intro));
    }
    for mark in marks {
        match mark {
            Mark::Block(i) => steps.push(Step::Block(*i)),
            Mark::Rule => steps.push(Step::Sting(Sting::Break)),
            Mark::Comment(c) => {
                for part in audio_only_parts(c).unwrap_or_default() {
                    match part {
                        Some(text) => extra(text, &mut blocks, &mut steps),
                        None => steps.push(Step::Sting(Sting::Break)),
                    }
                }
            }
        }
    }
    Script {
        blocks,
        page_blocks,
        steps,
    }
}

/// A site's sounds, as read from `sites/<domain>/`: `intro.wav` after the spoken intro,
/// `break.wav` at breaks (falls back to the intro sound). Missing files mean no sound.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SiteSounds {
    /// (mono samples, sample rate)
    pub intro: Option<(Vec<f32>, u32)>,
    pub brk: Option<(Vec<f32>, u32)>,
}

impl SiteSounds {
    pub fn load(dir: &Path) -> Result<Self> {
        let read = |name: &str| -> Result<Option<(Vec<f32>, u32)>> {
            let path = dir.join(name);
            if path.exists() {
                read_wav(&path).map(Some)
            } else {
                Ok(None)
            }
        };
        let intro = read("intro.wav")?;
        let brk = match read("break.wav")? {
            Some(b) => Some(b),
            None => intro.clone(),
        };
        Ok(Self { intro, brk })
    }

    pub fn get(&self, sting: Sting) -> Option<&(Vec<f32>, u32)> {
        match sting {
            Sting::Intro => self.intro.as_ref(),
            Sting::Break => self.brk.as_ref(),
        }
    }
}
