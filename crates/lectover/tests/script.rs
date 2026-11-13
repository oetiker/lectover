//! Spoken order: intro, `<hr>` and `AUDIO ONLY` comments, site sounds, sting placement.

use std::f64::consts::PI;

use lectover::audio::assemble::{
    START_LEAD_SECS, STING_LEAD_SECS, STING_MAX_PEAK, STING_TAIL_SECS,
};
use lectover::audio::{assemble_parts, assemble_parts_with, level_sting};
use lectover::ghost::{Author, Post};
use lectover::plan::{plan_steps, ChunkKind};
use lectover::script::{audio_only_parts, build_script, intro_text, SiteSounds, Step, Sting};
use lectover::text::normalize::{spoken_groups, Lexicon};
use lectover::text::{extract_blocks, extract_marks, Mark};

fn sine(sr: u32, secs: f64, amp: f64) -> Vec<f32> {
    (0..(sr as f64 * secs) as usize)
        .map(|i| (amp * (2.0 * PI * 220.0 * i as f64 / sr as f64).sin()) as f32)
        .collect()
}

fn take(sr: u32, secs: f64, kind: ChunkKind) -> (Vec<f32>, String, ChunkKind) {
    let pad = vec![0.0f32; (sr as f64 * 0.2) as usize];
    let mut wav = pad.clone();
    wav.extend(sine(sr, secs, 0.3));
    wav.extend(pad);
    (wav, "ein Satz mit Silben".to_string(), kind)
}

fn post(title: &str, authors: &[&str], published_at: &str) -> Post {
    Post {
        title: title.into(),
        published_at: published_at.into(),
        authors: authors
            .iter()
            .map(|n| Author {
                name: n.to_string(),
            })
            .collect(),
        ..Default::default()
    }
}

#[test]
fn audio_only_comment_as_ghost_delivers_it() {
    // verbatim from olten.jetzt/frohburg-spiel-satz-patt
    assert_eq!(
        audio_only_parts(" AUDIO ONLY\n\nTest Bla Bla audio only \n\n"),
        Some(vec![Some("Test Bla Bla audio only".into())])
    );
}

#[test]
fn audio_only_paragraphs_and_breaks() {
    let parts =
        audio_only_parts("audio only Erstens.\nNoch erstens.\n\nZweitens. --- Drittens.\n-----\n")
            .unwrap();
    assert_eq!(
        parts,
        vec![
            Some("Erstens. Noch erstens.".into()),
            Some("Zweitens.".into()),
            None,
            Some("Drittens.".into()),
            None,
        ]
    );
    // two dashes are text, not a break
    assert_eq!(
        audio_only_parts("AUDIO ONLY a -- b"),
        Some(vec![Some("a -- b".into())])
    );
}

#[test]
fn other_comments_are_not_audio() {
    assert_eq!(audio_only_parts("kg-card-begin: html"), None);
    assert_eq!(audio_only_parts("AUDIO"), None);
    assert_eq!(audio_only_parts(" Hinweis: AUDIO ONLY später"), None);
}

#[test]
fn intro_names_title_authors_and_date() {
    let p = post(
        "Frohburg: Spiel, Satz, Patt?",
        &["Tobias Oetiker"],
        "2026-10-16T10:27:51.000+02:00",
    );
    assert_eq!(
        intro_text(&p).unwrap(),
        "Sie hören: «Frohburg: Spiel, Satz, Patt?» Verfasst von Tobias Oetiker. Publiziert am 16. Oktober 2026."
    );
    let p = post("T", &["A", "B"], "2026-03-01T00:00:00.000+01:00");
    assert_eq!(
        intro_text(&p).unwrap(),
        "Sie hören: «T». Verfasst von A und B. Publiziert am 1. März 2026."
    );
    let p = post("T", &["A", "B", "C"], "");
    assert_eq!(
        intro_text(&p).unwrap(),
        "Sie hören: «T». Verfasst von A, B und C."
    );
    assert_eq!(
        intro_text(&post("T", &[], "kaputt")).unwrap(),
        "Sie hören: «T»."
    );
    assert_eq!(intro_text(&post(" ", &["A"], "")), None);
}

#[test]
fn intro_date_is_spoken() {
    let p = post("T", &["A"], "2026-10-16T10:27:51.000+02:00");
    let spoken: Vec<String> = spoken_groups(&intro_text(&p).unwrap(), &Lexicon::new())
        .into_iter()
        .map(|g| g.spoken)
        .collect();
    let spoken = spoken.join(" ");
    assert!(
        spoken.contains("sechzehnten Oktober zweitausendsechsundzwanzig"),
        "{spoken}"
    );
}

const HTML: &str =
    "<!--kg-card-begin: html--><!-- AUDIO ONLY Vorab. --- Danach. --><!--kg-card-end: html-->\
<p>Eins.</p><hr><h2>Zwei</h2><figure><!-- AUDIO ONLY versteckt --></figure>\
<p>Drei <!-- AUDIO ONLY Nachsatz. --> vier.</p>";

