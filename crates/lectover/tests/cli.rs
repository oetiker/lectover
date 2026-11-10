//! CLI (Task 14): argument parsing, the order of the render checks and `models fetch`.
//! No model files, no network: Ghost and Hugging Face are injected fakes.

use std::cell::{Cell, RefCell};
use std::fs;
use std::io::Cursor;
use std::net::TcpListener;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Result};
use clap::Parser;
use lectover::cli::{overrides, Cli, Commands, ModelsCommand};
use lectover::config::SiteVoice;
use lectover::ghost::{GhostConfig, HttpReply};
use lectover::models::{fetch_all, Download, ModelFiles, ModelSource, Origin, MODEL_SOURCES};
use lectover::render::run::{prepare, select_voices};
use lectover::settings::Settings;
use serde_json::json;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_lectover"))
}

fn cfg() -> GhostConfig {
    GhostConfig { url: "https://example.org".into(), key: "k".into() }
}

fn post_reply(visibility: &str, html: &str) -> HttpReply {
    let body = json!({"posts": [{
        "slug": "s", "html": html, "visibility": visibility,
        "updated_at": "2026-10-31T00:00:00.000Z", "title": "T"
    }]});
    HttpReply { status: 200, body: body.to_string().into_bytes() }
}

/// A registry with voices `a` and `b` (empty clips are enough: nothing is synthesised).
fn registry(dir: &Path) {
    fs::write(dir.join("voices.json"), r#"{"a": {"label": "A"}, "b": {"label": "B"}}"#).unwrap();
    for v in ["a", "b"] {
        fs::write(dir.join(format!("{v}.wav")), b"").unwrap();
        fs::write(dir.join(format!("{v}.txt")), b"x").unwrap();
    }
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn sv(id: &str, label: Option<&str>) -> SiteVoice {
    SiteVoice { id: id.into(), label: label.map(String::from) }
}

fn labels(p: &lectover::render::run::Prepared) -> Vec<(&str, &str)> {
    p.labels.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
}

#[test]
fn help_lists_subcommands() {
    let out = bin().arg("--help").output().unwrap();
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("render"), "{help}");
    assert!(help.contains("models"), "{help}");
    let out = bin().args(["models", "--help"]).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("fetch"));
}

/// Review Focus 1: missing models are reported before any HTTP request.
#[test]
fn render_without_models_names_fetch_command() {
    let models = tempfile::tempdir().unwrap();
    // A port nobody listens on: a request would fail with a connection error.
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    // A temp HOME and config: nothing touches the real ~/.config or ~/.local.
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    fs::write(&config, format!("[site.\"127.0.0.1\"]\nghost_url = \"http://127.0.0.1:{port}\"\n")).unwrap();
    let out = bin()
        .args(["render", "some-post", "--models"])
        .arg(models.path())
        .arg("--config")
        .arg(&config)
        .env("HOME", home.path())
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_STATE_HOME")
        .env_remove("XDG_CACHE_HOME")
        .env_remove("LECTOVER_CONFIG")
        .env("GHOST_CONTENT_KEY", "dummy")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("lectover models fetch"), "{err}");
    let talker = models.path().join("qwen-talker-1.7b-base-BF16.gguf");
    assert!(err.contains(&talker.display().to_string()), "{err}");
    assert!(!err.contains("cannot reach Ghost"), "an HTTP request was made: {err}");
    assert!(out.stdout.is_empty(), "nothing on stdout: {}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn model_files_check_lists_every_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("qwen-tokenizer-12hz-F32.gguf"), b"").unwrap();
    let err = ModelFiles::check(dir.path()).unwrap_err().to_string();
    for f in ["qwen-talker-1.7b-base-BF16.gguf", "qwen3-asr-0.6b-f16.gguf", "qwen3-forced-aligner-0.6b-f16.gguf"] {
        assert!(err.contains(&dir.path().join(f).display().to_string()), "{err}");
    }
    assert!(!err.contains("qwen-tokenizer"), "present file reported missing: {err}");
    for src in MODEL_SOURCES {
        fs::write(dir.path().join(src.file), b"").unwrap();
    }
    let files = ModelFiles::check(dir.path()).unwrap();
    assert_eq!(files.talker, dir.path().join("qwen-talker-1.7b-base-BF16.gguf"));
    assert_eq!(files.aligner, dir.path().join("qwen3-forced-aligner-0.6b-f16.gguf"));
}

