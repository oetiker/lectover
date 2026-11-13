//! Resolves paths and per-site settings with the precedence
//! command-line flag > environment variable > config file > default.

use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Result};

use crate::cli::parse_device;
use crate::config::{Config, Loaded, SecretRef, SiteConfig, SiteVoice};
use crate::dirs::BaseDirs;
use crate::ffi::Device;
use crate::ghost::domain_of_url;

/// Path overrides taken from command-line flags.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub config: Option<PathBuf>,
    pub models: Option<PathBuf>,
    pub takes: Option<PathBuf>,
    pub site: Option<PathBuf>,
    pub voices: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub dirs: BaseDirs,
    pub config: Option<Loaded>,
    /// Why the config file could not be used; only set by [`Settings::resolve_for_doctor`].
    pub config_error: Option<String>,
    /// `[defaults] device` after parsing; `None` for "auto" or unset.
    pub device: Option<Device>,
    /// Why `[defaults] device` is invalid; only set by [`Settings::resolve_for_doctor`].
    pub device_error: Option<String>,
    /// The config file consulted (or, if absent, the one to create).
    pub config_path: PathBuf,
    pub models: PathBuf,
    pub takes: PathBuf,
    pub site_root: PathBuf,
    pub voices: PathBuf,
    pub sounds_root: PathBuf,
    pub lexicon_dir: PathBuf,
    pub attic: PathBuf,
    pub say_out: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SiteSettings {
    pub domain: String,
    pub ghost_url: String,
    pub language: String,
    pub voices: Vec<SiteVoice>,
    pub ghost_key: SecretRef,
    pub sounds_dir: PathBuf,
    pub lexicon: PathBuf,
}

