//! The word list of the forced aligner: a port of mlx-audio's
//! `ForceAlignProcessor.tokenize_space_lang` so our words always equal its.

use unicode_general_category::{get_general_category, GeneralCategory as Gc};

/// Python `str.isspace`: Unicode White_Space plus the ASCII separators U+001C..U+001F.
fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Letters, numbers and the apostrophe are kept.
fn is_kept_char(c: char) -> bool {
    use Gc::*;
    c == '\''
        || matches!(
            get_general_category(c),
            UppercaseLetter
                | LowercaseLetter
                | TitlecaseLetter
                | ModifierLetter
                | OtherLetter
                | DecimalNumber
                | LetterNumber
                | OtherNumber
        )
}

fn is_cjk_char(c: char) -> bool {
    matches!(u32::from(c),
        0x4E00..=0x9FFF
        | 0x3400..=0x4DBF
        | 0x20000..=0x2A6DF
        | 0x2A700..=0x2B73F
        | 0x2B740..=0x2B81F
        | 0x2B820..=0x2CEAF
        | 0xF900..=0xFAFF)
}

/// Every CJK ideograph becomes its own token; other runs stay together.
fn split_segment_with_chinese(seg: &str, out: &mut Vec<String>) {
    let mut buf = String::new();
    for c in seg.chars() {
        if is_cjk_char(c) {
            if !buf.is_empty() {
                out.push(std::mem::take(&mut buf));
            }
            out.push(c.to_string());
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
}

pub fn spoken_words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for seg in text.split(is_py_space).filter(|s| !s.is_empty()) {
        let cleaned: String = seg.chars().filter(|&c| is_kept_char(c)).collect();
        if !cleaned.is_empty() {
            split_segment_with_chinese(&cleaned, &mut out);
        }
    }
    out
}
