//! Registry of render voices: `voices.json` plus `<id>.wav` / `<id>.txt` per voice.

use std::fs;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use indexmap::IndexMap;
use serde_json::{json, Map, Value};

/// Documentation of the id grammar; see [`is_valid_id`] for the matcher.
pub const VOICE_ID_RE: &str = "^[a-z0-9][a-z0-9-]*$";

/// Mirrors Python's `re.match(VOICE_ID_RE, id)`, where `$` also matches before one trailing `\n`.
pub fn is_valid_id(id: &str) -> bool {
    let id = id.strip_suffix('\n').unwrap_or(id);
    let mut chars = id.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Python `repr()` of a str.
fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::from(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Return voice id -> label in file order; error with a clear message if the registry is broken.
pub fn load_registry(dir: &Path) -> Result<IndexMap<String, String>> {
    let path = dir.join("voices.json");
    if !path.is_file() {
        bail!("voice registry missing: {}", path.display());
    }
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let data: Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let obj = data
        .as_object()
        .ok_or_else(|| anyhow!("{}: top level must be an object", path.display()))?;
    let mut voices = IndexMap::new();
    for (id, entry) in obj {
        if !is_valid_id(id) {
            bail!("invalid voice id in {}: {}", path.display(), py_repr(id));
        }
        for suffix in [".wav", ".txt"] {
            let file = dir.join(format!("{id}{suffix}"));
            if !file.is_file() {
                bail!("voice {}: missing {}", py_repr(id), file.display());
            }
        }
        let label = entry
            .get("label")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("voice {}: missing string 'label'", py_repr(id)))?;
        voices.insert(id.clone(), label.to_string());
    }
    Ok(voices)
}

/// Insert or update a voice entry; new ids are appended, existing ones keep their position.
pub fn add_voice(dir: &Path, id: &str, label: &str) -> Result<()> {
    if !is_valid_id(id) {
        bail!("invalid voice id: {}", py_repr(id));
    }
    let path = dir.join("voices.json");
    let mut data: Map<String, Value> = if path.is_file() {
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?
    } else {
        Map::new()
    };
    data.insert(id.to_string(), json!({ "label": label }));
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    // serde_json pretty output: 2-space indent, ": " separator, raw UTF-8 (= ensure_ascii=False).
    let mut out = serde_json::to_string_pretty(&data)?;
    out.push('\n');
    fs::write(&path, out).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
