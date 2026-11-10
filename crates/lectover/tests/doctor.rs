//! `lectover doctor` offline checks against a temporary home and fake probes.

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use lectover::doctor::{checks, exit_code, render_report, Check, HttpReply, Probe, Status};
use lectover::models::{ALIGNER, ASR, CODEC, TALKER};
use lectover::settings::{Overrides, Settings};

const PLENTY: u64 = 100 << 30;

struct Install {
    home: tempfile::TempDir,
}

impl Install {
    /// A home with a complete models dir, a registry holding voice `a`, and the given config.
    fn new(config: &str) -> Install {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let models = h.join(".cache/lectover/models");
        std::fs::create_dir_all(&models).unwrap();
        for f in [TALKER, CODEC, ASR, ALIGNER] {
            std::fs::write(models.join(f), "").unwrap();
        }
        let voices = h.join(".local/share/lectover/voices");
        std::fs::create_dir_all(&voices).unwrap();
        std::fs::write(voices.join("voices.json"), r#"{"a":{"label":"A"}}"#).unwrap();
        std::fs::write(voices.join("a.wav"), "").unwrap();
        std::fs::write(voices.join("a.txt"), "").unwrap();
        let cfg = h.join(".config/lectover");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(cfg.join("config.toml"), config).unwrap();
        Install { home }
    }

    fn settings(&self) -> Settings {
        let home = self.home.path().to_str().unwrap().to_string();
        let env = move |k: &str| (k == "HOME").then(|| OsString::from(&home));
        Settings::resolve_for_doctor(&Overrides::default(), &env).unwrap()
    }

    fn run(&self, site: Option<&str>, free: u64) -> Vec<Check> {
        let settings = self.settings();
        let env = |k: &str| (k == "GHOST_CONTENT_KEY").then(|| "secret-value".to_string());
        let http = |_: &str| -> anyhow::Result<HttpReply> { unreachable!("offline") };
        let free_bytes = move |_: &Path| Some(free);
        checks(&settings, site, true, &Probe { env: &env, http_get: &http, free_bytes: &free_bytes })
    }
}

fn site_cfg(domain: &str, extra: &str) -> String {
    format!("[site.\"{domain}\"]\nghost_url = \"https://{domain}\"\n{extra}\n")
}

fn failing<'a>(cs: &'a [Check]) -> Vec<&'a Check> {
    cs.iter().filter(|c| c.status == Status::Fail).collect()
}

#[test]
fn healthy_install_is_all_ok() {
    let i = Install::new(&site_cfg("a.example", "voices = [{ id = \"a\" }]"));
    let cs = i.run(None, PLENTY);
    assert!(failing(&cs).is_empty(), "{}", render_report(&cs));
    assert_eq!(exit_code(&cs), 0);
    assert!(!render_report(&cs).contains("secret-value"));
}

#[test]
fn missing_site_voice_fails_with_hint() {
    let i = Install::new(&site_cfg("a.example", "voices = [{ id = \"zz\" }]"));
    let cs = i.run(None, PLENTY);
    let f = failing(&cs);
    assert_eq!(f.len(), 1, "{}", render_report(&cs));
    assert!(f[0].detail.contains("zz"));
    assert!(f[0].hint.as_deref().unwrap().contains("lectover voice record"));
    assert_eq!(exit_code(&cs), 1);
}

#[test]
fn unknown_language_fails() {
    let i = Install::new(&site_cfg("a.example", "language = \"fr-FR\""));
    let cs = i.run(None, PLENTY);
    let f = failing(&cs);
    assert_eq!(f.len(), 1, "{}", render_report(&cs));
    assert!(f[0].detail.contains("de-CH") && f[0].detail.contains("fr-FR"));
}

#[test]
fn unknown_key_is_a_warning_not_a_failure() {
    let i = Install::new(&format!("bogus = 1\n{}", site_cfg("a.example", "")));
    let cs = i.run(None, PLENTY);
    assert!(failing(&cs).is_empty(), "{}", render_report(&cs));
    assert!(cs.iter().any(|c| c.status == Status::Warn && c.detail.contains("unknown key bogus")));
}

#[test]
fn broken_config_is_a_failure_with_the_error() {
    let i = Install::new("this is = = not toml");
    let cs = i.run(None, PLENTY);
    let f = failing(&cs);
    assert_eq!(f.len(), 1, "{}", render_report(&cs));
    assert_eq!(f[0].what, "config");
    assert!(f[0].detail.contains("invalid config file"));
}

#[test]
fn loose_ssh_key_permissions_fail() {
    let i = Install::new("");
    let key = i.home.path().join("id");
    std::fs::write(&key, "k").unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    let cfg = i.home.path().join(".config/lectover/config.toml");
    std::fs::write(&cfg, format!("[upload]\ntarget = \"h:/x\"\nssh_key = \"{}\"\n", key.display())).unwrap();
    let cs = i.run(None, PLENTY);
    let f = failing(&cs);
    assert_eq!(f.len(), 1, "{}", render_report(&cs));
    assert!(f[0].hint.as_deref().unwrap().contains("chmod 600"));
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(failing(&i.run(None, PLENTY)).is_empty());
}

#[test]
fn failing_ghost_key_shows_only_its_source() {
    let i = Install::new(&site_cfg("a.example", "ghost_key = { env = \"NOPE\" }"));
    let cs = i.run(None, PLENTY);
    let f = failing(&cs);
    assert_eq!(f.len(), 1, "{}", render_report(&cs));
    assert!(f[0].detail.contains("env NOPE"));
}

