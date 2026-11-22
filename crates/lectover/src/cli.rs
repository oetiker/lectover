//! Command line: `lectover render …`, `models fetch`, `voice design` and `dev build|serve`.

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Result};
use clap::{ArgAction, Args, Parser, Subcommand};

use crate::ffi::Device;
use crate::models::{ensure_reuse_gguf, fetch_all, fetch_sources, http_download};
use crate::settings::{Overrides, Settings};

#[derive(Debug, Parser)]
#[command(
    name = "lectover",
    version,
    about = "Render Ghost posts as narrated audio for the read-aloud player"
)]
pub struct Cli {
    /// Config file [default: ~/.config/lectover/config.toml; env LECTOVER_CONFIG]
    #[arg(long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Render a public Ghost post, or a local Markdown/HTML file, in one or more voices into
    /// the site output.
    Render(RenderArgs),
    /// Manage the GGUF model files.
    #[command(subcommand)]
    Models(ModelsCommand),
    /// Manage the reference voices.
    #[command(subcommand)]
    Voice(VoiceCommand),
    /// Manage the render cache (takes, transcripts, alignments).
    #[command(subcommand)]
    Cache(CacheCommand),
    /// Local Ghost-like page for trying the player.
    #[command(subcommand)]
    Dev(DevCommand),
    /// Speak lines of text (after the lexicon) to try pronunciations by ear.
    Say(SayArgs),
    /// Check the installation: config, directories, models, voices, sites, keys, GPU.
    Doctor {
        /// Check only this site [default: every configured site].
        #[arg(long, value_name = "DOMAIN")]
        site: Option<String>,
        /// Skip the checks that need the network.
        #[arg(long)]
        offline: bool,
    },
}

