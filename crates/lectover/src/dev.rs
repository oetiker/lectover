//! Local Ghost-like dev page for trying the read-aloud player (port of `dev_page.py`).
//!
//! `lectover dev build <post>` writes `<site>/<domain>/<slug>/index.html` in the deploy tree,
//! next to the post's audio; `lectover dev serve` serves the deploy tree with single-range
//! support (RFC 7233) and answers `/js` with `player/lectover.js`, like the production embed.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::{anyhow, Context, Result};
use percent_encoding::percent_decode_str;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use crate::ghost::{fetch_post, require_public, slug_from_arg, GhostConfig, HttpReply, Post};

/// The player `/js` answers with, relative to the working directory.
pub const PLAYER: &str = "player/lectover.js";

const CSS: &str = r#"
body { margin: 0; padding: 2rem 1rem; font: 18px/1.6 Georgia, "Times New Roman", serif; color: #15171a; background: #fff; }
article { max-width: 720px; margin: 0 auto; }
h1 { font: 700 2.2rem/1.2 system-ui, sans-serif; margin: 0 0 .5rem; }
.article-byline-meta { font: 14px system-ui, sans-serif; color: #738a94; margin-bottom: 2rem; }
.bull { margin-right: .2em; }
"#;

/// Python `html.escape(s)` (quote=True).
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

/// The parts of the Ghost theme the player relies on, with the post's HTML as is.
pub fn render_dev_page(post: &Post, domain: &str) -> String {
    let title = escape(&post.title);
    let minutes = post.reading_time.unwrap_or(1);
    // spec D12: singular for one minute
    let unit = if minutes == 1 { "Minute" } else { "Minuten" };
    format!(
        r#"<!DOCTYPE html>
<html lang="de">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<style>{CSS}</style>
</head>
<body class="post-template">
<article>
<h1>{title}</h1>
<div class="article-byline-meta"><span class="byline-reading-time"><span class="bull">•</span> {minutes} {unit} Lesezeit</span></div>
<section class="gh-content gh-canvas">{html}</section>
</article>
<script src="/js?domain={domain}" defer></script>
</body>
</html>
"#,
        html = post.html,
        domain = escape(domain),
    )
}

/// Dev page next to the post's audio, so the player finds it at `/<domain>/<slug>/`.
pub fn page_path(site: &Path, domain: &str, slug: &str) -> PathBuf {
    site.join(domain).join(slug).join("index.html")
}

/// Fetch a public post and write its dev page under `site/<domain>`; returns the page's path.
/// `domain` is the configured site domain, which names the deploy folder.
pub fn build(
    cfg: &GhostConfig,
    domain: &str,
    arg: &str,
    site: &Path,
    get: &dyn Fn(&str) -> Result<HttpReply>,
) -> Result<PathBuf> {
    let slug = slug_from_arg(arg)?;
    let post = fetch_post(cfg, &slug, get)?;
    require_public(&post)?;
    let page = page_path(site, domain, &post.slug);
    let dir = page
        .parent()
        .ok_or_else(|| anyhow!("no parent of {}", page.display()))?;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    fs::write(&page, render_dev_page(&post, domain))
        .with_context(|| format!("writing {}", page.display()))?;
    Ok(page)
}

// --- server ------------------------------------------------------------------------------

/// A running dev server: one accept thread, one thread per request.
pub struct DevServer {
    server: Arc<Server>,
    stopping: Arc<AtomicBool>,
    accept: JoinHandle<()>,
    url: String,
}

impl DevServer {
    /// Listen on `addr` (port 0 picks a free one) and serve `site`, with `/js` answered by `player`.
    pub fn start(addr: &str, site: &Path, player: &Path) -> Result<DevServer> {
        let server =
            Arc::new(Server::http(addr).map_err(|e| anyhow!("cannot listen on {addr}: {e}"))?);
        let local = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| anyhow!("{addr} is not an IP address"))?;
        let roots = Arc::new(Roots::new(site, player)?);
        let stopping = Arc::new(AtomicBool::new(false));
        let accept = {
            let (server, stopping) = (server.clone(), stopping.clone());
            std::thread::spawn(move || loop {
                match server.recv() {
                    Ok(rq) => {
                        let roots = roots.clone();
                        std::thread::spawn(move || handle(rq, &roots));
                    }
                    Err(_) if stopping.load(Ordering::SeqCst) => break,
                    Err(e) => eprintln!("accept failed: {e}"),
                }
            })
        };
        Ok(DevServer {
            server,
            stopping,
            accept,
            url: format!("http://{local}"),
        })
    }

    /// `http://<ip>:<port>` the server listens on.
    pub fn url(&self) -> String {
        self.url.clone()
    }

    /// Stop accepting and wait for the accept thread (requests in flight finish on their own).
    pub fn stop(self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.server.unblock();
        let _ = self.accept.join();
    }

    /// Block while the server runs, i.e. until the process is killed.
    pub fn wait(self) {
        let _ = self.accept.join();
    }
}

/// `lectover dev serve`: serve `site` on 127.0.0.1:`port` until killed.
pub fn serve(site: &Path, player: &Path, port: u16) -> Result<()> {
    let srv = DevServer::start(&format!("127.0.0.1:{port}"), site, player)?;
    println!("serving {} at http://localhost:{port}/", site.display());
    srv.wait();
    Ok(())
}

fn header(name: &str, value: &str) -> Header {
    // Names and values are ASCII built here; from_bytes only fails on invalid bytes.
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

type Reply = Response<Box<dyn Read + Send>>;

fn reply(status: u16, headers: Vec<Header>, body: Box<dyn Read + Send>, len: u64) -> Reply {
    let mut headers = headers;
    headers.push(header("Accept-Ranges", "bytes"));
    // A fixed Content-Length even for large files (tiny_http would switch to chunked).
    Response::new(StatusCode(status), headers, body, Some(len as usize), None)
        .with_chunked_threshold(usize::MAX)
}

fn text_reply(status: u16, msg: &str) -> Reply {
    let body = msg.as_bytes().to_vec();
    let len = body.len() as u64;
    reply(
        status,
        vec![header("Content-Type", "text/plain; charset=utf-8")],
        Box::new(std::io::Cursor::new(body)),
        len,
    )
}

/// Where files come from. Every served file is canonicalised (symlinks resolved) and must lie
/// under its root: the site for site paths, the player's directory for `/js`. Symlinks that
/// stay inside the root are fine; those leading out of it get a 404.
struct Roots {
    /// Canonical site directory.
    site: PathBuf,
    player: PathBuf,
    /// Canonical directory of the player.
    player_root: PathBuf,
}

impl Roots {
    fn new(site: &Path, player: &Path) -> Result<Roots> {
        let site_canon = fs::canonicalize(site).with_context(|| {
            format!(
                "site directory {} not found (run `lectover dev build` first)",
                site.display()
            )
        })?;
        let dir = match player.parent() {
            Some(d) if !d.as_os_str().is_empty() => d,
            _ => Path::new("."),
        };
        let player_root = fs::canonicalize(dir)
            .with_context(|| format!("player directory {} not found", dir.display()))?;
        Ok(Roots {
            site: site_canon,
            player: player.to_path_buf(),
            player_root,
        })
    }
}

/// `path` with symlinks resolved, if it exists and lies under the canonical `root`.
fn confine(path: &Path, root: &Path) -> Option<PathBuf> {
    fs::canonicalize(path).ok().filter(|c| c.starts_with(root))
}

fn handle(rq: Request, roots: &Roots) {
    let range = rq
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .map(|h| h.value.as_str().to_string());
    let resp = match rq.method() {
        Method::Get | Method::Head => respond(rq.url(), range.as_deref(), roots),
        _ => text_reply(501, "unsupported method"),
    };
    eprintln!("{} {} {}", rq.method(), rq.url(), resp.status_code().0);
    let _ = rq.respond(resp);
}

/// The file a request path names under `site`, like `SimpleHTTPRequestHandler.translate_path`:
/// percent-decoded, `.` and `..` resolved without leaving `site`.
fn translate(path: &str, site: &Path) -> PathBuf {
    let decoded = percent_decode_str(path).decode_utf8_lossy();
    let mut parts: Vec<&str> = Vec::new();
    for word in decoded.split('/') {
        match word {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            w => parts.push(w),
        }
    }
    parts.iter().fold(site.to_path_buf(), |p, w| p.join(w))
}

fn respond(url: &str, range: Option<&str>, roots: &Roots) -> Reply {
    let (path, query) = match url.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (url, None),
    };
    let path = path.split('#').next().unwrap_or_default();
    if path == "/js" {
        return serve_file(&roots.player, &roots.player_root, range);
    }
    let requested = translate(path, &roots.site);
    let Some(dir) = confine(&requested, &roots.site).filter(|p| p.is_dir()) else {
        return serve_file(&requested, &roots.site, range);
    };
    if !path.ends_with('/') {
        let location = match query {
            Some(q) => format!("{path}/?{q}"),
            None => format!("{path}/"),
        };
        let empty: Box<dyn Read + Send> = Box::new(std::io::empty());
        return reply(301, vec![header("Location", &location)], empty, 0);
    }
    match ["index.html", "index.htm"]
        .iter()
        .map(|i| dir.join(i))
        .find(|p| p.is_file())
    {
        Some(index) => serve_file(&index, &roots.site, range),
        None => listing(&dir, path),
    }
}

fn listing(dir: &Path, path: &str) -> Reply {
    let mut names: Vec<String> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if e.path().is_dir() {
                    format!("{name}/")
                } else {
                    name
                }
            })
            .collect(),
        Err(_) => return text_reply(404, "not found"),
    };
    names.sort_by_key(|n| n.to_lowercase());
    let items: String = names
        .iter()
        .map(|n| format!("<li><a href=\"{0}\">{0}</a></li>\n", escape(n)))
        .collect();
    let title = format!("Directory listing for {}", escape(path));
    let body = format!(
        "<!DOCTYPE html>\n<html>\n<head><meta charset=\"utf-8\"><title>{title}</title></head>\n\
         <body>\n<h1>{title}</h1>\n<hr>\n<ul>\n{items}</ul>\n<hr>\n</body>\n</html>\n"
    )
    .into_bytes();
    let len = body.len() as u64;
    reply(
        200,
        vec![header("Content-Type", "text/html; charset=utf-8")],
        Box::new(std::io::Cursor::new(body)),
        len,
    )
}

