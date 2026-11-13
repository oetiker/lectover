//! Port of `site_output.py`: hashed Opus files, manifest.json upkeep and stale-voice pruning.

use std::fs;
use std::io;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::ser::Formatter;
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};

use crate::ghost::Post;
use crate::voices::is_valid_id;

/// serde_json formatter that writes like Python's `json.dumps(doc, ensure_ascii=False)`:
/// `", "` and `": "` separators and floats in Python `repr` form.
struct PyFormatter;

impl Formatter for PyFormatter {
    fn begin_array_value<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            w.write_all(b", ")
        }
    }

    fn begin_object_key<W: ?Sized + io::Write>(
        &mut self,
        w: &mut W,
        first: bool,
    ) -> io::Result<()> {
        if first {
            Ok(())
        } else {
            w.write_all(b", ")
        }
    }

    fn begin_object_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
        w.write_all(b": ")
    }

    fn write_f64<W: ?Sized + io::Write>(&mut self, w: &mut W, value: f64) -> io::Result<()> {
        w.write_all(py_float_repr(value).as_bytes())
    }
}

/// Python's `repr(float)`: shortest round-trip digits, scientific below 1e-4 and from 1e16.
fn py_float_repr(x: f64) -> String {
    if !x.is_finite() {
        return if x.is_nan() {
            "NaN"
        } else if x > 0.0 {
            "Infinity"
        } else {
            "-Infinity"
        }
        .into();
    }
    let sci = format!("{x:e}"); // e.g. "-1.2345e-5", shortest round-trip digits
    let (mantissa, exp) = sci.split_once('e').expect("LowerExp has an exponent");
    let exp: i32 = exp.parse().expect("integer exponent");
    if (-4..16).contains(&exp) {
        let fixed = format!("{x}"); // Display: shortest round-trip, never scientific
        if fixed.contains('.') {
            fixed
        } else {
            format!("{fixed}.0")
        }
    } else {
        format!(
            "{mantissa}e{}{:02}",
            if exp < 0 { '-' } else { '+' },
            exp.abs()
        )
    }
}

/// `json.dumps(doc, ensure_ascii=False)`.
pub fn to_python_json(doc: &Value) -> String {
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, PyFormatter);
    doc.serialize(&mut ser)
        .expect("serialising a Value to memory cannot fail");
    String::from_utf8(buf).expect("serde_json writes UTF-8")
}

