//! Local posts: a Markdown or HTML file on disk instead of a Ghost post.
//!
//! `lectover render <file>` and `lectover dev build <file>` turn the file into the same
//! [`Post`] the Ghost Content API would return, without a key or the network: the first
//! `<h1>` becomes the title and leaves the body (Ghost's `html` never holds the title; the
//! intro speaks it), the slug comes from the file name, the date is today.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use fancy_regex::Regex;
use pulldown_cmark::{html, Options, Parser};
use unicode_normalization::UnicodeNormalization;

use crate::ghost::{Author, Post};
use crate::text::extract_blocks;

/// File extensions read as a local post (lowercase).
pub const MARKDOWN_EXTENSIONS: [&str; 2] = ["md", "markdown"];
pub const HTML_EXTENSIONS: [&str; 2] = ["html", "htm"];

/// Words per minute of the reading time shown on the dev page (Ghost's figure).
const WORDS_PER_MINUTE: usize = 275;

/// What the command line says about a local post; `None` takes the default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalMeta {
    pub title: Option<String>,
    pub authors: Vec<String>,
    /// `YYYY-MM-DD`.
    pub date: Option<String>,
    pub slug: Option<String>,
}

impl LocalMeta {
    /// True when any option was given (they only apply to a local file).
    pub fn is_set(&self) -> bool {
        self.title.is_some()
            || !self.authors.is_empty()
            || self.date.is_some()
            || self.slug.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Html,
}

/// The format a post argument names by its extension, if it is a local file name rather than
/// a post URL or a Ghost slug (slugs never contain a dot).
pub fn local_format(arg: &str) -> Option<Format> {
    let arg = arg.trim();
    if arg.starts_with("http://") || arg.starts_with("https://") {
        return None;
    }
    let ext = Path::new(arg).extension()?.to_str()?.to_ascii_lowercase();
    if MARKDOWN_EXTENSIONS.contains(&ext.as_str()) {
        Some(Format::Markdown)
    } else if HTML_EXTENSIONS.contains(&ext.as_str()) {
        Some(Format::Html)
    } else {
        None
    }
}

/// The local file a post argument names: a `.md`, `.markdown`, `.html` or `.htm` name, or any
/// existing file. `None` means a Ghost post URL or slug.
pub fn local_path(arg: &str) -> Option<PathBuf> {
    let trimmed = arg.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return None;
    }
    let path = Path::new(trimmed);
    (local_format(trimmed).is_some() || path.is_file()).then(|| path.to_path_buf())
}

/// Ghost-style slug: lowercase ASCII letters and digits, runs of anything else one hyphen.
/// Accents are dropped (`é` → `e`, `ä` → `a`), `ß` becomes `ss`.
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut hyphen = false;
    let mut push = |c: char, out: &mut String| {
        if c.is_ascii_alphanumeric() {
            if hyphen && !out.is_empty() {
                out.push('-');
            }
            hyphen = false;
            out.push(c.to_ascii_lowercase());
        } else {
            hyphen = true;
        }
    };
    for c in s.nfkd() {
        match c {
            'ß' => {
                push('s', &mut out);
                push('s', &mut out);
            }
            'æ' | 'Æ' => {
                push('a', &mut out);
                push('e', &mut out);
            }
            'ø' | 'Ø' => push('o', &mut out),
            // combining marks left by NFKD: part of the letter before
            c if ('\u{300}'..='\u{36f}').contains(&c) => {}
            c => push(c, &mut out),
        }
    }
    out
}

/// Markdown to HTML (CommonMark plus tables, strikethrough and footnotes; raw HTML and
/// comments pass through, so `<!-- AUDIO ONLY … -->` works as in Ghost).
pub fn markdown_to_html(md: &str) -> String {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_FOOTNOTES;
    let mut out = String::new();
    html::push_html(&mut out, Parser::new_ext(md, opts));
    out
}

/// The content of `<body>` of a full HTML document; a fragment as is.
pub fn html_body(doc: &str) -> &str {
    let re = Regex::new(r"(?is)<body\b[^>]*>(.*)</body\s*>").expect("valid regex");
    if let Ok(Some(c)) = re.captures(doc) {
        return c.get(1).map(|m| m.as_str()).unwrap_or(doc);
    }
    let open = Regex::new(r"(?is)<body\b[^>]*>").expect("valid regex");
    match open.find(doc) {
        Ok(Some(m)) => &doc[m.end()..],
        _ => doc,
    }
}

