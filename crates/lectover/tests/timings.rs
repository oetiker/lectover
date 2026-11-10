use lectover::plan::{Chunk, ChunkKind};
use lectover::text::normalize::{spoken_groups, Group};
use lectover::text::{block_hash, collapse, tokens, Block};
use lectover::timings::*;
use serde_json::json;

fn mk(tag: &str, text: &str) -> Block {
    let text = collapse(text);
    Block { tag: tag.into(), hash: block_hash(&text), tokens: tokens(&text), text }
}

fn g(first: usize, last: usize, spoken: &str) -> Group {
    Group { first, last, spoken: spoken.into() }
}

fn ws(w: &[&str]) -> Vec<String> {
    w.iter().map(|s| s.to_string()).collect()
}

fn close(a: &[WordTime], b: &[WordTime]) {
    assert_eq!(a.len(), b.len(), "{a:?} vs {b:?}");
    for (x, y) in a.iter().zip(b) {
        assert!((x.0 - y.0).abs() < 1e-9 && (x.1 - y.1).abs() < 1e-9, "{a:?} vs {b:?}");
    }
}

fn starts_monotonic(ts: &[WordTime]) -> bool {
    ts.windows(2).all(|w| w[1].0 >= w[0].0 && w[0].0 <= w[0].1) && ts.last().is_none_or(|l| l.0 <= l.1)
}

#[test]
fn place_words_passthrough() {
    let (t, f) = place_words(&[(0.1, 0.4), (0.4, 0.9)], &ws(&["ab", "cd"]), 1.0);
    assert_eq!(t, vec![(0.1, 0.4), (0.4, 0.9)]);
    assert_eq!(f, vec![false, false]);
}

#[test]
fn place_words_zero_length_is_interpolated() {
    let (t, f) = place_words(&[(0.1, 0.4), (0.5, 0.5), (0.6, 0.9)], &ws(&["ab", "cd", "ef"]), 1.0);
    close(&t[1..2], &[(0.4, 0.6)]);
    assert_eq!(f, vec![false, true, false]);
}

#[test]
fn place_words_out_of_order_is_interpolated() {
    let (t, f) = place_words(&[(0.1, 0.4), (0.2, 0.3), (0.5, 0.8)], &ws(&["ab", "cd", "ef"]), 1.0);
    close(&t[1..2], &[(0.4, 0.5)]);
    assert!(f[1]);
}

#[test]
fn place_words_beyond_duration_is_interpolated() {
    let (t, f) = place_words(&[(0.1, 0.4), (0.9, 5.0)], &ws(&["ab", "cd"]), 1.0);
    close(&t[1..2], &[(0.4, 1.0)]);
    assert!(f[1]);
}

#[test]
fn place_words_all_unplaced_spread_by_length() {
    let (t, _) = place_words(&[(0.0, 0.0), (0.0, 0.0)], &ws(&["a", "bbb"]), 2.0);
    close(&t, &[(0.0, 0.5), (0.5, 2.0)]);
}

#[test]
fn token_times_one_to_one() {
    let gs = [g(0, 1, "Am"), g(1, 2, "fünfundzwanzigsten"), g(2, 3, "November")];
    let wt = [(0.0, 0.2), (0.2, 0.9), (0.9, 1.3)];
    let (t, _) = token_times(&["Am", "25.", "November"], &gs, &wt, &[false; 3], 0.0);
    assert_eq!(t, wt.to_vec());
}

#[test]
fn token_times_multi_token_group_split_by_length() {
    let gs = [g(0, 2, "zwölf Franken fünfzig")];
    let (t, _) = token_times(&["CHF", "12.50"], &gs, &[(1.0, 1.3), (1.3, 1.8), (1.8, 2.2)], &[false; 3], 0.0);
    close(&t, &[(1.0, 1.45), (1.45, 2.2)]);
}

#[test]
fn wordless_tokens_take_previous_end() {
    let gs = [g(0, 1, "Aktien"), g(1, 2, "–"), g(2, 3, "am")];
    let (t, _) = token_times(&["Aktien", "–", "am"], &gs, &[(0.0, 0.5), (0.6, 0.8)], &[false; 2], 0.0);
    assert_eq!(t[1], (0.5, 0.5));
}

#[test]
fn wordless_tokens_at_block_start_take_next_start() {
    let gs = [g(0, 1, "«"), g(1, 2, "rund»")];
    let (t, _) = token_times(&["«", "rund»"], &gs, &[(3.0, 3.4)], &[false], 2.0);
    assert_eq!(t[0], (3.0, 3.0));
}

