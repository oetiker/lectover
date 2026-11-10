//! Split Ghost post HTML into readable blocks, tokens and hashes.
//!
//! Must match `ghost_text.py` and `player/lectover.js` exactly.

use html5ever::tendril::TendrilSink;
use html5ever::{local_name, ns, parse_fragment, ParseOpts, QualName};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use sha1::{Digest, Sha1};
use unicode_normalization::UnicodeNormalization;

/// Whitespace characters of the contract (never use `char::is_whitespace`:
/// Python and JS disagree on it, e.g. U+0085 and U+FEFF).
pub const SPACE_CLASS: &str = " \t\n\r\x0c\x0b\u{a0}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200a}\u{202f}\u{205f}\u{3000}";

pub const HEADING_TAGS: [&str; 6] = ["h1", "h2", "h3", "h4", "h5", "h6"];
const BLOCK_TAGS: [&str; 3] = ["p", "li", "blockquote"];
const SKIPPED_TAGS: [&str; 7] = [
    "figure",
    "figcaption",
    "pre",
    "table",
    "script",
    "style",
    "template",
];

fn is_space(c: char) -> bool {
    SPACE_CLASS.contains(c)
}

/// Collapse whitespace runs to one space and trim, using `SPACE_CLASS` only.
pub fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending = false;
    for c in s.chars() {
        if is_space(c) {
            pending = true;
        } else {
            if pending && !out.is_empty() {
                out.push(' ');
            }
            pending = false;
            out.push(c);
        }
    }
    out
}

/// First 8 hex of SHA-1 over the UTF-8 bytes of the NFC-normalised text.
pub fn block_hash(text: &str) -> String {
    let nfc: String = text.nfc().collect();
    let digest = Sha1::digest(nfc.as_bytes());
    digest.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

pub fn tokens(text: &str) -> Vec<String> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.split(' ').map(str::to_owned).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub tag: String,
    pub text: String,
    pub hash: String,
    pub tokens: Vec<String>,
}

fn element(node: &Handle) -> Option<(String, bool, bool)> {
    // (local name, has kg-callout-text, kg-card without kg-callout-card)
    if let NodeData::Element { name, attrs, .. } = &node.data {
        let attrs = attrs.borrow();
        let class = attrs
            .iter()
            .find(|a| &*a.name.local == "class")
            .map(|a| a.value.to_string())
            .unwrap_or_default();
        let has = |c: &str| class.split(is_space).any(|x| x == c);
        let callout_text = has("kg-callout-text");
        let card = has("kg-card") && !has("kg-callout-card");
        return Some((name.local.to_string(), callout_text, card));
    }
    None
}

fn is_block(tag: &str, callout_text: bool) -> bool {
    BLOCK_TAGS.contains(&tag) || HEADING_TAGS.contains(&tag) || callout_text
}

fn is_skipped(tag: &str, kg_card: bool) -> bool {
    SKIPPED_TAGS.contains(&tag) || kg_card
}

fn own_text(node: &Handle, out: &mut String) {
    for child in node.children.borrow().iter() {
        match &child.data {
            NodeData::Text { contents } => out.push_str(&contents.borrow()),
            NodeData::Element { .. } => {
                let (tag, ct, card) = element(child).expect("element");
                if tag == "br" {
                    out.push(' ');
                } else if is_block(&tag, ct) || is_skipped(&tag, card) {
                    // handled as its own block, or not read at all
                } else {
                    own_text(child, out);
                }
            }
            _ => {}
        }
    }
}

/// What the renderer meets walking the post, in document order. Only [`Mark::Block`]
/// is part of the page contract; the other marks change the audio, never the blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    /// Index into the blocks.
    Block(usize),
    /// An `<hr>`.
    Rule,
    /// The text of an HTML comment (anything outside a skipped element).
    Comment(String),
}

fn walk(node: &Handle, blocks: &mut Vec<Block>, marks: &mut Vec<Mark>) {
    for child in node.children.borrow().iter() {
        if let NodeData::Comment { contents } = &child.data {
            marks.push(Mark::Comment(contents.to_string()));
            continue;
        }
        let Some((tag, ct, card)) = element(child) else {
            continue;
        };
        if is_skipped(&tag, card) {
            continue;
        }
        if tag == "hr" {
            marks.push(Mark::Rule);
        }
        if is_block(&tag, ct) {
            let mut raw = String::new();
            own_text(child, &mut raw);
            let text = collapse(&raw);
            if !text.is_empty() {
                marks.push(Mark::Block(blocks.len()));
                blocks.push(Block {
                    tag,
                    hash: block_hash(&text),
                    tokens: tokens(&text),
                    text,
                });
            }
        }
        walk(child, blocks, marks);
    }
}

pub fn extract_blocks(html: &str) -> Vec<Block> {
    extract_marks(html).0
}

/// The blocks of [`extract_blocks`] plus the document-order [`Mark`]s around them.
pub fn extract_marks(html: &str) -> (Vec<Block>, Vec<Mark>) {
    let context = QualName::new(None, ns!(html), local_name!("body"));
    let dom = parse_fragment(RcDom::default(), ParseOpts::default(), context, vec![], false)
        .one(html);
    // The fragment parser wraps the content in a synthetic <html> root.
    let root = dom.document.children.borrow().first().cloned();
    let mut blocks = Vec::new();
    let mut marks = Vec::new();
    if let Some(root) = root {
        walk(&root, &mut blocks, &mut marks);
    }
    (blocks, marks)
}
