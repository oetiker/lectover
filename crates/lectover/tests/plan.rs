use lectover::plan::{bucket_for, max_tokens_for, plan_chunks, Chunk, ChunkKind};
use lectover::text::normalize::{load_lexicon, spoken_groups, Group, Lexicon};
use lectover::text::spoken_words::spoken_words;
use lectover::text::{collapse, tokens, Block};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn blk(tag: &str, text: &str) -> (Block, Vec<Group>) {
    let text = collapse(text);
    let block = Block {
        tag: tag.to_string(),
        hash: String::new(),
        tokens: tokens(&text),
        text: text.clone(),
    };
    let groups = spoken_groups(&text, &Lexicon::new());
    (block, groups)
}

fn plan(bs: &[(Block, Vec<Group>)], max_chars: usize) -> Vec<Chunk> {
    let blocks: Vec<Block> = bs.iter().map(|b| b.0.clone()).collect();
    let groups: Vec<Vec<Group>> = bs.iter().map(|b| b.1.clone()).collect();
    plan_chunks(&blocks, &groups, max_chars)
}

#[test]
fn heading_is_single_title_chunk() {
    let b = blk("h2", "Erstens: das Patt");
    assert_eq!(
        plan(&[b], 600),
        vec![Chunk {
            block: 0,
            g0: 0,
            g1: 3,
            text: "Erstens: das Patt".into(),
            kind: ChunkKind::Title
        }]
    );
}

#[test]
fn short_paragraph_is_one_paragraph_chunk() {
    let b = blk("p", "Es war kalt. Dann kam der Wind. Wir gingen heim.");
    let n = b.1.len();
    let chunks = plan(&[b], 600);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].kind, ChunkKind::Paragraph);
    assert_eq!((chunks[0].g0, chunks[0].g1), (0, n));
}

#[test]
fn long_block_splits_at_sentence_ends_only() {
    let b = blk("p", "Am 25. November war es. Dann kam der Schnee.");
    let chunks = plan(&[b], 30);
    let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "Am fünfundzwanzigsten November war es.",
            "Dann kam der Schnee."
        ]
    );
    let kinds: Vec<ChunkKind> = chunks.iter().map(|c| c.kind).collect();
    assert_eq!(kinds, [ChunkKind::Sentence, ChunkKind::Paragraph]);
}

#[test]
fn chunks_never_span_blocks_and_cover_groups_in_order() {
    let a = blk(
        "p",
        "Das ist der erste Satz hier. Und noch ein zweiter Satz folgt.",
    );
    let c = blk("p", "Ein weiterer Absatz beginnt. Er hat auch zwei Sätze.");
    let lens = [a.1.len(), c.1.len()];
    let chunks = plan(&[a, c], 40);
    for (idx, len) in lens.iter().enumerate() {
        let mine: Vec<&Chunk> = chunks.iter().filter(|ch| ch.block == idx).collect();
        assert!(mine.len() > 1);
        assert_eq!(mine[0].g0, 0);
        assert_eq!(mine.last().unwrap().g1, *len);
        assert!(mine.windows(2).all(|w| w[0].g1 == w[1].g0));
    }
    assert!(chunks.windows(2).all(|w| w[0].block <= w[1].block));
}

#[test]
fn wordless_block_gets_no_chunk() {
    let bs = [blk("p", "Vorher."), blk("p", "–"), blk("p", "Nachher.")];
    let chunks = plan(&bs, 600);
    let mut got: Vec<usize> = chunks.iter().map(|c| c.block).collect();
    got.dedup();
    assert_eq!(got, [0, 2]);
}

#[test]
fn abbreviation_does_not_end_sentence() {
    let b = blk("p", "Es kostet ca. drei Franken. Gut so.");
    let chunks = plan(&[b], 30);
    assert!(chunks[0].text.starts_with("Es kostet circa"));
    assert!(chunks[0].text.ends_with("Franken."));
}

#[test]
fn spoken_words_match_aligner_tokenizer() {
    assert_eq!(
        spoken_words("S-B-O sagt: – «hallo»"),
        ["SBO", "sagt", "hallo"]
    );
}

#[test]
fn spoken_words_match_goldens() {
    let path = root().join("tests/fixtures/goldens/spoken_words.json");
    let cases: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let cases = cases.as_array().unwrap();
    assert!(!cases.is_empty());
    for c in cases {
        let want: Vec<&str> = c["out"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(spoken_words(c["in"].as_str().unwrap()), want, "{}", c["in"]);
    }
}

#[test]
fn max_tokens_scales_and_caps() {
    assert!(max_tokens_for("Erstens") <= 80);
    assert!(
        max_tokens_for("Erstens")
            < max_tokens_for("Das ist ein etwas längerer Satz mit vielen Silben.")
    );
    assert_eq!(max_tokens_for(&"Silbe ".repeat(300)), 1000);
}

#[test]
fn bucket_for_picks_smallest_fitting_bucket() {
    let got: Vec<usize> = [60, 100, 101, 999, 1000].map(bucket_for).to_vec();
    assert_eq!(got, [100, 100, 250, 1000, 1000]);
}

#[test]
fn chunks_match_goldens() {
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
        let blocks: Vec<Block> = golden["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| Block {
                tag: b["tag"].as_str().unwrap().to_string(),
                text: b["text"].as_str().unwrap().to_string(),
                hash: b["hash"].as_str().unwrap().to_string(),
                tokens: b["tokens"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| t.as_str().unwrap().to_string())
                    .collect(),
            })
            .collect();
        let groups: Vec<Vec<Group>> = blocks
            .iter()
            .map(|b| spoken_groups(&b.text, &lexicon))
            .collect();
        let got = plan_chunks(&blocks, &groups, 600);
        let want = golden["chunks"].as_array().unwrap();
        assert_eq!(got.len(), want.len(), "{stem}: chunk count");
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            let at = format!("{stem} chunk {i}");
            assert_eq!(g.block as u64, w["block"].as_u64().unwrap(), "{at}");
            assert_eq!(g.g0 as u64, w["g0"].as_u64().unwrap(), "{at}");
            assert_eq!(g.g1 as u64, w["g1"].as_u64().unwrap(), "{at}");
            assert_eq!(g.text, w["text"].as_str().unwrap(), "{at}");
            assert_eq!(g.kind.name(), w["kind"].as_str().unwrap(), "{at}");
            assert_eq!(
                max_tokens_for(&g.text) as u64,
                w["max_tokens"].as_u64().unwrap(),
                "{at}"
            );
            let words: Vec<&str> = w["spoken_words"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            assert_eq!(spoken_words(&g.text), words, "{at}");
        }
        checked += 1;
    }
    assert_eq!(checked, 10);
}

#[test]
fn title_tts_text_ends_with_a_full_stop() {
    let chunk = |text: &str, kind| Chunk {
        block: 0,
        g0: 0,
        g1: 1,
        text: text.into(),
        kind,
    };
    assert_eq!(
        chunk("Die Debatte", ChunkKind::Title).tts_text(),
        "Die Debatte."
    );
    assert_eq!(chunk("Erstens:", ChunkKind::Title).tts_text(), "Erstens.");
    assert_eq!(chunk("Was nun?", ChunkKind::Title).tts_text(), "Was nun?");
    assert_eq!(
        chunk("Was Olten jetzt! dazu sagt", ChunkKind::Title).tts_text(),
        "Was Olten jetzt! dazu sagt."
    );
    // body chunks are sent as they are
    assert_eq!(
        chunk("und dann", ChunkKind::Sentence).tts_text(),
        "und dann"
    );
}
