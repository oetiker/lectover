//! Settings resolution: flag > env > config file > default, through a fake environment.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

use lectover::settings::{Overrides, Settings};

fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let map: HashMap<String, OsString> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), OsString::from(v)))
        .collect();
    move |name: &str| map.get(name).cloned()
}

/// A home directory holding `~/.config/lectover/config.toml` with `toml`.
fn home_with(toml: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".config/lectover");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), toml).unwrap();
    home
}

fn home_str(home: &tempfile::TempDir) -> String {
    home.path().to_str().unwrap().to_string()
}

#[test]
fn precedence_flag_env_config_default() {
    type Get = fn(&Settings) -> &PathBuf;
    type Set = fn(&mut Overrides, PathBuf);
    let rows: [(&str, &str, Get, Set, &str); 4] = [
        ("models", "LECTOVER_MODELS", |s| &s.models, |o, p| o.models = Some(p), ".cache/lectover/models"),
        ("takes", "LECTOVER_TAKES", |s| &s.takes, |o, p| o.takes = Some(p), ".cache/lectover/takes"),
        ("site", "LECTOVER_SITE", |s| &s.site_root, |o, p| o.site = Some(p), ".local/share/lectover/site"),
        ("voices", "LECTOVER_VOICES", |s| &s.voices, |o, p| o.voices = Some(p), ".local/share/lectover/voices"),
    ];
    for (key, var, get, set, default) in rows {
        let home = home_with(&format!("[paths]\n{key} = \"/cfg/{key}\"\n"));
        let h = home_str(&home);
        let env_val = format!("/env/{key}");

        // default (no config)
        let bare = tempfile::tempdir().unwrap();
        let s = Settings::resolve(&Overrides::default(), &env_from(&[("HOME", bare.path().to_str().unwrap())])).unwrap();
        assert_eq!(get(&s), &bare.path().join(default), "{key} default");

        // config
        let s = Settings::resolve(&Overrides::default(), &env_from(&[("HOME", &h)])).unwrap();
        assert_eq!(get(&s), &PathBuf::from(format!("/cfg/{key}")), "{key} config");

        // env beats config
        let s = Settings::resolve(&Overrides::default(), &env_from(&[("HOME", &h), (var, &env_val)])).unwrap();
        assert_eq!(get(&s), &PathBuf::from(&env_val), "{key} env");

        // empty env is unset
        let s = Settings::resolve(&Overrides::default(), &env_from(&[("HOME", &h), (var, "")])).unwrap();
        assert_eq!(get(&s), &PathBuf::from(format!("/cfg/{key}")), "{key} empty env");

        // flag beats env
        let mut o = Overrides::default();
        set(&mut o, PathBuf::from("rel/flag"));
        let s = Settings::resolve(&o, &env_from(&[("HOME", &h), (var, &env_val)])).unwrap();
        assert_eq!(get(&s), &PathBuf::from("rel/flag"), "{key} flag");
    }
}

#[test]
fn defaults_without_config() {
    let bare = tempfile::tempdir().unwrap();
    let h = bare.path();
    let s = Settings::resolve(&Overrides::default(), &env_from(&[("HOME", h.to_str().unwrap())])).unwrap();
    assert!(s.config.is_none());
    assert_eq!(s.config_path, h.join(".config/lectover/config.toml"));
    assert_eq!(s.voices, h.join(".local/share/lectover/voices"));
    assert_eq!(s.site_root, h.join(".local/share/lectover/site"));
    assert_eq!(s.sounds_root, h.join(".local/share/lectover/sites"));
    assert_eq!(s.attic, h.join(".local/share/lectover/attic"));
    assert_eq!(s.lexicon_dir, h.join(".config/lectover/lexicon"));
    assert_eq!(s.say_out, h.join(".local/state/lectover/say"));
}

#[test]
fn sounds_path_overrides_sounds_root() {
    let home = home_with("[paths]\nsounds = \"/snd\"\n");
    let s = Settings::resolve(&Overrides::default(), &env_from(&[("HOME", &home_str(&home))])).unwrap();
    assert_eq!(s.sounds_root, PathBuf::from("/snd"));
}

#[test]
fn config_flag_and_env() {
    let home = home_with("");
    let h = home_str(&home);
    let a = home.path().join("a.toml");
    let b = home.path().join("b.toml");
    std::fs::write(&a, "").unwrap();
    std::fs::write(&b, "").unwrap();
    let env = env_from(&[("HOME", &h), ("LECTOVER_CONFIG", a.to_str().unwrap())]);
    let s = Settings::resolve(&Overrides::default(), &env).unwrap();
    assert_eq!(s.config_path, a);
    let o = Overrides { config: Some(b.clone()), ..Default::default() };
    let s = Settings::resolve(&o, &env).unwrap();
    assert_eq!(s.config_path, b);
    assert!(s.config.is_some());
}

#[test]
fn explicit_missing_config_is_an_error() {
    let bare = tempfile::tempdir().unwrap();
    let env = env_from(&[("HOME", bare.path().to_str().unwrap()), ("LECTOVER_CONFIG", "/nope/c.toml")]);
    let err = Settings::resolve(&Overrides::default(), &env).unwrap_err().to_string();
    assert!(err.contains("/nope/c.toml"), "{err}");
    let o = Overrides { config: Some(PathBuf::from("/nope2.toml")), ..Default::default() };
    assert!(Settings::resolve(&o, &env_from(&[("HOME", "/h")])).is_err());
}

