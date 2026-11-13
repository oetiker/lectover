//! The four GGUF model files the renderer needs (plus the VoiceDesign talker of
//! `voice design`): where they live, which are missing, and `models fetch`.
//!
//! File names and sizes are the `ModelFiles` constants below.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

pub const TALKER: &str = "qwen-talker-1.7b-base-BF16.gguf";
pub const CODEC: &str = "qwen-tokenizer-12hz-F32.gguf";
pub const ASR: &str = "qwen3-asr-0.6b-f16.gguf";
pub const ALIGNER: &str = "qwen3-forced-aligner-0.6b-f16.gguf";
/// VoiceDesign talker for `voice design` (model type `voice_design`); shares the codec.
pub const DESIGN_TALKER: &str = "qwen-talker-1.7b-voicedesign-BF16.gguf";

/// Where a model file comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A pre-converted GGUF on Hugging Face, pinned to a revision.
    HuggingFace {
        repo: &'static str,
        revision: &'static str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelSource {
    /// File name at the origin.
    pub file: &'static str,
    /// File name in the models directory (differs from `file` when the remote name is generic).
    pub local: &'static str,
    /// Expected size in bytes.
    pub size: u64,
    pub origin: Origin,
}

/// Revision of `Serveurperso/Qwen3-TTS-GGUF` the sizes below were taken from.
const TTS_REVISION: &str = "b7ee2e8c7459c3bea99da23e3d178125a7d1713c";
/// Revision of `megaice/Qwen3-ASR-GGUF-Models` the sizes below were taken from.
const ASR_REVISION: &str = "ec843129c9f0b1760aa231fad5f83dbc39e1efa9";

/// The models the renderer loads, with their origins.
pub const MODEL_SOURCES: [ModelSource; 4] = [
    ModelSource {
        file: TALKER,
        local: TALKER,
        size: 3_876_199_616,
        origin: Origin::HuggingFace {
            repo: "Serveurperso/Qwen3-TTS-GGUF",
            revision: TTS_REVISION,
        },
    },
    ModelSource {
        file: CODEC,
        local: CODEC,
        size: 647_263_104,
        origin: Origin::HuggingFace {
            repo: "Serveurperso/Qwen3-TTS-GGUF",
            revision: TTS_REVISION,
        },
    },
    ModelSource {
        file: ASR,
        local: ASR,
        size: 1_882_543_424,
        origin: Origin::HuggingFace {
            repo: "megaice/Qwen3-ASR-GGUF-Models",
            revision: ASR_REVISION,
        },
    },
    ModelSource {
        file: ALIGNER,
        local: ALIGNER,
        size: 1_842_214_208,
        origin: Origin::HuggingFace {
            repo: "megaice/Qwen3-ASR-GGUF-Models",
            revision: ASR_REVISION,
        },
    },
];

/// Local name of the RE-USE weights (the remote file is a generic `model.safetensors`).
pub const REUSE_SAFETENSORS: &str = "reuse-model.safetensors";
/// RE-USE weights converted to GGUF (f32) by `reuse_convert`.
pub const REUSE_GGUF: &str = "reuse-f32.gguf";

/// Revision of `nvidia/NVIDIA-NemotronLabs-RE-USE` the size below was taken from.
const REUSE_REVISION: &str = "31f82f2efad1095034492f236e8dac9239ba713b";

/// Speech-cleanup weights; fetched by default and converted once to [`REUSE_GGUF`].
pub const REUSE_SOURCE: ModelSource = ModelSource {
    file: "model.safetensors",
    local: REUSE_SAFETENSORS,
    size: 38_592_940,
    origin: Origin::HuggingFace {
        repo: "nvidia/NVIDIA-NemotronLabs-RE-USE",
        revision: REUSE_REVISION,
    },
};

/// What `models fetch` downloads: render models and RE-USE weights, plus the VoiceDesign talker
/// with `design`.
pub fn fetch_sources(design: bool) -> Vec<ModelSource> {
    let mut sources = MODEL_SOURCES.to_vec();
    sources.push(REUSE_SOURCE);
    if design {
        sources.extend(DESIGN_SOURCES);
    }
    sources
}

