//! `config.toml` schema: paths, defaults, per-site settings, upload target and secret references.
//!
//! Unknown keys produce warnings, never errors. Secrets are only ever referenced
//! (`{ env = "NAME" }` or `{ file = "PATH" }`), never stored inline, and no secret
//! value appears in any error message.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use indexmap::IndexMap;
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::Deserialize;

use crate::dirs::expand_path;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub paths: PathsSection,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub site: IndexMap<String, SiteConfig>,
    #[serde(default)]
    pub upload: Option<UploadConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PathsSection {
    pub models: Option<PathBuf>,
    pub takes: Option<PathBuf>,
    pub site: Option<PathBuf>,
    pub voices: Option<PathBuf>,
    /// Parent of the per-domain sound directories.
    pub sounds: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Defaults {
    /// "auto" | cpu | metal | cuda | hip | vulkan
    pub device: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SiteConfig {
    pub ghost_url: String,
    #[serde(default = "default_language")]
    pub language: String,
    /// `YYYY-MM-DD`, validated after parsing; accepts a TOML local date or a string.
    #[serde(default, deserialize_with = "deserialize_since")]
    pub since: Option<String>,
    #[serde(default)]
    pub voices: Vec<SiteVoice>,
    pub sounds: Option<String>,
    pub lexicon: Option<String>,
    #[serde(default = "default_ghost_key")]
    pub ghost_key: SecretRef,
}

fn default_language() -> String {
    "de-CH".to_string()
}

fn default_ghost_key() -> SecretRef {
    SecretRef::Env {
        env: "GHOST_CONTENT_KEY".to_string(),
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SiteVoice {
    pub id: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UploadConfig {
    pub target: String,
    pub ssh_key: PathBuf,
    pub known_hosts: Option<PathBuf>,
}

/// A reference to a secret; the value is never stored in the configuration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum SecretRef {
    Env { env: String },
    File { file: PathBuf },
}

#[derive(Debug, Clone)]
pub struct Loaded {
    pub config: Config,
    pub path: PathBuf,
    pub warnings: Vec<String>,
}

/// Reads `since` as a TOML local date or a string; calendar validation happens later.
fn deserialize_since<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = Option<String>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a date (YYYY-MM-DD)")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(Some(v.to_string()))
        }
        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
            let dt =
                toml::value::Datetime::deserialize(de::value::MapAccessDeserializer::new(map))?;
            Ok(Some(dt.to_string()))
        }
    }
    d.deserialize_any(V)
}

/// Accepts exactly `YYYY-MM-DD` naming a real calendar date.
fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if !(digits(0..4) && digits(5..7) && digits(8..10)) {
        return false;
    }
    let year: u32 = s[0..4].parse().unwrap();
    let month: u32 = s[5..7].parse().unwrap();
    let day: u32 = s[8..10].parse().unwrap();
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day)
}

/// Collects the key segments of a serde_ignored path, outermost first.
fn segments(path: &serde_ignored::Path, out: &mut Vec<String>) {
    use serde_ignored::Path as P;
    match path {
        P::Root => {}
        P::Seq { parent, index } => {
            segments(parent, out);
            out.push(index.to_string());
        }
        P::Map { parent, key } => {
            segments(parent, out);
            out.push(key.clone());
        }
        P::Some { parent } | P::NewtypeStruct { parent } | P::NewtypeVariant { parent } => {
            segments(parent, out)
        }
    }
}

/// Keys that belong to later phases: accepted silently, not modelled.
fn is_later_phase(segs: &[String]) -> bool {
    segs.len() >= 3
        && segs[0] == "site"
        && matches!(segs[2].as_str(), "intro" | "player_base" | "webhook")
}

/// Formats a TOML error without the source snippet, which could echo a secret.
fn describe_toml_error(text: &str, e: &toml::de::Error) -> String {
    match e.span() {
        Some(span) => {
            let upto = &text.as_bytes()[..span.start.min(text.len())];
            let line = upto.iter().filter(|&&c| c == b'\n').count() + 1;
            format!("line {line}: {}", e.message())
        }
        None => e.message().to_string(),
    }
}

impl Config {
    /// Loads and validates the config file; `Ok(None)` if it does not exist.
    /// Relative paths are resolved against the file's directory, `~` against `home`.
    pub fn load(path: &Path, home: &Path) -> Result<Option<Loaded>> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(anyhow!(e)).with_context(|| format!("reading {}", path.display()))
            }
        };
        let mut ignored = Vec::new();
        let parsed: Result<Config, toml::de::Error> =
            toml::Deserializer::parse(&text).and_then(|de| {
                serde_ignored::deserialize(de, |p| {
                    let mut segs = Vec::new();
                    segments(&p, &mut segs);
                    if !is_later_phase(&segs) {
                        ignored.push(segs.join("."));
                    }
                })
            });
        let mut config = parsed
            .map_err(|e| anyhow!(describe_toml_error(&text, &e)))
            .with_context(|| format!("invalid config file {}", path.display()))?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        config
            .validate_and_expand(home, base)
            .with_context(|| format!("invalid config file {}", path.display()))?;
        let warnings = ignored
            .into_iter()
            .map(|k| format!("unknown key {k}"))
            .collect();
        Ok(Some(Loaded {
            config,
            path: path.to_path_buf(),
            warnings,
        }))
    }

    fn validate_and_expand(&mut self, home: &Path, base: &Path) -> Result<()> {
        let ex = |p: &mut PathBuf| *p = expand_path(&p.to_string_lossy(), home, base);
        let paths = &mut self.paths;
        for p in [
            &mut paths.models,
            &mut paths.takes,
            &mut paths.site,
            &mut paths.voices,
            &mut paths.sounds,
        ]
        .into_iter()
        .flatten()
        {
            ex(p);
        }
        if let Some(u) = &mut self.upload {
            ex(&mut u.ssh_key);
            if let Some(k) = &mut u.known_hosts {
                ex(k);
            }
        }
        for (name, site) in &mut self.site {
            if let Some(since) = &site.since {
                if !valid_date(since) {
                    bail!("site \"{name}\": since must be a calendar date YYYY-MM-DD");
                }
            }
            if let SecretRef::File { file } = &mut site.ghost_key {
                ex(file);
            }
        }
        Ok(())
    }
}

impl SecretRef {
    /// Reads the secret. Errors name the source, never the value.
    pub fn resolve(&self, env: &dyn Fn(&str) -> Option<String>) -> Result<String> {
        match self {
            SecretRef::Env { env: name } => env(name)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| anyhow!("environment variable {name} is not set")),
            SecretRef::File { file } => {
                use std::os::unix::fs::PermissionsExt;
                let meta = std::fs::metadata(file)
                    .with_context(|| format!("reading secret file {}", file.display()))?;
                if meta.permissions().mode() & 0o077 != 0 {
                    bail!(
                        "secret file {} is readable by others; run: chmod 600 {}",
                        file.display(),
                        file.display()
                    );
                }
                let raw = std::fs::read_to_string(file)
                    .with_context(|| format!("reading secret file {}", file.display()))?;
                let value = raw.trim();
                if value.is_empty() {
                    bail!("secret file {} is empty", file.display());
                }
                Ok(value.to_string())
            }
        }
    }

    /// Human-readable source of the secret, never its value.
    pub fn describe(&self) -> String {
        match self {
            SecretRef::Env { env } => format!("env {env}"),
            SecretRef::File { file } => format!("file {}", file.display()),
        }
    }
}
