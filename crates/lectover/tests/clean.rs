//! `voice clean` tests: a fake denoiser stands in for RE-USE.

use std::fs;
use std::path::Path;

use anyhow::Result;
use clap::Parser;
use lectover::audio::denoise::Denoise;
use lectover::audio::read_wav;
use lectover::clean::clean_voice;
use lectover::cli::{Cli, Commands, VoiceCommand};
use lectover::voices::{add_voice, load_registry};

/// Halves the signal: a clearly different, same-length result.
struct Half;
impl Denoise for Half {
    fn denoise(&mut self, pcm: &[f32], _sr: u32) -> Result<Vec<f32>> {
        Ok(pcm.iter().map(|s| s * 0.5).collect())
    }
}

const SR: u32 = 16_000;

fn setup(dir: &Path) {
    let spec = hound::WavSpec { channels: 1, sample_rate: SR, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(dir.join("sample.wav"), spec).unwrap();
    for i in 0..SR {
        w.write_sample(((i % 100) as i16 - 50) * 400).unwrap();
    }
    w.finalize().unwrap();
    fs::write(dir.join("sample.txt"), "Hallo Welt\n").unwrap();
    add_voice(dir, "sample", "Sample").unwrap();
}

#[test]
fn clean_writes_new_voice_beside_old() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    setup(d);
    let (wav_before, txt_before) = (fs::read(d.join("sample.wav")).unwrap(), fs::read(d.join("sample.txt")).unwrap());
    let mut log = Vec::new();
    let out = clean_voice(&mut Half, d, "sample", "samplereuse", "Sample (RE-USE)", false, &mut log).unwrap();
    assert_eq!(out, d.join("samplereuse.wav"));
    assert_eq!(fs::read(d.join("sample.wav")).unwrap(), wav_before);
    assert_eq!(fs::read(d.join("sample.txt")).unwrap(), txt_before);
    assert_eq!(fs::read(d.join("samplereuse.txt")).unwrap(), txt_before);
    let (src, sr0) = read_wav(&d.join("sample.wav")).unwrap();
    let (new, sr1) = read_wav(&out).unwrap();
    assert_eq!((sr0, sr1), (SR, SR));
    assert_eq!(src.len(), new.len());
    for (a, b) in src.iter().zip(&new) {
        assert!((a * 0.5 - b).abs() < 2e-5, "{a} {b}");
    }
    let reg = load_registry(d).unwrap();
    assert_eq!(reg.get("sample").map(String::as_str), Some("Sample"));
    assert_eq!(reg.get("samplereuse").map(String::as_str), Some("Sample (RE-USE)"));
}

#[test]
fn clean_refuses_existing_target_without_force() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    setup(d);
    clean_voice(&mut Half, d, "sample", "samplereuse", "x", false, &mut Vec::new()).unwrap();
    let err = clean_voice(&mut Half, d, "sample", "samplereuse", "x", false, &mut Vec::new()).unwrap_err();
    assert!(format!("{err:#}").contains("already exists"), "{err:#}");
    clean_voice(&mut Half, d, "sample", "samplereuse", "y", true, &mut Vec::new()).unwrap();
    let kept = fs::read_dir(d).unwrap().filter_map(|e| e.ok()).filter(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        n.starts_with("samplereuse.") && n.len() > "samplereuse.wav".len() && n.ends_with(".wav")
    });
    assert_eq!(kept.count(), 1);
    assert_eq!(load_registry(d).unwrap()["samplereuse"], "y");
}

#[test]
fn clean_refuses_unknown_source() {
    let tmp = tempfile::tempdir().unwrap();
    setup(tmp.path());
    let err = clean_voice(&mut Half, tmp.path(), "nope", "nopereuse", "x", false, &mut Vec::new()).unwrap_err();
    assert!(format!("{err:#}").contains("unknown voice"), "{err:#}");
    assert!(!tmp.path().join("nopereuse.wav").exists());
}

#[test]
fn clean_refuses_same_id() {
    let tmp = tempfile::tempdir().unwrap();
    setup(tmp.path());
    let before = fs::read(tmp.path().join("sample.wav")).unwrap();
    let err = clean_voice(&mut Half, tmp.path(), "sample", "sample", "x", true, &mut Vec::new()).unwrap_err();
    assert!(format!("{err:#}").contains("same"), "{err:#}");
    assert_eq!(fs::read(tmp.path().join("sample.wav")).unwrap(), before);
}

#[test]
fn clean_logs_noise_floor() {
    let tmp = tempfile::tempdir().unwrap();
    setup(tmp.path());
    let mut log = Vec::new();
    clean_voice(&mut Half, tmp.path(), "sample", "samplereuse", "x", false, &mut log).unwrap();
    let log = String::from_utf8(log).unwrap();
    assert!(log.contains("noise floor") && log.contains("->") && log.contains(" dB"), "{log}");
}

#[test]
fn cli_parses_clean() {
    let cli = Cli::try_parse_from(["lectover", "voice", "clean", "sample", "--as", "t2", "--label", "L", "--force"]).unwrap();
    match cli.command {
        Commands::Voice(VoiceCommand::Clean(a)) => {
            assert_eq!((a.id.as_str(), a.as_id.as_deref(), a.label.as_deref(), a.force), ("sample", Some("t2"), Some("L"), true));
            assert!(a.voices.is_none());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn clean_ignores_unrelated_broken_voice() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    setup(d);
    let mut reg: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&fs::read_to_string(d.join("voices.json")).unwrap()).unwrap();
    reg.insert("ghost".into(), serde_json::json!({"label": "Ghost"}));
    fs::write(d.join("voices.json"), serde_json::to_string(&reg).unwrap()).unwrap();
    assert!(load_registry(d).is_err(), "precondition: registry is broken");
    clean_voice(&mut Half, d, "sample", "sample-clean", "Clean", false, &mut Vec::new()).unwrap();
    assert!(d.join("sample-clean.wav").exists());
}