/// The text of the first `<h1>` of `html` and `html` without it; `None` without an `<h1>`
/// or when it holds no text.
pub fn take_first_h1(html: &str) -> Option<(String, String)> {
    let re = Regex::new(r"(?is)<h1\b[^>]*>(.*?)</h1\s*>").expect("valid regex");
    let caps = re.captures(html).ok()??;
    let whole = caps.get(0)?;
    // the block extractor decodes entities and collapses whitespace like the player
    let title = extract_blocks(&format!("<p>{}</p>", &caps[1]))
        .into_iter()
        .map(|b| b.text)
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        return None;
    }
    let rest = format!(
        "{}{}",
        &html[..whole.start()],
        html[whole.end()..].trim_start_matches(['\n', '\r'])
    );
    Some((title, rest))
}

/// Check `YYYY-MM-DD`.
pub fn parse_date(s: &str) -> Result<String> {
    let s = s.trim();
    let ok = s.len() == 10
        && s.as_bytes()[4] == b'-'
        && s.as_bytes()[7] == b'-'
        && s.bytes()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
    if ok {
        let (m, d): (u32, u32) = (s[5..7].parse()?, s[8..10].parse()?);
        if (1..=12).contains(&m) && (1..=31).contains(&d) {
            return Ok(s.to_string());
        }
    }
    bail!("invalid date {s:?}; expected YYYY-MM-DD")
}

/// Today in local time, as `YYYY-MM-DD`, and the local UTC offset as `+HH:MM`.
pub fn today_local() -> (String, String) {
    // SAFETY: localtime_r fills the zeroed tm from the current time; both pointers are valid.
    let tm = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        tm
    };
    let date = format!(
        "{:04}-{:02}-{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday
    );
    let off = tm.tm_gmtoff / 60;
    let sign = if off < 0 { '-' } else { '+' };
    (
        date,
        format!("{sign}{:02}:{:02}", off.abs() / 60, off.abs() % 60),
    )
}

/// Build the post from the file's text. `stem` is the file name without extension (fallback
/// title and slug), `today` the date without `--date`, `offset` its UTC offset (`+01:00`).
pub fn post_from_source(
    source: &str,
    format: Format,
    stem: &str,
    meta: &LocalMeta,
    today: &str,
    offset: &str,
) -> Result<Post> {
    let body = match format {
        Format::Markdown => markdown_to_html(source),
        Format::Html => html_body(source).to_string(),
    };
    let (title, html) = match &meta.title {
        Some(t) => (t.trim().to_string(), body),
        None => match take_first_h1(&body) {
            Some((t, rest)) => (t, rest),
            None => (stem.to_string(), body),
        },
    };
    let slug = match &meta.slug {
        Some(s) => s.trim().to_string(),
        None => slugify(stem),
    };
    if slug.is_empty() || slug.contains(['/', '\\']) || slug.starts_with('.') {
        bail!("no usable slug from {stem:?}; give one with --slug");
    }
    let date = match &meta.date {
        Some(d) => parse_date(d)?,
        None => parse_date(today)?,
    };
    let stamp = format!("{date}T00:00:00.000{offset}");
    let words: usize = extract_blocks(&html).iter().map(|b| b.tokens.len()).sum();
    Ok(Post {
        slug,
        html,
        visibility: "public".into(),
        updated_at: stamp.clone(),
        title,
        reading_time: Some(words.div_ceil(WORDS_PER_MINUTE).max(1) as u32),
        published_at: stamp,
        authors: meta
            .authors
            .iter()
            .map(|n| Author {
                name: n.trim().to_string(),
            })
            .collect(),
    })
}

/// Read a local post file. The format follows the extension; any other existing file is read
/// as Markdown.
pub fn load_post(path: &Path, meta: &LocalMeta) -> Result<Post> {
    let source = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let format = local_format(&path.to_string_lossy()).unwrap_or(Format::Markdown);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (today, offset) = today_local();
    post_from_source(&source, format, &stem, meta, &today, &offset)
}
