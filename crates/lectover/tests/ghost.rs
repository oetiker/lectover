use lectover::config::SecretRef;
use lectover::ghost::{
    domain_of_url, fetch_post, list_posts, post_api_url, require_public, slug_from_arg, DateRange,
    GhostConfig, HttpReply, Post,
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
        slug_from_arg("https://olten.jetzt/")
            .unwrap_err()
            .to_string(),
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
        since: None,
    }
}

#[test]
fn ghost_for_site_uses_secret_ref() {
    let key = SecretRef::Env {
        env: "KEY_EXAMPLE".into(),
    };
    let c = GhostConfig::for_site(
        &site("https://example.org", key.clone()),
        &env_of(&[("KEY_EXAMPLE", "k")]),
    )
    .unwrap();
    assert_eq!(c, cfg("https://example.org", "k"));
    let err = GhostConfig::for_site(
        &site("https://example.org", key),
        &env_of(&[("GHOST_CONTENT_KEY", "other")]),
    )
    .unwrap_err();
    let text = format!("{err:#}");
    assert!(
        text.contains("KEY_EXAMPLE") && text.contains("example.org"),
        "{text}"
    );
    assert!(!text.contains("other"), "{text}");
}

#[test]
fn ghost_for_site_strips_trailing_slash() {
    let key = SecretRef::Env { env: "K".into() };
    let c =
        GhostConfig::for_site(&site("https://example.org/", key), &env_of(&[("K", "k")])).unwrap();
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
    let msg = require_public(&post_with("members"))
        .unwrap_err()
        .to_string();
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

fn range(since: Option<&str>, until: Option<&str>) -> DateRange {
    DateRange {
        since: since.map(String::from),
        until: until.map(String::from),
    }
}

#[test]
fn list_posts_pages_filters_by_local_date_and_sorts_oldest_first() {
    // Two pages, newest first as Ghost returns them; dates are site-local.
    let pages = [
        br#"{"posts":[
            {"slug":"dec","published_at":"2027-01-01T00:30:00.000+01:00","visibility":"public"},
            {"slug":"late","published_at":"2026-12-31T23:59:00.000+01:00","visibility":"public"},
            {"slug":"paid","published_at":"2026-06-01T10:00:00.000+02:00","visibility":"paid"}
         ],"meta":{"pagination":{"page":1,"next":2}}}"#
            .to_vec(),
        br#"{"posts":[
            {"slug":"mid","published_at":"2026-03-08T09:00:00.000+01:00","visibility":"public"},
            {"slug":"first","published_at":"2026-01-01T00:10:00.000+01:00","visibility":"public"},
            {"slug":"old","published_at":"2025-12-31T23:50:00.000+01:00","visibility":"public"}
         ],"meta":{"pagination":{"page":2,"next":null}}}"#
            .to_vec(),
    ];
    let urls = std::cell::RefCell::new(Vec::new());
    let get = |url: &str| {
        urls.borrow_mut().push(url.to_string());
        let page = if url.contains("page=2") { 1 } else { 0 };
        Ok(HttpReply {
            status: 200,
            body: pages[page].clone(),
        })
    };
    let listing = list_posts(
        &cfg("u", "k"),
        &range(Some("2026-01-01"), Some("2026-12-31")),
        &get,
    )
    .unwrap();
    let slugs: Vec<&str> = listing.public.iter().map(|p| p.slug.as_str()).collect();
    assert_eq!(slugs, ["first", "mid", "late"]);
    let skipped: Vec<&str> = listing.not_public.iter().map(|p| p.slug.as_str()).collect();
    assert_eq!(skipped, ["paid"]);
    assert_eq!(urls.borrow().len(), 2);
    assert!(urls.borrow()[0].starts_with("u/ghost/api/content/posts/?key=k&"));

    // Open ends.
    let all = list_posts(&cfg("u", "k"), &range(None, None), &get).unwrap();
    assert_eq!(all.public.len(), 5);
    let upto = list_posts(&cfg("u", "k"), &range(None, Some("2026-03-08")), &get).unwrap();
    let slugs: Vec<&str> = upto.public.iter().map(|p| p.slug.as_str()).collect();
    assert_eq!(slugs, ["old", "first", "mid"]);
}

#[test]
fn list_posts_error_hides_key() {
    let get = |_url: &str| {
        Ok(HttpReply {
            status: 401,
            body: vec![],
        })
    };
    let err = list_posts(&cfg("u", "secret"), &range(None, None), &get).unwrap_err();
    let text = format!("{err:#}");
    assert!(text.contains("401"));
    assert!(!text.contains("secret"));
}
