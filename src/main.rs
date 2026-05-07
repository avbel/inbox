mod cli;
mod config;
mod ephemeral;
mod error;
mod profile;
mod review;
mod rules;
mod snapshot;
mod spawner;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

use clap::Parser;
use cli::{Cli, RecoveryCmd};
use config::Config;
use ephemeral::{EphemeralManager, discard_orphan, restore_orphan, scan_orphaned};
use error::Result;
use rules::{Mode, RuleSet};
use snapshot::resolve_snapshot_dir;

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle recovery subcommands
    if let Some(recovery) = cli.recovery {
        let config = Config::load().unwrap_or_default();
        let snap_dir = resolve_snapshot_dir(
            cli.snapshot_dir.clone(),
            config.settings.snapshot_dir.clone(),
        );
        return match recovery {
            RecoveryCmd::Restore { uuid } => restore_orphan(&snap_dir, &uuid),
            RecoveryCmd::Discard { uuid } => discard_orphan(&snap_dir, &uuid),
        };
    }

    if cli.command.is_empty() {
        eprintln!("error: no command specified");
        std::process::exit(1);
    }

    let config = Config::load().unwrap_or_default();
    let snap_dir = resolve_snapshot_dir(cli.snapshot_dir, config.settings.snapshot_dir.clone());

    // Warn about orphaned snapshots
    for orphan in scan_orphaned(&snap_dir) {
        eprintln!("warning: found unrestored snapshot from a previous run (was inbox killed?)");
        eprintln!("  uuid: {}", orphan.uuid);
        eprintln!(
            "  paths: {}",
            orphan
                .paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        eprintln!(
            "  run `inbox restore {}` to recover, or `inbox discard {}` to clean up",
            orphan.uuid, orphan.uuid
        );
    }

    // Build rule patterns from CLI + optional profile
    let mut patterns: Vec<(String, Mode)> = vec![];

    if let Some(profile_name) = &cli.profile {
        let resolved = profile::resolve_profile(profile_name, &config, &mut vec![])?;
        patterns.extend(resolved);
    }

    for p in &cli.ro {
        patterns.push((p.clone(), Mode::Ro));
    }
    for p in &cli.rw {
        patterns.push((p.clone(), Mode::Rw));
    }
    for p in &cli.ephemeral {
        patterns.push((p.clone(), Mode::Ephemeral));
    }
    for p in &cli.hide {
        patterns.push((p.clone(), Mode::Hide));
    }

    let rules = RuleSet::from_patterns(&patterns)?;
    let cmd = cli.command.join(" ");

    // Set up EphemeralManager if needed
    let mut ephemeral_mgr = if rules.has_ephemeral() || cli.review_ephemeral {
        let mut mgr = EphemeralManager::new(snap_dir);
        let ephemeral_paths: Vec<_> = rules
            .ephemeral_paths()
            .iter()
            .map(|p| p.to_path_buf())
            .collect();
        mgr.before(&ephemeral_paths)?;
        Some(mgr)
    } else {
        None
    };

    setup_sigterm();

    let exit_code = run_backend(&rules, &cmd, cli.review_ephemeral)?;

    if let Some(mut mgr) = ephemeral_mgr.take() {
        if cli.review_ephemeral {
            let diff = mgr.diff()?;
            let kept = review::show_review_tui(exit_code, diff)?;
            mgr.after_review(&kept)?;
        } else {
            mgr.after_silent()?;
        }
    }

    std::process::exit(exit_code);
}

#[cfg(target_os = "macos")]
fn run_backend(rules: &RuleSet, cmd: &str, deny_all: bool) -> Result<i32> {
    macos::MacOsBackend::new().run(rules, cmd, deny_all)
}

#[cfg(target_os = "linux")]
fn run_backend(rules: &RuleSet, cmd: &str, deny_all: bool) -> Result<i32> {
    linux::LinuxBackend::new().run(rules, cmd, deny_all)
}

fn setup_sigterm() {
    // SAFETY: handle_sigterm only calls async-signal-safe functions (process::exit).
    unsafe {
        nix::sys::signal::signal(
            nix::sys::signal::Signal::SIGTERM,
            nix::sys::signal::SigHandler::Handler(handle_sigterm),
        )
        .ok();
    }
}

extern "C" fn handle_sigterm(_: nix::libc::c_int) {
    // Note: process::exit does NOT run Drop — EphemeralManager cleanup is skipped.
    // The snapshot manifest was written before the command ran; use
    // `inbox restore <uuid>` to recover any orphaned snapshots after a SIGTERM.
    std::process::exit(1);
}
