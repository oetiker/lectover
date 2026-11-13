//! `lectover doctor`: checks that an installation can render, without changing anything.
//!
//! Every check is a pure function of the settings and a [`Probe`] (environment, HTTP, free disk
//! space), so tests run against temporary directories and fakes. No secret value is ever put
//! into a check; a failing key shows only where it is read from.

use std::collections::HashSet;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::UploadConfig;
use crate::ffi::{
    backend_banner, resolve_device, unified_memory_warning, Device, UNIFIED_MEMORY_ENV,
};
use crate::ghost::{settings_api_url, GhostConfig};
use crate::models::{DesignFiles, ModelFiles};
use crate::settings::{Settings, SiteSettings};
use crate::text::normalize::load_lexicon;
use crate::voices::load_registry;

/// An HTTP reply as the online checks see it: non-2xx statuses are replies, not errors.
pub use crate::ghost::HttpReply;

/// The only language with built-in rules in phase 1.
const LANGUAGES: [&str; 1] = ["de-CH"];
const GB: u64 = 1 << 30;
const MODELS_MIN_FREE: u64 = 10 * GB;
const SITE_MIN_FREE: u64 = 2 * GB;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub status: Status,
    pub what: String,
    pub detail: String,
    pub hint: Option<String>,
}

impl Check {
    fn new(status: Status, what: impl Into<String>, detail: impl Into<String>) -> Check {
        Check {
            status,
            what: what.into(),
            detail: detail.into(),
            hint: None,
        }
    }
    fn ok(what: impl Into<String>, detail: impl Into<String>) -> Check {
        Check::new(Status::Ok, what, detail)
    }
    fn warn(what: impl Into<String>, detail: impl Into<String>) -> Check {
        Check::new(Status::Warn, what, detail)
    }
    fn fail(what: impl Into<String>, detail: impl Into<String>) -> Check {
        Check::new(Status::Fail, what, detail)
    }
    fn hint(mut self, hint: impl Into<String>) -> Check {
        self.hint = Some(hint.into());
        self
    }
}

/// What the checks may look at besides the file system.
pub struct Probe<'a> {
    pub env: &'a dyn Fn(&str) -> Option<String>,
    pub http_get: &'a dyn Fn(&str) -> Result<HttpReply>,
    pub free_bytes: &'a dyn Fn(&Path) -> Option<u64>,
}

/// Runs all checks; `site` limits the per-site checks to one domain.
pub fn checks(settings: &Settings, site: Option<&str>, offline: bool, probe: &Probe) -> Vec<Check> {
    let mut out = Vec::new();
    check_config(settings, &mut out);
    check_dirs(settings, probe, &mut out);
    check_models(settings, &mut out);
    let registry = load_registry(&settings.voices);
    match &registry {
        Ok(r) => out.push(Check::ok(
            "voice registry",
            format!("{} voices in {}", r.len(), settings.voices.display()),
        )),
        Err(e) => out.push(
            Check::fail("voice registry", format!("{e:#}"))
                .hint("record one with `lectover voice record <id> --label <name>`"),
        ),
    }
    let known: Option<HashSet<&str>> = registry
        .as_ref()
        .ok()
        .map(|r| r.keys().map(String::as_str).collect());
    for domain in site_domains(settings, site) {
        match settings.site(&domain) {
            Ok(s) => check_site(&s, known.as_ref(), offline, probe, &mut out),
            Err(e) => out.push(Check::fail(
                format!("site {domain}"),
                format!("{e:#}").lines().next().unwrap_or(""),
            )),
        }
    }
    if let Some(u) = settings
        .config
        .as_ref()
        .and_then(|l| l.config.upload.as_ref())
    {
        check_upload(u, &mut out);
    }
    check_gpu(settings, &mut out);
    let env_set = (probe.env)(UNIFIED_MEMORY_ENV).is_some();
    if let Some(w) =
        unified_memory_warning(env_set, Device::Cuda.compiled() || Device::Hip.compiled())
    {
        out.push(
            Check::warn(UNIFIED_MEMORY_ENV, w.trim_start_matches("warning: "))
                .hint(format!("unset {UNIFIED_MEMORY_ENV}")),
        );
    }
    out
}

/// One line per check, `ok   what — detail`, with the hint indented below.
pub fn render_report(checks: &[Check]) -> String {
    let mut s = String::new();
    for c in checks {
        let word = match c.status {
            Status::Ok => "ok  ",
            Status::Warn => "warn",
            Status::Fail => "FAIL",
        };
        s.push_str(word);
        s.push(' ');
        s.push_str(&c.what);
        if !c.detail.is_empty() {
            s.push_str(" — ");
            s.push_str(&c.detail);
        }
        s.push('\n');
        if let Some(h) = &c.hint {
            s.push_str("     hint: ");
            s.push_str(h);
            s.push('\n');
        }
    }
    s
}

