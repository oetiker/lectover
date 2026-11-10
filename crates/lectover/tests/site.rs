//! Port of tests/test_site_output.py plus the encode-and-publish failure path.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::bail;
use lectover::audio::encode_opus;
use lectover::ghost::Post;
use lectover::site::{encode_and_publish, prune_stale, publish_voice};
use serde_json::{json, Value};

fn post() -> Post {
    Post {
        slug: "my-post".into(),
        html: String::new(),
        visibility: "public".into(),
        updated_at: "2026-10-31T10:00:00.000+01:00".into(),
        title: "T".into(),
        reading_time: None,
        ..Default::default()
    }
}

fn timings(voice: &str, hashes: &[&str]) -> Value {
    let blocks: Vec<Value> = hashes
        .iter()
        .map(|h| json!({"tag": "p", "hash": h, "words": []}))
        .collect();
    json!({"version": 1, "voice": voice, "duration": 12.5, "blocks": blocks})
}

fn publish_h(
    tmp: &Path,
    voice: &str,
    label: &str,
    data: &[u8],
    mp3_data: &[u8],
    hashes: &[&str],
    default: &str,
) -> anyhow::Result<Value> {
    let ogg = tmp.join("work.ogg");
    let mp3 = tmp.join("work.mp3");
    fs::write(&ogg, data).unwrap();
    fs::write(&mp3, mp3_data).unwrap();
    publish_voice(
        &tmp.join("site"),
        voice,
        label,
        &ogg,
        &mp3,
        &timings(voice, hashes),
        &post(),
        default,
    )
}

fn publish(tmp: &Path, voice: &str, label: &str, data: &[u8], mp3: &[u8]) -> Value {
    publish_h(tmp, voice, label, data, mp3, &["aaaaaaaa"], "erzaehler").unwrap()
}

/// Files in `dir` whose name starts with `prefix` and ends with `suffix`, sorted.
fn glob(dir: &Path, prefix: &str, suffix: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let n = p.file_name().unwrap().to_str().unwrap();
            n.starts_with(prefix) && n.ends_with(suffix)
        })
        .collect();
    out.sort();
    out
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

fn name(p: &Path) -> &str {
    p.file_name().unwrap().to_str().unwrap()
}

#[test]
fn encode_opus_is_mono_opus() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("a.ogg");
    let sr = 24000;
    encode_opus(&vec![0.0f32; sr as usize], sr, &p).unwrap();
    let mut rdr = ogg::PacketReader::new(fs::File::open(&p).unwrap());
    let head = rdr.read_packet_expected().unwrap();
    assert_eq!(&head.data[..8], b"OpusHead");
    assert_eq!(head.data[9], 1, "channel count");
    let pre_skip = u16::from_le_bytes([head.data[10], head.data[11]]) as u64;
    assert_eq!(
        u32::from_le_bytes(head.data[12..16].try_into().unwrap()),
        sr
    );
    let tags = rdr.read_packet_expected().unwrap();
    assert_eq!(&tags.data[..8], b"OpusTags");
    let mut last = None;
    while let Some(pkt) = rdr.read_packet().unwrap() {
        last = Some(pkt);
    }
    let last = last.unwrap();
    assert!(last.last_in_stream());
    // final granule = pre-skip + input duration in 48 kHz samples
    assert_eq!(last.absgp_page(), pre_skip + 48000);
}

