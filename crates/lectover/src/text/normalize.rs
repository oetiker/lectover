//! Spell out numbers, dates and abbreviations in German text before TTS.
//!
//! Port of `normalize_de.py`. TTS models read digits unreliably, so we write
//! them out. The result is token-aware: every display token (split on single
//! spaces) belongs to exactly one spoken group, so the player can highlight
//! the words that were spoken.
//!
//! Python `re` semantics are reproduced explicitly: its `\w` (str patterns)
//! is `str.isalnum() or "_"`, i.e. exactly `[\p{L}\p{N}_]`, which differs from
//! the Rust regex `\w` (marks, connector punctuation, no `No`/`Nl`). Every
//! `\w` and `\b` of the Python patterns is therefore spelled with [`W`].
//! `\d` is Unicode `Nd` in both engines.
//!
//! Offsets: the Python code compares code-point offsets of matches with
//! code-point token starts. Here matches and token starts are both byte
//! offsets into the same string; byte order is monotone in code-point order
//! and token starts are char boundaries, so every comparison and slice
//! gives the same result.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::LazyLock;

use fancy_regex::{Captures, Regex};

type Caps<'t> = Captures<'t, str>;

use super::numwords::{cardinal, ordinal, year};
use super::{collapse, tokens};

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

/// Spoken form of abbreviations, in Python dict order. Matched as whole
/// words, case-sensitive.
const ABBREVIATIONS: [(&str, &str); 17] = [
    ("sbo", "S-B-O"),
    ("StWZ", "S-T-W-Z"),
    ("a.en", "A-E-N"),
    ("NOZ", "N-O-Z"),
    ("z.B.", "zum Beispiel"),
    ("z. B.", "zum Beispiel"),
    ("u.a.", "unter anderem"),
    ("bzw.", "beziehungsweise"),
    ("usw.", "und so weiter"),
    ("ca.", "circa"),
    ("Mio.", "Millionen"),
    ("Mrd.", "Milliarden"),
    ("Fr.", "Franken"),
    ("CHF", "Franken"),
    ("m²", "Quadratmeter"),
    ("km²", "Quadratkilometer"),
    ("m³", "Kubikmeter"),
];

/// Body of a character class equal to Python's `\w` in str patterns.
const W: &str = r"\p{L}\p{N}_";

/// Display tokens `[first, last)` are spoken as `spoken`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub first: usize,
    pub last: usize,
    pub spoken: String,
}

/// Pronunciation lexicon `(written, spoken)` in file order, keys unique.
pub type Lexicon = Vec<(String, String)>;

/// Python `str.isspace()`: Unicode White_Space plus U+001C..U+001F.
fn py_is_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// Python `str.splitlines()` boundaries (`\r\n` yields an extra empty line,
/// which the lexicon parser ignores).
fn py_is_line_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r'
            | '\x0b'
            | '\x0c'
            | '\x1c'
            | '\x1d'
            | '\x1e'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// Load `written = spoken` lines; `#` starts a comment. A missing file is an
/// empty lexicon. Later duplicates replace the value but keep the position
/// (Python dict semantics).
pub fn load_lexicon(path: &Path) -> io::Result<Lexicon> {
    let content = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Lexicon::new()),
        Err(e) => return Err(e),
    };
    let mut entries = Lexicon::new();
    for line in content.split(py_is_line_break) {
        let line = line
            .split('#')
            .next()
            .unwrap_or("")
            .trim_matches(py_is_space);
        let Some((written, spoken)) = line.split_once('=') else {
            continue;
        };
        let (written, spoken) = (
            written.trim_matches(py_is_space),
            spoken.trim_matches(py_is_space),
        );
        match entries.iter_mut().find(|(w, _)| w == written) {
            Some(entry) => entry.1 = spoken.to_string(),
            None => entries.push((written.to_string(), spoken.to_string())),
        }
    }
    Ok(entries)
}

fn is_nd(c: char) -> bool {
    static ND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d$").unwrap());
    c.is_ascii_digit()
        || (!c.is_ascii() && ND.is_match(c.encode_utf8(&mut [0; 4])).unwrap_or(false))
}