/// `say` and `models` need no config file: with an empty HOME the settings still resolve.
#[test]
fn say_and_models_work_without_config() {
    let home = tempfile::tempdir().unwrap();
    let env = |k: &str| -> Option<OsString> { (k == "HOME").then(|| home.path().into()) };
    for argv in [
        vec!["lectover", "say", "Hallo."],
        vec!["lectover", "models", "fetch"],
        vec!["lectover", "say", "Hallo.", "--voices", "/v", "--models", "/m"],
    ] {
        let cli = Cli::try_parse_from(&argv).unwrap();
        let s = Settings::resolve(&overrides(&cli), &env).unwrap();
        assert!(s.config.is_none(), "{argv:?}");
        let Commands::Say(args) = cli.command else { continue };
        let lexicon = lectover::say::lexicon_path(args.lexicon.as_deref(), args.site.as_deref(), &s).unwrap();
        assert_eq!(lexicon, None, "{argv:?}: no config, no lexicon");
        if args.models.is_some() {
            assert_eq!((s.voices.as_path(), s.models.as_path()), (Path::new("/v"), Path::new("/m")));
        } else {
            assert_eq!(s.models, home.path().join(".cache/lectover/models"));
        }
    }
}

/// `say` takes the lexicon of `--site`, else of the only configured site, else none;
/// `--lexicon` wins over both.
#[test]
fn say_lexicon_follows_the_site() {
    let home = tempfile::tempdir().unwrap();
    let env = |k: &str| -> Option<OsString> { (k == "HOME").then(|| home.path().into()) };
    let cfg_dir = home.path().join(".config/lectover");
    fs::create_dir_all(&cfg_dir).unwrap();
    let one = "[site.\"a.org\"]\nghost_url = \"https://a.org\"\n";
    fs::write(cfg_dir.join("config.toml"), one).unwrap();
    let s = Settings::resolve(&Default::default(), &env).unwrap();
    let lex = |flag: Option<&Path>, site: Option<&str>| lectover::say::lexicon_path(flag, site, &s);
    assert_eq!(lex(None, None).unwrap(), Some(cfg_dir.join("lexicon/a.org.txt")));
    assert_eq!(lex(Some(Path::new("/l.txt")), None).unwrap(), Some(Path::new("/l.txt").into()));
    assert!(lex(None, Some("b.org")).is_err(), "an unknown --site is an error");

    let two = format!("{one}[site.\"b.org\"]\nghost_url = \"https://b.org\"\n");
    fs::write(cfg_dir.join("config.toml"), two).unwrap();
    let s = Settings::resolve(&Default::default(), &env).unwrap();
    let lex = |flag: Option<&Path>, site: Option<&str>| lectover::say::lexicon_path(flag, site, &s);
    assert_eq!(lex(None, None).unwrap(), None, "several sites and no --site: no lexicon");
    assert_eq!(lex(None, Some("b.org")).unwrap(), Some(cfg_dir.join("lexicon/b.org.txt")));
}

/// Review Focus 4: a non-public post is refused by `prepare`, which never loads a model.
#[test]
fn non_public_post_refused_before_models_load() {
    let voices = tempfile::tempdir().unwrap();
    registry(voices.path());
    let calls = Cell::new(0);
    let get = |_: &str| -> Result<HttpReply> {
        calls.set(calls.get() + 1);
        Ok(post_reply("members", "<p>Geheimer Text.</p>"))
    };
    let err = prepare(&cfg(), "s", &get, &[], voices.path(), &Vec::new(), 600).unwrap_err().to_string();
    assert_eq!(err, "post s is not public (visibility='members'); refusing to render");
    assert_eq!(calls.get(), 1);
}