#[derive(Debug, Args)]
pub struct SayArgs {
    /// Lines to speak, one take each, e.g. "Der Sessjonsbericht vom Mai."
    #[arg(value_name = "TEXT")]
    pub lines: Vec<String>,
    /// Voice id; repeat the flag for more voices (default: the first one).
    #[arg(long = "voice", value_name = "ID", action = ArgAction::Append, num_args = 1)]
    pub voices: Vec<String>,
    /// Where the takes go, as <voice>-<n>.wav [default: ~/.local/state/lectover/say].
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
    /// Write the takes without playing them.
    #[arg(long)]
    pub no_play: bool,
    #[arg(long, default_value_t = 42)]
    pub seed: u64,
    /// Pronunciation lexicon applied to the lines [default: the lexicon of --site].
    #[arg(long, value_name = "FILE")]
    pub lexicon: Option<PathBuf>,
    /// Site whose lexicon applies [default: the only configured site; with several, none].
    #[arg(long, value_name = "DOMAIN")]
    pub site: Option<String>,
    /// Voice registry directory [default: ~/.local/share/lectover/voices; env LECTOVER_VOICES]
    #[arg(long = "voices", value_name = "DIR")]
    pub voices_dir: Option<PathBuf>,
    /// TTS compute device (cpu, metal, cuda, hip, vulkan) [default: the first GPU, else the CPU].
    #[arg(long, value_name = "D", value_parser = parse_device)]
    pub device: Option<Device>,
    /// Models directory [default: ~/.cache/lectover/models]
    #[arg(long, value_name = "DIR")]
    pub models: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum VoiceCommand {
    /// Create a reference voice from a description (VoiceDesign model) and register it.
    Design(DesignArgs),
    /// Record a reference voice from the microphone.
    Record(RecordArgs),
    /// Write a RE-USE-cleaned copy of a registered voice beside the original.
    Clean(CleanArgs),
}

#[derive(Debug, Args)]
pub struct CleanArgs {
    /// Id of the registered voice to clean.
    pub id: String,
    /// Id of the new voice [default: <id>reuse].
    #[arg(long = "as", value_name = "ID")]
    pub as_id: Option<String>,
    /// Display label of the new voice [default: "<source label> (RE-USE)"].
    #[arg(long)]
    pub label: Option<String>,
    /// Replace an existing target voice; its old clip and transcript are kept with a timestamp.
    #[arg(long)]
    pub force: bool,
    /// Voice registry directory [default: ~/.local/share/lectover/voices; env LECTOVER_VOICES]
    #[arg(long, value_name = "DIR")]
    pub voices: Option<PathBuf>,
    /// Compute device for the noise cleaner (cpu, metal, cuda, hip, vulkan) [default: the first GPU, else the CPU].
    #[arg(long, value_name = "D", value_parser = parse_device)]
    pub device: Option<Device>,
    /// Models directory [default: ~/.cache/lectover/models]
    #[arg(long, value_name = "DIR")]
    pub models: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct RecordArgs {
    /// Voice id (lowercase letters, digits, dashes): writes <voices>/<id>.wav and .txt.
    pub id: String,
    /// Display label in the player.
    #[arg(long)]
    pub label: String,
    /// Text to read [default: standard reference text].
    #[arg(long)]
    pub text: Option<String>,
    /// Longest take in seconds; it ends earlier when you pause [default: twice the estimate from the text length].
    #[arg(long)]
    pub seconds: Option<u32>,
    /// Replace an existing voice; its old clip, raw take and transcript are kept with a timestamp.
    #[arg(long)]
    pub force: bool,
    /// Voice registry directory [default: ~/.local/share/lectover/voices; env LECTOVER_VOICES]
    #[arg(long, value_name = "DIR")]
    pub voices: Option<PathBuf>,
    /// Compute device for the noise cleaner (cpu, metal, cuda, hip, vulkan) [default: the first GPU, else the CPU].
    #[arg(long, value_name = "D", value_parser = parse_device)]
    pub device: Option<Device>,
    /// Models directory [default: ~/.cache/lectover/models]
    #[arg(long, value_name = "DIR")]
    pub models: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct DesignArgs {
    /// Voice id (lowercase letters, digits, dashes): writes <voices>/<id>.wav and .txt.
    pub id: String,
    /// Display label in the player.
    #[arg(long)]
    pub label: String,
    /// Voice description for the VoiceDesign model, e.g. "Eine ruhige Stimme mittleren Alters …".
    #[arg(long, value_name = "TEXT")]
    pub instruct: String,
    #[arg(long, default_value_t = 42)]
    pub seed: u64,
    #[arg(long, default_value_t = 0.9)]
    pub temperature: f32,
    /// Replace an existing voice; its old clip, raw take and transcript are kept with a timestamp.
    #[arg(long)]
    pub force: bool,
    /// Voice registry directory [default: ~/.local/share/lectover/voices; env LECTOVER_VOICES]
    #[arg(long, value_name = "DIR")]
    pub voices: Option<PathBuf>,
    /// TTS compute device (cpu, metal, cuda, hip, vulkan) [default: the first GPU, else the CPU].
    #[arg(long, value_name = "D", value_parser = parse_device)]
    pub device: Option<Device>,
    /// Models directory [default: ~/.cache/lectover/models]
    #[arg(long, value_name = "DIR")]
    pub models: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum DevCommand {
    /// Write <out>/<domain>/<slug>/index.html for a public post of a configured site, or for a
    /// local Markdown/HTML file.
    Build {
        /// Post URL, slug, or local file (.md, .markdown, .html, .htm).
        post: String,
        #[command(flatten)]
        local: LocalArgs,
        /// Deploy tree [default: ~/.local/share/lectover/site; env LECTOVER_SITE]
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
        /// Site of a slug or local file [default: the only configured site]; a post URL names
        /// its site.
        #[arg(long, value_name = "DOMAIN")]
        site: Option<String>,
    },
    /// Serve the site directory on 127.0.0.1 with Range support; /js answers with the player.
    Serve {
        #[arg(long, default_value_t = 8000)]
        port: u16,
        /// Deploy tree [default: ~/.local/share/lectover/site; env LECTOVER_SITE]
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ModelsCommand {
    /// Download the model files and the RE-USE speech-cleanup weights (converted to GGUF) into the
    /// models directory.
    Fetch {
        /// Models directory [default: ~/.cache/lectover/models]
        #[arg(long, value_name = "DIR")]
        models: Option<PathBuf>,
        /// Also fetch the VoiceDesign talker for `voice design`.
        #[arg(long)]
        design: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum CacheCommand {
    /// Remove cached takes, transcripts and alignments no render has used for a while (the
    /// `takes` directory of the config [default: ~/.cache/lectover/takes]).
    Prune {
        /// Remove what no render has read or written for this many days.
        #[arg(long, value_name = "N", default_value_t = 90)]
        days: u64,
        /// Only report what would go.
        #[arg(long)]
        dry_run: bool,
    },
}

/// Options of a local post file (no Ghost key, no network); rejected for a Ghost post.
#[derive(Debug, Clone, Default, Args)]
pub struct LocalArgs {
    /// Title of a local file [default: its first <h1> / `# heading`, which then leaves the
    /// body; else the file name].
    #[arg(long, value_name = "TEXT")]
    pub title: Option<String>,
    /// Author of a local file, spoken in the intro; repeat the flag for more authors.
    #[arg(long = "author", value_name = "NAME", action = ArgAction::Append, num_args = 1)]
    pub authors: Vec<String>,
    /// Publication date of a local file, spoken in the intro [default: today].
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub date: Option<String>,
    /// Slug of a local file, the output folder <domain>/<slug>/ [default: the file name,
    /// lowercase ASCII with hyphens].
    #[arg(long, value_name = "SLUG")]
    pub slug: Option<String>,
}

impl LocalArgs {
    pub fn meta(&self) -> crate::local::LocalMeta {
        crate::local::LocalMeta {
            title: self.title.clone(),
            authors: self.authors.clone(),
            date: self.date.clone(),
            slug: self.slug.clone(),
        }
    }
}

/// The post a command names: a local file (with its options) or a Ghost post URL or slug.
pub fn local_post(post: &str, local: &LocalArgs) -> Result<Option<crate::ghost::Post>> {
    let meta = local.meta();
    match crate::local::local_path(post) {
        Some(path) => crate::local::load_post(&path, &meta).map(Some),
        None if meta.is_set() => {
            bail!("--title, --author, --date and --slug apply to a local file only, not to {post}")
        }
        None => Ok(None),
    }
}

#[derive(Debug, Args)]
pub struct RenderArgs {
    /// Post URL, slug, or local file (.md, .markdown, .html, .htm) to render without Ghost.
    pub post: String,
    #[command(flatten)]
    pub local: LocalArgs,
    /// Voice id from the voice registry; repeat the flag for more voices
    /// [default: the site's voices, else the registry's first].
    #[arg(long = "voice", value_name = "ID", action = ArgAction::Append, num_args = 1)]
    pub voices: Vec<String>,
    /// Site of a slug or local file [default: the only configured site]; a post URL names
    /// its site.
    #[arg(long, value_name = "DOMAIN")]
    pub site: Option<String>,
    /// Deploy tree; the post goes to <DIR>/<domain>/<slug>/
    /// [default: ~/.local/share/lectover/site; env LECTOVER_SITE]
    #[arg(long, value_name = "DIR")]
    pub out: Option<PathBuf>,
    #[arg(long, default_value_t = 42)]
    pub seed: u64,
    #[arg(long, default_value_t = 0.9)]
    pub temperature: f32,
    /// Sampling temperature of the residual codebooks (timbre, room acoustics, noise).
    /// 0.6 gave the fewest re-rolls in listening tests; 0.3 clips endings.
    #[arg(long, default_value_t = 0.6)]
    pub subtalker_temperature: f32,
    /// Longest chunk (characters) synthesised in one take; longer blocks split at sentences.
    /// 1000 keeps a typical paragraph in one take (one room, one level).
    #[arg(long, default_value_t = 1000)]
    pub max_chars: usize,
    /// Feed the chunk text to the TTS step by step (upstream streaming mode) instead of
    /// putting it all in the prompt; the streaming feed drifts in room and timbre mid-take.
    #[arg(long)]
    pub streaming: bool,
    /// Stretch each chunk towards the post's median speaking rate (±12 %). Off by default:
    /// takes vary by about as much as one reader does between paragraphs, and stretching
    /// neighbours apart is heard as a tempo jump at the paragraph break.
    #[arg(long)]
    pub pace: bool,
    /// Re-synthesise the chunks the tempo report flags (up to 4 new takes each, fresh seeds,
    /// same ASR check) and keep a take within 10 % of its neighbours' rate; regenerates
    /// instead of stretching, so it cannot be combined with --pace.
    #[arg(long, conflicts_with = "pace")]
    pub reroll_tempo: bool,
    /// Accepted for old scripts; not pacing is the default now.
    #[arg(long, hide = true, conflicts_with = "pace")]
    pub no_pace: bool,
    /// Run every model call afresh instead of reusing cached takes, transcripts and alignments
    /// (~/.cache/lectover/takes).
    #[arg(long)]
    pub no_cache: bool,
    /// Takes the TTS decodes together (1 = one at a time). Batched, spare slots re-roll
    /// failing chunks with several seeds at once; takes then also depend on what shares the
    /// batch, so they differ from unbatched ones. 6 halved a long render on an M1 Max.
    #[arg(long, default_value_t = 6, value_name = "N", value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..=64))]
    pub max_batch: usize,
    /// TTS compute device (cpu, metal, cuda, hip, vulkan) [default: the first GPU, else the CPU].
    #[arg(long, value_name = "D", value_parser = parse_device)]
    pub device: Option<Device>,
    /// Models directory [default: ~/.cache/lectover/models]
    #[arg(long, value_name = "DIR")]
    pub models: Option<PathBuf>,
    /// Pronunciation lexicon (`written = spoken` lines); a missing file means none
    /// [default: the site's lexicon, ~/.config/lectover/lexicon/<domain>.txt].
    #[arg(long, value_name = "FILE")]
    pub lexicon: Option<PathBuf>,
    /// The site's sounds: `<DIR>/intro.wav` plays after the spoken intro, `break.wav` at
    /// `<hr>` and `---` in audio-only comments (default: the intro sound)
    /// [default: ~/.local/share/lectover/sites/<domain>].
    #[arg(long, value_name = "DIR")]
    pub sounds: Option<PathBuf>,
}

pub fn parse_device(s: &str) -> Result<Device> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "cpu" => Device::Cpu,
        "metal" => Device::Metal,
        "cuda" => Device::Cuda,
        "hip" => Device::Hip,
        "vulkan" => Device::Vulkan,
        _ => bail!("unknown device {s:?} (cpu, metal, cuda, hip, vulkan)"),
    })
}

/// The path overrides given on the command line, for [`Settings::resolve`].
pub fn overrides(cli: &Cli) -> Overrides {
    let mut o = Overrides {
        config: cli.config.clone(),
        ..Default::default()
    };
    match &cli.command {
        Commands::Render(a) => {
            o.models = a.models.clone();
            o.site = a.out.clone();
        }
        Commands::Models(ModelsCommand::Fetch { models, .. }) => o.models = models.clone(),
        Commands::Voice(VoiceCommand::Design(a)) => {
            (o.models, o.voices) = (a.models.clone(), a.voices.clone())
        }
        Commands::Voice(VoiceCommand::Record(a)) => {
            (o.models, o.voices) = (a.models.clone(), a.voices.clone())
        }
        Commands::Voice(VoiceCommand::Clean(a)) => {
            (o.models, o.voices) = (a.models.clone(), a.voices.clone())
        }
        Commands::Dev(DevCommand::Build { out, .. } | DevCommand::Serve { out, .. }) => {
            o.site = out.clone()
        }
        Commands::Say(a) => (o.models, o.voices) = (a.models.clone(), a.voices_dir.clone()),
        Commands::Doctor { .. } | Commands::Cache(_) => {}
    }
    o
}

/// Run a command; `settings` were resolved from [`overrides`] of the same `cli`.
pub fn run(cli: Cli, settings: &Settings) -> Result<ExitCode> {
    if let Commands::Doctor { site, offline } = &cli.command {
        return Ok(run_doctor(settings, site.as_deref(), *offline));
    }
    run_command(cli, settings).map(|()| ExitCode::SUCCESS)
}

fn run_command(cli: Cli, settings: &Settings) -> Result<()> {
    match cli.command {
        Commands::Render(args) => crate::render::run::render(&args, settings),
        Commands::Models(ModelsCommand::Fetch { design, .. }) => {
            let dir = &settings.models;
            let mut out = io::stdout();
            fetch_all(dir, &fetch_sources(design), &http_download, &mut out)?;
            ensure_reuse_gguf(dir, &crate::ffi::convert_reuse, &mut out)?;
            Ok(())
        }
        Commands::Voice(VoiceCommand::Design(args)) => crate::design::run(&args, settings),
        Commands::Voice(VoiceCommand::Record(args)) => crate::record::run(&args, settings),
        Commands::Voice(VoiceCommand::Clean(args)) => crate::clean::run(&args, settings),
        Commands::Dev(DevCommand::Build {
            post, local, site, ..
        }) => {
            let s = settings.site_for_post(&post, site.as_deref())?;
            let p = match local_post(&post, &local)? {
                Some(p) => p,
                None => {
                    let cfg = crate::ghost::GhostConfig::for_site(&s, &|k| std::env::var(k).ok())?;
                    crate::dev::fetch(&cfg, &post, &crate::ghost::http_get)?
                }
            };
            let page = crate::dev::write_page(&p, &s.domain, &settings.site_root)?;
            println!("wrote {}", page.display());
            let stale = crate::dev::stale_voices(&p, &s.domain, &settings.site_root)?;
            if let Some(w) = crate::dev::stale_warning(&stale) {
                eprintln!("{w}");
            }
            Ok(())
        }
        Commands::Say(args) => crate::say::run(&args, settings),
        Commands::Cache(CacheCommand::Prune { days, dry_run }) => {
            let r = crate::render::prune(
                &settings.takes,
                std::time::Duration::from_secs(days * 86_400),
                dry_run,
            )?;
            let mb = |b: u64| b as f64 / 1e6;
            let verb = if dry_run { "would remove" } else { "removed" };
            println!(
                "{}: {verb} {} file(s), {:.1} MB unused for {days} day(s); kept {} file(s), {:.1} MB",
                settings.takes.display(),
                r.removed,
                mb(r.removed_bytes),
                r.kept,
                mb(r.kept_bytes)
            );
            Ok(())
        }
        Commands::Doctor { .. } => unreachable!("handled in run"),
        Commands::Dev(DevCommand::Serve { port, .. }) => crate::dev::serve(
            &settings.site_root,
            std::path::Path::new(crate::dev::PLAYER),
            port,
        ),
    }
}

/// Prints the doctor report and returns its exit code.
fn run_doctor(settings: &Settings, site: Option<&str>, offline: bool) -> ExitCode {
    use crate::doctor::{checks, exit_code, render_report, Probe};
    let env = |k: &str| std::env::var(k).ok();
    let probe = Probe {
        env: &env,
        http_get: &crate::ghost::http_get,
        free_bytes: &free_bytes,
    };
    let results = checks(settings, site, offline, &probe);
    print!("{}", render_report(&results));
    ExitCode::from(exit_code(&results))
}

/// Bytes available to unprivileged users on the file system holding `path`.
fn free_bytes(path: &std::path::Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0)
        .then(|| st.f_bavail as u64 * st.f_frsize as u64)
}