#[test]
fn republish_replaces_previous_set() {
    let tmp = tempfile::tempdir().unwrap();
    publish(tmp.path(), "erzaehler", "Erzähler", b"one", b"mone");
    publish(tmp.path(), "other", "Other", b"o", b"mo");
    let m = publish(tmp.path(), "erzaehler", "Erzähler", b"two", b"mtwo");
    let site = tmp.path().join("site");
    assert_eq!(glob(&site, "other.", "").len(), 3, "other voice untouched");
    let oggs = glob(&site, "erzaehler.", ".ogg");
    let jsons = glob(&site, "erzaehler.", ".json");
    let mp3s = glob(&site, "erzaehler.", ".mp3");
    assert_eq!((oggs.len(), mp3s.len(), jsons.len()), (1, 1, 1));
    assert_eq!(fs::read(&mp3s[0]).unwrap(), b"mtwo");
    assert_eq!(m["voices"][0]["mp3"], name(&mp3s[0]));
    let entry = &m["voices"][0];
    assert_eq!(m["voices"].as_array().unwrap().len(), 2, "one entry per voice");
    assert_eq!(entry["audio"], name(&oggs[0]));
    assert_eq!(entry["timings"], name(&jsons[0]));
    assert_eq!(oggs[0].file_stem(), jsons[0].file_stem());
    assert_eq!(fs::read(&oggs[0]).unwrap(), b"two");
    assert_eq!(entry["duration"], 12.5);
    assert_eq!(read_json(&site.join("manifest.json")), m);
    assert_eq!(m["version"], 1);
    assert_eq!(m["slug"], "my-post");
    assert_eq!(m["post_updated_at"], post().updated_at);
    assert!(fs::read_to_string(site.join("manifest.json"))
        .unwrap()
        .contains("Erzähler"));
}

#[test]
fn manifest_bytes_match_python_json_dumps() {
    let tmp = tempfile::tempdir().unwrap();
    let m = publish(tmp.path(), "erzaehler", "Erzähler", b"one", b"mone");
    let audio = m["voices"][0]["audio"].as_str().unwrap();
    let stem = audio.strip_suffix(".ogg").unwrap();
    let mp3 = m["voices"][0]["mp3"].as_str().unwrap();
    let site = tmp.path().join("site");
    // key order and separators exactly as Python's json.dumps(ensure_ascii=False)
    let want = format!(
        "{{\"voices\": [{{\"id\": \"erzaehler\", \"label\": \"Erzähler\", \"audio\": \"{stem}.ogg\", \
         \"mp3\": \"{mp3}\", \"timings\": \"{stem}.json\", \"duration\": 12.5}}], \"version\": 1, \"slug\": \"my-post\", \
         \"post_updated_at\": \"2026-10-31T10:00:00.000+01:00\", \"default\": \"erzaehler\"}}"
    );
    assert_eq!(
        fs::read_to_string(site.join("manifest.json")).unwrap(),
        want
    );
    assert_eq!(
        fs::read_to_string(site.join(format!("{stem}.json"))).unwrap(),
        "{\"version\": 1, \"voice\": \"erzaehler\", \"duration\": 12.5, \"blocks\": \
         [{\"tag\": \"p\", \"hash\": \"aaaaaaaa\", \"words\": []}]}"
    );
}

fn sha8(data: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    Sha1::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()[..8]
        .to_string()
}

#[test]
fn manifest_names_mp3() {
    let tmp = tempfile::tempdir().unwrap();
    let m = publish(tmp.path(), "erzaehler", "E", b"ogg", b"the mp3");
    let want = format!("erzaehler.{}.mp3", sha8(b"the mp3"));
    assert_eq!(m["voices"][0]["mp3"], want.as_str());
    assert!(tmp.path().join("site").join(want).exists());
}

#[test]
fn republish_same_ogg_new_mp3_keeps_both() {
    let tmp = tempfile::tempdir().unwrap();
    publish(tmp.path(), "erzaehler", "E", b"same", b"mp3-one");
    let m = publish(tmp.path(), "erzaehler", "E", b"same", b"mp3-two");
    let site = tmp.path().join("site");
    assert_eq!(glob(&site, "erzaehler.", ".ogg").len(), 1);
    assert_eq!(glob(&site, "erzaehler.", ".json").len(), 1);
    let mp3s = glob(&site, "erzaehler.", ".mp3");
    assert_eq!(mp3s.len(), 1);
    assert_eq!(fs::read(&mp3s[0]).unwrap(), b"mp3-two");
    assert_eq!(m["voices"][0]["mp3"], name(&mp3s[0]));
    assert!(site
        .join(m["voices"][0]["audio"].as_str().unwrap())
        .exists());
    assert!(site
        .join(m["voices"][0]["timings"].as_str().unwrap())
        .exists());
}

