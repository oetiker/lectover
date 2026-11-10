use lectover::text::{block_hash, collapse, extract_blocks, tokens};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

#[test]
fn fixture_blocks_match_expected() {
    let dir = fixtures().join("contract");
    let blocks = extract_blocks(&fs::read_to_string(dir.join("post.html")).unwrap());
    let expected: Vec<Value> =
        serde_json::from_str(&fs::read_to_string(dir.join("expected.json")).unwrap()).unwrap();
    assert_eq!(blocks.len(), expected.len());
    for (b, e) in blocks.iter().zip(&expected) {
        assert_eq!(b.tag, e["tag"].as_str().unwrap());
        assert_eq!(b.text, e["text"].as_str().unwrap());
        assert_eq!(b.hash, e["hash"].as_str().unwrap());
        assert_eq!(b.tokens.len() as u64, e["tokens"].as_u64().unwrap());
    }
}

#[test]
fn collapse_uses_explicit_class_not_backslash_s() {
    assert_eq!(collapse(" a  b  c\n"), "a b c");
    assert_eq!(collapse("a\u{feff}b"), "a\u{feff}b");
    assert_eq!(collapse("a\u{85}b"), "a\u{85}b");
}

#[test]
fn hash_is_nfc_sha1_prefix() {
    assert_eq!(block_hash("Grüezi"), "94a9bb2c");
    // decomposed input hashes like the composed form
    assert_eq!(block_hash("Gru\u{308}ezi"), "94a9bb2c");
}

#[test]
fn tokens_keep_punctuation() {
    assert_eq!(tokens("Spiel, Satz, Patt?"), ["Spiel,", "Satz,", "Patt?"]);
    assert!(tokens("").is_empty());
}

#[test]
fn empty_html_gives_no_blocks() {
    assert!(extract_blocks("").is_empty());
}

#[test]
fn blocks_match_goldens() {
    let root = fixtures();
    let mut checked = 0;
    let mut cases: Vec<(PathBuf, PathBuf)> = vec![(
        root.join("goldens/contract.json"),
        root.join("contract/post.html"),
    )];
    for entry in fs::read_dir(root.join("goldens")).unwrap() {
        let path = entry.unwrap().path();
        let stem = path.file_stem().unwrap().to_str().unwrap().to_owned();
        if path.extension().is_some_and(|e| e == "json")
            && !["normalize", "numbers", "spoken_words", "contract"].contains(&stem.as_str())
        {
            cases.push((path, root.join(format!("goldens/posts/{stem}.html"))));
        }
    }
    for (json, html) in cases {
        let golden: Value = serde_json::from_str(&fs::read_to_string(&json).unwrap()).unwrap();
        let blocks = extract_blocks(&fs::read_to_string(&html).unwrap());
        let want = golden["blocks"].as_array().unwrap();
        assert_eq!(blocks.len(), want.len(), "{}", json.display());
        for (i, (b, w)) in blocks.iter().zip(want).enumerate() {
            let ctx = format!("{} block {i}", json.display());
            assert_eq!(b.tag, w["tag"].as_str().unwrap(), "{ctx}");
            assert_eq!(b.text, w["text"].as_str().unwrap(), "{ctx}");
            assert_eq!(b.hash, w["hash"].as_str().unwrap(), "{ctx}");
            let toks: Vec<&str> = w["tokens"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t.as_str().unwrap())
                .collect();
            assert_eq!(b.tokens, toks, "{ctx}");
        }
        checked += 1;
    }
    assert_eq!(checked, 11);
}