/// Review Focus 4: a post without readable text is refused by `prepare`.
#[test]
fn empty_post_refused() {
    let voices = tempfile::tempdir().unwrap();
    registry(voices.path());
    let get = |_: &str| -> Result<HttpReply> { Ok(post_reply("public", "")) };
    let err = prepare(&cfg(), "s", &get, &[], voices.path(), &Vec::new(), 600).unwrap_err().to_string();
    assert_eq!(err, "post s has no readable text");
}

#[test]
fn prepare_plans_chunks_and_defaults_to_first_voice() {
    let voices = tempfile::tempdir().unwrap();
    registry(voices.path());
    let get = |_: &str| -> Result<HttpReply> { Ok(post_reply("public", "<h2>Titel</h2><p>Ein kurzer Satz.</p>")) };
    let p = prepare(&cfg(), "s", &get, &[], voices.path(), &Vec::new(), 600).unwrap();
    assert_eq!(p.voices, strings(&["a"]));
    assert_eq!(labels(&p), [("a", "A")]);
    // the two page blocks, then the spoken intro (title "T")
    assert_eq!(p.page_blocks, 2);
    assert_eq!(p.blocks.len(), 3);
    assert_eq!(p.chunks.len(), 3);
    assert_eq!(p.chunks[0].text, "Sie hören: «T».");
    assert_eq!(p.stings, [(1, lectover::script::Sting::Intro)]);
}

#[test]
fn prepare_uses_site_voices_and_labels() {
    let voices = tempfile::tempdir().unwrap();
    registry(voices.path());
    let get = |_: &str| -> Result<HttpReply> { Ok(post_reply("public", "<p>Ein Satz.</p>")) };
    let site = [sv("b", Some("Bee")), sv("a", None)];
    let p = prepare(&cfg(), "s", &get, &site, voices.path(), &Vec::new(), 600).unwrap();
    assert_eq!(p.voices, strings(&["b", "a"]));
    assert_eq!(labels(&p), [("b", "Bee"), ("a", "A")]);
}

#[test]
fn prepare_unknown_site_voice() {
    let voices = tempfile::tempdir().unwrap();
    registry(voices.path());
    let get = |_: &str| -> Result<HttpReply> { Ok(post_reply("public", "<p>Ein Satz.</p>")) };
    let site = [sv("a", None), sv("zz", Some("Zett"))];
    let err = prepare(&cfg(), "s", &get, &site, voices.path(), &Vec::new(), 600).unwrap_err().to_string();
    assert_eq!(err, "unknown voice(s): zz; known: a, b");
}

#[test]
fn prepare_voice_flag_overrides_site() {
    let voices = tempfile::tempdir().unwrap();
    registry(voices.path());
    let get = |_: &str| -> Result<HttpReply> { Ok(post_reply("public", "<p>Ein Satz.</p>")) };
    let site = [sv("b", Some("Bee")), sv("a", None)];
    // no flag: the site's voices
    let p = prepare(&cfg(), "s", &get, &select_voices(&[], &site), voices.path(), &Vec::new(), 600).unwrap();
    assert_eq!(labels(&p), [("b", "Bee"), ("a", "A")]);
    // flag: exactly those, in flag order, the site label where the site has one
    let chosen = select_voices(&strings(&["a"]), &site);
    let p = prepare(&cfg(), "s", &get, &chosen, voices.path(), &Vec::new(), 600).unwrap();
    assert_eq!((p.voices.clone(), labels(&p)), (strings(&["a"]), vec![("a", "A")]));
    let chosen = select_voices(&strings(&["a", "b"]), &site);
    let p = prepare(&cfg(), "s", &get, &chosen, voices.path(), &Vec::new(), 600).unwrap();
    assert_eq!(labels(&p), [("a", "A"), ("b", "Bee")]);
}

/// Review Focus 5: one id per `--voice`; the slug is never swallowed.
#[test]
fn voice_flags_keep_order_and_slug() {
    for argv in [
        ["lectover", "render", "slug", "--voice", "b", "--voice", "a"],
        ["lectover", "render", "--voice", "b", "slug", "--voice", "a"],
    ] {
        let cli = Cli::try_parse_from(argv).unwrap();
        let Commands::Render(args) = cli.command else { panic!("not render: {argv:?}") };
        assert_eq!(args.post, "slug", "{argv:?}");
        assert_eq!(args.voices, strings(&["b", "a"]), "{argv:?}");
    }
    // `--voice a b` no longer takes two ids: b is the slug.
    let cli = Cli::try_parse_from(["lectover", "render", "--voice", "a", "b"]).unwrap();
    let Commands::Render(args) = cli.command else { panic!() };
    assert_eq!((args.post.as_str(), args.voices), ("b", strings(&["a"])));
}