#[test]
fn republish_same_bytes_keeps_files() {
    let tmp = tempfile::tempdir().unwrap();
    publish(tmp.path(), "erzaehler", "Erzähler", b"same", b"msame");
    publish(tmp.path(), "erzaehler", "Erzähler", b"same", b"msame");
    let site = tmp.path().join("site");
    assert_eq!(glob(&site, "erzaehler.", ".ogg").len(), 1);
    assert_eq!(glob(&site, "erzaehler.", ".json").len(), 1);
}

#[test]
fn second_voice_is_appended_and_default_kept() {
    let tmp = tempfile::tempdir().unwrap();
    publish(tmp.path(), "erzaehler", "Erzähler", b"one", b"mone");
    let m = publish_h(
        tmp.path(),
        "zweite-2",
        "Zweite",
        b"two",
        b"mtwo",
        &["aaaaaaaa"],
        "erzaehler",
    )
    .unwrap();
    let ids: Vec<&str> = m["voices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["erzaehler", "zweite-2"]);
    assert_eq!(m["default"], "erzaehler");
    assert_eq!(glob(&tmp.path().join("site"), "", ".ogg").len(), 2);
}

#[test]
fn default_falls_back_to_first_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let m = publish_h(tmp.path(), "b", "B", b"x", b"mx", &["aaaaaaaa"], "nope").unwrap();
    assert_eq!(m["default"], "b");
}

#[test]
fn prefix_voice_ids_do_not_clobber_each_other() {
    let tmp = tempfile::tempdir().unwrap();
    publish(tmp.path(), "a", "A", b"1", b"m1");
    publish(tmp.path(), "a-b", "AB", b"2", b"m2");
    publish(tmp.path(), "a", "A", b"3", b"m3");
    assert_eq!(glob(&tmp.path().join("site"), "a-b.", ".ogg").len(), 1);
}

fn seed_two_voices(tmp: &Path) -> Value {
    publish_h(tmp, "a", "A", b"1", b"m1", &["h1", "h2"], "erzaehler").unwrap();
    publish_h(tmp, "b", "B", b"2", b"m2", &["old1", "old2"], "a").unwrap()
}

fn current() -> Vec<String> {
    vec!["h1".into(), "h2".into()]
}

#[test]
fn prune_drops_mismatched_voice() {
    let tmp = tempfile::tempdir().unwrap();
    let mut m = seed_two_voices(tmp.path());
    let site = tmp.path().join("site");
    let dropped = prune_stale(&site, &mut m, &current(), "a").unwrap();
    assert_eq!(dropped, ["b"]);
    let saved = read_json(&site.join("manifest.json"));
    let ids: Vec<&str> = saved["voices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["a"]);
    for ext in [".ogg", ".mp3", ".json"] {
        assert_eq!(glob(&site, "b.", ext).len(), 1, "{ext} left in place");
    }
}

#[test]
fn prune_drops_voice_with_missing_timings() {
    let tmp = tempfile::tempdir().unwrap();
    let mut m = seed_two_voices(tmp.path());
    let site = tmp.path().join("site");
    fs::remove_file(&glob(&site, "b.", ".json")[0]).unwrap();
    assert_eq!(prune_stale(&site, &mut m, &current(), "a").unwrap(), ["b"]);
}

#[test]
fn prune_keeps_matching_and_resets_default() {
    let tmp = tempfile::tempdir().unwrap();
    let mut m = seed_two_voices(tmp.path());
    let site = tmp.path().join("site");
    m["default"] = json!("b");
    let dropped = prune_stale(&site, &mut m, &current(), "a").unwrap();
    assert_eq!(dropped, ["b"]);
    assert_eq!(m["default"], "a");
    assert_eq!(read_json(&site.join("manifest.json"))["default"], "a");
}

#[test]
fn prune_keeps_everything_when_hashes_match() {
    let tmp = tempfile::tempdir().unwrap();
    let mut m = seed_two_voices(tmp.path());
    let site = tmp.path().join("site");
    let before = fs::read_to_string(site.join("manifest.json")).unwrap();
    let hashes = vec!["old1".to_string(), "old2".to_string()];
    // "a" no longer matches, but it is the voice being kept
    assert!(prune_stale(&site, &mut m, &hashes, "a").unwrap().is_empty());
    assert_eq!(
        fs::read_to_string(site.join("manifest.json")).unwrap(),
        before
    );
}

#[test]
fn invalid_voice_id_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let ogg = tmp.path().join("w.ogg");
    let mp3 = tmp.path().join("w.mp3");
    fs::write(&ogg, b"x").unwrap();
    fs::write(&mp3, b"x").unwrap();
    for bad in ["Bad", "-x", "a/b", "", "../x"] {
        let r = publish_voice(
            &tmp.path().join("site"),
            bad,
            "L",
            &ogg,
            &mp3,
            &timings("x", &[]),
            &post(),
            "x",
        );
        assert!(r.is_err(), "{bad:?} accepted");
    }
    assert!(ogg.exists() && mp3.exists());
}

