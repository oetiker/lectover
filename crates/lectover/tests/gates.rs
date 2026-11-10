//! Acceptance gate G4 (spec §8.2): the Qwen3-ASR take checker scores the published Python (mlx)
//! takes of the frohburg post >= 0.90 and two deliberately wrong takes (audio of one chunk,
//! text of another) < 0.90. It also re-aligns the Python takes with the Rust aligner, which
//! gives the interpolated-token share G5 compares against (the Python run did not log it).
//!
//! The published Ogg is decoded in-process (ogg + libopus decoder); this is gate time, not
//! render time. Needs the models and the Python render:
//!
//! `LECTOVER_MODELS=~/.cache/lectover/models LECTOVER_G4_POST=~/.local/share/lectover/site/olten.jetzt/frohburg-spiel-satz-patt cargo test --release -p lectover --features metal --test gates -- --ignored --nocapture`

use std::fs;
use std::path::{Path, PathBuf};

use audiopus::coder::Decoder;
use audiopus::{Channels, SampleRate};
use lectover::check::score;
use lectover::ffi::{asr_library_device, QwenAligner, QwenAsr};
use lectover::plan::plan_chunks;
use lectover::render::align_segments;
use lectover::text::extract_blocks;
use lectover::text::normalize::{load_lexicon, spoken_groups, Group};
use lectover::timings::{alignment_report, assemble_token_times};
use serde_json::Value;

const SLUG: &str = "frohburg-spiel-satz-patt";
const MIN_SCORE: f64 = 0.90;
const OPUS_SR: u32 = 48000;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn env_dir(name: &str) -> PathBuf {
    let dir = std::env::var_os(name).unwrap_or_else(|| panic!("{name} is unset (see the module docs)"));
    PathBuf::from(dir)
}

/// Decode a mono Ogg Opus file to 48 kHz samples, pre-skip removed.
fn decode_ogg_opus(path: &Path) -> Vec<f32> {
    let mut rdr = ogg::PacketReader::new(fs::File::open(path).unwrap());
    let head = rdr.read_packet_expected().unwrap();
    assert_eq!(&head.data[..8], b"OpusHead");
    assert_eq!(head.data[9], 1, "expected a mono stream");
    let pre_skip = u16::from_le_bytes([head.data[10], head.data[11]]) as usize;
    let tags = rdr.read_packet_expected().unwrap();
    assert_eq!(&tags.data[..8], b"OpusTags");
    let mut dec = Decoder::new(SampleRate::Hz48000, Channels::Mono).unwrap();
    let mut out = Vec::new();
    let mut buf = vec![0f32; 5760];
    while let Some(pkt) = rdr.read_packet().unwrap() {
        let n = dec
            .decode_float(Some((&pkt.data).try_into().unwrap()), (&mut buf[..]).try_into().unwrap(), false)
            .unwrap();
        out.extend_from_slice(&buf[..n]);
    }
    out.drain(..pre_skip.min(out.len()));
    out
}