/// Value of a Unicode decimal digit (`Nd` comes in contiguous runs of 0..9).
fn digit_value(c: char) -> u64 {
    if let Some(d) = c.to_digit(10) {
        return d.into();
    }
    let mut zero = c as u32;
    while let Some(prev) = zero.checked_sub(1).and_then(char::from_u32) {
        if !is_nd(prev) {
            break;
        }
        zero -= 1;
    }
    u64::from((c as u32 - zero) % 10)
}

/// Python `int(s)` for a run of decimal digits; `None` beyond `u64`.
fn int(s: &str) -> Option<u64> {
    s.chars()
        .try_fold(0u64, |n, c| n.checked_mul(10)?.checked_add(digit_value(c)))
}

/// Python `re.sub(r"\D", "", s)`.
fn only_digits(s: &str) -> String {
    s.chars().filter(|&c| is_nd(c)).collect()
}

/// Python `str.isdigit()`: decimal digits plus Numeric_Type=Digit (Unicode 15).
fn py_is_digit(c: char) -> bool {
    const DIGIT_RANGES: [(u32, u32); 20] = [
        (0xB2, 0xB3),
        (0xB9, 0xB9),
        (0x1369, 0x1371),
        (0x19DA, 0x19DA),
        (0x2070, 0x2070),
        (0x2074, 0x2079),
        (0x2080, 0x2089),
        (0x2460, 0x2468),
        (0x2474, 0x247C),
        (0x2488, 0x2490),
        (0x24EA, 0x24EA),
        (0x24F5, 0x24FD),
        (0x24FF, 0x24FF),
        (0x2776, 0x277E),
        (0x2780, 0x2788),
        (0x278A, 0x2792),
        (0x10A40, 0x10A43),
        (0x10E60, 0x10E68),
        (0x11052, 0x1105A),
        (0x1F100, 0x1F10A),
    ];
    is_nd(c)
        || DIGIT_RANGES
            .iter()
            .any(|&(a, b)| (a..=b).contains(&(c as u32)))
}

// Numbers beyond `u64` (Python spells them out) make the rule reject the
// match; all helpers below return `None` in that case.

fn cardinal_s(s: &str) -> Option<String> {
    int(s).map(cardinal)
}

/// 1984 -> "neunzehnhundertvierundachtzig"; from 2000 on years read like cardinals.
fn year_or_cardinal(s: &str) -> Option<String> {
    let n = int(s)?;
    Some(if (1100..=1999).contains(&n) {
        year(n)
    } else {
        cardinal(n)
    })
}

/// Cardinal used before a masculine/neuter noun: 1 -> "ein", not "eins".
fn counted(s: &str) -> Option<String> {
    match int(s)? {
        1 => Some("ein".to_string()),
        n => Some(cardinal(n)),
    }
}

fn group<'t>(c: &Caps<'t>, i: usize) -> &'t str {
    c.get(i).map_or("", |m| m.as_str())
}

fn money(c: &Caps, _: &str) -> Option<String> {
    let francs = only_digits(c.name("fr").map_or("", |m| m.as_str()));
    let cents = c.name("rp").map_or("", |m| m.as_str());
    let mut spoken = format!("{} Franken", counted(&francs)?);
    if !cents.is_empty() && int(cents)? != 0 {
        spoken = format!("{spoken} {}", cardinal_s(cents)?);
    }
    Some(spoken)
}