#[test]
fn render_args_have_no_relative_defaults() {
    let cli = Cli::try_parse_from(["lectover", "render", "p"]).unwrap();
    assert!(cli.config.is_none());
    let Commands::Render(a) = cli.command else { panic!() };
    assert!(a.out.is_none() && a.lexicon.is_none() && a.sounds.is_none() && a.site.is_none());
    assert_eq!((a.seed, a.temperature, a.no_pace), (42, 0.9, false));
    assert!(a.models.is_none() && a.device.is_none() && a.voices.is_empty());
    let cli = Cli::try_parse_from(["lectover", "render", "p", "--config", "/c.toml", "--out", "/o"]).unwrap();
    let o = overrides(&cli);
    assert_eq!((o.config.as_deref(), o.site.as_deref()), (Some(Path::new("/c.toml")), Some(Path::new("/o"))));
    let cli = Cli::try_parse_from(["lectover", "models", "fetch", "--models", "/m"]).unwrap();
    let Commands::Models(ModelsCommand::Fetch { models, design }) = cli.command else { panic!() };
    assert!(!design);
    assert_eq!(models.as_deref(), Some(Path::new("/m")));
}

/// Review Focus 5: an unknown voice names every known one.
#[test]
fn unknown_voice_lists_known() {
    let voices = tempfile::tempdir().unwrap();
    registry(voices.path());
    let get = |_: &str| -> Result<HttpReply> { Ok(post_reply("public", "<p>Ein Satz.</p>")) };
    let want: Vec<SiteVoice> = ["b", "x", "a", "y"].iter().map(|v| sv(v, None)).collect();
    let err = prepare(&cfg(), "s", &get, &want, voices.path(), &Vec::new(), 600).unwrap_err().to_string();
    assert_eq!(err, "unknown voice(s): x, y; known: a, b");
}

// --- models fetch -------------------------------------------------------------------------

const DATA: &[u8] = b"0123456789abcdefghij";

fn hf(file: &'static str) -> ModelSource {
    ModelSource { file, local: file, size: DATA.len() as u64, origin: Origin::HuggingFace { repo: "o/r", revision: "rev" } }
}

/// A fake HTTP server for `DATA` that honours (or ignores) `Range` and records each request.
struct Server {
    honour_range: bool,
    /// Bytes actually sent per request (a short value simulates a dropped connection).
    cut_at: Option<usize>,
    requests: RefCell<Vec<(String, u64)>>,
}

impl Server {
    fn new(honour_range: bool) -> Self {
        Server { honour_range, cut_at: None, requests: RefCell::new(Vec::new()) }
    }

    fn get(&self, url: &str, offset: u64) -> Result<Download> {
        self.requests.borrow_mut().push((url.to_string(), offset));
        let (status, start) = if offset > 0 && self.honour_range { (206, offset as usize) } else { (200, 0) };
        let body = &DATA[start..];
        let sent = &body[..self.cut_at.unwrap_or(body.len()).min(body.len())];
        Ok(Download { status, content_length: Some(body.len() as u64), body: Box::new(Cursor::new(sent.to_vec())) })
    }
}

#[test]
fn fetch_downloads_via_part_file() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::new(true);
    let mut log = Vec::new();
    fetch_all(dir.path(), &[hf("m.gguf")], &|u, o| server.get(u, o), &mut log).unwrap();
    assert_eq!(fs::read(dir.path().join("m.gguf")).unwrap(), DATA);
    assert!(!dir.path().join("m.gguf.part").exists());
    assert_eq!(*server.requests.borrow(), vec![("https://huggingface.co/o/r/resolve/rev/m.gguf".to_string(), 0)]);
    let log = String::from_utf8(log).unwrap();
    assert!(log.contains(&dir.path().join("m.gguf").display().to_string()), "{log}");
}