fn write_json_with(
    path: &Path,
    doc: &Value,
    rename: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> Result<()> {
    let mut tmp_name = path
        .file_name()
        .ok_or_else(|| anyhow!("no file name: {}", path.display()))?
        .to_owned();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    let result = fs::write(&tmp, to_python_json(doc))
        .with_context(|| format!("writing {}", tmp.display()))
        .and_then(|()| rename(&tmp, path).with_context(|| format!("replacing {}", path.display())));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Write atomically: temp file in the same directory, then rename over `path`.
pub fn write_json(path: &Path, doc: &Value) -> Result<()> {
    write_json_with(path, doc, |from, to| fs::rename(from, to))
}

/// `<voice>.<8 hex>.(ogg|mp3|json)`.
fn is_voice_pair_file(name: &str, voice: &str) -> bool {
    let Some(rest) = name.strip_prefix(voice).and_then(|r| r.strip_prefix('.')) else {
        return false;
    };
    match (rest.get(..8), rest.get(8..)) {
        (Some(hash), Some(ext)) => {
            hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                && matches!(ext, ".ogg" | ".mp3" | ".json")
        }
        _ => false,
    }
}

fn check_voice_id(voice: &str) -> Result<()> {
    if !is_valid_id(voice) {
        bail!("invalid voice id: {voice:?}");
    }
    Ok(())
}

/// First 8 hex digits of the SHA-1 of `bytes`.
fn short_digest(bytes: &[u8]) -> String {
    Sha1::digest(bytes)
        .iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Where a voice's files go and how the manifest lists it.
pub struct VoiceTarget<'a> {
    /// The post's output directory, holding manifest.json and the voices' files.
    pub slug_dir: &'a Path,
    /// Voice id; prefixes the file names and is the manifest entry's `id`.
    pub voice: &'a str,
    /// Display label of the manifest entry.
    pub label: &'a str,
    /// Voice id the manifest names as `default`; if the manifest has no such voice,
    /// its first voice becomes the default.
    pub default: &'a str,
    /// The post whose slug and update time go into the manifest.
    pub post: &'a Post,
}

/// Move the rendered Ogg and MP3 into `target.slug_dir` under content hashes and update
/// manifest.json.
///
/// Order: both files renamed into place, timings written, manifest rewritten atomically, and
/// only then the voice's previous files removed. The timings file is named by the Ogg digest.
pub fn publish_voice(
    target: &VoiceTarget,
    ogg_tmp: &Path,
    mp3_tmp: &Path,
    timings: &Value,
) -> Result<Value> {
    let VoiceTarget {
        slug_dir,
        voice,
        label,
        default,
        post,
    } = *target;
    check_voice_id(voice)?;
    fs::create_dir_all(slug_dir).with_context(|| format!("creating {}", slug_dir.display()))?;
    let read = |p: &Path| fs::read(p).with_context(|| format!("reading {}", p.display()));
    let ogg_digest = short_digest(&read(ogg_tmp)?);
    let mp3_digest = short_digest(&read(mp3_tmp)?);
    let audio = format!("{voice}.{ogg_digest}.ogg");
    let mp3 = format!("{voice}.{mp3_digest}.mp3");
    let tfile = format!("{voice}.{ogg_digest}.json");

    for (tmp, name) in [(ogg_tmp, &audio), (mp3_tmp, &mp3)] {
        fs::rename(tmp, slug_dir.join(name))
            .with_context(|| format!("moving {} into {}", tmp.display(), slug_dir.display()))?;
    }
    write_json(&slug_dir.join(&tfile), timings)?;

    let mpath = slug_dir.join("manifest.json");
    let mut manifest: Map<String, Value> = if mpath.exists() {
        let text =
            fs::read_to_string(&mpath).with_context(|| format!("reading {}", mpath.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", mpath.display()))?
    } else {
        let mut m = Map::new();
        m.insert("voices".into(), json!([]));
        m
    };
    let duration = timings
        .get("duration")
        .cloned()
        .ok_or_else(|| anyhow!("timings without duration"))?;
    let entry = json!({"id": voice, "label": label, "audio": audio, "mp3": mp3, "timings": tfile, "duration": duration});
    let mut voices = match manifest.get("voices") {
        Some(Value::Array(v)) => v.clone(),
        Some(_) => bail!("{}: 'voices' is not a list", mpath.display()),
        None => Vec::new(),
    };
    match voices.iter_mut().find(|v| v["id"] == voice) {
        Some(slot) => *slot = entry,
        None => voices.push(entry),
    }
    let default = if voices.iter().any(|v| v["id"] == default) {
        default.to_string()
    } else {
        voices[0]["id"].as_str().unwrap_or_default().to_string()
    };
    // dict.update order: existing keys keep their place, new ones are appended in this order
    manifest.insert("version".into(), json!(1));
    manifest.insert("slug".into(), json!(post.slug));
    manifest.insert("post_updated_at".into(), json!(post.updated_at));
    manifest.insert("voices".into(), Value::Array(voices));
    manifest.insert("default".into(), json!(default));
    let manifest = Value::Object(manifest);
    write_json(&mpath, &manifest)?;

    // Only after the manifest points at the new set, remove the old files of this voice.
    for e in fs::read_dir(slug_dir).with_context(|| format!("listing {}", slug_dir.display()))? {
        let e = e?;
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if e.file_type()?.is_file()
            && is_voice_pair_file(name, voice)
            && name != audio
            && name != mp3
            && name != tfile
        {
            fs::remove_file(e.path())
                .with_context(|| format!("removing {}", e.path().display()))?;
        }
    }
    Ok(manifest)
}

/// Encode `audio` to `<voice>.tmp.ogg` and `<voice>.tmp.mp3` in `target.slug_dir` and
/// publish both.
///
/// On any error both temp files are removed, so the published set and manifest stay as they were.
pub fn encode_and_publish(
    audio: &[f32],
    sr: u32,
    target: &VoiceTarget,
    timings: &Value,
    ogg_encoder: impl FnOnce(&[f32], u32, &Path) -> Result<()>,
    mp3_encoder: impl FnOnce(&[f32], u32, &Path) -> Result<()>,
) -> Result<Value> {
    let VoiceTarget {
        slug_dir, voice, ..
    } = *target;
    check_voice_id(voice)?; // before the id becomes part of a path
    fs::create_dir_all(slug_dir).with_context(|| format!("creating {}", slug_dir.display()))?;
    let ogg_tmp = slug_dir.join(format!("{voice}.tmp.ogg"));
    let mp3_tmp = slug_dir.join(format!("{voice}.tmp.mp3"));
    let result = ogg_encoder(audio, sr, &ogg_tmp)
        .with_context(|| format!("encoding {}", ogg_tmp.display()))
        .and_then(|()| {
            mp3_encoder(audio, sr, &mp3_tmp)
                .with_context(|| format!("encoding {}", mp3_tmp.display()))
        })
        .and_then(|()| publish_voice(target, &ogg_tmp, &mp3_tmp, timings));
    if result.is_err() {
        for tmp in [&ogg_tmp, &mp3_tmp] {
            let _ = fs::remove_file(tmp);
        }
    }
    result
}

/// Drop voices (except `keep`) whose timings no longer match the post's block hashes.
pub fn prune_stale(
    slug_dir: &Path,
    manifest: &mut Value,
    current_hashes: &[String],
    keep: &str,
) -> Result<Vec<String>> {
    let current: Vec<Value> = current_hashes.iter().map(|h| json!(h)).collect();
    let voices = manifest
        .get_mut("voices")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("manifest without a 'voices' list"))?;
    let mut dropped = Vec::new();
    let mut kept = Vec::with_capacity(voices.len());
    for v in voices.drain(..) {
        let id = v["id"].as_str().unwrap_or_default().to_string();
        if id == keep {
            kept.push(v);
            continue;
        }
        let tfile = v["timings"]
            .as_str()
            .ok_or_else(|| anyhow!("voice {id}: no timings file"))?;
        let hashes: Option<Vec<Value>> = fs::read_to_string(slug_dir.join(tfile))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|doc| {
                doc.get("blocks")?
                    .as_array()?
                    .iter()
                    .map(|b| b.get("hash").cloned())
                    .collect()
            });
        if hashes.as_deref() == Some(&current[..]) {
            kept.push(v);
        } else {
            println!("voice {id} no longer matches the post text; re-render it");
            dropped.push(id);
        }
    }
    *voices = kept;
    if !dropped.is_empty() {
        if manifest
            .get("default")
            .and_then(Value::as_str)
            .is_some_and(|d| dropped.iter().any(|x| x == d))
        {
            manifest["default"] = json!(keep);
        }
        write_json(&slug_dir.join("manifest.json"), manifest)?;
    }
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_write_is_atomic() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("m.json");
        fs::write(&target, r#"{"ok": 1}"#).unwrap();
        let boom = |_: &Path, _: &Path| Err(io::Error::other("replace failed"));
        assert!(write_json_with(&target, &json!({"new": 2}), boom).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), r#"{"ok": 1}"#);
        assert!(
            !tmp.path().join("m.json.tmp").exists(),
            "temp file cleaned up"
        );
    }

    #[test]
    fn floats_match_python_repr() {
        for (x, want) in [
            (12.5, "12.5"),
            (2.0, "2.0"),
            (0.08, "0.08"),
            (123.115, "123.115"),
            (0.0001, "0.0001"),
            (1e-5, "1e-05"),
            (1.5e-7, "1.5e-07"),
            (1e16, "1e+16"),
            (1234567890123456.0, "1234567890123456.0"),
            (-0.0, "-0.0"),
        ] {
            assert_eq!(py_float_repr(x), want, "{x}");
        }
    }

    #[test]
    fn python_json_separators_and_unicode() {
        let doc = json!({"a": [1, 2.0, "Ä\n\u{1}"], "b": {}});
        assert_eq!(
            to_python_json(&doc),
            "{\"a\": [1, 2.0, \"Ä\\n\\u0001\"], \"b\": {}}"
        );
    }
}