#[test]
fn manifest_references_existing_files() {
    let tmp = tempfile::tempdir().unwrap();
    publish(tmp.path(), "erzaehler", "E", b"one", b"mone");
    let m = publish(tmp.path(), "erzaehler", "E", b"two", b"mtwo");
    let site = tmp.path().join("site");
    let on_disk = read_json(&site.join("manifest.json"));
    assert_eq!(on_disk, m);
    for v in on_disk["voices"].as_array().unwrap() {
        assert!(site.join(v["audio"].as_str().unwrap()).exists());
        assert!(site.join(v["mp3"].as_str().unwrap()).exists());
        assert!(site.join(v["timings"].as_str().unwrap()).exists());
    }
    assert!(glob(&site, "", ".tmp").is_empty());
}

#[test]
fn old_pair_survives_failed_manifest_write() {
    let tmp = tempfile::tempdir().unwrap();
    publish(tmp.path(), "erzaehler", "E", b"one", b"mone");
    let site = tmp.path().join("site");
    let before = fs::read_to_string(site.join("manifest.json")).unwrap();
    let before_v: Value = serde_json::from_str(&before).unwrap();
    // a directory in the way of the manifest's temp file makes the manifest write fail
    fs::create_dir(site.join("manifest.json.tmp")).unwrap();
    assert!(publish_h(
        tmp.path(),
        "erzaehler",
        "E",
        b"two",
        b"mtwo",
        &["aaaaaaaa"],
        "erzaehler"
    )
    .is_err());
    assert!(site
        .join(before_v["voices"][0]["audio"].as_str().unwrap())
        .exists());
    assert!(site
        .join(before_v["voices"][0]["timings"].as_str().unwrap())
        .exists());
    assert_eq!(
        fs::read_to_string(site.join("manifest.json")).unwrap(),
        before
    );
}

fn write_bytes(data: &'static [u8]) -> impl FnOnce(&[f32], u32, &Path) -> anyhow::Result<()> {
    move |_, _, path| Ok(fs::write(path, data)?)
}

#[test]
fn encode_and_publish_publishes_encoder_output() {
    let tmp = tempfile::tempdir().unwrap();
    let site = tmp.path().join("site");
    let m = encode_and_publish(
        &[0.0; 10],
        24000,
        &site,
        "erzaehler",
        "E",
        &timings("erzaehler", &["h"]),
        &post(),
        "erzaehler",
        write_bytes(b"opus"),
        write_bytes(b"mp3"),
    )
    .unwrap();
    assert_eq!(
        fs::read(site.join(m["voices"][0]["audio"].as_str().unwrap())).unwrap(),
        b"opus"
    );
    assert_eq!(
        fs::read(site.join(m["voices"][0]["mp3"].as_str().unwrap())).unwrap(),
        b"mp3"
    );
    assert!(glob(&site, "", ".tmp.ogg").is_empty());
    assert!(glob(&site, "", ".tmp.mp3").is_empty());
}

