use crate::error::{InboxError, Result};
use crate::rules::{Mode, RuleSet};
use landlock::{
    ABI, Access, AccessFs, BitFlags, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset,
    RulesetAttr, RulesetCreatedAttr, RulesetStatus,
};
use nix::mount::{MsFlags, mount};
use nix::sched::{CloneFlags, unshare};
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::{ForkResult, fork};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub struct LinuxBackend;

impl LinuxBackend {
    pub fn new() -> Self {
        Self
    }

    pub fn run(&self, rules: &RuleSet, cmd: &str, deny_all: bool) -> Result<i32> {
        warn_unsupported_escalations(rules);
        check_required_features(rules)?;
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

/// Collect paths marked as Hide from the ruleset.
fn hidden_paths(rules: &RuleSet) -> Vec<&Path> {
    rules
        .rules()
        .iter()
        .filter(|r| r.mode == Mode::Hide)
        .map(|r| r.path.as_path())
        .collect()
}

/// Probe kernel features required by the current ruleset, before forking the
/// real sandbox.  Returns `Err(Unsupported(…))` with a user-facing message if
/// a required feature is missing.  Each probe is fork-based and only runs once
/// per process; subsequent calls returns the cached result instantly.
fn check_required_features(rules: &RuleSet) -> Result<()> {
    static MOUNT_NS_PROBED: AtomicBool = AtomicBool::new(false);
    static MOUNT_NS_OK: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);
    static LANDLOCK_PROBED: AtomicBool = AtomicBool::new(false);
    static LANDLOCK_OK: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);

    let needs_hide = rules.rules().iter().any(|r| r.mode == Mode::Hide);
    let needs_ro = rules.rules().iter().any(|r| r.mode == Mode::Ro);

    if needs_hide {
        let mut guard = MOUNT_NS_OK.lock().unwrap();
        if !MOUNT_NS_PROBED.swap(true, Ordering::SeqCst) {
            *guard = Some(probe_mount_ns());
        }
        if !guard.unwrap() {
            return Err(InboxError::Unsupported(diagnose_hide_failure()));
        }
    }

    if needs_ro {
        let mut guard = LANDLOCK_OK.lock().unwrap();
        if !LANDLOCK_PROBED.swap(true, Ordering::SeqCst) {
            *guard = Some(probe_landlock_enforce());
        }
        if !guard.unwrap() {
            return Err(InboxError::Unsupported(diagnose_landlock_failure()));
        }
    }

    Ok(())
}

/// Build a specific error message for --hide failure, checking actual system state.
fn diagnose_hide_failure() -> String {
    let mut lines = vec!["--hide requires mount namespace support.".to_string()];

    // Check if unprivileged user namespaces are disabled.
    match std::fs::read_to_string("/proc/sys/kernel/unprivileged_userns_clone") {
        Ok(content) => {
            let val = content.trim();
            if val == "0" {
                lines.push("unprivileged_userns_clone is disabled (set to 0).".to_string());
                lines.push("Fix: sysctl -w kernel.unprivileged_userns_clone=1".to_string());
            }
        }
        Err(_) => {
            // File doesn't exist — kernel may not support the sysctl at all
            // (e.g. Ubuntu/Debian patch removed it in newer kernels, or it's
            // always allowed).  Not actionable, skip.
        }
    }

    lines.push("The kernel denied unshare(CLONE_NEWUSER) or uid_map write (EPERM).".to_string());
    lines.push("Remove --hide flags from the command.".to_string());
    lines.join("\n")
}

/// Build a specific error message for --ro (Landlock) failure, checking actual
/// kernel version and LSM configuration.
fn diagnose_landlock_failure() -> String {
    let mut lines = vec![
        "--ro requires Landlock filesystem restrictions.".to_string(),
        "A test write to a read-only file was not blocked, meaning Landlock is not enforcing."
            .to_string(),
    ];

    // Check kernel version (Landlock needs 5.13+)
    if let Some(ver) = kernel_version() {
        let (major, minor) = ver;
        if major < 5 || (major == 5 && minor < 13) {
            lines.push(format!(
                "Kernel too old: {major}.{minor} (Landlock requires 5.13+).",
            ));
        } else {
            lines.push(format!(
                "Kernel {major}.{minor} meets the 5.13+ Landlock requirement."
            ));
        }
    } else {
        lines.push(
            "Could not determine kernel version (unable to read /proc/sys/kernel/osrelease)."
                .to_string(),
        );
    }

    // Check if Landlock appears in the active LSM list
    match std::fs::read_to_string("/sys/kernel/security/lsm") {
        Ok(content) => {
            if content.trim().split(',').any(|l| l.trim() == "landlock") {
                lines.push(
                    "Landlock is listed in /sys/kernel/security/lsm but is not enforcing. \
                     This may indicate a kernel configuration issue."
                        .to_string(),
                );
            } else {
                lines.push(format!(
                    "Landlock is NOT in the active LSM list: {}",
                    content.trim()
                ));
                lines.push(
                    "Add lsm=landlock to your kernel boot parameters and reboot.".to_string(),
                );
            }
        }
        Err(_) => {
            lines.push("Could not read /sys/kernel/security/lsm to check LSM status.".to_string());
        }
    }

    lines.push("Remove --ro flags from the command.".to_string());
    lines.join("\n")
}

