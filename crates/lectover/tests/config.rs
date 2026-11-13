use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use lectover::config::{Config, SecretRef};

const SPEC: &str = r#"
[paths]            # all optional
models = "/big/disk/lectover/models"
site   = "/big/disk/lectover/site"

[defaults]
device = "auto"    # cpu | metal | cuda | hip | vulkan | auto

[site."olten.jetzt"]
ghost_url    = "https://olten.jetzt"
language     = "de-CH"
since        = 2026-01-01          # sync ignores older posts
voices       = [ { id = "sample", label = "Sample" }, { id = "other", label = "Other" } ]
intro        = "Sie hören: «{title}». Verfasst von {authors}. Publiziert am {date}."   # optional; else the language default
sounds       = "olten.jetzt"       # dir under the sites data dir; optional
lexicon      = "olten.jetzt"       # file under lexicon/; optional
player_base  = "https://lectover.oetiker.ch"
ghost_key    = { env = "GHOST_CONTENT_KEY_OLTEN" }        # or { file = "~/.config/lectover/secrets/olten.key" }

[upload]                           # one static host for the whole deploy tree
target = "lectover@lectover.oetiker.ch:/srv/lectover/"   # = deploy tree root: lectover.js + <domain>/<slug>/
ssh_key = "~/.config/lectover/secrets/upload_ed25519"
known_hosts = "~/.config/lectover/known_hosts"           # optional; else the user's

[site."olten.jetzt".webhook]       # only used by `serve`
secret = { env = "LECTOVER_WEBHOOK_SECRET_OLTEN" }
"#;

const HOME: &str = "/home/u";

fn write(dir: &Path, text: &str) -> PathBuf {
    let p = dir.join("config.toml");
    fs::write(&p, text).unwrap();
    p
}

fn load(text: &str) -> anyhow::Result<Option<lectover::config::Loaded>> {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), text);
    Config::load(&p, Path::new(HOME))
}

#[test]
fn parses_spec_example() {
    let l = load(SPEC).unwrap().unwrap();
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    let c = &l.config;
    assert_eq!(
        c.paths.models.as_deref(),
        Some(Path::new("/big/disk/lectover/models"))
    );
    assert_eq!(
        c.paths.site.as_deref(),
        Some(Path::new("/big/disk/lectover/site"))
    );
    assert_eq!(c.defaults.device.as_deref(), Some("auto"));
    let s = &c.site["olten.jetzt"];
    assert_eq!(s.ghost_url, "https://olten.jetzt");
    assert_eq!(s.language, "de-CH");
    assert_eq!(s.since.as_deref(), Some("2026-01-01"));
    assert_eq!(s.voices.len(), 2);
    assert_eq!(s.voices[0].id, "sample");
    assert_eq!(s.voices[0].label.as_deref(), Some("Sample"));
    assert_eq!(s.voices[1].id, "other");
    assert_eq!(s.sounds.as_deref(), Some("olten.jetzt"));
    assert_eq!(s.lexicon.as_deref(), Some("olten.jetzt"));
    assert_eq!(
        s.ghost_key,
        SecretRef::Env {
            env: "GHOST_CONTENT_KEY_OLTEN".into()
        }
    );
    let u = c.upload.as_ref().unwrap();
    assert_eq!(u.target, "lectover@lectover.oetiker.ch:/srv/lectover/");
    assert_eq!(
        u.ssh_key,
        Path::new("/home/u/.config/lectover/secrets/upload_ed25519")
    );
    assert_eq!(
        u.known_hosts.as_deref(),
        Some(Path::new("/home/u/.config/lectover/known_hosts"))
    );
}

#[test]
fn missing_file_is_none() {
    let dir = tempfile::tempdir().unwrap();
    assert!(Config::load(&dir.path().join("nope.toml"), Path::new(HOME))
        .unwrap()
        .is_none());
}

#[test]
fn unknown_key_warns_with_dotted_path() {
    let l = load("[site.\"a.b\"]\nghost_url = \"x\"\nghost_urll = \"x\"\n")
        .unwrap()
        .unwrap();
    assert_eq!(
        l.warnings,
        vec!["unknown key site.a.b.ghost_urll".to_string()]
    );
    let err = load("[site.\"a.b\"]\nghost_urll = \"x\"\n").unwrap_err();
    assert!(format!("{err:#}").contains("ghost_url"), "{err:#}");
}

#[test]
fn later_phase_keys_are_silent() {
    let l = load(
        "[site.x]\nghost_url = \"u\"\nintro = \"i\"\nplayer_base = \"p\"\n\
         [site.x.webhook]\nsecret = { env = \"S\" }\nother = 1\n",
    )
    .unwrap()
    .unwrap();
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
}