#[test]
#[ignore = "needs model files (LECTOVER_MODELS) and the Python render (LECTOVER_G4_POST)"]
fn g4_checker_scores_python_takes() {
    let models = env_dir("LECTOVER_MODELS");
    let post_dir = env_dir("LECTOVER_G4_POST");
    let manifest: Value = serde_json::from_str(&fs::read_to_string(post_dir.join("manifest.json")).unwrap()).unwrap();
    let entry = &manifest["voices"][0];
    let timings: Value =
        serde_json::from_str(&fs::read_to_string(post_dir.join(entry["timings"].as_str().unwrap())).unwrap()).unwrap();
    let audio = decode_ogg_opus(&post_dir.join(entry["audio"].as_str().unwrap()));
    let duration = audio.len() as f64 / OPUS_SR as f64;
    println!("decoded {:.3} s, timings say {}", duration, timings["duration"]);
    assert!((duration - timings["duration"].as_f64().unwrap()).abs() < 0.05);

    // the text plan of the saved post, checked against the published timings and the golden
    let lexicon = load_lexicon(&root().join("examples/olten.jetzt/lexicon.txt")).unwrap();
    let html = fs::read_to_string(root().join(format!("tests/fixtures/goldens/posts/{SLUG}.html"))).unwrap();
    let blocks = extract_blocks(&html);
    let groups: Vec<Vec<Group>> = blocks.iter().map(|b| spoken_groups(&b.text, &lexicon)).collect();
    let chunks = plan_chunks(&blocks, &groups, 600);
    let golden: Value =
        serde_json::from_str(&fs::read_to_string(root().join(format!("tests/fixtures/goldens/{SLUG}.json"))).unwrap())
            .unwrap();
    let golden_texts: Vec<&str> = golden["chunks"].as_array().unwrap().iter().map(|c| c["text"].as_str().unwrap()).collect();
    assert_eq!(chunks.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), golden_texts);
    let tblocks = timings["blocks"].as_array().unwrap();
    assert_eq!(tblocks.len(), blocks.len());
    for (b, t) in blocks.iter().zip(tblocks) {
        assert_eq!(t["hash"].as_str().unwrap(), b.hash);
        assert_eq!(t["words"].as_array().unwrap().len(), b.tokens.len());
    }

    // chunk spans from the token times; cut each segment at the middle of the pauses around it
    let word = |bi: usize, ti: usize, k: usize| tblocks[bi]["words"][ti][k].as_f64().unwrap();
    let spans: Vec<(f64, f64)> = chunks
        .iter()
        .map(|c| {
            let first = groups[c.block][c.g0].first;
            let last = groups[c.block][c.g1 - 1].last - 1;
            (word(c.block, first, 0), word(c.block, last, 1))
        })
        .collect();
    let cuts: Vec<(f64, f64)> = (0..spans.len())
        .map(|i| {
            let s = if i == 0 { 0.0 } else { (spans[i - 1].1 + spans[i].0) / 2.0 };
            let e = if i + 1 == spans.len() { duration } else { (spans[i].1 + spans[i + 1].0) / 2.0 };
            (s, e)
        })
        .collect();
    let at = |t: f64| ((t * OPUS_SR as f64).round() as usize).min(audio.len());
    let parts: Vec<(f64, Vec<f32>)> = cuts.iter().map(|&(s, e)| (s, audio[at(s)..at(e)].to_vec())).collect();

    let device = asr_library_device();
    let mut asr = QwenAsr::load(&models.join("qwen3-asr-0.6b-f16.gguf"), device).unwrap();
    let mut real = Vec::new();
    for (i, ((_, wav), c)) in parts.iter().zip(&chunks).enumerate() {
        let (s, heard) = score(&mut asr, wav, OPUS_SR, &c.text, &lexicon).unwrap();
        println!("G4 real  chunk {}: {:.3} ({:.2}-{:.2} s) heard: {heard}", i + 1, s, cuts[i].0, cuts[i].1);
        real.push(s);
    }
    // wrong takes: the audio of one chunk scored against the text of another
    let mut wrong = Vec::new();
    for (a, t) in [(0usize, 1usize), (2, 3)] {
        let (s, _) = score(&mut asr, &parts[a].1, OPUS_SR, &chunks[t].text, &lexicon).unwrap();
        println!("G4 wrong audio {} vs text {}: {:.3}", a + 1, t + 1, s);
        wrong.push(s);
    }
    drop(asr);

    // Python's interpolated share, measured with the Rust aligner on the Python takes
    let mut aligner = QwenAligner::load(&models.join("qwen3-forced-aligner-0.6b-f16.gguf"), device).unwrap();
    let (ct, ci) = align_segments(&mut aligner, &parts, OPUS_SR, &chunks).unwrap();
    drop(aligner);
    let (_, interp) = assemble_token_times(&blocks, &groups, &chunks, &ct, &ci).unwrap();
    println!("G5 baseline (Python takes, Rust aligner): {}", alignment_report(&blocks, &interp));

    assert!(real.iter().all(|&s| s >= MIN_SCORE), "real takes below {MIN_SCORE}: {real:?}");
    assert!(wrong.iter().all(|&s| s < MIN_SCORE), "wrong takes not below {MIN_SCORE}: {wrong:?}");
}
