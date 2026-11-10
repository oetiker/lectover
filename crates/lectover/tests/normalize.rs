use lectover::text::normalize::{load_lexicon, normalize, spoken_groups, Group, Lexicon};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn g(first: usize, last: usize, spoken: &str) -> Group {
    Group {
        first,
        last,
        spoken: spoken.to_string(),
    }
}

fn spoken(text: &str) -> String {
    spoken_groups(text, &Lexicon::new())
        .iter()
        .map(|g| g.spoken.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Verbatim from `tests/test_normalize_de.py`.
const GOLDEN: &[(&str, &str)] = &[
    (
        "Am 25. November entscheidet das Parlament.",
        "Am fünfundzwanzigsten November entscheidet das Parlament.",
    ),
    ("am 1. Mai 2026", "am ersten Mai zweitausendsechsundzwanzig"),
    (
        "am 15.10.2025 erschienen",
        "am fünfzehnten Oktober zweitausendfünfundzwanzig erschienen",
    ),
    (
        "am 15.10. in der NOZ",
        "am fünfzehnten Oktober in der N-O-Z",
    ),
    ("am 15.10 in der NOZ", "am fünfzehnten Oktober in der N-O-Z"),
    ("Saison 24.25", "Saison vierundzwanzig Komma zwei fünf"),
    ("1.000 Franken", "eintausend Franken"),
    ("1'000 Franken", "eintausend Franken"),
    ("1’000’000 Einwohner", "eine Million Einwohner"),
    ("60% der Aktien", "sechzig Prozent der Aktien"),
    ("60 % der Aktien", "sechzig Prozent der Aktien"),
    ("4,2 Millionen", "vier Komma zwei Millionen"),
    ("Es steht 3:1.", "Es steht drei zu eins."),
    (
        "von 2020–2025",
        "von zweitausendzwanzig bis zweitausendfünfundzwanzig",
    ),
    ("10-12 Personen", "zehn bis zwölf Personen"),
    ("um 14:30 Uhr", "um vierzehn Uhr dreißig"),
    ("um 14:00 Uhr", "um vierzehn Uhr"),
    ("CHF 12.50", "zwölf Franken fünfzig"),
    ("Fr. 5.–", "fünf Franken"),
    ("12.50 Franken", "zwölf Franken fünfzig"),
    ("CHF 12", "zwölf Franken"),
    ("Fr. 12 bezahlt.", "zwölf Franken bezahlt."),
    ("12 Franken", "zwölf Franken"),
    ("CHF 1.20", "ein Franken zwanzig"),
    ("Fr. 1.–", "ein Franken"),
    ("1 Franken und 1 Rappen", "ein Franken und ein Rappen"),
    ("1 Million Franken", "eine Million Franken"),
    ("1 Milliarde", "eine Milliarde"),
    ("CHF 1'000", "eintausend Franken"),
    ("CHF 1’000", "eintausend Franken"),
    ("Fr. 1.000", "eintausend Franken"),
    ("CHF 1'000.50", "eintausend Franken fünfzig"),
    ("Fr. 1'000.–", "eintausend Franken"),
    ("1'000.50 Franken", "eintausend Franken fünfzig"),
    ("3.5 Prozent", "drei Komma fünf Prozent"),
    ("3.5%", "drei Komma fünf Prozent"),
    ("3.5 %", "drei Komma fünf Prozent"),
    ("4.2 Millionen", "vier Komma zwei Millionen"),
    ("3.5 Prozentpunkte", "drei Komma fünf Prozentpunkte"),
    ("1.5 Prozentpunkt", "eins Komma fünf Prozentpunkt"),
    ("1.2 Million", "eins Komma zwei Million"),
    ("2.5 Milliarden", "zwei Komma fünf Milliarden"),
    ("1.1 Milliarde", "eins Komma eins Milliarde"),
    ("1.1 Franken", "eins Komma eins Franken"),
    ("2.5 Rappen", "zwei Komma fünf Rappen"),
    ("am 3.5 in Olten", "am dritten Mai in Olten"),
    ("1 Prozent", "ein Prozent"),
    ("1%", "ein Prozent"),
    ("im Jahr 1984", "im Jahr neunzehnhundertvierundachtzig"),
    ("2025 stammten", "zweitausendfünfundzwanzig stammten"),
    (
        "Die sbo (sbo) und die StWZ",
        "Die S-B-O (S-B-O) und die S-T-W-Z",
    ),
    ("das a.en-Gebäude", "das A-E-N-Gebäude"),
    ("z.B. Gas", "zum Beispiel Gas"),
    ("z. B. Gas", "zum Beispiel Gas"),
    ("ca. 100 Anschlüsse", "circa einhundert Anschlüsse"),
    (
        "Olten hält – wie gesagt – «rund» 60 Prozent",
        "Olten hält – wie gesagt – «rund» sechzig Prozent",
    ),
];

#[test]
fn golden() {
    for (text, want) in GOLDEN {
        assert_eq!(spoken(text), *want, "{text:?}");
    }
}

#[test]
fn lexicon_applies_whole_words() {
    let lexicon = vec![("Kleinholz".to_string(), "Klein-holz".to_string())];
    let got: Vec<String> = spoken_groups("Kleinholz. Kleinholzhaufen", &lexicon)
        .into_iter()
        .map(|g| g.spoken)
        .collect();
    assert_eq!(got, ["Klein-holz.", "Kleinholzhaufen"]);
}

#[test]
fn lexicon_applies_to_hyphen_compound_parts() {
    let lexicon = vec![("FDP".to_string(), "Äf-De-Pe".to_string()), ("sbo".to_string(), "Äs-Be-O".to_string())];
    let got: Vec<String> = spoken_groups("FDP-Fraktion, Rot-FDP-Achse und sbo-Rat; FDPler", &lexicon)
        .into_iter()
        .map(|g| g.spoken)
        .collect();
    assert_eq!(got, ["Äf-De-Pe-Fraktion,", "Rot-Äf-De-Pe-Achse", "und", "Äs-Be-O-Rat;", "FDPler"]);
}

#[test]
fn groups_single_token_replacements_stay_per_token() {
    assert_eq!(
        spoken_groups("Am 25. November", &Lexicon::new()),
        [
            g(0, 1, "Am"),
            g(1, 2, "fünfundzwanzigsten"),
            g(2, 3, "November")
        ]
    );
}

#[test]
fn replacement_spanning_space_merges_tokens() {
    assert_eq!(
        spoken_groups("CHF 12.50 bezahlt", &Lexicon::new()),
        [g(0, 2, "zwölf Franken fünfzig"), g(2, 3, "bezahlt")]
    );
    assert_eq!(
        spoken_groups("z. B. Gas", &Lexicon::new())[0],
        g(0, 2, "zum Beispiel")
    );
}

#[test]
fn percent_as_own_token() {
    assert_eq!(
        spoken_groups("das 60 % Ziel", &Lexicon::new())[1..3],
        [g(1, 2, "sechzig"), g(2, 3, "Prozent")]
    );
}

#[test]
fn groups_cover_all_tokens_contiguously() {
    let extra = ["– «»", "💡 Hinweis", "Nr. 7.", "0", "99.99.99"];
    for text in GOLDEN.iter().map(|(t, _)| *t).chain(extra) {
        let groups = spoken_groups(text, &Lexicon::new());
        assert_eq!(groups[0].first, 0, "{text:?}");
        assert_eq!(
            groups.last().unwrap().last,
            text.split(' ').count(),
            "{text:?}"
        );
        for (a, b) in groups.iter().zip(&groups[1..]) {
            assert!(a.last == b.first && a.first < a.last, "{text:?}");
        }
    }
}

#[test]
fn normalize_wrapper_keeps_lines() {
    let lexicon = load_lexicon(&root().join("examples/olten.jetzt/lexicon.txt")).unwrap();
    assert_eq!(
        normalize("60%\n\nam 15.10.", &lexicon),
        "sechzig Prozent\n\nam fünfzehnten Oktober"
    );
}

#[test]
fn missing_lexicon_is_empty() {
    assert!(load_lexicon(&root().join("no-such-lexicon.txt"))
        .unwrap()
        .is_empty());
}

#[test]
fn lexicon_file_parsing_matches_python() {
    let path = std::env::temp_dir().join(format!("lectover-lexicon-{}.txt", std::process::id()));
    fs::write(
        &path,
        "# comment = no\n a = b # tail\nnoequals\nx = 1\na = c\n = empty\nk=v=w\n",
    )
    .unwrap();
    let lexicon = load_lexicon(&path).unwrap();
    fs::remove_file(&path).unwrap();
    // dict semantics: a later duplicate replaces the value, keeps the first position
    let want: Lexicon = [("a", "c"), ("x", "1"), ("", "empty"), ("k", "v=w")]
        .iter()
        .map(|(w, s)| (w.to_string(), s.to_string()))
        .collect();
    assert_eq!(lexicon, want);
}

/// Swiss spot checks; expected values computed with Python `spoken_groups(text, lexicon={})`.
#[test]
fn swiss_spot_checks_match_python() {
    type Case = (&'static str, &'static [(usize, usize, &'static str)]);
    let cases: &[Case] = &[
        (
            "Die Kosten betragen CHF 1'250'000.– total",
            &[
                (0, 1, "Die"),
                (1, 2, "Kosten"),
                (2, 3, "betragen"),
                (3, 5, "eine Million zweihundertfünfzigtausend Franken"),
                (5, 6, "total"),
            ],
        ),
        (
            "CHF 12'345.65 und Fr. 2’500.50",
            &[
                (
                    0,
                    2,
                    "zwölftausenddreihundertfünfundvierzig Franken fünfundsechzig",
                ),
                (2, 3, "und"),
                (3, 5, "zweitausendfünfhundert Franken fünfzig"),
            ],
        ),
        (
            "Er zahlte 1'000'000 Franken.",
            &[
                (0, 1, "Er"),
                (1, 2, "zahlte"),
                (2, 3, "eine Million"),
                (3, 4, "Franken."),
            ],
        ),
        (
            "rund 45% bzw. 12.5 % mehr",
            &[
                (0, 1, "rund"),
                (1, 2, "fünfundvierzig Prozent"),
                (2, 3, "beziehungsweise"),
                (3, 4, "zwölf Komma fünf"),
                (4, 5, "Prozent"),
                (5, 6, "mehr"),
            ],
        ),
        ("100 %", &[(0, 1, "einhundert"), (1, 2, "Prozent")]),
        (
            "Saison 24.25",
            &[(0, 1, "Saison"), (1, 2, "vierundzwanzig Komma zwei fünf")],
        ),
        (
            "die Saison 2024/25 und 24.25.",
            &[
                (0, 1, "die"),
                (1, 2, "Saison"),
                (2, 3, "zweitausendvierundzwanzig/fünfundzwanzig"),
                (3, 4, "und"),
                (4, 5, "vierundzwanzig Komma zwei fünf."),
            ],
        ),
        (
            "4.2 Millionen Franken",
            &[
                (0, 1, "vier Komma zwei"),
                (1, 2, "Millionen"),
                (2, 3, "Franken"),
            ],
        ),
        (
            "3.5 Prozentpunkte mehr",
            &[
                (0, 1, "drei Komma fünf"),
                (1, 2, "Prozentpunkte"),
                (2, 3, "mehr"),
            ],
        ),
        (
            "am 1.5.2026 um 9:05 Uhr",
            &[
                (0, 1, "am"),
                (1, 2, "ersten Mai zweitausendsechsundzwanzig"),
                (2, 3, "um"),
                (3, 5, "neun Uhr fünf"),
            ],
        ),
        ("1.1.0150", &[(0, 1, "ersten Januar einshundertfünfzig")]),
        (
            "am 31.12.1999.",
            &[
                (0, 1, "am"),
                (
                    1,
                    2,
                    "einunddreißigsten Dezember neunzehnhundertneunundneunzig.",
                ),
            ],
        ),
        (
            "32.1.2026",
            &[(0, 1, "zweiunddreißig.eins.zweitausendsechsundzwanzig")],
        ),
        ("Fr. 1.–", &[(0, 2, "ein Franken")]),
        ("10²", &[(0, 1, "10²")]),
        ("٣ Tage", &[(0, 1, "drei"), (1, 2, "Tage")]),
        ("²%", &[(0, 1, "² Prozent")]),
        (
            "über 1.5 Mio. Einwohner",
            &[
                (0, 1, "über"),
                (1, 2, "ersten Mai"),
                (2, 3, "Millionen"),
                (3, 4, "Einwohner"),
            ],
        ),
        ("Kleinholz-Lager", &[(0, 1, "Kleinholz-Lager")]),
        (
            "Version 1.2.3",
            &[(0, 1, "Version"), (1, 2, "eins.zwei.drei")],
        ),
        (
            "Seite 5-7, 12:3, 25:61 Uhr",
            &[
                (0, 1, "Seite"),
                (1, 2, "fünf bis sieben,"),
                (2, 3, "zwölf zu drei,"),
                (3, 4, "fünfundzwanzig zu einundsechzig"),
                (4, 5, "Uhr"),
            ],
        ),
        (
            "das kostet 99.99.99",
            &[
                (0, 1, "das"),
                (1, 2, "kostet"),
                (2, 3, "neunundneunzig.neunundneunzig.neunundneunzig"),
            ],
        ),
        ("Café 3", &[(0, 1, "Café"), (1, 2, "drei")]),
        ("e\u{301} 5", &[(0, 1, "e\u{301}"), (1, 2, "fünf")]),
        ("e\u{301}5", &[(0, 1, "e\u{301}fünf")]),
        ("x_5", &[(0, 1, "x_5")]),
        ("\u{663}\u{660} %", &[(0, 1, "dreißig"), (1, 2, "Prozent")]),
    ];
    for (text, want) in cases {
        let want: Vec<Group> = want.iter().map(|&(f, l, s)| g(f, l, s)).collect();
        assert_eq!(spoken_groups(text, &Lexicon::new()), want, "{text:?}");
    }
}

#[test]
fn normalize_matches_goldens() {
    // the lexicon the goldens were made with: edits to examples/olten.jetzt/lexicon.txt must not move them
    let lexicon = load_lexicon(&root().join("tests/fixtures/goldens/lexicon.txt")).unwrap();
    let path = root().join("tests/fixtures/goldens/normalize.json");
    let cases: Vec<Value> = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert!(cases.len() > 1000);
    for case in &cases {
        let text = case["in"].as_str().unwrap();
        assert_eq!(
            normalize(text, &lexicon),
            case["out"].as_str().unwrap(),
            "{text:?}"
        );
    }
}

#[test]
fn groups_match_goldens() {
    // the lexicon the goldens were made with: edits to examples/olten.jetzt/lexicon.txt must not move them
    let lexicon = load_lexicon(&root().join("tests/fixtures/goldens/lexicon.txt")).unwrap();
    let dir = root().join("tests/fixtures/goldens");
    let mut checked = 0;
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let stem = path.file_stem().unwrap().to_str().unwrap().to_owned();
        if path.extension().is_none_or(|e| e != "json")
            || ["normalize", "numbers", "spoken_words", "contract"].contains(&stem.as_str())
        {
            continue;
        }
        let golden: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let blocks = golden["blocks"].as_array().unwrap();
        let groups = golden["groups"].as_array().unwrap();
        assert_eq!(blocks.len(), groups.len(), "{stem}");
        for (i, (b, want)) in blocks.iter().zip(groups).enumerate() {
            let want: Vec<Group> = want
                .as_array()
                .unwrap()
                .iter()
                .map(|w| Group {
                    first: w["first"].as_u64().unwrap() as usize,
                    last: w["last"].as_u64().unwrap() as usize,
                    spoken: w["spoken"].as_str().unwrap().to_string(),
                })
                .collect();
            let got = spoken_groups(b["text"].as_str().unwrap(), &lexicon);
            assert_eq!(got, want, "{stem} block {i}");
        }
        checked += 1;
    }
    assert_eq!(checked, 10);
}

/// Sessionsbericht September 2026: "CHF 46,2 Mio." was read as "Franken
/// sechsundvierzig Komma zwei Millionen", "CHF 157 Mio." as
/// "hundertsiebenundfünfzig Franken Millionen" and "CHF 300 000" (narrow
/// no-break space, collapsed to a space) as "dreihundert Franken null".
/// With a scale word the amount is read without "Franken".
#[test]
fn money_with_scale_words_and_spaced_thousands() {
    let cases = [
        ("Das Projekt kostet CHF 157 Mio. Stadt", "Das Projekt kostet einhundertsiebenundfünfzig Millionen. Stadt"),
        ("einen Bruttokredit von CHF 46,2 Mio.; nach", "einen Bruttokredit von sechsundvierzig Komma zwei Millionen; nach"),
        ("netto CHF 29,1 Mio. An den", "netto neunundzwanzig Komma eins Millionen. An den"),
        ("CHF 1 Mio. bewertet", "eine Million bewertet"),
        ("CHF 2 Mrd.", "zwei Milliarden."),
        ("CHF 1,5 Milliarden", "eins Komma fünf Milliarden"),
        ("Fr. 3.2 Millionen", "drei Komma zwei Millionen"),
        ("davon CHF 4,5 Mio", "davon vier Komma fünf Millionen"),
        ("davon CHF 4,5 für", "davon vier Franken fünfzig für"),
        ("CHF 4,25", "vier Franken fünfundzwanzig"),
        ("CHF 1,05", "ein Franken fünf"),
        ("kostet CHF 4,50 pro", "kostet vier Franken fünfzig pro"),
        ("kostet 4,50 Franken", "kostet vier Franken fünfzig"),
        ("um CHF 300 000 jährlich", "um dreihunderttausend Franken jährlich"),
        ("rund CHF 14 000 pro Jahr", "rund vierzehntausend Franken pro Jahr"),
        ("für CHF 10 pro m²", "für zehn Franken pro Quadratmeter"),
        ("CHF 30 und CHF 60 verlangen", "dreißig Franken und sechzig Franken verlangen"),
        ("für CHF 1 an die Stadt", "für ein Franken an die Stadt"),
        ("20 845 m²", "zwanzigtausendachthundertfünfundvierzig Quadratmeter"),
        ("3 km² Wald", "drei Quadratkilometer Wald"),
        ("500 m³ Beton", "fünfhundert Kubikmeter Beton"),
    ];
    for (text, want) in cases {
        assert_eq!(spoken(text), want, "{text:?}");
    }
}

/// "Mio." and friends also end sentences: the expansion keeps the full stop
/// when the text ends or a capitalised word other than a unit noun follows.
#[test]
fn sentence_final_abbreviations_keep_the_full_stop() {
    let cases = [
        ("Es kostet 157 Mio. Die Stadt zahlt.", "Es kostet einhundertsiebenundfünfzig Millionen. Die Stadt zahlt."),
        ("Es kostet 157 Mio.", "Es kostet einhundertsiebenundfünfzig Millionen."),
        ("Es kostet 157 Mio. Franken.", "Es kostet einhundertsiebenundfünfzig Millionen Franken."),
        ("rund 3 Mio. Einwohner", "rund drei Millionen Einwohner"),
        ("rund 3 Mio. Einwohnerinnen und Einwohner", "rund drei Millionen Einwohnerinnen und Einwohner"),
        ("2 Mrd. Euro", "zwei Milliarden Euro"),
        ("3 Mio. für die Schule", "drei Millionen für die Schule"),
        ("wenn die Stadt 32 Mio. Überschuss ausweist.", "wenn die Stadt zweiunddreißig Millionen Überschuss ausweist."),
        ("Velo, Bus usw. Danach", "Velo, Bus und so weiter. Danach"),
        ("Velo, Bus usw.", "Velo, Bus und so weiter."),
        ("Velo, Bus usw. und mehr", "Velo, Bus und so weiter und mehr"),
        ("ca. Mitte Mai", "circa Mitte Mai"),
        ("u.a. Schulen", "unter anderem Schulen"),
    ];
    for (text, want) in cases {
        assert_eq!(spoken(text), want, "{text:?}");
    }
}
