//! German number words, reproducing num2words (lang="de") spellings.

const LOW: [&str; 20] = [
    "null",
    "eins",
    "zwei",
    "drei",
    "vier",
    "fünf",
    "sechs",
    "sieben",
    "acht",
    "neun",
    "zehn",
    "elf",
    "zwölf",
    "dreizehn",
    "vierzehn",
    "fünfzehn",
    "sechzehn",
    "siebzehn",
    "achtzehn",
    "neunzehn",
];
const TENS: [&str; 10] = [
    "", "", "zwanzig", "dreißig", "vierzig", "fünfzig", "sechzig", "siebzig", "achtzig", "neunzig",
];
/// Large scales, descending: (value, singular name).
const SCALES: [(u64, &str); 5] = [
    (1_000_000_000_000_000_000, "Trillion"),
    (1_000_000_000_000_000, "Billiarde"),
    (1_000_000_000_000, "Billion"),
    (1_000_000_000, "Milliarde"),
    (1_000_000, "Million"),
];

fn below_100(n: u64) -> String {
    if n < 20 {
        return LOW[n as usize].to_string();
    }
    let (t, u) = (n / 10, n % 10);
    if u == 0 {
        TENS[t as usize].to_string()
    } else {
        let unit = if u == 1 { "ein" } else { LOW[u as usize] };
        format!("{unit}und{}", TENS[t as usize])
    }
}

fn below_1000(n: u64) -> String {
    let (h, r) = (n / 100, n % 100);
    let mut s = String::new();
    if h > 0 {
        s.push_str(if h == 1 { "ein" } else { LOW[h as usize] });
        s.push_str("hundert");
    }
    if r > 0 || h == 0 {
        s.push_str(&below_100(r));
    }
    s
}

fn below_million(n: u64) -> String {
    let (t, r) = (n / 1000, n % 1000);
    let mut s = String::new();
    if t > 0 {
        // num2words quirk: only a bare 1 becomes "ein"; "einhunderteinstausend".
        s.push_str(&if t == 1 {
            "ein".to_string()
        } else {
            below_1000(t)
        });
        s.push_str("tausend");
    }
    if r > 0 || t == 0 {
        s.push_str(&below_1000(r));
    }
    s
}

pub fn cardinal(n: u64) -> String {
    let mut rest = n;
    let mut parts: Vec<String> = Vec::new();
    for (scale, name) in SCALES {
        let q = rest / scale;
        rest %= scale;
        if q == 0 {
            continue;
        }
        if q == 1 {
            parts.push(format!("eine {name}"));
        } else {
            let plural = if name.ends_with('e') { "n" } else { "en" };
            parts.push(format!("{} {name}{plural}", cardinal(q)));
        }
    }
    if rest > 0 || parts.is_empty() {
        parts.push(below_million(rest));
    }
    parts.join(" ")
}

pub fn ordinal(n: u64) -> String {
    const ORDS: [(&str, &str); 11] = [
        ("eins", "ers"),
        ("drei", "drit"),
        ("acht", "ach"),
        ("sieben", "sieb"),
        ("ig", "igs"),
        ("ert", "erts"),
        ("end", "ends"),
        ("ion", "ions"),
        ("nen", "ns"),
        ("rde", "rds"),
        ("rden", "rds"),
    ];
    let mut word = cardinal(n).to_lowercase();
    for (key, rep) in ORDS {
        if let Some(stem) = word.strip_suffix(key) {
            word = format!("{stem}{rep}");
            break;
        }
    }
    word.push_str("te");
    if word == "eintausendste" || word == "einhundertste" {
        word = word[3..].to_string();
    }
    // "eine Millionste" / "zwei Millionste" -> written together, no article.
    if let Some(pos) = word.rfind(' ') {
        let tail = &word[pos + 1..];
        if tail.ends_with("illionste") || tail.ends_with("illiardste") {
            let tail = tail.to_string();
            word.truncate(pos);
            word = match word.as_str() {
                "eine" => tail,
                _ => format!("{word}{tail}"),
            };
        }
    }
    word
}

pub fn year(n: u64) -> String {
    if (n / 100).is_multiple_of(10) {
        return cardinal(n);
    }
    let (h, r) = (n / 100, n % 100);
    let mut s = format!("{}hundert", cardinal(h));
    if r > 0 {
        s.push_str(&below_100(r));
    }
    s.replace(' ', "")
}