/// Content type by extension, as Python's `mimetypes` (plus `.ogg` → `audio/ogg`) answers.
fn content_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "ogg" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "opus" => "audio/opus",
        "wav" => "audio/x-wav",
        "txt" => "text/plain",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "application/octet-stream",
    }
}

/// What a `Range` header asks of a file of `size` bytes (RFC 7233, a single byte range).
#[derive(Debug, PartialEq, Eq)]
enum RangeAnswer {
    /// No header, another unit, several ranges or invalid syntax: send the whole file.
    Whole,
    /// Bytes `start..=end`.
    Part(u64, u64),
    /// 416.
    Unsatisfiable,
}

fn digits(s: &str) -> Option<Option<u64>> {
    match s {
        "" => Some(None),
        s if s.bytes().all(|b| b.is_ascii_digit()) => Some(Some(s.parse().unwrap_or(u64::MAX))),
        _ => None,
    }
}

fn parse_range(header: Option<&str>, size: u64) -> RangeAnswer {
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return RangeAnswer::Whole;
    };
    let Some((first, last)) = spec.trim().split_once('-') else {
        return RangeAnswer::Whole;
    };
    let (Some(first), Some(last)) = (digits(first), digits(last)) else {
        return RangeAnswer::Whole;
    };
    match (first, last) {
        (None, None) => RangeAnswer::Whole,
        // last-byte-pos < first-byte-pos makes the spec invalid (2.1): ignore the header
        (Some(f), Some(l)) if l < f => RangeAnswer::Whole,
        (Some(f), _) if f >= size => RangeAnswer::Unsatisfiable,
        (Some(f), l) => RangeAnswer::Part(f, l.unwrap_or(u64::MAX).min(size - 1)),
        (None, Some(0)) => RangeAnswer::Unsatisfiable,
        (None, Some(_)) if size == 0 => RangeAnswer::Unsatisfiable,
        (None, Some(n)) => RangeAnswer::Part(size.saturating_sub(n), size - 1),
    }
}

