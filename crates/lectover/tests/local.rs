//! Local post files: Markdown/HTML to a Ghost-like post, and file-vs-slug detection.
//! No network, no models.

use std::fs;

use lectover::local::{
    html_body, load_post, local_format, local_path, parse_date, post_from_source, slugify,
    take_first_h1, Format, LocalMeta,
};
use lectover::script::{build_script, intro_text};
use lectover::text::extract_marks;

fn md(source: &str, meta: &LocalMeta) -> lectover::ghost::Post {
    post_from_source(
        source,
        Format::Markdown,
        "Mein Entwurf",
        meta,
        "2026-11-20",
        "+01:00",
    )
    .unwrap()
}

#[test]
fn markdown_title_from_first_h1_leaves_the_body() {
    let p = md(
        "# Der *Titel* &amp; mehr\n\nErster Absatz.\n\n## Teil\n\nZweiter.\n",
        &LocalMeta::default(),
    );
    assert_eq!(p.title, "Der Titel & mehr");
    assert!(!p.html.contains("<h1"), "{}", p.html);
    assert!(p.html.starts_with("<p>Erster Absatz.</p>"), "{}", p.html);
    assert!(p.html.contains("<h2>Teil</h2>"), "{}", p.html);
    assert_eq!(p.visibility, "public");
    let (blocks, _) = extract_marks(&p.html);
    let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(texts, ["Erster Absatz.", "Teil", "Zweiter."]);
}

#[test]
fn setext_heading_is_a_title_too() {
    let p = md("Titel\n=====\n\nText.\n", &LocalMeta::default());
    assert_eq!(p.title, "Titel");
    assert_eq!(p.html.trim(), "<p>Text.</p>");
}

#[test]
fn title_flag_wins_and_keeps_the_body() {
    let meta = LocalMeta {
        title: Some("Anders".into()),
        ..Default::default()
    };
    let p = md("# Überschrift\n\nText.\n", &meta);
    assert_eq!(p.title, "Anders");
    assert!(p.html.contains("<h1>Überschrift</h1>"), "{}", p.html);
}

#[test]
fn no_h1_falls_back_to_file_stem() {
    let p = md("## Nur Teil\n\nText.\n", &LocalMeta::default());
    assert_eq!(p.title, "Mein Entwurf");
    assert_eq!(p.slug, "mein-entwurf");
}

#[test]
fn slug_date_authors_and_intro() {
    let meta = LocalMeta {
        authors: vec!["Anna Muster".into(), "Ben Beispiel".into()],
        date: Some("2026-10-16".into()),
        slug: Some("eigener-slug".into()),
        ..Default::default()
    };
    let p = md("# Patt?\n\nText.\n", &meta);
    assert_eq!(p.slug, "eigener-slug");
    assert_eq!(p.published_at, "2026-10-16T00:00:00.000+01:00");
    assert_eq!(p.reading_time, Some(1));
    assert_eq!(
        intro_text(&p).unwrap(),
        "Sie hören: «Patt?» Verfasst von Anna Muster und Ben Beispiel. Publiziert am 16. Oktober 2026."
    );
    // default date: today
    let p = md("Text.\n", &LocalMeta::default());
    assert_eq!(p.published_at, "2026-11-20T00:00:00.000+01:00");
    assert!(p.authors.is_empty());
}

#[test]
fn bad_date_is_refused() {
    let meta = LocalMeta {
        date: Some("16.10.2026".into()),
        ..Default::default()
    };
    let err = post_from_source(
        "Text.",
        Format::Markdown,
        "a",
        &meta,
        "2026-11-20",
        "+01:00",
    )
    .unwrap_err();
    assert!(err.to_string().contains("YYYY-MM-DD"), "{err}");
    assert!(parse_date("2026-13-01").is_err());
    assert_eq!(parse_date(" 2026-01-31 ").unwrap(), "2026-01-31");
}

#[test]
fn slugify_like_ghost() {
    assert_eq!(slugify("Mein Entwurf"), "mein-entwurf");
    assert_eq!(slugify("Größe & Café_2026!"), "grosse-cafe-2026");
    assert_eq!(slugify("--Über  alles--"), "uber-alles");
    assert_eq!(slugify("ÆØ"), "aeo");
    assert_eq!(slugify("!!!"), "");
}

#[test]
fn unusable_slug_is_refused() {
    let err = post_from_source(
        "Text.",
        Format::Markdown,
        "!!!",
        &LocalMeta::default(),
        "2026-11-20",
        "+01:00",
    )
    .unwrap_err();
    assert!(err.to_string().contains("--slug"), "{err}");
}

#[test]
fn leading_comments_are_silent_unless_audio_only() {
    let src = "<!--\nNotiz: nicht vorlesen.\n-->\n\n<!-- AUDIO ONLY Nur im Audio. -->\n\n# Titel\n\nText.\n";
    let p = md(src, &LocalMeta::default());
    assert_eq!(p.title, "Titel");
    let (page, marks) = extract_marks(&p.html);
    let texts: Vec<&str> = page.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(texts, ["Text."], "{}", p.html);
    let script = build_script(page, &marks, None);
    let spoken: Vec<&str> = script.blocks.iter().map(|b| b.text.as_str()).collect();
    assert!(spoken.contains(&"Nur im Audio."), "{spoken:?}");
    assert!(!spoken.iter().any(|t| t.contains("Notiz")), "{spoken:?}");
}

#[test]
fn markdown_extensions() {
    let p = md(
        "| a | b |\n|---|---|\n| 1 | 2 |\n\n~~weg~~ da\n",
        &LocalMeta::default(),
    );
    assert!(p.html.contains("<table>"), "{}", p.html);
    assert!(p.html.contains("<del>weg</del>"), "{}", p.html);
}

