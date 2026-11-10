//! XDG base directory resolution: pure lookups through a fake environment.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use lectover::dirs::{expand_path, BaseDirs};

fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let map: HashMap<String, OsString> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), OsString::from(v)))
        .collect();
    move |name: &str| map.get(name).cloned()
}

#[test]
fn defaults_follow_home() {
    let dirs = BaseDirs::from_env(&env_from(&[("HOME", "/h")])).unwrap();
    assert_eq!(dirs.config, PathBuf::from("/h/.config/lectover"));
    assert_eq!(dirs.data, PathBuf::from("/h/.local/share/lectover"));
    assert_eq!(dirs.state, PathBuf::from("/h/.local/state/lectover"));
    assert_eq!(dirs.cache, PathBuf::from("/h/.cache/lectover"));
}

#[test]
fn xdg_vars_win() {
    let dirs = BaseDirs::from_env(&env_from(&[
        ("HOME", "/h"),
        ("XDG_CONFIG_HOME", "/c"),
        ("XDG_DATA_HOME", "/d"),
        ("XDG_STATE_HOME", "/s"),
        ("XDG_CACHE_HOME", "/k"),
    ]))
    .unwrap();
    assert_eq!(dirs.config, PathBuf::from("/c/lectover"));
    assert_eq!(dirs.data, PathBuf::from("/d/lectover"));
    assert_eq!(dirs.state, PathBuf::from("/s/lectover"));
    assert_eq!(dirs.cache, PathBuf::from("/k/lectover"));
}

#[test]
fn empty_or_relative_xdg_is_ignored() {
    let dirs = BaseDirs::from_env(&env_from(&[
        ("HOME", "/h"),
        ("XDG_DATA_HOME", ""),
        ("XDG_STATE_HOME", "rel"),
        ("XDG_CACHE_HOME", "rel/cache"),
    ]))
    .unwrap();
    assert_eq!(dirs.data, PathBuf::from("/h/.local/share/lectover"));
    assert_eq!(dirs.state, PathBuf::from("/h/.local/state/lectover"));
    assert_eq!(dirs.cache, PathBuf::from("/h/.cache/lectover"));
}

#[test]
fn relative_home_is_treated_as_missing() {
    let err = BaseDirs::from_env(&env_from(&[("HOME", "h")])).unwrap_err();
    assert!(err.to_string().contains("HOME"), "{err}");
}

#[test]
fn no_home_is_an_error() {
    let err = BaseDirs::from_env(&env_from(&[])).unwrap_err();
    assert!(err.to_string().contains("HOME"), "{err}");
}

#[test]
fn all_xdg_vars_without_home_succeed() {
    let dirs = BaseDirs::from_env(&env_from(&[
        ("XDG_CONFIG_HOME", "/c"),
        ("XDG_DATA_HOME", "/d"),
        ("XDG_STATE_HOME", "/s"),
        ("XDG_CACHE_HOME", "/k"),
    ]))
    .unwrap();
    assert_eq!(dirs.config, PathBuf::from("/c/lectover"));
    assert_eq!(dirs.cache, PathBuf::from("/k/lectover"));
}

#[test]
fn expand_path_rules() {
    let home = Path::new("/h");
    let base = Path::new("/base");
    assert_eq!(expand_path("~", home, base), PathBuf::from("/h"));
    assert_eq!(expand_path("~/x", home, base), PathBuf::from("/h/x"));
    assert_eq!(expand_path("/abs", home, base), PathBuf::from("/abs"));
    assert_eq!(
        expand_path("rel/x", home, base),
        PathBuf::from("/base/rel/x")
    );
    assert_eq!(
        expand_path("~user/x", home, base),
        PathBuf::from("/base/~user/x")
    );
}