/// "CHF 46,2 Mio." -> "sechsundvierzig Komma zwei Millionen": with a scale
/// word the currency is clear and "Millionen Franken" after every amount
/// sounds heavy when read aloud (user decision); plain amounts keep "Franken".
fn money_scaled(c: &Caps, _: &str) -> Option<String> {
    let int_part = c.name("int").map_or("", |m| m.as_str());
    let dec = c.name("dec").map(|m| m.as_str());
    let billions = matches!(
        group(c, 0).rsplit(' ').next(),
        Some("Mrd." | "Mrd" | "Milliarden" | "Milliarde")
    );
    let singular = dec.is_none() && int(int_part)? == 1;
    let number = match dec {
        Some(d) => {
            let digits: Vec<String> = d.chars().map(|x| cardinal(digit_value(x))).collect();
            format!("{} Komma {}", cardinal_s(int_part)?, digits.join(" "))
        }
        None if singular => "eine".to_string(),
        None => cardinal_s(int_part)?,
    };
    let scale = match (billions, singular) {
        (false, true) => "Million",
        (false, false) => "Millionen",
        (true, true) => "Milliarde",
        (true, false) => "Milliarden",
    };
    Some(format!("{number} {scale}"))
}

/// "CHF 4,5" -> "vier Franken fünfzig": one decimal after the comma is tens of Rappen.
fn money_tenths(c: &Caps, _: &str) -> Option<String> {
    let mut spoken = format!("{} Franken", counted(group(c, 1))?);
    let rappen = int(group(c, 2))? * 10;
    if rappen != 0 {
        spoken = format!("{spoken} {}", cardinal(rappen));
    }
    Some(spoken)
}

fn clock(c: &Caps, _: &str) -> Option<String> {
    let (h, mins) = (int(group(c, 1))?, int(group(c, 2))?);
    if h > 23 || mins > 59 {
        return None;
    }
    let mut spoken = format!("{} Uhr", cardinal(h));
    if mins != 0 {
        spoken = format!("{spoken} {}", cardinal(mins));
    }
    Some(spoken)
}

/// Dative/accusative ordinal fits the usual "am 25. November", "vom 3. Mai".
fn dative_ordinal(day: u64) -> String {
    ordinal(day) + "n"
}

fn numeric_date(c: &Caps, _: &str) -> Option<String> {
    let (day, month) = (int(group(c, 1))?, int(group(c, 2))?);
    if !((1..=31).contains(&day) && (1..=12).contains(&month)) {
        return None;
    }
    let mut spoken = format!("{} {}", dative_ordinal(day), MONTHS[month as usize - 1]);
    if let Some(y) = c.get(3) {
        spoken = format!("{spoken} {}", year(int(y.as_str())?));
    }
    Some(spoken)
}

fn day_of_month(c: &Caps, _: &str) -> Option<String> {
    let day = int(group(c, 1))?;
    (1..=31).contains(&day).then(|| dative_ordinal(day))
}

fn decimal(c: &Caps, _: &str) -> Option<String> {
    let digits: Vec<String> = group(c, 2)
        .chars()
        .map(|d| cardinal(digit_value(d)))
        .collect();
    Some(format!(
        "{} Komma {}",
        cardinal_s(group(c, 1))?,
        digits.join(" ")
    ))
}

fn thousands(c: &Caps, _: &str) -> Option<String> {
    cardinal_s(&only_digits(group(c, 0)))
}

fn range(c: &Caps, _: &str) -> Option<String> {
    Some(format!(
        "{} bis {}",
        year_or_cardinal(group(c, 1))?,
        year_or_cardinal(group(c, 2))?
    ))
}

fn score(c: &Caps, _: &str) -> Option<String> {
    Some(format!(
        "{} zu {}",
        cardinal_s(group(c, 1))?,
        cardinal_s(group(c, 2))?
    ))
}

fn percent(c: &Caps, text: &str) -> Option<String> {
    let start = c.get(0)?.start();
    let after_digit = text[..start].chars().next_back().is_some_and(py_is_digit);
    Some(if after_digit { " Prozent" } else { "Prozent" }.to_string())
}

fn bare_number(c: &Caps, _: &str) -> Option<String> {
    year_or_cardinal(group(c, 0))
}

/// Maps a match (and the whole text) to its spoken form, or `None` to reject it.
type RuleFn = fn(&Caps, &str) -> Option<String>;