/// Serve `path` if it resolves to a file under the canonical `root`; the content type follows
/// the requested name.
fn serve_file(path: &Path, root: &Path, range: Option<&str>) -> Reply {
    let Some(real) = confine(path, root) else {
        return text_reply(404, "not found");
    };
    let Ok(mut f) = File::open(&real) else {
        return text_reply(404, "not found");
    };
    let size = match f.metadata() {
        Ok(m) if m.is_file() => m.len(),
        _ => return text_reply(404, "not found"),
    };
    let ctype = header("Content-Type", content_type(path));
    match parse_range(range, size) {
        RangeAnswer::Whole => reply(200, vec![ctype], Box::new(f), size),
        RangeAnswer::Unsatisfiable => {
            let empty: Box<dyn Read + Send> = Box::new(std::io::empty());
            reply(
                416,
                vec![header("Content-Range", &format!("bytes */{size}"))],
                empty,
                0,
            )
        }
        RangeAnswer::Part(start, end) => {
            if f.seek(SeekFrom::Start(start)).is_err() {
                return text_reply(500, "seek failed");
            }
            let len = end - start + 1;
            let cr = header("Content-Range", &format!("bytes {start}-{end}/{size}"));
            reply(206, vec![ctype, cr], Box::new(f.take(len)), len)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_grammar() {
        use RangeAnswer::*;
        assert_eq!(parse_range(Some("bytes=2-5"), 10), Part(2, 5));
        assert_eq!(parse_range(Some(" bytes=7- "), 10), Part(7, 9));
        assert_eq!(
            parse_range(Some("bytes=8-99999999999999999999999"), 10),
            Part(8, 9)
        );
        assert_eq!(
            parse_range(Some("bytes=99999999999999999999999-"), 10),
            Unsatisfiable
        );
        assert_eq!(parse_range(Some("bytes=-3"), 10), Part(7, 9));
        assert_eq!(parse_range(Some("bytes=-3"), 0), Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-"), 0), Unsatisfiable);
        assert_eq!(parse_range(None, 10), Whole);
        assert_eq!(parse_range(Some("bytes=+1-2"), 10), Whole);
    }

    #[test]
    fn translate_stays_inside_site() {
        let site = Path::new("/s");
        assert_eq!(translate("/a/../../b", site), Path::new("/s/b"));
        assert_eq!(translate("/a/%2e%2e/%2E%2E/c", site), Path::new("/s/c"));
        assert_eq!(translate("/", site), Path::new("/s"));
    }
}
