use lectover::text::numwords::{cardinal, ordinal, year};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

fn golden(section: &str) -> Vec<(u64, String)> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/goldens/numbers.json");
    let v: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let entries: Vec<_> = v[section]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, w)| (k.parse().unwrap(), w.as_str().unwrap().to_string()))
        .collect();
    assert!(!entries.is_empty());
    entries
}

#[test]
fn cardinals_match_num2words() {
    for (n, w) in golden("cardinal") {
        assert_eq!(cardinal(n), w, "cardinal({n})");
    }
}

#[test]
fn ordinals_match_num2words() {
    for (n, w) in golden("ordinal") {
        assert_eq!(ordinal(n), w, "ordinal({n})");
    }
}

#[test]
fn years_match_num2words() {
    for (n, w) in golden("year") {
        assert_eq!(year(n), w, "year({n})");
    }
}

#[test]
fn years_100_to_199_use_cardinal_hundreds() {
    // num2words' to_splitnum spells the hundreds part with the cardinal: "eins".
    assert_eq!(year(100), "einshundert");
    assert_eq!(year(105), "einshundertfünf");
    assert_eq!(year(150), "einshundertfünfzig");
}