/// Fetched only by `models fetch --design`: the render never loads it.
pub const DESIGN_SOURCES: [ModelSource; 1] = [ModelSource {
    file: DESIGN_TALKER,
    local: DESIGN_TALKER,
    size: 3_839_585_184,
    origin: Origin::HuggingFace {
        repo: "Serveurperso/Qwen3-TTS-GGUF",
        revision: TTS_REVISION,
    },
}];

/// Paths of the four model files, all known to exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFiles {
    pub talker: PathBuf,
    pub codec: PathBuf,
    pub asr: PathBuf,
    pub aligner: PathBuf,
}

/// Error naming every one of `files` missing in `dir` and the `fetch` command that gets them.
fn require_files(dir: &Path, files: &[&str], fetch: &str) -> Result<()> {
    let missing: Vec<PathBuf> = files
        .iter()
        .map(|f| dir.join(f))
        .filter(|p| !p.is_file())
        .collect();
    if !missing.is_empty() {
        let list: Vec<String> = missing
            .iter()
            .map(|p| format!("  {}", p.display()))
            .collect();
        bail!(
            "model files missing:\n{}\nrun `{fetch} --models {}` to get them",
            list.join("\n"),
            dir.display()
        );
    }
    Ok(())
}

impl ModelFiles {
    /// Error naming every missing file and the command that fetches them.
    pub fn check(dir: &Path) -> Result<ModelFiles> {
        let files: Vec<&str> = MODEL_SOURCES.iter().map(|s| s.file).collect();
        require_files(dir, &files, "lectover models fetch")?;
        Ok(ModelFiles {
            talker: dir.join(TALKER),
            codec: dir.join(CODEC),
            asr: dir.join(ASR),
            aligner: dir.join(ALIGNER),
        })
    }
}

/// Paths of the two model files `voice design` loads, both known to exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesignFiles {
    pub talker: PathBuf,
    pub codec: PathBuf,
}

impl DesignFiles {
    /// Error naming every missing file and `models fetch --design`.
    pub fn check(dir: &Path) -> Result<DesignFiles> {
        require_files(
            dir,
            &[DESIGN_TALKER, CODEC],
            "lectover models fetch --design",
        )?;
        Ok(DesignFiles {
            talker: dir.join(DESIGN_TALKER),
            codec: dir.join(CODEC),
        })
    }
}

/// Path of the RE-USE weights, known to exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanFiles {
    pub reuse: PathBuf,
}

impl CleanFiles {
    /// Error naming the missing file and the command that fetches it.
    pub fn check(dir: &Path) -> Result<CleanFiles> {
        require_files(dir, &[REUSE_SAFETENSORS], "lectover models fetch")?;
        Ok(CleanFiles {
            reuse: dir.join(REUSE_SAFETENSORS),
        })
    }
}

/// Path of the RE-USE GGUF in `dir`, converting it from the safetensors weights first when it
/// is missing. `convert(safetensors, gguf)` writes to `<gguf>.part`, renamed once it succeeds,
/// so a failed conversion leaves no GGUF behind.
pub fn ensure_reuse_gguf(
    dir: &Path,
    convert: &dyn Fn(&Path, &Path) -> Result<()>,
    log: &mut dyn Write,
) -> Result<PathBuf> {
    let gguf = dir.join(REUSE_GGUF);
    if gguf.is_file() {
        return Ok(gguf);
    }
    let clean = CleanFiles::check(dir)?;
    let mut part = gguf.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    writeln!(
        log,
        "converting {} to {}",
        clean.reuse.display(),
        gguf.display()
    )?;
    if let Err(e) = convert(&clean.reuse, &part) {
        let _ = fs::remove_file(&part);
        return Err(e.context(format!("converting {}", clean.reuse.display())));
    }
    fs::rename(&part, &gguf)
        .with_context(|| format!("renaming {} to {}", part.display(), gguf.display()))?;
    writeln!(log, "{}", gguf.display())?;
    Ok(gguf)
}