/// Parse /proc/sys/kernel/osrelease and return (major, minor) version.
fn kernel_version() -> Option<(u32, u32)> {
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease").ok()?;
    let rest = release.trim();
    // Format: "6.1.0-generic" or "5.15.0-1024-aws"
    let major: u32 = rest.split('.').next()?.parse().ok()?;
    let minor: u32 = rest.split('.').nth(1)?.parse().ok()?;
    Some((major, minor))
}

/// Check whether we can create a user+mount namespace and write uid_map.
/// Forks a child to probe — the parent is never affected.
fn probe_mount_ns() -> bool {
    match unsafe { fork() } {
        Ok(ForkResult::Child) => {
            let ok = unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS).is_ok()
                && std::fs::write("/proc/self/uid_map", "0 0 1").is_ok()
                && std::fs::write("/proc/self/setgroups", "deny").is_ok()
                && std::fs::write("/proc/self/gid_map", "0 0 1").is_ok();
            std::process::exit(if ok { 0 } else { 1 });
        }
        Ok(ForkResult::Parent { child }) => match waitpid(child, None) {
            Ok(WaitStatus::Exited(_, code)) => code == 0,
            _ => false,
        },
        Err(_) => false,
    }
}

/// Check whether Landlock can actually enforce a read-only restriction.
/// Forks a child to test — creates a temp file, applies --ro via Landlock,
/// attempts a write, and checks if it was denied (EPERM).
/// Uses tempfile::tempdir() so the temp directory is cleaned up even if the
/// child is signalled.
fn probe_landlock_enforce() -> bool {
    match unsafe { fork() } {
        Ok(ForkResult::Child) => {
            use std::fs::File;
            use std::io::Write;

            let abi = ABI::V4;
            let all: BitFlags<AccessFs> = Access::from_all(abi);
            let read_only: BitFlags<AccessFs> = AccessFs::ReadFile | AccessFs::ReadDir;

            let ok = (|| -> std::result::Result<bool, ()> {
                // Create a temp directory that is auto-cleaned on drop.
                let tmp = tempfile::tempdir().map_err(|_| ())?;
                let test_file = tmp.path().join("probe.txt");
                std::fs::write(&test_file, "original").map_err(|_| ())?;

                // Build a Landlock ruleset: deny-all baseline, then allow read-only on
                // the test file.  Landlock takes the UNION of matching PathBeneath
                // rules within a single ruleset, so a root allow-all rule would
                // override the read-only restriction.  We must NOT add an allow-all
                // root rule.
                let mut ruleset = Ruleset::default()
                    .handle_access(all)
                    .map_err(|_| ())?
                    .create()
                    .map_err(|_| ())?
                    .set_compatibility(CompatLevel::BestEffort);

                // Allow traversal to the test file's parent directory
                let parent_fd = PathFd::new(tmp.path()).map_err(|_| ())?;
                ruleset = ruleset
                    .add_rule(PathBeneath::new(parent_fd, AccessFs::Execute | AccessFs::ReadDir))
                    .map_err(|_| ())?;

                // Read-only rule on the test file
                let file_fd = PathFd::new(&test_file).map_err(|_| ())?;
                ruleset = ruleset
                    .add_rule(PathBeneath::new(file_fd, read_only))
                    .map_err(|_| ())?;

                let status = ruleset.restrict_self().map_err(|_| ())?;
                if status.ruleset == RulesetStatus::NotEnforced {
                    return Ok(false);
                }

                // Try to write — should fail with EPERM if Landlock is enforcing
                let write_ok = File::options()
                    .write(true)
                    .open(&test_file)
                    .and_then(|mut f| f.write_all(b"pwned"))
                    .is_ok();

                Ok(!write_ok) // enforce = write was denied
            })()
            .unwrap_or(false);

            std::process::exit(if ok { 0 } else { 1 });
        }
        Ok(ForkResult::Parent { child }) => match waitpid(child, None) {
            Ok(WaitStatus::Exited(_, code)) => code == 0,
            _ => false,
        },
        Err(_) => false,
    }
}

