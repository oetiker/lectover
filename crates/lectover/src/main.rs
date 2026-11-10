//! Lectover CLI.

use std::process::ExitCode;

use clap::Parser;
use lectover::cli::{overrides, run, Cli, Commands};
use lectover::settings::Settings;

fn main() -> ExitCode {
    // `run` owns every model handle and drops it before returning (ggml Metal aborts on handles
    // still alive at exit), so exiting here is safe.
    lectover::ffi::init_ggml_logging();
    lectover::ffi::warn_unified_memory();
    let cli = Cli::parse();
    let doctor = matches!(cli.command, Commands::Doctor { .. });
    let env = |k: &str| std::env::var_os(k);
    // Doctor reports a broken config and the config warnings itself.
    let resolved = if doctor {
        Settings::resolve_for_doctor(&overrides(&cli), &env)
    } else {
        Settings::resolve(&overrides(&cli), &env)
    };
    let result = resolved.and_then(|settings| {
        if !doctor {
            for w in settings.config.iter().flat_map(|l| &l.warnings) {
                eprintln!("warning: {w}");
            }
        }
        run(cli, &settings)
    });
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