#[test]
fn fetch_resumes_part_file() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("m.gguf.part"), &DATA[..7]).unwrap();
    let server = Server::new(true);
    fetch_all(dir.path(), &[hf("m.gguf")], &|u, o| server.get(u, o), &mut Vec::new()).unwrap();
    assert_eq!(server.requests.borrow()[0].1, 7, "asks for the rest only");
    assert_eq!(fs::read(dir.path().join("m.gguf")).unwrap(), DATA);
}

#[test]
fn fetch_restarts_when_server_ignores_range() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("m.gguf.part"), b"0123XXX").unwrap();
    let server = Server::new(false);
    fetch_all(dir.path(), &[hf("m.gguf")], &|u, o| server.get(u, o), &mut Vec::new()).unwrap();
    assert_eq!(fs::read(dir.path().join("m.gguf")).unwrap(), DATA);
}

#[test]
fn fetch_keeps_part_on_short_body() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = Server::new(true);
    server.cut_at = Some(5);
    let err = fetch_all(dir.path(), &[hf("m.gguf")], &|u, o| server.get(u, o), &mut Vec::new()).unwrap_err();
    assert!(err.to_string().contains("m.gguf"), "{err}");
    assert!(!dir.path().join("m.gguf").exists(), "incomplete file must not be published");
    assert_eq!(fs::read(dir.path().join("m.gguf.part")).unwrap(), &DATA[..5]);
    // the next run resumes from byte 5
    server.cut_at = None;
    fetch_all(dir.path(), &[hf("m.gguf")], &|u, o| server.get(u, o), &mut Vec::new()).unwrap();
    assert_eq!(server.requests.borrow()[1].1, 5);
    assert_eq!(fs::read(dir.path().join("m.gguf")).unwrap(), DATA);
}

#[test]
fn fetch_rejects_unexpected_content_length() {
    let dir = tempfile::tempdir().unwrap();
    let get = |_: &str, _: u64| -> Result<Download> {
        Ok(Download { status: 200, content_length: Some(99), body: Box::new(Cursor::new(vec![0u8; 99])) })
    };
    let err = fetch_all(dir.path(), &[hf("m.gguf")], &get, &mut Vec::new()).unwrap_err().to_string();
    assert!(err.contains("99") && err.contains("20"), "{err}");
    assert!(!dir.path().join("m.gguf").exists());
}

#[test]
fn fetch_skips_complete_files() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("m.gguf"), DATA).unwrap();
    let get = |_: &str, _: u64| -> Result<Download> { Err(anyhow!("no request expected")) };
    fetch_all(dir.path(), &[hf("m.gguf")], &get, &mut Vec::new()).unwrap();
}

#[test]
fn model_sources_match_spike_log() {
    let tts_rev = "b7ee2e8c7459c3bea99da23e3d178125a7d1713c";
    let asr_rev = "ec843129c9f0b1760aa231fad5f83dbc39e1efa9";
    let want = [
        (
            "qwen-talker-1.7b-base-BF16.gguf",
            3_876_199_616,
            Origin::HuggingFace { repo: "Serveurperso/Qwen3-TTS-GGUF", revision: tts_rev },
        ),
        (
            "qwen-tokenizer-12hz-F32.gguf",
            647_263_104,
            Origin::HuggingFace { repo: "Serveurperso/Qwen3-TTS-GGUF", revision: tts_rev },
        ),
        (
            "qwen3-asr-0.6b-f16.gguf",
            1_882_543_424,
            Origin::HuggingFace { repo: "megaice/Qwen3-ASR-GGUF-Models", revision: asr_rev },
        ),
        (
            "qwen3-forced-aligner-0.6b-f16.gguf",
            1_842_214_208,
            Origin::HuggingFace { repo: "megaice/Qwen3-ASR-GGUF-Models", revision: asr_rev },
        ),
    ];
    let got: Vec<(&str, u64, Origin)> = MODEL_SOURCES.iter().map(|s| (s.file, s.size, s.origin)).collect();
    assert_eq!(got, want);
}