/// 1 if any check failed, else 0.
pub fn exit_code(checks: &[Check]) -> u8 {
    u8::from(checks.iter().any(|c| c.status == Status::Fail))
}

fn site_domains(settings: &Settings, site: Option<&str>) -> Vec<String> {
    match site {
        Some(s) => vec![s.to_string()],
        None => settings
            .config
            .iter()
            .flat_map(|l| l.config.site.keys().cloned())
            .collect(),
    }
}

fn check_config(settings: &Settings, out: &mut Vec<Check>) {
    let path = settings.config_path.display();
    if let Some(e) = &settings.config_error {
        out.push(Check::fail("config", e.clone()).hint(format!("fix {path}")));
        return;
    }
    match &settings.config {
        Some(l) => {
            out.push(Check::ok("config", format!("{path} parsed")));
            for w in &l.warnings {
                out.push(
                    Check::warn("config", w.clone())
                        .hint("check the spelling against the documented keys"),
                );
            }
        }
        None => out.push(
            Check::warn("config", format!("no config file at {path}"))
                .hint("create it to configure sites"),
        ),
    }
}

/// The closest ancestor of `p` (or `p` itself) that exists.
fn nearest_existing(p: &Path) -> Option<&Path> {
    p.ancestors().find(|a| a.exists())
}

fn writable(p: &Path) -> bool {
    CString::new(p.as_os_str().as_bytes())
        .map(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0)
        .unwrap_or(false)
}

fn check_dir(what: &str, dir: &Path, out: &mut Vec<Check>) {
    let shown = dir.display();
    if dir.is_dir() {
        out.push(Check::ok(what, format!("{shown} exists")));
    } else if dir.exists() {
        out.push(Check::fail(
            what,
            format!("{shown} exists but is not a directory"),
        ));
    } else {
        match nearest_existing(dir) {
            Some(parent) if parent.is_dir() && writable(parent) => out.push(Check::ok(
                what,
                format!("{shown} will be created ({} is writable)", parent.display()),
            )),
            Some(parent) => out.push(
                Check::fail(
                    what,
                    format!(
                        "{shown} is missing and {} is not writable",
                        parent.display()
                    ),
                )
                .hint(format!("create {shown} or choose another path")),
            ),
            None => out.push(Check::fail(
                what,
                format!("{shown} is missing and has no existing parent"),
            )),
        }
    }
}

fn check_free(what: &str, dir: &Path, min: u64, probe: &Probe, out: &mut Vec<Check>) {
    let Some(free) = nearest_existing(dir).and_then(|d| (probe.free_bytes)(d)) else {
        return;
    };
    let gb = |b: u64| format!("{:.1} GB", b as f64 / GB as f64);
    if free < min {
        out.push(
            Check::warn(what, format!("{} free, {} recommended", gb(free), gb(min))).hint(format!(
                "free some space on the file system of {}",
                dir.display()
            )),
        );
    } else {
        out.push(Check::ok(what, format!("{} free", gb(free))));
    }
}

fn check_dirs(settings: &Settings, probe: &Probe, out: &mut Vec<Check>) {
    check_dir("models dir", &settings.models, out);
    check_dir("takes dir", &settings.takes, out);
    check_dir("site dir", &settings.site_root, out);
    check_dir("voices dir", &settings.voices, out);
    check_dir("state dir", &settings.dirs.state, out);
    check_free(
        "disk space models",
        &settings.models,
        MODELS_MIN_FREE,
        probe,
        out,
    );
    check_free(
        "disk space site",
        &settings.site_root,
        SITE_MIN_FREE,
        probe,
        out,
    );
}

fn check_models(settings: &Settings, out: &mut Vec<Check>) {
    match ModelFiles::check(&settings.models) {
        Ok(_) => out.push(Check::ok("models", "all four render models present")),
        Err(e) => out.push(
            Check::fail("models", e.to_string().replace('\n', " ")).hint("lectover models fetch"),
        ),
    }
    match DesignFiles::check(&settings.models) {
        Ok(_) => out.push(Check::ok("voice design model", "present")),
        Err(_) => out.push(
            Check::warn(
                "voice design model",
                "missing; only `voice design` needs it",
            )
            .hint("lectover models fetch --design"),
        ),
    }
}