#[test]
fn failed_encode_leaves_no_tmp_and_old_pair() {
    let tmp = tempfile::tempdir().unwrap();
    let site = tmp.path().join("site");
    let t = timings("erzaehler", &["h"]);
    encode_and_publish(
        &[0.0; 10],
        24000,
        &site,
        "erzaehler",
        "E",
        &t,
        &post(),
        "erzaehler",
        write_bytes(b"old"),
        write_bytes(b"oldmp3"),
    )
    .unwrap();
    let snapshot = |dir: &Path| -> Vec<(String, Vec<u8>)> {
        let mut v: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let p = e.unwrap().path();
                (name(&p).to_string(), fs::read(&p).unwrap())
            })
            .collect();
        v.sort();
        v
    };
    let before = snapshot(&site);
    assert_eq!(before.len(), 4, "ogg, mp3, json, manifest");

    let failing = |_: &[f32], _: u32, path: &Path| -> anyhow::Result<()> {
        fs::write(path, b"partial")?; // a half-written file, then the encoder dies
        bail!("encoder crashed")
    };
    let err = encode_and_publish(
        &[0.0; 10],
        24000,
        &site,
        "erzaehler",
        "E",
        &t,
        &post(),
        "erzaehler",
        failing,
        write_bytes(b"new"),
    );
    assert!(err.is_err());
    assert!(
        glob(&site, "", ".tmp.ogg").is_empty() && glob(&site, "", ".tmp.mp3").is_empty(),
        "temp file left behind"
    );
    assert_eq!(snapshot(&site), before, "old pair and manifest unchanged");
}

#[test]
fn encode_and_publish_rejects_bad_voice_before_encoding() {
    let tmp = tempfile::tempdir().unwrap();
    let site = tmp.path().join("site");
    let never =
        |_: &[f32], _: u32, _: &Path| -> anyhow::Result<()> { panic!("encoder must not run") };
    let r = encode_and_publish(
        &[0.0; 10],
        24000,
        &site,
        "../x",
        "E",
        &timings("x", &[]),
        &post(),
        "x",
        never,
        never,
    );
    assert!(r.is_err());
}

#[test]
fn failed_mp3_encode_leaves_no_tmp_and_old_set() {
    let tmp = tempfile::tempdir().unwrap();
    let site = tmp.path().join("site");
    let t = timings("erzaehler", &["h"]);
    encode_and_publish(
        &[0.0; 10],
        24000,
        &site,
        "erzaehler",
        "E",
        &t,
        &post(),
        "erzaehler",
        write_bytes(b"old"),
        write_bytes(b"oldmp3"),
    )
    .unwrap();
    let snapshot = |dir: &Path| -> Vec<(String, Vec<u8>)> {
        let mut v: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let p = e.unwrap().path();
                (name(&p).to_string(), fs::read(&p).unwrap())
            })
            .collect();
        v.sort();
        v
    };
    let before = snapshot(&site);
    let failing = |_: &[f32], _: u32, path: &Path| -> anyhow::Result<()> {
        fs::write(path, b"partial")?;
        bail!("mp3 encoder crashed")
    };
    let err = encode_and_publish(
        &[0.0; 10],
        24000,
        &site,
        "erzaehler",
        "E",
        &t,
        &post(),
        "erzaehler",
        write_bytes(b"new"),
        failing,
    );
    assert!(err.is_err());
    assert!(
        glob(&site, "", ".tmp.ogg").is_empty(),
        "ogg temp left behind"
    );
    assert!(
        glob(&site, "", ".tmp.mp3").is_empty(),
        "mp3 temp left behind"
    );
    assert_eq!(snapshot(&site), before, "old set and manifest unchanged");
}