enum Action {
    Fixed(&'static str),
    Call(RuleFn),
}

/// Built-in rules in priority order (they follow the lexicon rules); earlier
/// rules win overlapping matches.
static RULES: LazyLock<Vec<(Regex, Action)>> = LazyLock::new(|| {
    // franc amount, optionally with thousands separators: 12 / 1'000 / 1’000 / 1.000 /
    // 300 000 (a narrow no-break space, collapsed to a plain space)
    let francs = r"(?P<fr>\d{1,3}(?:['’.]\d{3})+|\d{1,3}(?: \d{3})+(?!\d)|\d+)";
    // units after which "D.M" is a decimal, not a date: "3.5 Prozent", "4.2 Millionen"
    let decimal_unit = format!(
        r" ?(?:%|Prozent|Millionen|Million|Milliarden|Milliarde|Franken|Rappen|Prozentpunkte|Prozentpunkt)(?![{W}])"
    );
    // number not glued to further digits/decimal parts on its right
    let num_end = format!(r"(?![{W}]|[.,]\d)");
    let months = MONTHS.join("|");
    let call = |pattern: String, f: RuleFn| (pattern, Action::Call(f));
    let fixed = |pattern: String, s: &'static str| (pattern, Action::Fixed(s));
    let mut rules: Vec<(String, Action)> = vec![
        // money with a scale word: CHF 157 Mio. / CHF 46,2 Mio. / Fr. 2 Milliarden
        call(
            format!(
                r"(?<![{W}.])(?:CHF|Fr\.) (?P<int>\d+)(?:[.,](?P<dec>\d+))? (?P<sc>Mio\.?|Millionen|Million|Mrd\.?|Milliarden|Milliarde)(?![{W}])"
            ),
            money_scaled,
        ),
        // money: CHF 12.50 / Fr. 5.– / 12.50 Franken
        call(
            // "CHF 4,50" is Rappen too
            format!(r"(?<![{W}.])(?:CHF|Fr\.) {francs}[.,](?P<rp>\d{{2}}){num_end}"),
            money,
        ),
        call(
            format!(r"(?<![{W}.])(?:CHF|Fr\.) {francs}\.[–-](?P<rp>)"),
            money,
        ),
        call(
            format!(r"(?<![{W}.,'’]){francs}[.,](?P<rp>\d{{2}}) Franken(?![{W}])"),
            money,
        ),
        call(
            format!(r"(?<![{W}.])(?:CHF|Fr\.) {francs}(?P<rp>)(?![{W}'’]|[.,]\d)"),
            money,
        ),
        call(
            format!(r"(?<![{W}.])(?:CHF|Fr\.) (\d+),(\d){num_end}"),
            money_tenths,
        ),
        // "ein"/"eine" before a counted noun (the noun stays its own token)
        fixed(
            format!(r"(?<![{W}.,'’])1(?= (?:Franken|Rappen|Prozent)(?![{W}])| ?%)"),
            "ein",
        ),
        fixed(
            format!(r"(?<![{W}.,'’])1(?= (?:Million|Milliarde)(?![{W}]))"),
            "eine",
        ),
        // clock time: 14:30 Uhr
        call(
            format!(r"(?<![{W}:.,])(\d{{1,2}}):(\d{{2}}) Uhr(?![{W}])"),
            clock,
        ),
        // dates: "25." before a month name (month stays its own token), 15.10.2025 / 15.10. / 15.10
        call(
            format!(r"(?<![{W}.,])(\d{{1,2}})\.(?= (?:{months})(?![{W}]))"),
            day_of_month,
        ),
        call(
            format!(
                r"(?<![{W}.,])(\d{{1,2}})\.(\d{{1,2}})(?:\.(\d{{4}})(?![{W}]|[.,]\d)|\.(?![{W}])|(?![{W}.,])(?!{decimal_unit}))"
            ),
            numeric_date,
        ),
        // thousands separators: 1'000 / 1’000 / 1.000.000
        call(
            format!(r"(?<![{W}.,'’])\d{{1,3}}(?:['’.]\d{{3}})+(?!\d)"),
            thousands,
        ),
        // 20 845: thin/no-break space as thousands separator (collapsed to a space)
        call(
            format!(r"(?<![{W}.,'’])\d{{1,3}}(?: \d{{3}})+(?![{W}]|[.,]\d)"),
            thousands,
        ),
        // ranges: 2020–2025 / 10-12
        call(format!(r"(?<![{W}.,])(\d+)[–-](\d+){num_end}"), range),
        // scores: 3:1
        call(format!(r"(?<![{W}.,:])(\d+):(\d+)(?![{W}]|[.,:]\d)"), score),
        // decimals: 4,2 / 24.25
        call(format!(r"(?<![{W}.,])(\d+)[,.](\d+){num_end}"), decimal),
    ];
    // abbreviations, longest first (stable: dict order among equal lengths)
    let mut abbreviations = ABBREVIATIONS.to_vec();
    abbreviations.sort_by_key(|(abbr, _)| std::cmp::Reverse(abbr.chars().count()));
    for (abbr, spoken) in abbreviations {
        let pattern = format!(r"(?<![{W}.]){}(?![{W}])", fancy_regex::escape(abbr));
        rules.push(fixed(pattern, spoken));
    }
    rules.push(call("%".to_string(), percent));
    // Python `\b\d+\b`: \d is a subset of \w, so both boundaries are look-arounds.
    rules.push(call(format!(r"(?<![{W}])\d+(?![{W}])"), bare_number));
    rules
        .into_iter()
        .map(|(p, a)| {
            let re = Regex::new(&p).unwrap_or_else(|e| panic!("rule {p:?}: {e}"));
            (re, a)
        })
        .collect()
});

/// Whole words; a hyphen separates words, so the parts of a compound match ("FDP-Fraktion").
fn lexicon_regex(written: &str) -> Regex {
    let pattern = format!(r"(?<![{W}]){}(?![{W}])", fancy_regex::escape(written));
    Regex::new(&pattern).unwrap_or_else(|e| panic!("lexicon {pattern:?}: {e}"))
}

/// Abbreviations that also end sentences: their full stop is both.
const SENTENCE_FINAL_ABBREVIATIONS: &[&str] = &["Mio.", "Mrd.", "usw.", "etc."];

/// Capitalised words that continue a "3 Mio. ..." phrase instead of starting a sentence:
/// units, counted things and the finance nouns that follow an amount ("32 Mio. Überschuss").
/// Any other capitalised word after "Mio." starts a sentence ("157 Mio. Stadt und Kanton
/// tragen ...": 12 of 13 cases in the golden posts).
const CONTINUATION_NOUNS: &[&str] = &[
    "Franken",
    "Rappen",
    "CHF",
    "Fr",
    "Euro",
    "EUR",
    "Dollar",
    "USD",
    "Einwohner",
    "Einwohnerinnen",
    "Menschen",
    "Personen",
    "Leute",
    "Besucher",
    "Besucherinnen",
    "Fahrgäste",
    "Passagiere",
    "Tonnen",
    "Liter",
    "Kilometer",
    "Meter",
    "Quadratmeter",
    "Kubikmeter",
    "Kilowattstunden",
    "Stunden",
    "Tage",
    "Jahre",
    "Stimmen",
    "Mitglieder",
    "Überschuss",
    "Ertragsüberschuss",
    "Defizit",
    "Gewinn",
    "Verlust",
    "Umsatz",
    "Schulden",
    "Kosten",
    "Mehrkosten",
    "Einnahmen",
    "Ausgaben",
    "Ertrag",
    "Aufwand",
    "Mehrertrag",
    "Mehraufwand",
    "Budget",
    "Kredit",
    "Steuern",
    "Steuereinnahmen",
    "Vermögen",
    "Eigenkapital",
    "Beitrag",
    "Beiträge",
    "Subventionen",
    "Investitionen",
    "Reserven",
];

/// True when the abbreviation matched at `text[..end]` also ends its sentence: the text
/// ends there, or the next word is capitalised and not a unit noun ("157 Mio. Die Stadt",
/// not "157 Mio. Franken").
fn ends_sentence_at(text: &str, end: usize) -> bool {
    let matched = &text[..end];
    if !SENTENCE_FINAL_ABBREVIATIONS
        .iter()
        .any(|a| matched.ends_with(a))
    {
        return false;
    }
    let rest = &text[end..];
    let Some(next) = rest.strip_prefix(' ').or(rest.is_empty().then_some("")) else {
        return false; // "Mio.;" or similar: the punctuation that follows decides
    };
    let word: String = next
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '-')
        .collect();
    match word.chars().next() {
        None => next.is_empty(),
        Some(c) => c.is_uppercase() && !CONTINUATION_NOUNS.contains(&word.as_str()),
    }
}

