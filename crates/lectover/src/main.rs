//! Lectover CLI.

use std::process::ExitCode;

use clap::Parser;
use lectover::cli::{overrides, run, Cli, Commands};
use lectover::settings::Settings;

fn main() -> ExitCode {
    // `--help` and `--version` exit here, before any device check.
    let cli = Cli::parse();
    // Before any ggml call: ggml-vulkan reads its device list once.
    if cli.command.uses_gpu() {
        lectover::ffi::vkpick::limit_vulkan_devices();
    }
    // `run` owns every model handle and drops it before returning (ggml Metal aborts on handles
    // still alive at exit), so exiting here is safe.
    if let Err(e) = lectover::ffi::check_cpu_baseline() {
        eprintln!("error: {e:#}");
        return ExitCode::FAILURE;
    }
    lectover::ffi::init_ggml_logging();
    // The device test of a parent lectover: no config, no settings.
    if matches!(cli.command, Commands::VkProbe) {
        return match lectover::ffi::vkprobe::run_probe() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e:#}");
                ExitCode::FAILURE
            }
        };
    }
    lectover::ffi::warn_unified_memory();
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
