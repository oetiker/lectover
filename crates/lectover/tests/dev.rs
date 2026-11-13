//! Dev page and server (port of tests/test_dev_page.py) and `voice design` (Task 15).
//! No model files, no network, except the `#[ignore]`d smoke test, which loads the VoiceDesign
//! talker from `LECTOVER_MODELS`:
//!
//! `LECTOVER_MODELS=~/.cache/lectover/models cargo test -p lectover --features metal --test dev -- --ignored`

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use lectover::cli::{Cli, Commands, DevCommand, ModelsCommand, VoiceCommand};
use lectover::design::{design_voice, DesignRequest, VoiceDesigner, MAX_TOKENS, REF_TEXT};
use lectover::dev::{build, page_path, render_dev_page, DevServer};
use lectover::ghost::{GhostConfig, HttpReply, Post};
use lectover::models::{DesignFiles, DESIGN_SOURCES, DESIGN_TALKER};
use lectover::voices::load_registry;
use serde_json::json;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn post(title: &str, html: &str, reading_time: Option<u32>) -> Post {
    Post {
        title: title.into(),
        html: html.into(),
        reading_time,
        ..Post::default()
    }
}

// --- page --------------------------------------------------------------------------------

#[test]
fn render_dev_page_shell() {
    let p = post(
        "Hallo <b>Welt</b>",
        "<p>Erster Absatz.</p><p>Zweiter <em>Absatz</em>.</p>",
        Some(3),
    );
    let page = render_dev_page(&p, "olten.jetzt");
    assert!(page.contains(r#"<body class="post-template">"#));
    assert!(page.contains("<h1>Hallo &lt;b&gt;Welt&lt;/b&gt;</h1>"));
    assert!(!page.contains("<b>Welt</b>"));
    assert!(page.contains(
        r#"<span class="byline-reading-time"><span class="bull">•</span> 3 Minuten Lesezeit</span>"#
    ));
    assert!(page.contains(r#"class="article-byline-meta""#));
    assert!(page.contains(&format!(
        r#"<section class="gh-content gh-canvas">{}</section>"#,
        p.html
    )));
    assert!(page.contains(r#"<script src="/js?domain=olten.jetzt" defer></script>"#));
}

/// Python printed "1 Minuten Lesezeit" here; spec D12 fixes the singular.
#[test]
fn render_dev_page_default_reading_time() {
    let page = render_dev_page(&post("T", "<p>x</p>", None), "olten.jetzt");
    assert!(page.contains("> 1 Minute Lesezeit</span>"), "{page}");
}

#[test]
fn reading_time_singular() {
    let page = render_dev_page(&post("T", "<p>x</p>", Some(1)), "olten.jetzt");
    assert!(
        page.contains("<span class=\"bull\">•</span> 1 Minute Lesezeit</span>"),
        "{page}"
    );
    assert!(!page.contains("Minuten"), "{page}");
    let page = render_dev_page(&post("T", "<p>x</p>", Some(2)), "olten.jetzt");
    assert!(page.contains("> 2 Minuten Lesezeit</span>"), "{page}");
}

#[test]
fn title_and_domain_are_escaped_like_python_html_escape() {
    let page = render_dev_page(&post(r#"A & "B" 'C'"#, "", None), r#"x"y"#);
    assert!(
        page.contains("<title>A &amp; &quot;B&quot; &#x27;C&#x27;</title>"),
        "{page}"
    );
    assert!(
        page.contains(r#"<script src="/js?domain=x&quot;y" defer></script>"#),
        "{page}"
    );
}

#[test]
fn page_path_matches_audio_layout() {
    let tmp = Path::new("/t");
    assert_eq!(
        page_path(tmp, "olten.jetzt", "a-b"),
        tmp.join("olten.jetzt").join("a-b").join("index.html")
    );
}

#[test]
fn build_writes_page_next_to_audio_and_refuses_non_public() {
    let site = tempfile::tempdir().unwrap();
    let cfg = GhostConfig {
        url: "https://www.Olten.jetzt".into(),
        key: "k".into(),
    };
    let reply = |vis: &str| {
        let body = json!({"posts": [{"slug": "a-b", "html": "<p>Text.</p>", "visibility": vis,
                                      "title": "Titel", "reading_time": 4}]});
        HttpReply {
            status: 200,
            body: body.to_string().into_bytes(),
        }
    };
    let get = |url: &str| -> Result<HttpReply> {
        assert!(url.contains("/posts/slug/a-b/"), "{url}");
        Ok(reply("public"))
    };
    let page = build(
        &cfg,
        "olten.jetzt",
        "https://olten.jetzt/a-b/",
        site.path(),
        &get,
    )
    .unwrap();
    assert_eq!(page, site.path().join("olten.jetzt/a-b/index.html"));
    let html = fs::read_to_string(&page).unwrap();
    assert!(
        html.contains("<h1>Titel</h1>") && html.contains("4 Minuten Lesezeit"),
        "{html}"
    );

    let get = |_: &str| -> Result<HttpReply> { Ok(reply("members")) };
    let err = build(&cfg, "olten.jetzt", "geheim", site.path(), &get)
        .unwrap_err()
        .to_string();
    assert!(err.contains("not public"), "{err}");
}

#[test]
fn build_uses_the_configured_domain_not_the_ghost_host() {
    let site = tempfile::tempdir().unwrap();
    let cfg = GhostConfig {
        url: "https://b.example".into(),
        key: "k".into(),
    };
    let body = json!({"posts": [{"slug": "a-b", "html": "<p>x</p>", "visibility": "public",
                                  "title": "T", "reading_time": 1}]});
    let get = |_: &str| -> Result<HttpReply> {
        Ok(HttpReply {
            status: 200,
            body: body.to_string().into_bytes(),
        })
    };
    let page = build(&cfg, "a.example", "a-b", site.path(), &get).unwrap();
    assert_eq!(page, site.path().join("a.example/a-b/index.html"));
    assert!(fs::read_to_string(&page)
        .unwrap()
        .contains("/js?domain=a.example"));
}

// --- server ------------------------------------------------------------------------------

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn get(url: &str, range: Option<&str>) -> Reply {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    let mut req = agent.get(url);
    if let Some(r) = range {
        req = req.header("Range", r);
    }
    let resp = req.call().unwrap();
    let status = resp.status().as_u16();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                v.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let mut body = Vec::new();
    resp.into_body()
        .into_reader()
        .read_to_end(&mut body)
        .unwrap();
    Reply {
        status,
        headers,
        body,
    }
}

/// A server on an ephemeral port over a temp dir with `ten.bin` and `a.ogg`.
fn server() -> (tempfile::TempDir, DevServer) {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("ten.bin"), b"0123456789").unwrap();
    fs::write(dir.path().join("a.ogg"), b"OggS").unwrap();
    let srv = DevServer::start(
        "127.0.0.1:0",
        dir.path(),
        &root().join("player/lectover.js"),
    )
    .unwrap();
    (dir, srv)
}

#[test]
fn range_request_returns_206() {
    let (_dir, srv) = server();
    let r = get(&format!("{}/ten.bin", srv.url()), Some("bytes=2-5"));
    assert_eq!(r.status, 206);
    assert_eq!(r.body, b"2345");
    assert_eq!(r.header("Content-Range"), Some("bytes 2-5/10"));
    assert_eq!(r.header("Content-Length"), Some("4"));
    assert_eq!(r.header("Accept-Ranges"), Some("bytes"));

    let r = get(&format!("{}/ten.bin", srv.url()), None);
    assert_eq!(r.status, 200);
    assert_eq!(r.body, b"0123456789");
    assert_eq!(r.header("Accept-Ranges"), Some("bytes"));
    srv.stop();
}

#[test]
fn open_ended_and_clamped_ranges() {
    let (_dir, srv) = server();
    let url = format!("{}/ten.bin", srv.url());
    let r = get(&url, Some("bytes=7-"));
    assert_eq!(r.status, 206);
    assert_eq!(r.body, b"789");
    assert_eq!(r.header("Content-Range"), Some("bytes 7-9/10"));

    let r = get(&url, Some("bytes=8-99"));
    assert_eq!(r.status, 206);
    assert_eq!(r.body, b"89");
    assert_eq!(r.header("Content-Range"), Some("bytes 8-9/10"));

    let r = get(&url, Some("bytes=50-"));
    assert_eq!(r.status, 416);
    assert_eq!(r.header("Content-Range"), Some("bytes */10"));
    assert!(r.body.is_empty());
    srv.stop();
}

#[test]
fn suffix_and_unsatisfiable_ranges() {
    let (_dir, srv) = server();
    let url = format!("{}/ten.bin", srv.url());
    let r = get(&url, Some("bytes=-3"));
    assert_eq!((r.status, r.body.as_slice()), (206, b"789".as_slice()));
    assert_eq!(r.header("Content-Range"), Some("bytes 7-9/10"));
    // a suffix longer than the file is the whole file (RFC 7233 2.1)
    let r = get(&url, Some("bytes=-99"));
    assert_eq!(
        (r.status, r.body.as_slice()),
        (206, b"0123456789".as_slice())
    );
    assert_eq!(r.header("Content-Range"), Some("bytes 0-9/10"));
    // a zero-length suffix and a first byte at the end are unsatisfiable
    for range in ["bytes=-0", "bytes=10-", "bytes=10-12"] {
        let r = get(&url, Some(range));
        assert_eq!(r.status, 416, "{range}");
        assert_eq!(r.header("Content-Range"), Some("bytes */10"), "{range}");
    }
    // the last byte inclusive
    let r = get(&url, Some("bytes=9-9"));
    assert_eq!((r.status, r.body.as_slice()), (206, b"9".as_slice()));
    // syntactically invalid or multi-range headers are ignored: the whole file (RFC 7233 3.1)
    for range in [
        "bytes=5-2",
        "bytes=-",
        "items=0-1",
        "bytes=0-1,3-4",
        "bytes=x-1",
    ] {
        let r = get(&url, Some(range));
        assert_eq!(
            (r.status, r.body.as_slice()),
            (200, b"0123456789".as_slice()),
            "{range}"
        );
    }
    srv.stop();
}

#[test]
fn ogg_content_type() {
    let (_dir, srv) = server();
    let r = get(&format!("{}/a.ogg", srv.url()), None);
    assert_eq!(r.header("Content-Type"), Some("audio/ogg"));
    let r = get(&format!("{}/a.ogg", srv.url()), Some("bytes=0-1"));
    assert_eq!(
        (r.status, r.header("Content-Type")),
        (206, Some("audio/ogg"))
    );
    srv.stop();
}

#[test]
fn js_route_serves_the_player() {
    let (_dir, srv) = server();
    let r = get(&format!("{}/js?domain=olten.jetzt", srv.url()), None);
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Content-Type"), Some("text/javascript"));
    assert_eq!(r.body, fs::read(root().join("player/lectover.js")).unwrap());
    srv.stop();
}

#[test]
fn directories_serve_index_html_and_missing_files_404() {
    let (dir, srv) = server();
    let page = page_path(dir.path(), "olten.jetzt", "a-b");
    fs::create_dir_all(page.parent().unwrap()).unwrap();
    fs::write(&page, "<html>dev</html>").unwrap();
    let r = get(&format!("{}/olten.jetzt/a-b/", srv.url()), None);
    assert_eq!(
        (r.status, r.body.as_slice()),
        (200, b"<html>dev</html>".as_slice())
    );
    assert_eq!(r.header("Content-Type"), Some("text/html"));
    // like Python's handler: a directory without the trailing slash redirects
    let r = get(&format!("{}/olten.jetzt/a-b?x=1", srv.url()), None);
    assert_eq!(r.status, 301);
    assert_eq!(r.header("Location"), Some("/olten.jetzt/a-b/?x=1"));
    assert_eq!(get(&format!("{}/nope.ogg", srv.url()), None).status, 404);
    srv.stop();
}

#[test]
fn paths_cannot_escape_the_site() {
    let outer = tempfile::tempdir().unwrap();
    fs::write(outer.path().join("secret.txt"), b"secret").unwrap();
    let site = outer.path().join("site");
    fs::create_dir(&site).unwrap();
    fs::write(site.join("ok.txt"), b"ok").unwrap();
    let srv = DevServer::start("127.0.0.1:0", &site, &root().join("player/lectover.js")).unwrap();
    for path in [
        "/../secret.txt",
        "/%2e%2e/secret.txt",
        "/x/..%2F..%2Fsecret.txt",
    ] {
        let r = get(&format!("{}{path}", srv.url()), None);
        assert_ne!(r.body, b"secret", "{path}");
    }
    assert_eq!(get(&format!("{}/%6Fk.txt", srv.url()), None).body, b"ok");
    srv.stop();
}

/// Symlinks are followed only while their target stays inside the site.
#[test]
fn symlinks_out_of_the_site_are_404() {
    use std::os::unix::fs::symlink;
    let outer = tempfile::tempdir().unwrap();
    fs::write(outer.path().join("secret.txt"), b"secret").unwrap();
    let site = outer.path().join("site");
    fs::create_dir_all(site.join("real")).unwrap();
    fs::write(site.join("ok.txt"), b"ok").unwrap();
    fs::write(site.join("real/x.txt"), b"x").unwrap();
    // targets outside the site
    symlink("../secret.txt", site.join("link.txt")).unwrap();
    symlink(outer.path(), site.join("updir")).unwrap();
    fs::create_dir(site.join("page")).unwrap();
    symlink(
        outer.path().join("secret.txt"),
        site.join("page/index.html"),
    )
    .unwrap();
    // targets inside the site
    symlink("ok.txt", site.join("inner.txt")).unwrap();
    symlink("real", site.join("alias")).unwrap();
    let srv = DevServer::start("127.0.0.1:0", &site, &root().join("player/lectover.js")).unwrap();
    for path in ["/link.txt", "/updir/secret.txt", "/updir/", "/page/"] {
        let r = get(&format!("{}{path}", srv.url()), None);
        assert_eq!(r.status, 404, "{path}");
        assert_ne!(r.body, b"secret", "{path}");
    }
    let r = get(&format!("{}/link.txt", srv.url()), Some("bytes=0-2"));
    assert_eq!(r.status, 404);
    for (path, body) in [
        ("/inner.txt", b"ok".as_slice()),
        ("/alias/x.txt", b"x".as_slice()),
    ] {
        let r = get(&format!("{}{path}", srv.url()), None);
        assert_eq!((r.status, r.body.as_slice()), (200, body), "{path}");
    }
    let r = get(&format!("{}/inner.txt", srv.url()), Some("bytes=1-"));
    assert_eq!((r.status, r.body.as_slice()), (206, b"k".as_slice()));
    srv.stop();
}

/// `/js` follows the same rule against the player's directory.
#[test]
fn js_symlink_out_of_the_player_dir_is_404() {
    use std::os::unix::fs::symlink;
    let outer = tempfile::tempdir().unwrap();
    fs::write(outer.path().join("secret.js"), b"secret").unwrap();
    let site = outer.path().join("site");
    let players = outer.path().join("player");
    fs::create_dir_all(&site).unwrap();
    fs::create_dir_all(&players).unwrap();
    fs::write(players.join("real.js"), b"player()").unwrap();
    symlink("real.js", players.join("inside.js")).unwrap();
    symlink("../secret.js", players.join("outside.js")).unwrap();

    let srv = DevServer::start("127.0.0.1:0", &site, &players.join("outside.js")).unwrap();
    let r = get(&format!("{}/js", srv.url()), None);
    assert_eq!(r.status, 404);
    assert_ne!(r.body, b"secret");
    srv.stop();

    let srv = DevServer::start("127.0.0.1:0", &site, &players.join("inside.js")).unwrap();
    let r = get(&format!("{}/js?domain=x", srv.url()), None);
    assert_eq!((r.status, r.body.as_slice()), (200, b"player()".as_slice()));
    assert_eq!(r.header("Content-Type"), Some("text/javascript"));
    srv.stop();
}

// --- voice design ------------------------------------------------------------------------

/// Speaks a 440 Hz tone with silent edges; records what it was asked.
struct FakeDesigner {
    calls: Vec<(String, String, usize, u64, f32)>,
}

impl VoiceDesigner for FakeDesigner {
    fn sample_rate(&self) -> u32 {
        24000
    }

    fn design(
        &mut self,
        text: &str,
        instruct: &str,
        max_tokens: usize,
        seed: u64,
        temperature: f32,
    ) -> Result<Vec<f32>> {
        self.calls
            .push((text.into(), instruct.into(), max_tokens, seed, temperature));
        let mut wav = vec![0.0f32; 12000];
        wav.extend(
            (0..48000).map(|i| 0.3 * (i as f32 * 440.0 * std::f32::consts::TAU / 24000.0).sin()),
        );
        wav.extend(vec![0.0f32; 12000]);
        Ok(wav)
    }
}

fn req(
    id: &str,
    label: &str,
    instruct: &str,
    seed: u64,
    temperature: f32,
    force: bool,
) -> DesignRequest {
    DesignRequest {
        id: id.into(),
        label: label.into(),
        instruct: instruct.into(),
        seed,
        temperature,
        force,
    }
}

fn existing_voice(dir: &Path) {
    fs::write(
        dir.join("voices.json"),
        "{\n  \"alt\": {\n    \"label\": \"Alt\"\n  },\n  \"b\": {\n    \"label\": \"B\"\n  }\n}\n",
    )
    .unwrap();
    for v in ["alt", "b"] {
        fs::write(dir.join(format!("{v}.wav")), b"old wav").unwrap();
        fs::write(dir.join(format!("{v}.txt")), b"old text\n").unwrap();
    }
}

#[test]
fn design_writes_clip_transcript_and_registers() {
    let dir = tempfile::tempdir().unwrap();
    let mut fake = FakeDesigner { calls: Vec::new() };
    let mut log = Vec::new();
    let wav = design_voice(
        &mut fake,
        dir.path(),
        &req("neu", "Neue Stimme", "Eine Stimme.", 7, 0.8, false),
        &mut log,
    )
    .unwrap();
    assert_eq!(wav, dir.path().join("neu.wav"));
    assert_eq!(
        fake.calls,
        vec![(
            REF_TEXT.to_string(),
            "Eine Stimme.".to_string(),
            MAX_TOKENS,
            7,
            0.8
        )]
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("neu.txt")).unwrap(),
        format!("{REF_TEXT}\n")
    );
    let reader = hound::WavReader::open(&wav).unwrap();
    let spec = reader.spec();
    assert_eq!(
        (spec.channels, spec.sample_rate, spec.bits_per_sample),
        (1, 24000, 16)
    );
    assert_eq!(spec.sample_format, hound::SampleFormat::Int);
    // trimmed like design_voices.py (longform.trim_and_level): 2 s tone + 0.1 s kept per edge
    let secs = reader.duration() as f64 / 24000.0;
    assert!((2.0..2.4).contains(&secs), "{secs} s");
    let reg = load_registry(dir.path()).unwrap();
    assert_eq!(reg["neu"], "Neue Stimme");
    let log = String::from_utf8(log).unwrap();
    assert!(log.contains(&wav.display().to_string()), "{log}");
}

#[test]
fn design_refuses_existing_id_without_force() {
    let dir = tempfile::tempdir().unwrap();
    existing_voice(dir.path());
    let mut fake = FakeDesigner { calls: Vec::new() };
    let err = design_voice(
        &mut fake,
        dir.path(),
        &req("alt", "Neu", "I", 42, 0.9, false),
        &mut Vec::new(),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("alt") && err.contains("--force"), "{err}");
    assert!(
        fake.calls.is_empty(),
        "nothing is designed for a refused id"
    );
    assert_eq!(fs::read(dir.path().join("alt.wav")).unwrap(), b"old wav");

    // a stray clip without a registry entry counts as existing, too
    fs::write(dir.path().join("lose.wav"), b"x").unwrap();
    assert!(design_voice(
        &mut fake,
        dir.path(),
        &req("lose", "L", "I", 42, 0.9, false),
        &mut Vec::new()
    )
    .is_err());
    let err = design_voice(
        &mut fake,
        dir.path(),
        &req("Bad_Id", "L", "I", 42, 0.9, false),
        &mut Vec::new(),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("invalid voice id"), "{err}");
}

/// The adopt flow of design_voices.py: the old voice is kept with a timestamp.
#[test]
fn design_force_keeps_old_voice_with_timestamp() {
    let dir = tempfile::tempdir().unwrap();
    existing_voice(dir.path());
    let mut fake = FakeDesigner { calls: Vec::new() };
    let mut log = Vec::new();
    design_voice(
        &mut fake,
        dir.path(),
        &req("alt", "Neu", "I", 42, 0.9, true),
        &mut log,
    )
    .unwrap();
    let backups: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("alt.") && n != "alt.wav" && n != "alt.txt")
        .collect();
    assert_eq!(backups.len(), 2, "{backups:?}");
    for b in &backups {
        // alt.YYYYmmdd-HHMMSS.wav|txt
        let stamp = &b["alt.".len()..b.len() - 4];
        assert_eq!(stamp.len(), 15, "{b}");
        assert!(
            stamp.chars().enumerate().all(|(i, c)| if i == 8 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }),
            "{b}"
        );
        let old = fs::read(dir.path().join(b)).unwrap();
        assert!(old == b"old wav" || old == b"old text\n", "{b}");
    }
    assert_ne!(fs::read(dir.path().join("alt.wav")).unwrap(), b"old wav");
    // the existing entry keeps its position and takes the new label
    let reg = load_registry(dir.path()).unwrap();
    assert_eq!(
        reg.iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect::<Vec<_>>(),
        [("alt", "Neu"), ("b", "B")]
    );
    assert!(String::from_utf8(log).unwrap().contains("kept old voice"));
}

// --- CLI and models ----------------------------------------------------------------------

#[test]
fn cli_dev_and_voice_defaults() {
    let cli = Cli::try_parse_from(["lectover", "dev", "build", "a-b"]).unwrap();
    let Commands::Dev(DevCommand::Build { post, out, site }) = cli.command else {
        panic!()
    };
    assert_eq!((post.as_str(), out, site), ("a-b", None, None));
    let cli = Cli::try_parse_from(["lectover", "dev", "serve"]).unwrap();
    let Commands::Dev(DevCommand::Serve { port, out }) = cli.command else {
        panic!()
    };
    assert_eq!((port, out), (8000, None));

    let cli = Cli::try_parse_from([
        "lectover",
        "voice",
        "design",
        "neu",
        "--label",
        "Neu",
        "--instruct",
        "Eine Stimme.",
    ])
    .unwrap();
    let Commands::Voice(VoiceCommand::Design(a)) = cli.command else {
        panic!()
    };
    assert_eq!(
        (a.id.as_str(), a.label.as_str(), a.instruct.as_str()),
        ("neu", "Neu", "Eine Stimme.")
    );
    assert_eq!((a.seed, a.temperature, a.force), (42, 0.9, false));
    assert!(a.voices.is_none());
    assert!(a.models.is_none() && a.device.is_none());
    assert!(
        Cli::try_parse_from(["lectover", "voice", "design", "neu", "--label", "Neu"]).is_err(),
        "--instruct is required"
    );

    let cli = Cli::try_parse_from(["lectover", "models", "fetch", "--design"]).unwrap();
    let Commands::Models(ModelsCommand::Fetch { design, .. }) = cli.command else {
        panic!()
    };
    assert!(design);
}

#[test]
fn design_model_is_pinned_like_the_talker() {
    assert_eq!(DESIGN_TALKER, "qwen-talker-1.7b-voicedesign-BF16.gguf");
    assert_eq!(DESIGN_SOURCES.len(), 1);
    assert_eq!(
        (DESIGN_SOURCES[0].file, DESIGN_SOURCES[0].size),
        (DESIGN_TALKER, 3_839_585_184)
    );
    let talker = lectover::models::MODEL_SOURCES[0];
    assert_eq!(
        DESIGN_SOURCES[0].origin, talker.origin,
        "same repo and revision as the base talker"
    );

    let dir = tempfile::tempdir().unwrap();
    let err = DesignFiles::check(dir.path()).unwrap_err().to_string();
    assert!(
        err.contains(DESIGN_TALKER) && err.contains("qwen-tokenizer-12hz-F32.gguf"),
        "{err}"
    );
    assert!(err.contains("models fetch --design"), "{err}");
    assert!(
        !err.contains("qwen3-asr"),
        "voice design needs no ASR: {err}"
    );
}

/// `voice design` refuses an existing id before it looks for models.
#[test]
fn voice_design_cli_refuses_existing_id_before_models() {
    let voices = tempfile::tempdir().unwrap();
    existing_voice(voices.path());
    let models = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lectover"))
        .args([
            "voice",
            "design",
            "alt",
            "--label",
            "X",
            "--instruct",
            "I",
            "--voices",
        ])
        .arg(voices.path())
        .arg("--models")
        .arg(models.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--force"), "{err}");
    assert!(!err.contains("model files missing"), "{err}");
}

// --- smoke -------------------------------------------------------------------------------

#[test]
#[ignore = "needs the VoiceDesign talker (LECTOVER_MODELS)"]
fn design_smoke_with_qwentts_voice_design() {
    use lectover::design::load_designer;
    use lectover::ffi::resolve_device;
    let models =
        PathBuf::from(std::env::var_os("LECTOVER_MODELS").expect("LECTOVER_MODELS is unset"));
    let files = DesignFiles::check(&models).unwrap();
    let voices = tempfile::tempdir().unwrap();
    let instruct =
        "Ein ruhiger Mann mittleren Alters mit warmer, tiefer Stimme. Er spricht klares \
                    Schweizer Hochdeutsch, sachlich und überlegt.";
    {
        let mut designer = load_designer(&files, resolve_device(None).unwrap()).unwrap();
        let wav = design_voice(
            &mut *designer,
            voices.path(),
            &req("probe", "Probe", instruct, 42, 0.9, false),
            &mut Vec::new(),
        )
        .unwrap();
        let reader = hound::WavReader::open(&wav).unwrap();
        let secs = reader.duration() as f64 / 24000.0;
        // REF_TEXT read aloud takes roughly 6-12 s
        assert!((3.0..20.0).contains(&secs), "{secs} s");
        // same seed, same handle: identical take
        let a = designer
            .design(REF_TEXT, instruct, MAX_TOKENS, 42, 0.9)
            .unwrap();
        let b = designer
            .design(REF_TEXT, instruct, MAX_TOKENS, 42, 0.9)
            .unwrap();
        assert_eq!(a, b);
        // the handle is dropped here, before the process exits (Metal aborts otherwise)
    }
    assert_eq!(load_registry(voices.path()).unwrap()["probe"], "Probe");
}