/// Non-overlapping `(start, end, spoken)` replacements (byte offsets), sorted.
fn replacements(text: &str, lexicon: &Lexicon) -> Vec<(usize, usize, String)> {
    let lexicon_rules: Vec<(Regex, &str)> = lexicon
        .iter()
        .map(|(written, spoken)| (lexicon_regex(written), spoken.as_str()))
        .collect();
    let rules = lexicon_rules
        .iter()
        .map(|(re, spoken)| (re, Ok(*spoken)))
        .chain(RULES.iter().map(|(re, action)| match action {
            Action::Fixed(s) => (re, Ok(*s)),
            Action::Call(f) => (re, Err(*f)),
        }));
    let mut accepted: Vec<(usize, usize, String)> = Vec::new();
    for (re, action) in rules {
        // A backtrack-limit error ends this rule's matches; the patterns
        // backtrack little, so this only guards pathological input.
        for caps in re.captures_iter(text).map_while(Result::ok) {
            let Some(m) = caps.get(0) else { continue };
            let (start, end) = (m.start(), m.end());
            if accepted.iter().any(|&(s, e, _)| start < e && s < end) {
                continue;
            }
            let spoken = match action {
                Ok(s) => Some(s.to_string()),
                Err(f) => f(&caps, text),
            };
            if let Some(mut spoken) = spoken {
                if ends_sentence_at(text, end) && !spoken.ends_with('.') {
                    spoken.push('.');
                }
                accepted.push((start, end, spoken));
            }
        }
    }
    accepted.sort();
    accepted
}