#[test]
fn marks_keep_the_page_blocks() {
    let (blocks, marks) = extract_marks(HTML);
    assert_eq!(blocks, extract_blocks(HTML));
    let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(texts, ["Eins.", "Zwei", "Drei vier."]);
    assert_eq!(
        marks,
        vec![
            Mark::Comment("kg-card-begin: html".into()),
            Mark::Comment(" AUDIO ONLY Vorab. --- Danach. ".into()),
            Mark::Comment("kg-card-end: html".into()),
            Mark::Block(0),
            Mark::Rule,
            Mark::Block(1),
            Mark::Block(2),
            Mark::Comment(" AUDIO ONLY Nachsatz. ".into()),
        ]
    );
}

#[test]
fn script_orders_intro_page_and_audio_only_text() {
    let (page, marks) = extract_marks(HTML);
    let script = build_script(page, &marks, Some("Sie hören: «T».".into()));
    assert_eq!(script.page_blocks, 3);
    let texts: Vec<&str> = script.blocks[3..].iter().map(|b| b.text.as_str()).collect();
    assert_eq!(texts, ["Sie hören: «T».", "Vorab.", "Danach.", "Nachsatz."]);
    let (b, intro, brk) = (
        Step::Block,
        Step::Sting(Sting::Intro),
        Step::Sting(Sting::Break),
    );
    assert_eq!(
        script.steps,
        vec![b(3), intro, b(4), brk, b(5), b(0), brk, b(1), b(2), b(6)]
    );

    let lex = Lexicon::new();
    let groups: Vec<_> = script
        .blocks
        .iter()
        .map(|b| spoken_groups(&b.text, &lex))
        .collect();
    let (chunks, stings) = plan_steps(&script.blocks, &groups, &script.steps, 1000);
    let order: Vec<usize> = chunks.iter().map(|c| c.block).collect();
    assert_eq!(order, [3, 4, 5, 0, 1, 2, 6]);
    assert_eq!(
        stings,
        [(1, Sting::Intro), (2, Sting::Break), (4, Sting::Break)]
    );
}

#[test]
fn stings_sit_between_takes_with_their_pauses() {
    let sr = 24000;
    let takes = [
        take(sr, 1.0, ChunkKind::Paragraph),
        take(sr, 0.5, ChunkKind::Paragraph),
    ];
    let sting = sine(sr, 2.0, 0.1);
    let (audio, parts) = assemble_parts_with(&takes, sr, false, &[(1, Some(&sting)), (2, None)]);
    let tol = 2.0 / sr as f64;
    let expect = START_LEAD_SECS
        + parts[0].1.len() as f64 / sr as f64
        + STING_LEAD_SECS
        + 2.0
        + STING_TAIL_SECS;
    assert!(
        (parts[1].0 - expect).abs() <= tol,
        "{} vs {expect}",
        parts[1].0
    );
    // a trailing silent break: the end pause after the last take
    let end = parts[1].0 + parts[1].1.len() as f64 / sr as f64 + STING_LEAD_SECS + STING_TAIL_SECS;
    assert!((audio.len() as f64 / sr as f64 - end).abs() <= tol);
    // a sting before the first take has no lead pause of its own, only the file's start lead
    let (_, parts) = assemble_parts_with(&takes, sr, false, &[(0, Some(&sting))]);
    assert!((parts[0].0 - (START_LEAD_SECS + 2.0 + STING_TAIL_SECS)).abs() <= tol);
    // without stings it is assemble_parts
    assert_eq!(
        assemble_parts_with(&takes, sr, false, &[]),
        assemble_parts(&takes, sr, false)
    );
}

#[test]
fn sting_is_resampled_and_levelled() {
    let quiet = sine(44100, 1.0, 0.05);
    let out = level_sting(&quiet, 44100, 24000);
    assert_eq!(out.len(), 24000);
    // measured like the code: over the audible samples (|v| > 0.01)
    let loud: Vec<f64> = out
        .iter()
        .map(|v| *v as f64)
        .filter(|v| v.abs() > 0.01)
        .collect();
    let rms = (loud.iter().map(|v| v * v).sum::<f64>() / loud.len() as f64).sqrt();
    assert!(
        (20.0 * rms.log10() + 20.0).abs() < 0.5,
        "{}",
        20.0 * rms.log10()
    );
    // a spiky sound is held below the peak limit
    let mut spiky = sine(24000, 1.0, 0.05);
    spiky[100] = 1.0;
    let out = level_sting(&spiky, 24000, 24000);
    assert!(out.iter().fold(0.0f32, |m, v| m.max(v.abs())) <= STING_MAX_PEAK + 1e-6);
}

fn write_wav(path: &std::path::Path, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 24000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for s in samples {
        w.write_sample((s * 32767.0) as i16).unwrap();
    }
    w.finalize().unwrap();
}

#[test]
fn site_sounds_break_falls_back_to_intro() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(SiteSounds::load(dir.path()).unwrap(), SiteSounds::default());
    write_wav(&dir.path().join("intro.wav"), &[0.5; 10]);
    let s = SiteSounds::load(dir.path()).unwrap();
    assert_eq!(s.get(Sting::Break), s.get(Sting::Intro));
    assert_eq!(s.get(Sting::Intro).unwrap().0.len(), 10);
    write_wav(&dir.path().join("break.wav"), &[0.5; 4]);
    let s = SiteSounds::load(dir.path()).unwrap();
    assert_eq!(s.get(Sting::Break).unwrap().0.len(), 4);
    assert_eq!(s.get(Sting::Intro).unwrap().0.len(), 10);
}