fn check_site(
    s: &SiteSettings,
    registry: Option<&HashSet<&str>>,
    offline: bool,
    probe: &Probe,
    out: &mut Vec<Check>,
) {
    let d = &s.domain;
    if let Some(known) = registry {
        for v in s.voices.iter().filter(|v| !known.contains(v.id.as_str())) {
            out.push(
                Check::fail(format!("site {d}: voices"), format!("voice {} is not in the voice registry", v.id))
                    .hint(format!("record it with `lectover voice record {} --label <name>` or remove it from the site", v.id)),
            );
        }
        if s.voices.iter().all(|v| known.contains(v.id.as_str())) {
            let detail = if s.voices.is_empty() {
                "none configured; the registry's first voice is used".to_string()
            } else {
                s.voices
                    .iter()
                    .map(|v| v.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            out.push(Check::ok(format!("site {d}: voices"), detail));
        }
    }
    if LANGUAGES.contains(&s.language.as_str()) {
        out.push(Check::ok(format!("site {d}: language"), s.language.clone()));
    } else {
        out.push(Check::fail(
            format!("site {d}: language"),
            format!(
                "unsupported language \"{}\"; supported: {}",
                s.language,
                LANGUAGES.join(", ")
            ),
        ));
    }
    match load_lexicon(&s.lexicon) {
        Ok(l) if s.lexicon.exists() => out.push(Check::ok(
            format!("site {d}: lexicon"),
            format!("{} entries", l.len()),
        )),
        Ok(_) => out.push(Check::ok(format!("site {d}: lexicon"), "none")),
        Err(e) => out.push(Check::fail(
            format!("site {d}: lexicon"),
            format!("{}: {e}", s.lexicon.display()),
        )),
    }
    if !s.sounds_dir.exists() {
        out.push(Check::ok(format!("site {d}: sounds"), "no sounds"));
    } else {
        match std::fs::read_dir(&s.sounds_dir) {
            Ok(_) => out.push(Check::ok(
                format!("site {d}: sounds"),
                s.sounds_dir.display().to_string(),
            )),
            Err(e) => out.push(Check::fail(
                format!("site {d}: sounds"),
                format!("{}: {e}", s.sounds_dir.display()),
            )),
        }
    }
    let key_what = format!("site {d}: ghost key");
    match s.ghost_key.resolve(probe.env) {
        Ok(_) => {
            out.push(Check::ok(key_what, s.ghost_key.describe()));
            if !offline {
                out.push(check_ghost(s, probe));
            }
        }
        // The error names the source only, never the value.
        Err(e) => out.push(
            Check::fail(key_what, format!("cannot read {}", s.ghost_key.describe()))
                .hint(format!("{e:#}")),
        ),
    }
}

/// Replaces the value after `key=` (up to `&`, whitespace, a quote or the end) and any literal
/// occurrence of `secret` with `***`.
fn redact(text: &str, secret: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("key=") {
        let (head, tail) = rest.split_at(i + 4);
        out.push_str(head);
        out.push_str("***");
        let end = tail
            .find(|c: char| c == '&' || c == '"' || c == '\'' || c.is_whitespace())
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
    out.push_str(rest);
    if secret.is_empty() {
        out
    } else {
        out.replace(secret, "***")
    }
}

fn check_ghost(s: &SiteSettings, probe: &Probe) -> Check {
    let what = format!("site {}: Ghost Content API", s.domain);
    let cfg = match GhostConfig::for_site(s, probe.env) {
        Ok(c) => c,
        Err(e) => return Check::fail(what, redact(&format!("{e:#}"), "")),
    };
    let secret = cfg.key.as_str();
    match (probe.http_get)(&settings_api_url(&cfg)) {
        Ok(r) if (200..300).contains(&r.status) => {
            let title = serde_json::from_slice::<serde_json::Value>(&r.body)
                .ok()
                .and_then(|v| v["settings"]["title"].as_str().map(str::to_string));
            Check::ok(
                what,
                title.map_or_else(|| "reachable".to_string(), |t| redact(&t, secret)),
            )
        }
        Ok(r) if r.status == 401 || r.status == 403 => Check::fail(what, "key rejected")
            .hint(format!("check the key in {}", s.ghost_key.describe())),
        Ok(r) => Check::fail(what, format!("{} answered HTTP {}", s.ghost_url, r.status)),
        Err(e) => Check::fail(what, redact(&format!("{e:#}"), secret)).hint(format!(
            "check that {} is reachable",
            redact(&s.ghost_url, secret)
        )),
    }
}

fn check_upload(u: &UploadConfig, out: &mut Vec<Check>) {
    let key: &PathBuf = &u.ssh_key;
    match std::fs::metadata(key) {
        Err(e) => out.push(Check::fail(
            "upload ssh key",
            format!("{}: {e}", key.display()),
        )),
        Ok(m) if m.permissions().mode() & 0o077 != 0 => out.push(
            Check::fail(
                "upload ssh key",
                format!("{} is accessible by others", key.display()),
            )
            .hint(format!("chmod 600 {}", key.display())),
        ),
        Ok(_) => out.push(Check::ok("upload ssh key", key.display().to_string())),
    }
}

fn check_gpu(settings: &Settings, out: &mut Vec<Check>) {
    if let Some(e) = &settings.device_error {
        out.push(Check::fail("gpu", e.clone()));
        return;
    }
    match resolve_device(settings.device) {
        Ok(d) => out.push(Check::ok("gpu", backend_banner(d))),
        Err(e) => out.push(Check::warn("gpu", format!("{e:#}"))),
    }
}