/// Group the tokens of a collapsed block text by what is spoken for them.
pub fn spoken_groups(text: &str, lexicon: &Lexicon) -> Vec<Group> {
    let toks = tokens(text);
    let mut starts = Vec::with_capacity(toks.len());
    let mut pos = 0;
    for tok in &toks {
        starts.push(pos);
        pos += tok.len() + 1;
    }
    // Python `bisect_right(starts, at) - 1`; `at` is never before starts[0] == 0.
    let token_at = |at: usize| starts.partition_point(|&s| s <= at) - 1;

    let reps = replacements(text, lexicon);
    // merged[i] is true when token i is spoken together with token i + 1
    let mut merged = vec![false; toks.len()];
    for &(start, end, _) in &reps {
        if text[start..end].contains(' ') {
            for flag in &mut merged[token_at(start)..token_at(end - 1)] {
                *flag = true;
            }
        }
    }

    let mut groups = Vec::new();
    let mut first = 0;
    for (i, tok) in toks.iter().enumerate() {
        if merged[i] {
            continue;
        }
        let (g_start, g_end) = (starts[first], starts[i] + tok.len());
        let mut spoken = String::new();
        let mut cursor = g_start;
        for (start, end, s) in &reps {
            if g_start <= *start && *end <= g_end {
                spoken.push_str(&text[cursor..*start]);
                spoken.push_str(s);
                cursor = *end;
            }
        }
        spoken.push_str(&text[cursor..g_end]);
        groups.push(Group {
            first,
            last: i + 1,
            spoken,
        });
        first = i + 1;
    }
    groups
}

/// Normalise text line by line (each line collapsed first).
pub fn normalize(text: &str, lexicon: &Lexicon) -> String {
    text.split('\n')
        .map(|line| {
            spoken_groups(&collapse(line), lexicon)
                .into_iter()
                .map(|g| g.spoken)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}