#[test]
fn html_document_body_is_used() {
    let doc = "<!DOCTYPE html><html><head><title>X</title></head>\n<body class=\"a\">\n<h1>Der Titel</h1>\n<p>Text.</p>\n</body></html>";
    assert_eq!(html_body(doc), "\n<h1>Der Titel</h1>\n<p>Text.</p>\n");
    assert_eq!(html_body("<p>frag</p>"), "<p>frag</p>");
    let p = post_from_source(
        doc,
        Format::Html,
        "seite",
        &LocalMeta::default(),
        "2026-11-20",
        "+01:00",
    )
    .unwrap();
    assert_eq!(p.title, "Der Titel");
    assert_eq!(p.html.trim(), "<p>Text.</p>");
    assert_eq!(p.slug, "seite");
    assert_eq!(take_first_h1("<p>x</p>"), None);
}

#[test]
fn path_versus_slug() {
    assert_eq!(local_format("entwurf.md"), Some(Format::Markdown));
    assert_eq!(local_format("a/b/Entwurf.MARKDOWN"), Some(Format::Markdown));
    assert_eq!(local_format("x.htm"), Some(Format::Html));
    assert_eq!(local_format("x.html"), Some(Format::Html));
    assert_eq!(local_format("frohburg-spiel-satz-patt"), None);
    assert_eq!(local_format("https://olten.jetzt/post.html"), None);
    assert_eq!(local_path("https://olten.jetzt/a.md"), None);
    assert_eq!(local_path("frohburg-spiel-satz-patt"), None);
    // a name with a known extension is a file even before it exists (a slug has no dot)
    assert!(local_path("noch-nicht-da.md").is_some());
    // any existing file, whatever its extension
    let dir = tempfile::tempdir().unwrap();
    let txt = dir.path().join("notes.txt");
    fs::write(&txt, "# T\n\nText.\n").unwrap();
    assert_eq!(local_path(txt.to_str().unwrap()), Some(txt.clone()));
}

#[test]
fn load_post_reads_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("Mein Beitrag.md");
    fs::write(&file, "# Hallo Welt\n\nEin Satz.\n").unwrap();
    let p = load_post(&file, &LocalMeta::default()).unwrap();
    assert_eq!(
        (p.title.as_str(), p.slug.as_str()),
        ("Hallo Welt", "mein-beitrag")
    );
    assert_eq!(p.published_at.len(), "2026-11-20T00:00:00.000+01:00".len());
    let err = load_post(&dir.path().join("fehlt.md"), &LocalMeta::default()).unwrap_err();
    assert!(err.to_string().contains("fehlt.md"), "{err}");
}

fn bin(home: &std::path::Path, config: &std::path::Path) -> std::process::Command {
    let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_lectover"));
    c.arg("--config")
        .arg(config)
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_STATE_HOME")
        .env_remove("XDG_CACHE_HOME")
        .env_remove("LECTOVER_CONFIG")
        .env_remove("LECTOVER_SITE")
        .env_remove("GHOST_CONTENT_KEY");
    c
}

/// `dev build <file.md>` needs no Ghost key and no network; the page lands in <out>/<domain>/<slug>/.
#[test]
fn dev_build_local_markdown_end_to_end() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    // A port nobody listens on: any Ghost request would fail.
    fs::write(
        &config,
        "[site.\"example.org\"]\nghost_url = \"http://127.0.0.1:9\"\n",
    )
    .unwrap();
    let file = home.path().join("Entwurf Eins.md");
    fs::write(&file, "<!-- intern -->\n# Der Titel\n\nErster Absatz.\n").unwrap();
    let out_dir = home.path().join("site");
    let out = bin(home.path(), &config)
        .args(["dev", "build"])
        .arg(&file)
        .args(["--author", "Anna", "--date", "2026-10-16", "--out"])
        .arg(&out_dir)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    let page = fs::read_to_string(out_dir.join("example.org/entwurf-eins/index.html")).unwrap();
    assert!(page.contains("<h1>Der Titel</h1>\n<div"), "{page}");
    assert!(page.contains("<p>Erster Absatz.</p>"), "{page}");
    assert_eq!(page.matches("<h1>").count(), 1, "{page}");
    assert!(page.contains("/js?domain=example.org"), "{page}");
}

#[test]
fn local_options_are_refused_for_a_ghost_slug() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    fs::write(
        &config,
        "[site.\"example.org\"]\nghost_url = \"http://127.0.0.1:9\"\n",
    )
    .unwrap();
    let out = bin(home.path(), &config)
        .args(["dev", "build", "some-slug", "--title", "X", "--out"])
        .arg(home.path().join("site"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("local file only"), "{err}");
}

/// The render plan of a local post: the intro speaks the title taken from the file.
#[test]
fn local_post_plans_for_render() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("voices.json"), r#"{"a": {"label": "A"}}"#).unwrap();
    fs::write(dir.path().join("a.wav"), b"").unwrap();
    fs::write(dir.path().join("a.txt"), b"x").unwrap();
    let p = md("# Der Titel\n\nErster Absatz.\n", &LocalMeta::default());
    let lexicon = lectover::text::normalize::Lexicon::default();
    let prep = lectover::render::run::prepare_post(p, &[], dir.path(), &lexicon, 1000).unwrap();
    assert_eq!(prep.page_blocks, 1);
    assert!(
        prep.blocks[1].text.starts_with("Sie hören: «Der Titel»."),
        "{:?}",
        prep.blocks[1].text
    );
    assert_eq!(prep.voices, ["a"]);
}
