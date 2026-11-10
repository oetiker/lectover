use lectover::config::SecretRef;
use lectover::ghost::{
    fetch_post, post_api_url, require_public, domain_of_url, slug_from_arg, GhostConfig, HttpReply,
    Post,
};
use lectover::settings::SiteSettings;
use std::collections::HashMap;

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let m: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |k| m.get(k).cloned()
}

fn cfg(url: &str, key: &str) -> GhostConfig {
    GhostConfig {
        url: url.into(),
        key: key.into(),
    }
}

fn post_with(vis: &str) -> Post {
    Post {
        slug: "a".into(),
        html: String::new(),
        visibility: vis.into(),
        updated_at: String::new(),
        title: String::new(),
        reading_time: None,
        ..Default::default()
    }
}

#[test]
fn slug_from_url_and_plain() {
    assert_eq!(
        slug_from_arg("https://olten.jetzt/frohburg-spiel-satz-patt/").unwrap(),
        "frohburg-spiel-satz-patt"
    );
    assert_eq!(
        slug_from_arg("https://olten.jetzt/frohburg-spiel-satz-patt/?ref=x#top").unwrap(),
        "frohburg-spiel-satz-patt"
    );
    assert_eq!(
        slug_from_arg("frohburg-spiel-satz-patt").unwrap(),
        "frohburg-spiel-satz-patt"
    );
    assert_eq!(
        slug_from_arg("https://olten.jetzt/").unwrap_err().to_string(),
        "no post slug in URL: https://olten.jetzt/"
    );
}

fn site(url: &str, key: SecretRef) -> SiteSettings {
    SiteSettings {
        domain: "example.org".into(),
        ghost_url: url.into(),
        language: "de-CH".into(),
        voices: vec![],
        ghost_key: key,
        sounds_dir: "/s".into(),
        lexicon: "/l.txt".into(),
    }
}

#[test]
fn ghost_for_site_uses_secret_ref() {
    let key = SecretRef::Env { env: "KEY_EXAMPLE".into() };
    let c = GhostConfig::for_site(&site("https://example.org", key.clone()), &env_of(&[("KEY_EXAMPLE", "k")])).unwrap();
    assert_eq!(c, cfg("https://example.org", "k"));
    let err = GhostConfig::for_site(&site("https://example.org", key), &env_of(&[("GHOST_CONTENT_KEY", "other")]))
        .unwrap_err();
    let text = format!("{err:#}");
    assert!(text.contains("KEY_EXAMPLE") && text.contains("example.org"), "{text}");
    assert!(!text.contains("other"), "{text}");
}

#[test]
fn ghost_for_site_strips_trailing_slash() {
    let key = SecretRef::Env { env: "K".into() };
    let c = GhostConfig::for_site(&site("https://example.org/", key), &env_of(&[("K", "k")])).unwrap();
    assert_eq!(c, cfg("https://example.org", "k"));
}

#[test]
fn post_api_url_format() {
    assert_eq!(
        post_api_url(&cfg("https://olten.jetzt", "k"), "a-b"),
        "https://olten.jetzt/ghost/api/content/posts/slug/a-b/?key=k&formats=html&include=authors"
    );
}

#[test]
fn fetch_post_with_fake_opener() {
    let body = br#"{"posts":[{"slug":"a","html":"<p>x</p>","visibility":"public"}]}"#.to_vec();
    let get = move |_url: &str| {
        Ok(HttpReply {
            status: 200,
            body: body.clone(),
        })
    };
    let post = fetch_post(&cfg("u", "k"), "a", &get).unwrap();
    assert_eq!(post.html, "<p>x</p>");
    assert_eq!(post.visibility, "public");
}

#[test]
fn fetch_post_404_exits_with_slug() {
    let get = |_url: &str| {
        Ok(HttpReply {
            status: 404,
            body: vec![],
        })
    };
    let err = fetch_post(&cfg("u", "k"), "gone", &get).unwrap_err();
    assert_eq!(err.to_string(), "post not found: gone");
}

#[test]
fn fetch_post_other_http_error_hides_key() {
    let get = |_url: &str| {
        Ok(HttpReply {
            status: 403,
            body: vec![],
        })
    };
    let err = fetch_post(&cfg("u", "secret"), "a", &get).unwrap_err();
    let text = format!("{err:#}");
    assert!(text.contains("403"));
    assert!(!text.contains("secret"));
}

#[test]
fn require_public_refuses() {
    for vis in ["members", "paid", "tiers", ""] {
        assert!(require_public(&post_with(vis)).is_err(), "vis={vis:?}");
    }
    let msg = require_public(&post_with("members")).unwrap_err().to_string();
    assert_eq!(
        msg,
        "post a is not public (visibility='members'); refusing to render"
    );
}

#[test]
fn require_public_accepts_public() {
    require_public(&post_with("public")).unwrap();
}

#[test]
fn domain_of_url_is_bare_lowercase_host() {
    assert_eq!(domain_of_url("https://olten.jetzt"), "olten.jetzt");
    assert_eq!(
        domain_of_url("https://WWW.Example.org:8443/blog"),
        "example.org"
    );
}