fn spawn_with_landlock(rules: &RuleSet, cmd: &str, deny_all: bool) -> Result<i32> {
    let hide_paths = hidden_paths(rules);
    let cmd = cmd.to_string();
    let rules_clone = rules.clone();

    // SAFETY: single-threaded at this point; no async-signal-unsafe operations
    // occur between fork and exec in the child path.
    match unsafe { fork() }? {
        ForkResult::Child => {
            // Apply hide via bind mounts in a private mount namespace.
            // This must happen BEFORE Landlock, because Landlock restricts
            // the current view; we modify what the child sees via mounts.
            // Guard: skip entirely if no hide paths to avoid unshare(CLONE_NEWUSER).
            if !hide_paths.is_empty()
                && let Err(e) = setup_hide_mounts(&hide_paths)
            {
                eprintln!("inbox: hide mount setup failed: {e}");
                std::process::exit(1);
            }

            if let Err(e) = apply_landlock(&rules_clone, deny_all) {
                eprintln!("inbox: landlock setup failed: {e}");
                std::process::exit(1);
            }
            let err = std::process::Command::new("/bin/sh")
                .args(["-c", &cmd])
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

/// Hide paths by bind-mounting nothingness over them in a private mount namespace.
/// Files → bind-mount /dev/null (appears as empty character device).
/// Directories → mount tmpfs (appears empty, listing returns nothing).
///
/// Uses CLONE_NEWUSER + CLONE_NEWNS so this works without root (unprivileged
/// user namespaces must be enabled: /proc/sys/kernel/unprivileged_userns_clone=1).
fn setup_hide_mounts(paths: &[&Path]) -> Result<()> {
    // Create a new user namespace (gives us implicit root inside) + mount namespace.
    // After fork() we're single-threaded, so this is safe.
    unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS)?;

    // Write uid/gid mappings so the kernel treats us as root inside the namespace.
    // Without this, mount() returns EPERM even inside CLONE_NEWUSER.
    std::fs::write("/proc/self/uid_map", "0 0 1")
        .map_err(|e| InboxError::Io(std::io::Error::other(format!("uid_map write failed: {e}"))))?;
    // Must deny setgroups before writing gid_map (kernel requirement for unprivileged user ns).
    std::fs::write("/proc/self/setgroups", "deny").map_err(|e| {
        InboxError::Io(std::io::Error::other(format!(
            "setgroups write failed: {e}"
        )))
    })?;
    std::fs::write("/proc/self/gid_map", "0 0 1")
        .map_err(|e| InboxError::Io(std::io::Error::other(format!("gid_map write failed: {e}"))))?;

    // Make the mount tree private so bind mounts don't propagate.
    mount(
        Some(Path::new("/")),
        Path::new("/"),
        Some(Path::new("")),
        MsFlags::MS_REC | MsFlags::MS_PRIVATE,
        Some(Path::new("")),
    )
    .map_err(|e| InboxError::Io(std::io::Error::other(format!("MS_PRIVATE failed: {e}"))))?;

    for path in paths {
        if !(*path).exists() {
            continue; // Already absent — nothing to hide
        }
        if (*path).is_dir() {
            // Mount empty tmpfs over the directory.
            mount(
                Some(Path::new("tmpfs")),
                *path,
                Some(Path::new("tmpfs")),
                MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC,
                Some(Path::new("size=0,mode=000")),
            )
            .map_err(|e| {
                InboxError::Io(std::io::Error::other(format!(
                    "mount tmpfs on {:?} failed: {e}",
                    path
                )))
            })?;
        } else {
            // Bind-mount /dev/null over the file.
            mount(
                Some(Path::new("/dev/null")),
                *path,
                Some(Path::new("")),
                MsFlags::MS_BIND,
                Some(Path::new("")),
            )
            .map_err(|e| {
                InboxError::Io(std::io::Error::other(format!(
                    "bind /dev/null on {:?} failed: {e}",
                    path
                )))
            })?;
        }
    }
    Ok(())
}

fn apply_landlock(rules: &RuleSet, deny_all: bool) -> Result<()> {
    // If no filesystem restriction rules exist, skip Landlock entirely.
    // An empty ruleset with handle_access(all) but no allow rules would
    // deny everything — including /bin/sh execution.
    let has_landlock_rules = rules.rules().iter().any(|r| r.mode != Mode::Hide);
    if !has_landlock_rules && !deny_all {
        return Ok(());
    }

    // V4 covers all access types available as of kernel 6.1. set_best_effort(true)
    // silently downgrades to what the running kernel actually supports.
    let abi = ABI::V4;
    let all: BitFlags<AccessFs> = Access::from_all(abi);
    let read_only: BitFlags<AccessFs> = AccessFs::ReadFile | AccessFs::ReadDir;

    // Landlock within a single ruleset takes the UNION of matching PathBeneath
    // rules: if root=all and file=read_only both match a path, the effective
    // access is all ∪ read_only = all.  An allow-all root rule therefore makes
    // every per-path restriction a no-op.
    //
    // Correct approach: never add a root allow-all rule.  Instead, enumerate
    // every path that should be accessible:
    //   - --ro  paths get read-only access
    //   - --rw / --ephemeral paths get full access
    //   - everything else is denied (handled_access minus any matching rule)
    //
    // In deny_all mode (--hide present), even --ro paths are omitted so that
    // only Rw/Ephemeral paths remain accessible.

    // set_best_effort must be called on RulesetCreated (after .create()), not on Ruleset.
    let mut ruleset = Ruleset::default()
        .handle_access(all)
        .map_err(|e| InboxError::Io(std::io::Error::other(e.to_string())))?
        .create()
        .map_err(|e| InboxError::Io(std::io::Error::other(e.to_string())))?
        .set_compatibility(CompatLevel::BestEffort);

    // Collect every directory prefix leading to each rule path so that
    // traversal (cd into parent dirs) works.  Without these intermediate
    // directory allows, the process can't even reach /home/user/readonly
    // if /home or /home/user are denied.
    let mut parent_dirs: Vec<PathBuf> = Vec::new();

    for rule in rules.rules() {
        // Hide rules are handled by bind mounts (setup_hide_mounts), not Landlock.
        // Landlock cannot express "deny all access" — BitFlags::empty() is rejected.
        if rule.mode == Mode::Hide {
            continue;
        }

        // In deny_all mode, only Rw/Ephemeral rules add explicit allows.
        if deny_all && matches!(rule.mode, Mode::Ro) {
            continue;
        }

        let access = match rule.mode {
            Mode::Ro => read_only,
            Mode::Rw | Mode::Ephemeral => all,
            Mode::Hide => unreachable!(), // skipped above
        };

        // Collect parent directories for traversal.
        // E.g. for /home/user/dir we need /, /home, /home/user to be traversable.
        // ancestors() returns [/home/user/dir, /home/user, /home, /]; we want
        // all ancestors except the path itself (which gets its own rule above)
        // and the root (which is always implicitly traversable).
        if !deny_all {
            for ancestor in rule.path.ancestors().skip(1) {
                if ancestor == Path::new("/") {
                    continue; // root handled separately
                }
                parent_dirs.push(ancestor.to_path_buf());
            }
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

    // Add parent directory rules (execute + read_dir) for traversal.
    // These must come after per-path rules so they don't override per-path masks:
    // within a single ruleset the UNION is taken, so a parent dir with only
    // execute + read_dir won't "upgrade" its children beyond what their own
    // rules grant (because the parent rule doesn't include write).
    if !deny_all {
        let dir_access: BitFlags<AccessFs> = AccessFs::Execute | AccessFs::ReadDir;
        // Deduplicate parent dirs
        let mut seen = std::collections::HashSet::new();
        for dir in &parent_dirs {
            if seen.insert(dir.clone()) && let Ok(fd) = PathFd::new(dir) {
                let dir_rule = PathBeneath::new(fd, dir_access);
                ruleset = ruleset
                    .add_rule(dir_rule)
                    .map_err(|e| InboxError::Io(std::io::Error::other(e.to_string())))?;
            }
        }

        // Allow the sandboxed process to execute system binaries and read
        // shared libraries, device nodes, and locale/config files.  Without
        // these, the child can't even run /bin/sh.
        //
        // We grant Execute + ReadDir on directories and ReadFile on well-known
        // system paths.  This is a baseline set that every sandboxed command
        // needs.  (Write access is NOT granted here — only execution/reading.)
        let system_traverse = &["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc", "/dev"];
        for sys_path in system_traverse {
            if let Ok(fd) = PathFd::new(sys_path) {
                ruleset = ruleset
                    .add_rule(PathBeneath::new(fd, dir_access | AccessFs::ReadFile))
                    .map_err(|e| InboxError::Io(std::io::Error::other(e.to_string())))?;
            }
        }
        // /proc and /sys are needed for basic process info (whoami, etc.)
        for sys_path in &["/proc", "/sys"] {
            if let Ok(fd) = PathFd::new(sys_path) {
                ruleset = ruleset
                    .add_rule(PathBeneath::new(fd, AccessFs::ReadDir | AccessFs::ReadFile))
                    .map_err(|e| InboxError::Io(std::io::Error::other(e.to_string())))?;
            }
        }
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