#[test]
fn since_accepts_date_and_string_and_validates() {
    let l = load("[site.x]\nghost_url = \"u\"\nsince = \"2028-02-29\"\n")
        .unwrap()
        .unwrap();
    assert_eq!(l.config.site["x"].since.as_deref(), Some("2028-02-29"));
    for bad in [
        "\"2026-13-01\"",
        "\"2027-02-29\"",
        "\"nonsense\"",
        "2026-01-01T10:00:00",
    ] {
        let err = load(&format!("[site.x]\nghost_url = \"u\"\nsince = {bad}\n")).unwrap_err();
        let t = format!("{err:#}");
        assert!(t.contains("since") && t.contains("x"), "{bad}: {t}");
    }
}

#[test]
fn since_bare_invalid_date_is_an_error() {
    // The TOML parser itself rejects an impossible bare date.
    assert!(load("[site.x]\nghost_url = \"u\"\nsince = 2026-04-31\n").is_err());
}

#[test]
fn paths_expand_tilde_and_relative() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(
        dir.path(),
        "[paths]\nvoices = \"v\"\n[upload]\ntarget = \"t\"\nssh_key = \"~/k\"\n\
         [site.x]\nghost_url = \"u\"\nghost_key = { file = \"~/s.key\" }\n",
    );
    let l = Config::load(&p, Path::new(HOME)).unwrap().unwrap();
    assert_eq!(
        l.config.paths.voices.as_deref(),
        Some(dir.path().join("v").as_path())
    );
    assert_eq!(l.config.upload.unwrap().ssh_key, Path::new("/home/u/k"));
    assert_eq!(
        l.config.site["x"].ghost_key,
        SecretRef::File {
            file: "/home/u/s.key".into()
        }
    );
}

fn key_file(dir: &Path, mode: u32, content: &str) -> PathBuf {
    let p = dir.join("k");
    fs::write(&p, content).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
    p
}

#[test]
fn secret_env_and_file() {
    let r = SecretRef::Env { env: "FOO".into() };
    assert_eq!(
        r.resolve(&|n| (n == "FOO").then(|| "v".to_string()))
            .unwrap(),
        "v"
    );
    let err = r.resolve(&|_| None).unwrap_err();
    assert!(format!("{err:#}").contains("FOO"));

    let dir = tempfile::tempdir().unwrap();
    let ok = SecretRef::File {
        file: key_file(dir.path(), 0o600, "abc\n"),
    };
    assert_eq!(ok.resolve(&|_| None).unwrap(), "abc");
    let bad = SecretRef::File {
        file: key_file(dir.path(), 0o644, "abc\n"),
    };
    let err = bad.resolve(&|_| None).unwrap_err();
    assert!(format!("{err:#}").contains("readable by others"), "{err:#}");
}

#[test]
fn secret_errors_never_contain_the_value() {
    let dir = tempfile::tempdir().unwrap();
    let bad = SecretRef::File {
        file: key_file(dir.path(), 0o644, "s3cr3t"),
    };
    let err = bad.resolve(&|_| None).unwrap_err();
    assert!(!format!("{err:#}").contains("s3cr3t"));
    // Inline secrets are rejected without echoing the value.
    let err = load("[site.x]\nghost_url = \"u\"\nghost_key = \"s3cr3t\"\n").unwrap_err();
    assert!(!format!("{err:#}").contains("s3cr3t"), "{err:#}");
    assert!(!format!("{err:?}").contains("s3cr3t"));
}

#[test]
fn describe_never_contains_the_value() {
    assert_eq!(
        SecretRef::Env {
            env: "GHOST_CONTENT_KEY".into()
        }
        .describe(),
        "env GHOST_CONTENT_KEY"
    );
    assert_eq!(SecretRef::File { file: "/p".into() }.describe(), "file /p");
}

#[test]
fn empty_secret_file_is_an_error_without_content() {
    let dir = tempfile::tempdir().unwrap();
    for body in ["", " \n\t\n"] {
        let r = SecretRef::File {
            file: key_file(dir.path(), 0o600, body),
        };
        let err = format!("{:#}", r.resolve(&|_| None).unwrap_err());
        assert!(err.contains("is empty"), "{err}");
    }
}

#[test]
fn example_config_parses() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/olten.jetzt/config.toml");
    let l = Config::load(&path, Path::new(HOME))
        .unwrap()
        .expect("example config exists");
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
}