#[test]
fn site_filter_limits_site_checks() {
    let cfg = format!("{}{}", site_cfg("a.example", ""), site_cfg("b.example", ""));
    let i = Install::new(&cfg);
    let cs = i.run(Some("a.example"), PLENTY);
    assert!(cs.iter().any(|c| c.what.contains("a.example")));
    assert!(cs.iter().all(|c| !c.what.contains("b.example") && !c.detail.contains("b.example")));
}

#[test]
fn low_disk_space_warns() {
    let i = Install::new("");
    let cs = i.run(None, 1 << 30);
    assert!(cs.iter().any(|c| c.status == Status::Warn && c.what == "disk space models"));
}

#[test]
fn report_format() {
    let cs = vec![
        Check { status: Status::Ok, what: "config".into(), detail: "fine".into(), hint: None },
        Check { status: Status::Warn, what: "disk".into(), detail: "low".into(), hint: Some("free space".into()) },
        Check { status: Status::Fail, what: "models".into(), detail: "missing".into(), hint: None },
    ];
    assert_eq!(
        render_report(&cs),
        "ok   config — fine\nwarn disk — low\n     hint: free space\nFAIL models — missing\n"
    );
}

/// Runs the checks online with a fake HTTP layer and the key `s3cr3t`.
fn run_online(cfg: &str, http: &dyn Fn(&str) -> anyhow::Result<HttpReply>, offline: bool) -> Vec<Check> {
    let i = Install::new(cfg);
    let env = |k: &str| (k == "GHOST_CONTENT_KEY").then(|| "s3cr3t".to_string());
    let free = |_: &Path| Some(PLENTY);
    checks(&i.settings(), None, offline, &Probe { env: &env, http_get: http, free_bytes: &free })
}

fn ghost<'a>(cs: &'a [Check]) -> &'a Check {
    cs.iter().find(|c| c.what.contains("Ghost Content API")).expect("ghost check")
}

#[test]
fn ghost_ok() {
    let http = |u: &str| -> anyhow::Result<HttpReply> {
        assert!(u.starts_with("https://a.example/ghost/api/content/settings/?key="));
        Ok(HttpReply { status: 200, body: br#"{"settings":{"title":"T"}}"#.to_vec() })
    };
    let cs = run_online(&site_cfg("a.example", ""), &http, false);
    assert_eq!(ghost(&cs).status, Status::Ok);
    assert!(ghost(&cs).detail.contains('T'));
}

#[test]
fn ghost_ok_without_title() {
    let http = |_: &str| -> anyhow::Result<HttpReply> { Ok(HttpReply { status: 200, body: b"nope".to_vec() }) };
    let cs = run_online(&site_cfg("a.example", ""), &http, false);
    assert_eq!(ghost(&cs).status, Status::Ok);
    assert_eq!(ghost(&cs).detail, "reachable");
}

#[test]
fn ghost_key_rejected() {
    let http = |_: &str| -> anyhow::Result<HttpReply> { Ok(HttpReply { status: 401, body: vec![] }) };
    let cs = run_online(&site_cfg("a.example", ""), &http, false);
    let g = ghost(&cs);
    assert_eq!(g.status, Status::Fail);
    assert!(g.detail.contains("key rejected"));
    assert!(g.hint.as_deref().unwrap().contains("env GHOST_CONTENT_KEY"));
}

#[test]
fn ghost_other_status_fails() {
    let http = |_: &str| -> anyhow::Result<HttpReply> { Ok(HttpReply { status: 502, body: vec![] }) };
    let cs = run_online(&site_cfg("a.example", ""), &http, false);
    assert!(ghost(&cs).detail.contains("502"));
}

#[test]
fn ghost_errors_never_leak_the_key() {
    let http = |u: &str| -> anyhow::Result<HttpReply> {
        Err(anyhow::anyhow!("error sending request for url ({u}): connection refused; key s3cr3t"))
    };
    let cs = run_online(&site_cfg("a.example", ""), &http, false);
    assert_eq!(ghost(&cs).status, Status::Fail);
    let report = render_report(&cs);
    assert!(!report.contains("s3cr3t"), "{report}");
    assert!(report.contains("key=***"), "{report}");
}

#[test]
fn offline_skips_ghost() {
    let http = |_: &str| -> anyhow::Result<HttpReply> { panic!("must not be called offline") };
    let cs = run_online(&site_cfg("a.example", ""), &http, true);
    assert!(cs.iter().all(|c| !c.what.contains("Ghost Content API")));
}

#[test]
fn failed_key_skips_ghost_check() {
    let http = |_: &str| -> anyhow::Result<HttpReply> { panic!("no key, no request") };
    let cs = run_online(&site_cfg("a.example", "ghost_key = { env = \"NOPE\" }"), &http, false);
    assert!(cs.iter().all(|c| !c.what.contains("Ghost Content API")));
}

#[test]
fn invalid_default_device_is_a_failure() {
    let i = Install::new("[defaults]\ndevice = \"tpu\"\n");
    let cs = i.run(None, PLENTY);
    let f = failing(&cs);
    assert_eq!(f.len(), 1, "{}", render_report(&cs));
    assert_eq!(f[0].what, "gpu");
    assert!(f[0].detail.contains("defaults.device"), "{}", f[0].detail);
}