const OLTEN: &str = "[site.\"olten.jetzt\"]\nghost_url = \"https://olten.jetzt\"\n";

#[test]
fn site_paths_derive_from_domain() {
    let home = home_with(OLTEN);
    let h = home_str(&home);
    let s = Settings::resolve(&Overrides::default(), &env_from(&[("HOME", &h)])).unwrap();
    let site = s.site("olten.jetzt").unwrap();
    assert_eq!(site.domain, "olten.jetzt");
    assert_eq!(site.language, "de-CH");
    assert_eq!(site.sounds_dir, home.path().join(".local/share/lectover/sites/olten.jetzt"));
    assert_eq!(site.lexicon, home.path().join(".config/lectover/lexicon/olten.jetzt.txt"));

    let home = home_with(&format!("{OLTEN}sounds = \"x\"\nlexicon = \"y\"\n"));
    let s = Settings::resolve(&Overrides::default(), &env_from(&[("HOME", &home_str(&home))])).unwrap();
    let site = s.site("olten.jetzt").unwrap();
    assert_eq!(site.sounds_dir, home.path().join(".local/share/lectover/sites/x"));
    assert_eq!(site.lexicon, home.path().join(".config/lectover/lexicon/y.txt"));
}

#[test]
fn site_for_post_by_url_host() {
    let home = home_with(OLTEN);
    let s = Settings::resolve(&Overrides::default(), &env_from(&[("HOME", &home_str(&home))])).unwrap();
    let site = s.site_for_post("https://WWW.Olten.Jetzt/a/", None).unwrap();
    assert_eq!(site.domain, "olten.jetzt");
    assert!(s.site_for_post("https://olten.jetzt/a/", Some("olten.jetzt")).is_ok());
    let err = s.site_for_post("https://olten.jetzt/a/", Some("other.org")).unwrap_err().to_string();
    assert!(err.contains("other.org") && err.contains("olten.jetzt"), "{err}");
}

#[test]
fn site_for_post_unknown_host() {
    let home = home_with(OLTEN);
    let s = Settings::resolve(&Overrides::default(), &env_from(&[("HOME", &home_str(&home))])).unwrap();
    let err = s.site_for_post("https://example.org/p/", None).unwrap_err().to_string();
    assert!(err.lines().any(|l| l == "[site.\"example.org\"]"), "{err}");
    assert!(err.lines().any(|l| l == "ghost_url = \"https://example.org\""), "{err}");
    assert!(err.contains(s.config_path.to_str().unwrap()), "{err}");
}

#[test]
fn site_for_post_slug_needs_site_when_ambiguous() {
    let two = format!("{OLTEN}\n[site.\"example.org\"]\nghost_url = \"https://example.org\"\n");
    let home = home_with(&two);
    let s = Settings::resolve(&Overrides::default(), &env_from(&[("HOME", &home_str(&home))])).unwrap();
    let err = s.site_for_post("my-slug", None).unwrap_err().to_string();
    assert!(err.contains("olten.jetzt") && err.contains("example.org") && err.contains("--site"), "{err}");
    assert_eq!(s.site_for_post("my-slug", Some("example.org")).unwrap().domain, "example.org");

    let home = home_with(OLTEN);
    let s = Settings::resolve(&Overrides::default(), &env_from(&[("HOME", &home_str(&home))])).unwrap();
    assert_eq!(s.site_for_post("my-slug", None).unwrap().domain, "olten.jetzt");
}

#[test]
fn render_without_config_explains() {
    let bare = tempfile::tempdir().unwrap();
    let s = Settings::resolve(&Overrides::default(), &env_from(&[("HOME", bare.path().to_str().unwrap())])).unwrap();
    let err = s.site_for_post("https://example.org/p/", None).unwrap_err().to_string();
    assert!(err.lines().any(|l| l == "[site.\"example.org\"]"), "{err}");
    assert!(err.lines().any(|l| l == "ghost_url = \"https://example.org\""), "{err}");
    assert!(err.contains(s.config_path.to_str().unwrap()), "{err}");
}

#[test]
fn defaults_device_resolves_from_config() {
    use lectover::ffi::Device;
    let resolve = |toml: &str| {
        let home = home_with(toml);
        Settings::resolve(&Overrides::default(), &env_from(&[("HOME", &home_str(&home))]))
    };
    assert_eq!(resolve("").unwrap().device, None);
    assert_eq!(resolve("[defaults]\ndevice = \"auto\"\n").unwrap().device, None);
    assert_eq!(resolve("[defaults]\ndevice = \"cpu\"\n").unwrap().device, Some(Device::Cpu));
    assert_eq!(resolve("[defaults]\ndevice = \"Vulkan\"\n").unwrap().device, Some(Device::Vulkan));
    let err = format!("{:#}", resolve("[defaults]\ndevice = \"tpu\"\n").unwrap_err());
    assert!(err.contains("defaults.device") && err.contains("vulkan"), "{err}");
    // The flag wins over the config: `args.device.or(settings.device)`.
    let s = resolve("[defaults]\ndevice = \"cpu\"\n").unwrap();
    assert_eq!(Some(Device::Metal).or(s.device), Some(Device::Metal));
}

#[test]
fn site_domain_is_the_config_key_not_the_ghost_host() {
    let home = home_with("[site.\"a.example\"]\nghost_url = \"https://b.example\"\n");
    let s = Settings::resolve(&Overrides::default(), &env_from(&[("HOME", &home_str(&home))])).unwrap();
    assert_eq!(s.site("a.example").unwrap().domain, "a.example");
}
