//! XDG base directories for instance data (config, data, state, cache).
//!
//! Used on Linux and macOS alike; the macOS `~/Library` convention is deliberately not used.
//! Each returned directory already ends in `lectover`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

const APP: &str = "lectover";

/// Resolved per-user base directories for lectover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseDirs {
    pub config: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
    pub cache: PathBuf,
}

impl BaseDirs {
    /// Resolves the base directories through `env`, a lookup of environment variables.
    ///
    /// An XDG variable that is unset, empty or not absolute is ignored and the
    /// `HOME`-based default is used instead. `HOME` itself must be absolute to count.
    pub fn from_env(env: &dyn Fn(&str) -> Option<OsString>) -> Result<BaseDirs> {
        let home = absolute(env("HOME"));
        let base = |var: &str, fallback: &[&str]| -> Result<PathBuf> {
            if let Some(dir) = absolute(env(var)) {
                return Ok(dir.join(APP));
            }
            let home = home.as_ref().ok_or_else(|| {
                anyhow!(
                    "cannot resolve {var}: HOME is not set to an absolute path and {var} is unset"
                )
            })?;
            let mut path = home.clone();
            for part in fallback {
                path.push(part);
            }
            Ok(path.join(APP))
        };
        Ok(BaseDirs {
            config: base("XDG_CONFIG_HOME", &[".config"])?,
            data: base("XDG_DATA_HOME", &[".local", "share"])?,
            state: base("XDG_STATE_HOME", &[".local", "state"])?,
            cache: base("XDG_CACHE_HOME", &[".cache"])?,
        })
    }

    /// Resolves the base directories from the real process environment.
    pub fn from_process_env() -> Result<BaseDirs> {
        Self::from_env(&|name| std::env::var_os(name))
    }
}

/// Keeps a value only if it is non-empty and an absolute path.
fn absolute(value: Option<OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(value?);
    (!path.as_os_str().is_empty() && path.is_absolute()).then_some(path)
}

/// Expands a user-supplied path: `~` and `~/...` are relative to `home`,
/// absolute paths are kept, and anything else is relative to `base`.
/// `~user` is not expanded.
pub fn expand_path(raw: &str, home: &Path, base: &Path) -> PathBuf {
    if raw == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.join(rest);
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}
