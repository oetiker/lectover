//! Minimal Ghost Content API client.
//!
//! The HTTP transport is injected as a closure so tests never touch the network.

use anyhow::{anyhow, bail, Context, Result};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde::Deserialize;
use std::time::Duration;

use crate::settings::SiteSettings;

/// Everything except RFC 3986 unreserved characters is escaped (Python `quote(safe='')`).
const QUOTE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

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
        let last = parsed.path().split('/').rfind(|s| !s.is_empty());
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
    format!(
        "{}/ghost/api/content/settings/?key={}",
        cfg.url,
        utf8_percent_encode(&cfg.key, QUOTE)
    )
}

/// Publication dates to render, `YYYY-MM-DD`, both ends inclusive; `None` leaves that end open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DateRange {
    pub since: Option<String>,
    pub until: Option<String>,
}

impl DateRange {
    /// Whether the site-local date `date` (`YYYY-MM-DD`) lies in the range.
    pub fn contains(&self, date: &str) -> bool {
        self.since.as_deref().is_none_or(|s| date >= s)
            && self.until.as_deref().is_none_or(|u| date <= u)
    }

    /// `2026-01-01..2026-12-31`, with `…` for an open end.
    pub fn describe(&self) -> String {
        format!(
            "{}..{}",
            self.since.as_deref().unwrap_or("…"),
            self.until.as_deref().unwrap_or("…")
        )
    }
}

/// A published post as listed by [`list_posts`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PostSummary {
    pub slug: String,
    pub title: String,
    /// Site-local timestamp; its first ten characters are the publication date.
    pub published_at: String,
    pub visibility: String,
}

impl PostSummary {
    pub fn date(&self) -> &str {
        self.published_at.get(..10).unwrap_or(&self.published_at)
    }
}

/// The posts of a [`list_posts`] call, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PostListing {
    pub public: Vec<PostSummary>,
    /// In the range but not public (members or paid): never rendered.
    pub not_public: Vec<PostSummary>,
}

/// Posts per page of the listing (Ghost's maximum).
const LIST_PAGE: u32 = 100;

pub fn posts_api_url(cfg: &GhostConfig, page: u32) -> String {
    format!(
        "{}/ghost/api/content/posts/?key={}&fields=slug,title,published_at,visibility&limit={LIST_PAGE}&page={page}",
        cfg.url,
        utf8_percent_encode(&cfg.key, QUOTE)
    )
}

/// Every published post whose site-local publication date lies in `range`, oldest first.
/// The date is compared as Ghost reports it (site-local), not in UTC, so a post is counted
/// on the day its readers saw on the site.
pub fn list_posts(
    cfg: &GhostConfig,
    range: &DateRange,
    get: &dyn Fn(&str) -> Result<HttpReply>,
) -> Result<PostListing> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Pagination {
        next: Option<u32>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Meta {
        pagination: Pagination,
    }
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        posts: Vec<PostSummary>,
        #[serde(default)]
        meta: Meta,
    }
    let mut posts = Vec::new();
    let mut page = 1;
    loop {
        let reply = get(&posts_api_url(cfg, page))
            .map_err(|e| anyhow!("cannot reach Ghost at {}: {e}", cfg.url))?;
        if !(200..=299).contains(&reply.status) {
            // Do not echo the URL: it contains the API key.
            bail!("Ghost API error {} listing posts", reply.status);
        }
        let env: Envelope = serde_json::from_slice(&reply.body)
            .map_err(|e| anyhow!("invalid Ghost API response listing posts: {e}"))?;
        posts.extend(env.posts);
        match env.meta.pagination.next {
            Some(next) if next > page => page = next,
            _ => break,
        }
    }
    posts.retain(|p| range.contains(p.date()));
    posts.sort_by(|a, b| {
        a.date()
            .cmp(b.date())
            .then(a.published_at.cmp(&b.published_at))
    });
    let (public, not_public) = posts.into_iter().partition(|p| p.visibility == "public");
    Ok(PostListing { public, not_public })
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
        let slug = if post.slug.is_empty() {
            "?"
        } else {
            &post.slug
        };
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