/// An HTTP response to a (possibly ranged) GET.
pub struct Download {
    pub status: u16,
    pub content_length: Option<u64>,
    pub body: Box<dyn Read>,
}

/// Performs a GET of `url`, asking for the bytes from `offset` on when `offset > 0`.
pub type Get<'a> = &'a dyn Fn(&str, u64) -> Result<Download>;

fn hf_url(repo: &str, revision: &str, file: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/{revision}/{file}")
}

/// Fetch every model of `sources` not yet in `dir`. Downloads go to `<name>.part` (resumed with
/// a range request on the next run) and are renamed once their size matches.
pub fn fetch_all(dir: &Path, sources: &[ModelSource], get: Get, log: &mut dyn Write) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    for src in sources {
        let target = dir.join(src.local);
        let Origin::HuggingFace { repo, revision } = src.origin;
        fetch_one(
            &target,
            &hf_url(repo, revision, src.file),
            src.size,
            get,
            log,
        )?;
    }
    Ok(())
}

fn fetch_one(target: &Path, url: &str, size: u64, get: Get, log: &mut dyn Write) -> Result<()> {
    if target.is_file() {
        let have = fs::metadata(target)?.len();
        if have != size {
            bail!(
                "{} has {have} bytes, expected {size}; remove it and run `lectover models fetch` again",
                target.display()
            );
        }
        writeln!(log, "{}", target.display())?;
        return Ok(());
    }
    let mut part = target.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let mut offset = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if offset > size {
        offset = 0; // not a prefix of this file: start over
    }
    if offset < size {
        if offset > 0 {
            writeln!(
                log,
                "resuming {url} at {} of {} MB",
                offset / 1_000_000,
                size / 1_000_000
            )?;
        } else {
            writeln!(log, "downloading {url} ({} MB)", size / 1_000_000)?;
        }
        let mut resp = get(url, offset).with_context(|| format!("downloading {url}"))?;
        let mut file = match resp.status {
            206 if offset > 0 => OpenOptions::new().append(true).open(&part)?,
            200 => {
                offset = 0; // full body: the server ignored (or was not sent) the range
                File::create(&part)?
            }
            code => bail!("HTTP {code} for {url}"),
        };
        let expected = size - offset;
        if let Some(n) = resp.content_length {
            if n != expected {
                bail!("{url}: Content-Length {n}, expected {expected} of {size} bytes");
            }
        }
        let copied = io::copy(&mut resp.body.as_mut().take(expected), &mut file)
            .with_context(|| format!("writing {}", part.display()))?;
        file.sync_all()?;
        if copied != expected {
            bail!(
                "{url}: got {} of {size} bytes; run `lectover models fetch` again to resume {}",
                offset + copied,
                part.display()
            );
        }
    }
    let got = fs::metadata(&part)?.len();
    if got != size {
        return Err(anyhow!("{}: {got} bytes, expected {size}", part.display()));
    }
    fs::rename(&part, target)
        .with_context(|| format!("renaming {} to {}", part.display(), target.display()))?;
    writeln!(log, "{}", target.display())?;
    Ok(())
}

/// Real HTTPS GET for [`fetch_all`]: follows redirects (HF serves files from a CDN), sends a
/// `Range` header when resuming. No overall timeout: the talker is 3.9 GB.
pub fn http_download(url: &str, offset: u64) -> Result<Download> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.get(url);
    if offset > 0 {
        req = req.header("Range", format!("bytes={offset}-"));
    }
    let resp = req.call().map_err(|e| anyhow!("{e}"))?;
    let status = resp.status().as_u16();
    let content_length = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let body = resp.into_body().into_with_config().limit(u64::MAX).reader();
    Ok(Download {
        status,
        content_length,
        body: Box::new(body),
    })
}
