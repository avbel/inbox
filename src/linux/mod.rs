use crate::error::{InboxError, Result};
use crate::rules::{Mode, RuleSet};
use landlock::{
    ABI, Access, AccessFs, BitFlags, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset,
    RulesetAttr, RulesetCreatedAttr, RulesetStatus,
};
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::{ForkResult, fork};
use std::os::unix::process::CommandExt;

pub struct LinuxBackend;

impl LinuxBackend {
    pub fn new() -> Self {
        Self
    }

    pub fn run(&self, rules: &RuleSet, cmd: &str, deny_all: bool) -> Result<i32> {
        warn_unsupported_escalations(rules);
        spawn_with_landlock(rules, cmd, deny_all)
    }
}

/// Warn when --rw or --ephemeral is nested under --ro or --hide.
/// Landlock uses intersection semantics: a parent restriction cannot be overridden
/// by a more-permissive child rule. This is a known limitation vs. macOS SBPL.
fn warn_unsupported_escalations(rules: &RuleSet) {
    for parent in rules.rules() {
        if !matches!(parent.mode, Mode::Ro | Mode::Hide) {
            continue;
        }
        for child in rules.rules() {
            if !matches!(child.mode, Mode::Rw | Mode::Ephemeral) {
                continue;
            }
            if child.path.starts_with(&parent.path) && child.path != parent.path {
                let parent_flag = if parent.mode == Mode::Ro {
                    "ro"
                } else {
                    "hide"
                };
                eprintln!(
                    "warning: --rw/--ephemeral {:?} is under --{} {:?}; \
                     Landlock cannot grant more access than a parent rule allows — \
                     write access to the subpath will remain denied",
                    child.path, parent_flag, parent.path
                );
            }
        }
    }
}

fn spawn_with_landlock(rules: &RuleSet, cmd: &str, deny_all: bool) -> Result<i32> {
    // SAFETY: single-threaded at this point; no async-signal-unsafe operations
    // occur between fork and exec in the child path.
    match unsafe { fork() }? {
        ForkResult::Child => {
            if let Err(e) = apply_landlock(rules, deny_all) {
                eprintln!("inbox: landlock setup failed: {e}");
                std::process::exit(1);
            }
            let err = std::process::Command::new("/bin/sh")
                .args(["-c", cmd])
                .exec();
            eprintln!("inbox: exec failed: {err}");
            std::process::exit(127);
        }
        ForkResult::Parent { child } => match waitpid(child, None)? {
            WaitStatus::Exited(_, code) => Ok(code),
            WaitStatus::Signaled(_, _, _) => Ok(1),
            _ => Ok(1),
        },
    }
}

fn apply_landlock(rules: &RuleSet, deny_all: bool) -> Result<()> {
    // V4 covers all access types available as of kernel 6.1. set_best_effort(true)
    // silently downgrades to what the running kernel actually supports.
    let abi = ABI::V4;
    let all: BitFlags<AccessFs> = Access::from_all(abi);
    let read_only: BitFlags<AccessFs> = AccessFs::ReadFile | AccessFs::ReadDir;
    let no_access: BitFlags<AccessFs> = BitFlags::empty();

    // set_best_effort must be called on RulesetCreated (after .create()), not on Ruleset.
    let mut ruleset = Ruleset::default()
        .handle_access(all)
        .map_err(|e| InboxError::Io(std::io::Error::other(e.to_string())))?
        .create()
        .map_err(|e| InboxError::Io(std::io::Error::other(e.to_string())))?
        .set_compatibility(CompatLevel::BestEffort);

    if !deny_all {
        // Allow-all baseline: grant full access to the root.
        // Per-path rules intersect with this via Landlock's AND semantics, so
        // --ro /dir restricts writes under /dir while everything else stays open.
        let root_fd =
            PathFd::new("/").map_err(|e| InboxError::Io(std::io::Error::other(e.to_string())))?;
        // PathBeneath::new returns PathBeneath<PathFd> directly (not a Result).
        let root_rule = PathBeneath::new(root_fd, all);
        ruleset = ruleset
            .add_rule(root_rule)
            .map_err(|e| InboxError::Io(std::io::Error::other(e.to_string())))?;
    }
    // In deny_all mode we add NO root rule — everything is denied unless explicitly
    // allowed by an Rw/Ephemeral rule below.

    for rule in rules.rules() {
        let access = match rule.mode {
            Mode::Ro => read_only,
            Mode::Hide => no_access,
            // Rw/Ephemeral: full access (explicit allow needed in deny_all mode)
            Mode::Rw | Mode::Ephemeral => all,
        };

        // In allow-all mode, Ro/Hide intersect with the root rule to restrict subtrees.
        // In deny_all mode, only Rw/Ephemeral rules add explicit allows; Ro/Hide stay denied.
        if deny_all && matches!(rule.mode, Mode::Ro | Mode::Hide) {
            continue;
        }

        let fd = match PathFd::new(&rule.path) {
            Ok(fd) => fd,
            Err(e) => {
                eprintln!("warning: cannot open {:?} for landlock: {e}", rule.path);
                continue;
            }
        };
        let path_rule = PathBeneath::new(fd, access);
        ruleset = ruleset
            .add_rule(path_rule)
            .map_err(|e| InboxError::Io(std::io::Error::other(e.to_string())))?;
    }

    let status = ruleset
        .restrict_self()
        .map_err(|e| InboxError::Io(std::io::Error::other(e.to_string())))?;

    if status.ruleset == RulesetStatus::NotEnforced {
        eprintln!(
            "warning: Landlock not supported by this kernel — \
             running without filesystem restrictions"
        );
    }

    Ok(())
}