/// An environment value counts only if it is non-empty.
fn env_path(env: &dyn Fn(&str) -> Option<OsString>, name: &str) -> Option<PathBuf> {
    env(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

impl Settings {
    pub fn resolve(o: &Overrides, env: &dyn Fn(&str) -> Option<OsString>) -> Result<Settings> {
        Self::resolve_inner(o, env, false)
    }

    /// Like [`Settings::resolve`], but a config file that cannot be used does not fail:
    /// the settings come back without a config and `config_error` holds the reason.
    pub fn resolve_for_doctor(
        o: &Overrides,
        env: &dyn Fn(&str) -> Option<OsString>,
    ) -> Result<Settings> {
        Self::resolve_inner(o, env, true)
    }

    fn resolve_inner(
        o: &Overrides,
        env: &dyn Fn(&str) -> Option<OsString>,
        lenient: bool,
    ) -> Result<Settings> {
        let dirs = BaseDirs::from_env(env)?;
        // `from_env` succeeded, so HOME or all four XDG variables are usable; HOME is only
        // needed for `~` in the config file.
        let home = env("HOME").map(PathBuf::from).unwrap_or_default();

        let explicit = o
            .config
            .clone()
            .or_else(|| env_path(env, "LECTOVER_CONFIG"));
        let config_path = explicit
            .clone()
            .unwrap_or_else(|| dirs.config.join("config.toml"));
        let mut config_error = None;
        let config = match Config::load(&config_path, &home) {
            Ok(Some(l)) => Some(l),
            Ok(None) if explicit.is_some() => {
                let msg = format!("config file {} does not exist", config_path.display());
                if !lenient {
                    bail!(msg);
                }
                config_error = Some(msg);
                None
            }
            Ok(None) => None,
            Err(e) if lenient => {
                config_error = Some(format!("{e:#}"));
                None
            }
            Err(e) => return Err(e),
        };
        let paths = config.as_ref().map(|l| &l.config.paths);
        let from_config = |f: fn(&crate::config::PathsSection) -> &Option<PathBuf>| {
            paths.and_then(|p| f(p).clone())
        };

        let pick = |flag: &Option<PathBuf>, var: &str, cfg: Option<PathBuf>, default: PathBuf| {
            flag.clone()
                .or_else(|| env_path(env, var))
                .or(cfg)
                .unwrap_or(default)
        };
        let models = pick(
            &o.models,
            "LECTOVER_MODELS",
            from_config(|p| &p.models),
            dirs.cache.join("models"),
        );
        let takes = pick(
            &o.takes,
            "LECTOVER_TAKES",
            from_config(|p| &p.takes),
            dirs.cache.join("takes"),
        );
        let site_root = pick(
            &o.site,
            "LECTOVER_SITE",
            from_config(|p| &p.site),
            dirs.data.join("site"),
        );
        let voices = pick(
            &o.voices,
            "LECTOVER_VOICES",
            from_config(|p| &p.voices),
            dirs.data.join("voices"),
        );
        let sounds_root = from_config(|p| &p.sounds).unwrap_or_else(|| dirs.data.join("sites"));

        let (device, device_error) = match config_device(config.as_ref()) {
            Ok(d) => (d, None),
            Err(e) if lenient => (None, Some(format!("{e:#}"))),
            Err(e) => return Err(e),
        };

        Ok(Settings {
            device,
            device_error,
            lexicon_dir: dirs.config.join("lexicon"),
            attic: dirs.data.join("attic"),
            say_out: dirs.state.join("say"),
            dirs,
            config,
            config_error,
            config_path,
            models,
            takes,
            site_root,
            voices,
            sounds_root,
        })
    }

    /// Settings of the site configured under `domain`.
    pub fn site(&self, domain: &str) -> Result<SiteSettings> {
        let wanted = normalize_domain(domain);
        let found = self.config.as_ref().and_then(|l| {
            l.config
                .site
                .iter()
                .find(|(k, _)| normalize_domain(k) == wanted)
        });
        match found {
            Some((key, cfg)) => Ok(self.site_settings(key, cfg)),
            None => Err(self.missing_site(&wanted)),
        }
    }

    /// Settings of the site a post belongs to: a post URL selects it by host; a plain slug
    /// uses `--site`, else the only configured site.
    pub fn site_for_post(&self, post_arg: &str, site_flag: Option<&str>) -> Result<SiteSettings> {
        let arg = post_arg.trim();
        if arg.starts_with("http://") || arg.starts_with("https://") {
            let domain = domain_of_url(arg);
            if domain.is_empty() {
                bail!("no host in URL: {arg}");
            }
            if let Some(flag) = site_flag {
                let flag_domain = normalize_domain(flag);
                if flag_domain != domain {
                    bail!(
                        "--site {flag_domain} does not match the host of the post URL ({domain}); \
                         drop --site or use a URL from that site"
                    );
                }
            }
            return self.site(&domain);
        }
        if let Some(flag) = site_flag {
            return self.site(flag);
        }
        let sites = self.config.as_ref().map(|l| &l.config.site);
        match sites {
            Some(s) if s.len() == 1 => {
                let (key, cfg) = s.iter().next().unwrap();
                Ok(self.site_settings(key, cfg))
            }
            Some(s) if s.len() > 1 => {
                let names: Vec<&str> = s.keys().map(String::as_str).collect();
                Err(anyhow!(
                    "several sites are configured ({}); say which one with --site <domain>",
                    names.join(", ")
                ))
            }
            _ => Err(anyhow!(
                "no site is configured; add a section like this to {}:\n\n\
                 [site.\"<domain>\"]\nghost_url = \"https://<domain>\"",
                self.config_path.display()
            )),
        }
    }

    fn site_settings(&self, key: &str, cfg: &SiteConfig) -> SiteSettings {
        let domain = normalize_domain(key);
        SiteSettings {
            sounds_dir: self
                .sounds_root
                .join(cfg.sounds.as_deref().unwrap_or(&domain)),
            lexicon: self
                .lexicon_dir
                .join(format!("{}.txt", cfg.lexicon.as_deref().unwrap_or(&domain))),
            ghost_url: cfg.ghost_url.clone(),
            language: cfg.language.clone(),
            voices: cfg.voices.clone(),
            ghost_key: cfg.ghost_key.clone(),
            domain,
        }
    }

    fn missing_site(&self, domain: &str) -> anyhow::Error {
        let state = if self.config.is_some() {
            format!("{} has no section for {domain}", self.config_path.display())
        } else {
            format!("no config file at {}", self.config_path.display())
        };
        anyhow!(
            "{state}; add this to {}:\n\n[site.\"{domain}\"]\nghost_url = \"https://{domain}\"",
            self.config_path.display()
        )
    }
}

/// `[defaults] device`: "auto" or unset gives `None`, a backend name gives that device.
fn config_device(config: Option<&Loaded>) -> Result<Option<Device>> {
    let Some(raw) = config.and_then(|l| l.config.defaults.device.as_deref()) else {
        return Ok(None);
    };
    if raw.trim().eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    parse_device(raw.trim()).map(Some).map_err(|_| {
        anyhow!("invalid defaults.device {raw:?} (auto, cpu, metal, cuda, hip, vulkan)")
    })
}

fn normalize_domain(s: &str) -> String {
    let lower = s.trim().to_lowercase();
    lower.strip_prefix("www.").unwrap_or(&lower).to_string()
}
