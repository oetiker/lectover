use std::fs;
use std::path::Path;

use lectover::voices::{add_voice, load_registry};

fn voice(dir: &Path, id: &str) {
    fs::write(dir.join(format!("{id}.wav")), b"RIFF").unwrap();
    fs::write(dir.join(format!("{id}.txt")), "text\n").unwrap();
}

fn items(dir: &Path) -> Vec<(String, String)> {
    load_registry(dir).unwrap().into_iter().collect()
}

fn s(a: &str, b: &str) -> (String, String) {
    (a.to_string(), b.to_string())
}

#[test]
fn load_registry_in_order() {
    let t = tempfile::tempdir().unwrap();
    for v in ["zeta", "alpha"] {
        voice(t.path(), v);
    }
    fs::write(
        t.path().join("voices.json"),
        r#"{"zeta": {"label": "Z"}, "alpha": {"label": "A"}}"#,
    )
    .unwrap();
    assert_eq!(items(t.path()), vec![s("zeta", "Z"), s("alpha", "A")]);
}

#[test]
fn load_registry_missing_txt_errors() {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("a.wav"), b"RIFF").unwrap();
    fs::write(t.path().join("voices.json"), r#"{"a": {"label": "A"}}"#).unwrap();
    let err = load_registry(t.path()).unwrap_err().to_string();
    let missing = t.path().join("a.txt");
    assert_eq!(err, format!("voice 'a': missing {}", missing.display()));
}

#[test]
fn load_registry_missing_file_and_bad_id_error() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("voices.json");
    let err = load_registry(t.path()).unwrap_err().to_string();
    assert_eq!(err, format!("voice registry missing: {}", path.display()));
    fs::write(&path, r#"{"Bad Id": {"label": "x"}}"#).unwrap();
    let err = load_registry(t.path()).unwrap_err().to_string();
    assert_eq!(
        err,
        format!("invalid voice id in {}: 'Bad Id'", path.display())
    );
}

#[test]
fn add_voice_appends_and_updates_label() {
    let t = tempfile::tempdir().unwrap();
    for v in ["a", "b"] {
        voice(t.path(), v);
    }
    add_voice(t.path(), "a", "A").unwrap();
    add_voice(t.path(), "b", "B").unwrap();
    add_voice(t.path(), "a", "A2").unwrap();
    assert_eq!(items(t.path()), vec![s("a", "A2"), s("b", "B")]);
}

#[test]
fn add_voice_rejects_bad_id() {
    let t = tempfile::tempdir().unwrap();
    let err = add_voice(t.path(), "Erz Äh", "x").unwrap_err().to_string();
    assert_eq!(err, "invalid voice id: 'Erz Äh'");
    assert!(!t.path().join("voices.json").exists());
}

#[test]
fn add_voice_writes_python_identical_json() {
    let t = tempfile::tempdir().unwrap();
    add_voice(t.path(), "tief", "Tiefe Stimme").unwrap();
    add_voice(t.path(), "erz", "Erzähler \"x\"").unwrap();
    let got = fs::read_to_string(t.path().join("voices.json")).unwrap();
    assert_eq!(
        got,
        "{\n  \"tief\": {\n    \"label\": \"Tiefe Stimme\"\n  },\n  \"erz\": {\n    \"label\": \"Erzähler \\\"x\\\"\"\n  }\n}\n"
    );
}

#[test]
fn adopt_as_new_voice_registry_flow() {
    let t = tempfile::tempdir().unwrap();
    let voices = t.path().join("voices");
    fs::create_dir_all(&voices).unwrap();
    voice(&voices, "tief");
    add_voice(&voices, "tief", "Tiefe Stimme").unwrap();
    assert_eq!(items(&voices), vec![s("tief", "Tiefe Stimme")]);
}
