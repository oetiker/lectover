//! Minimal Ghost Content API client.
//!
//! The HTTP transport is injected as a closure so tests never touch the network.

use anyhow::{anyhow, bail, Context, Result};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::Deserialize;
use std::time::Duration;

use crate::settings::SiteSettings;

/// Everything except RFC 3986 unreserved characters is escaped (Python `quote(safe='')`).
const QUOTE: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhostConfig {
    pub url: String,
    pub key: String,
}

/// Minimal HTTP response: status code and raw body.
#[derive(Debug, Clone)]
pub struct HttpReply {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Post {
    pub slug: String,
    pub html: String,
    /// Empty when the API did not report a visibility.
    pub visibility: String,
    pub updated_at: String,
    pub title: String,
    pub reading_time: Option<u32>,
    /// Site-local timestamp, e.g. `2026-10-16T10:27:51.000+02:00`.
    pub published_at: String,
    pub authors: Vec<Author>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Author {
    pub name: String,
}

impl GhostConfig {
    /// The Content API of site `s`, its key read from the configured secret reference.
    /// Errors name the site and the key's source, never its value.
    pub fn for_site(s: &SiteSettings, env: &dyn Fn(&str) -> Option<String>) -> Result<GhostConfig> {
        let key = s.ghost_key.resolve(env).with_context(|| {
            format!(
                "no Ghost Content API key for site {} ({}); find it in the data-key attribute \
                 in the site's page source",
                s.domain,
                s.ghost_key.describe()
            )
        })?;
        Ok(GhostConfig {
            url: s.ghost_url.trim_end_matches('/').to_string(),
            key,
        })
    }
}

/// Bare lowercase host of `url` (no `www.`); empty if `url` has no host.
pub fn domain_of_url(url: &str) -> String {
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
        .unwrap_or_default();
    host.strip_prefix("www.").unwrap_or(&host).to_string()
}

/// Accept a post URL or a plain slug; return the slug.
pub fn slug_from_arg(arg: &str) -> Result<String> {
    let arg = arg.trim();
    if arg.starts_with("http://") || arg.starts_with("https://") {
        let parsed = url::Url::parse(arg).map_err(|_| anyhow!("no post slug in URL: {arg}"))?;
        let last = parsed
            .path()
            .split('/')
            .rfind(|s| !s.is_empty());
        return match last {
            Some(s) => Ok(s.to_string()),
            None => bail!("no post slug in URL: {arg}"),
        };
    }
    Ok(arg.to_string())
}

pub fn post_api_url(cfg: &GhostConfig, slug: &str) -> String {
    format!(
        "{}/ghost/api/content/posts/slug/{}/?key={}&formats=html&include=authors",
        cfg.url,
        utf8_percent_encode(slug, QUOTE),
        utf8_percent_encode(&cfg.key, QUOTE)
    )
}

/// The settings endpoint, a cheap authenticated request used to probe a site and its key.
pub fn settings_api_url(cfg: &GhostConfig) -> String {
    format!("{}/ghost/api/content/settings/?key={}", cfg.url, utf8_percent_encode(&cfg.key, QUOTE))
}

/// Fetch a post by slug. `get` performs the HTTP GET and returns `Err` only for
/// transport failures (its message must not contain the request URL: it holds the key).
pub fn fetch_post(
    cfg: &GhostConfig,
    slug: &str,
    get: &dyn Fn(&str) -> Result<HttpReply>,
) -> Result<Post> {
    let reply = get(&post_api_url(cfg, slug))
        .map_err(|e| anyhow!("cannot reach Ghost at {}: {e}", cfg.url))?;
    match reply.status {
        200..=299 => {}
        404 => bail!("post not found: {slug}"),
        // Do not echo the URL: it contains the API key.
        code => bail!("Ghost API error {code} for slug {slug}"),
    }
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        posts: Vec<Post>,
    }
    let env: Envelope = serde_json::from_slice(&reply.body)
        .map_err(|e| anyhow!("invalid Ghost API response for slug {slug}: {e}"))?;
    env.posts
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("post not found: {slug}"))
}

pub fn require_public(post: &Post) -> Result<()> {
    if post.visibility != "public" {
        let slug = if post.slug.is_empty() { "?" } else { &post.slug };
        let vis = if post.visibility.is_empty() {
            "None".to_string()
        } else {
            format!("'{}'", post.visibility)
        };
        bail!("post {slug} is not public (visibility={vis}); refusing to render");
    }
    Ok(())
}

/// Real HTTP GET with a 30 s timeout. Non-2xx statuses are returned as replies, and
/// transport errors are reduced to a short reason so the URL (with the key) never leaks.
pub fn http_get(url: &str) -> Result<HttpReply> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut resp = agent.get(url).call().map_err(|e| match e {
        ureq::Error::Io(io) => anyhow!("{}", io.kind()),
        ureq::Error::Timeout(_) => anyhow!("timed out"),
        ureq::Error::HostNotFound => anyhow!("host not found"),
        _ => anyhow!("request failed"),
    })?;
    let status = resp.status().as_u16();
    let body = resp
        .body_mut()
        .read_to_vec()
        .map_err(|_| anyhow!("could not read response body"))?;
    Ok(HttpReply { status, body })
}