#[test]
fn wordless_block_carries_previous_end() {
    let blocks = [mk("p", "Ja."), mk("p", "–")];
    let groups: Vec<Vec<Group>> = blocks.iter().map(|b| spoken_groups(&b.text, &Vec::new())).collect();
    let chunks = [Chunk { block: 0, g0: 0, g1: groups[0].len(), text: "Ja.".into(), kind: ChunkKind::Paragraph }];
    let (t, _) = assemble_token_times(&blocks, &groups, &chunks, &[vec![(0.2, 0.7)]], &[vec![false]]).unwrap();
    assert_eq!(t[1], vec![(0.7, 0.7)]);
}

#[test]
fn uncovered_group_gap_is_zero_length_at_neighbour() {
    let block = mk("p", "eins zwei drei");
    let groups = vec![g(0, 1, "eins"), g(1, 2, "zwei"), g(2, 3, "drei")];
    let chunks = [
        Chunk { block: 0, g0: 0, g1: 1, text: "eins".into(), kind: ChunkKind::Sentence },
        Chunk { block: 0, g0: 2, g1: 3, text: "drei".into(), kind: ChunkKind::Paragraph },
    ];
    let (t, _) = assemble_token_times(
        &[block],
        &[groups],
        &chunks,
        &[vec![(0.0, 0.4)], vec![(0.6, 1.0)]],
        &[vec![false], vec![false]],
    )
    .unwrap();
    assert_eq!(t[0], vec![(0.0, 0.4), (0.4, 0.4), (0.6, 1.0)]);
}

#[test]
fn assemble_rejects_word_count_mismatch() {
    let block = mk("p", "eins zwei");
    let groups = vec![g(0, 1, "eins"), g(1, 2, "zwei")];
    let chunks = [Chunk { block: 0, g0: 0, g1: 2, text: "eins zwei".into(), kind: ChunkKind::Paragraph }];
    assert!(assemble_token_times(&[block], &[groups], &chunks, &[vec![(0.0, 0.4)]], &[vec![false]]).is_err());
}

#[test]
fn build_timings_format_and_monotonic() {
    let (a, b) = (mk("p", "Ja klar"), mk("h2", "Titel"));
    let doc = build_timings("erzaehler", 2.0, &[a.clone(), b.clone()], &[vec![(0.0, 0.5), (0.5, 1.0)], vec![(1.2, 2.0)]])
        .unwrap();
    assert_eq!(
        doc,
        json!({"version": 1, "voice": "erzaehler", "duration": 2.0, "blocks": [
            {"tag": "p", "hash": a.hash, "words": [[0.0, 0.5], [0.5, 1.0]]},
            {"tag": "h2", "hash": b.hash, "words": [[1.2, 2.0]]}]})
    );
    assert!(build_timings("x", 2.0, &[a, b], &[vec![(0.0, 0.5), (0.5, 1.0)], vec![(0.4, 2.0)]]).is_err());
}

#[test]
fn round3_matches_python() {
    // values from Python's round(x, 3)
    for (x, want) in [(0.0005, 0.001), (1.2345, 1.234), (2.0015, 2.002), (0.0015, 0.002), (0.00025, 0.0), (1.0005, 1.0), (0.0625, 0.062), (0.1875, 0.188), (0.5625, 0.562)] {
        assert_eq!(round3(x), want, "round3({x})");
    }
}

#[test]
fn alignment_report_flags_bad_blocks() {
    let (a, c) = (mk("p", "Ja klar"), mk("p", "eins zwei drei vier"));
    let r = alignment_report(&[a, c], &[vec![false, false], vec![true, true, true, false]]);
    assert!(r.contains("aligned directly: 50.0 %") && r.contains("block 1"), "{r}");
}

#[test]
fn place_words_overlap_tolerance_keeps_starts_monotonic() {
    for items in [vec![(0.1, 0.5), (0.6, 0.6), (0.49, 0.9)], vec![(0.50, 0.51), (0.495, 0.8)]] {
        let words = vec!["ab".to_string(); items.len()];
        let (t, _) = place_words(&items, &words, 1.0);
        assert!(starts_monotonic(&t));
        assert!(t.windows(2).all(|w| w[1].0 >= w[0].1));
        build_timings("v", 1.0, &[mk("p", &vec!["x"; items.len()].join(" "))], &[t]).unwrap();
    }
}

#[test]
fn trailing_wordless_token_before_early_starting_word_stays_monotonic() {
    let (wt, wf) = place_words(&[(0.1, 0.5), (0.49, 0.9)], &ws(&["ab", "cd"]), 1.0);
    let gs = [g(0, 1, "ab"), g(1, 2, ""), g(2, 3, "cd")];
    let (t, _) = token_times(&["ab", ",", "cd"], &gs, &wt, &wf, 0.0);
    assert!(starts_monotonic(&t));
    let (t2, _) = token_times(&["ab", ",", "cd"], &gs, &[(0.1, 0.5), (0.49, 0.9)], &[false, false], 0.0);
    assert!(starts_monotonic(&t2));
}
