# inbox — Design Spec

_Date: 2026-05-07_

## Overview

`inbox` is a macOS (and future Linux) CLI utility that runs a shell command in a sandboxed environment with limited, configurable filesystem access. It protects specific files and directories from being read, written, or even seen by the command — without requiring root privileges or kernel extensions.

---

## Language

**Rust.** Memory safety matters for a security tool. The ecosystem provides everything needed: `clap` (CLI), `serde_yaml` (config), `globset` + `walkdir` (glob expansion), `nix` (PTY, signals, namespaces), `ratatui` + `crossterm` (TUI), `tempfile` (temp dirs).

---

## CLI Surface

```
inbox [OPTIONS] "<command>"
```

### Restriction flags

All flags accept globs and `~` (expanded to `$HOME`). Globs are matched against the real filesystem at spawn time; non-matching globs are silently ignored.

| Flag | Behavior | What the process sees on write |
|---|---|---|
| `--ro <path>` | True read-only | EPERM |
| `--rw <path>` | Explicitly writable (punch hole in a parent `--ro`) | normal |
| `--ephemeral <path>` | Fake writable — writes captured, discarded on exit | success (silent discard) |
| `--hide <path>` | Not visible | ENOENT |

### Mode flags

| Flag | Behavior |
|---|---|
| `--profile <name>` | Load profile from `~/.config/inbox.yaml` |
| `--review-ephemeral` | Deny-all mode: all unlisted paths are ephemeral; interactive TUI at exit to choose what to keep |
| `--snapshot-dir <path>` | Override snapshot directory |

### Recovery subcommands

| Command | Behavior |
|---|---|
| `inbox --restore <uuid>` | Restore files from an orphaned snapshot (SIGKILL recovery) |
| `inbox --discard <uuid>` | Delete an orphaned snapshot |

---

## Configuration File

`~/.config/inbox.yaml`

```yaml
settings:
  snapshot_dir: /custom/path   # optional; see Snapshot Directory section

profile1:
  ro:
    - ~/.zshrc
    - ~/.ssh
  hide:
    - "**/.env"
  ephemeral:
    - ~/

profile2:
  based_on: profile1      # inherits all rules, then applies overrides below
  rw:
    - ~/project           # punch hole in inherited ephemeral ~/
  hide:
    - ~/.aws
```

---

## Snapshot Directory

Priority order (highest to lowest):

1. `--snapshot-dir <path>` CLI flag
2. `settings.snapshot_dir` in `~/.config/inbox.yaml`
3. `std::env::temp_dir()/.inbox/snapshots` (default)

`EphemeralManager` calls `std::fs::create_dir_all(snapshot_dir)` before any snapshot is taken. The directory is created if it does not exist.

---

## Release Profile

`Cargo.toml` `[profile.release]` optimized for small binary size:

```toml
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true
panic = "abort"
```

---

## Profile Inheritance

Rules are resolved at load time:

1. Load base profile recursively
2. Merge base rules first, then apply derived rules on top
3. Chains are allowed: `c based_on b based_on a`
4. Cycles (`a based_on b based_on a`) → startup error naming the cycle
5. Unknown `based_on` target → startup error

### Conflict resolution

When the same path appears in both base and derived profiles:

| Base | Derived | Result | Warning? |
|---|---|---|---|
| `ro` | `rw` | `rw` | yes — escalation |
| `hide` | `rw` | `rw` | yes — escalation |
| `hide` | `ro` | `ro` | yes — escalation |
| `ro` | `ephemeral` | `ephemeral` | yes — escalation |
| `hide` | `ephemeral` | `ephemeral` | yes — escalation |
| `rw` | `ro` | `ro` | no — stricter |
| `rw` | `hide` | `hide` | no — stricter |
| `ro` | `hide` | `hide` | no — stricter |

Warning format (stderr, before spawn):
```
warning: profile2 escalates 'ro' → 'rw' for ~/.zshrc (inherited from profile1)
```

---

## Architecture

```
inbox binary
  ├── CLI parser (clap)
  ├── Profile loader  →  RuleSet (resolved globs, expanded ~)
  ├── EphemeralManager  →  snapshot before, restore/review after
  ├── SandboxBackend (trait)
  │     ├── MacOsBackend   (sandbox-exec + SBPL)
  │     └── LinuxBackend   (user namespaces + Landlock)
  └── ProcessSpawner  →  PTY + signal forwarding + exit code
```

**`SandboxBackend` trait:**
```rust
trait SandboxBackend {
    fn spawn(&self, rules: &RuleSet, cmd: &str, pty: &Pty) -> Result<ExitStatus>;
}
```

**`RuleSet`:** flat list of `{ path: PathBuf, mode: Ro | Rw | Ephemeral | Hide }` produced after glob expansion and `~` resolution.

**Build targets:**
- `#[cfg(target_os = "macos")]` → `MacOsBackend`
- `#[cfg(target_os = "linux")]` → `LinuxBackend`

---

## macOS Backend

### Spawning

```
sandbox-exec -p "<generated SBPL>" /bin/sh -c "<command>"
```

The SBPL profile is generated from the `RuleSet` and passed via `-p`. The profile applies to the spawned process and all its children automatically.

### SBPL generation

**Allow-all mode (default):**
```scheme
(version 1)
(allow default)

; --ro path
(deny file-write* (subpath "/Users/you/.zshrc"))

; --hide path (deny stat too so it appears as ENOENT)
(deny file-read-metadata (subpath "/Users/you/.env"))
(deny file-read-data     (subpath "/Users/you/.env"))
(deny file-write*        (subpath "/Users/you/.env"))
```

**Deny-all mode (`--review-ephemeral`):**
```scheme
(version 1)
(deny default)

; minimum required to run any command
(allow file-read* (subpath "/usr"))
(allow file-read* (subpath "/Library"))
(allow file-read* (subpath "/System"))
(allow process*)

; --rw path
(allow file-read* file-write* (subpath "/Users/you/project"))

; --ephemeral path: full access in SBPL, snapshot-restore handles protection
(allow file-read* file-write* (subpath "/Users/you/"))
```

`--ephemeral` paths are not restricted in SBPL — `EphemeralManager` handles them entirely via snapshot-restore.

### PTY handling

1. `isatty(stdin)` check
2. If TTY: `nix::pty::openpty()` — slave fd given to child, master bridged to real terminal in parent
3. SIGWINCH forwarded to child on terminal resize
4. Exit code from child propagated as `inbox`'s exit code

---

## Linux Backend

**Minimum kernel: 5.13.** Checked at startup; clear error message if not met.

### Simple case — no `--ephemeral` paths

Landlock only, no namespace needed:

```rust
let ruleset = Ruleset::new(AccessFs::all())?;
// --rw: allow read + write
ruleset.add_path_beneath(path, AccessFs::READ | AccessFs::WRITE)?;
// --ro: allow read only
ruleset.add_path_beneath(path, AccessFs::READ)?;
// --hide: add no rule → access denied entirely
ruleset.restrict_self()?;
// spawn command
```

### Complex case — with `--ephemeral` paths

1. `unshare(CLONE_NEWUSER | CLONE_NEWNS)` — unprivileged; grants `CAP_SYS_ADMIN` inside the namespace
2. Write uid/gid mappings (real user → same uid inside)
3. For each `--ephemeral` path, mount overlayfs:
   ```
   lower  = /real/path           (real FS, never touched)
   upper  = $snapshot_dir/upper  (writes land here)
   work   = $snapshot_dir/work
   merged = bind-mounted over /real/path inside the namespace
   ```
4. For `--hide` paths: mount tmpfs over the path (empty, content invisible)
5. For `--ro` paths: bind-remount with `MS_RDONLY`
6. Apply Landlock on top as belt-and-suspenders
7. Spawn command

After exit, `upper` dir contains exactly what changed — used directly by `EphemeralManager` for the review TUI diff. No separate snapshot walk needed.

### PTY handling

Identical to macOS — `nix::pty::openpty()`, bridge master↔terminal, forward SIGWINCH.

---

## EphemeralManager

Owns the snapshot lifecycle. Implemented as a `Drop` guard — restore runs even if `inbox` exits via a signal handler.

**`before()`:**
1. `create_dir_all(snapshot_dir)`
2. For each ephemeral path: recursively copy to `$snapshot_dir/<uuid>/snapshot/`
3. Write `$snapshot_dir/<uuid>/manifest.json`: `{ paths, snapshot_locations, started_at }`

**`after(mode)`:**
- **Silent:** restore from snapshot, delete new files created in ephemeral dirs, clean up snapshot dir
- **Review:** compute diff → TUI → apply selected items → clean up

**On `Drop`:** if `after()` was not called, run silent restore automatically.

### Orphaned snapshot recovery (macOS SIGKILL gap)

On every `inbox` invocation, before spawning, scan snapshot dir for leftover `manifest.json` files. If found:

```
warning: found unrestored snapshot from a previous run (was inbox killed?)
  uuid: a1b2c3d4
  paths: ~/.zshrc, ~/.npmrc
  run `inbox --restore a1b2c3d4` to recover, or `inbox --discard a1b2c3d4` to clean up
```

On Linux, the real FS is always safe (overlayfs lower is never modified). Orphaned `upper` dirs are cleaned up with `inbox --discard`.

---

## Review TUI (`--review-ephemeral`)

Built with `ratatui` + `crossterm`. Displayed after command exits.

```
┌─ inbox: review changes ─────────────────────────────────────────┐
│ Command exited 0                                                │
│                                                                 │
│  [x] + ~/.local/bin/foo          new file    12 KB             │
│  [ ] ~ ~/.npmrc                  modified     3 lines          │
│  [x] + ~/.config/tool/init.lua   new file     1 KB             │
│                                                                 │
│  space: toggle   a: all   n: none   enter: apply   q: discard  │
└─────────────────────────────────────────────────────────────────┘
```

- Selected items are copied from snapshot/upper to real FS
- Unselected are discarded
- `q` or non-zero exit code: all changes discarded, snapshot dir cleaned up

---

## Signal Handling

| Signal | Target | macOS | Linux |
|---|---|---|---|
| SIGINT / Ctrl+C | child process | `inbox` alive → Drop → restore | same |
| SIGTERM | `inbox` | signal handler → wait child → Drop → restore | same |
| SIGKILL | child only | `inbox` alive → Drop → restore | same |
| SIGKILL | `inbox` itself | Drop skipped; files in modified state; snapshot preserved | real FS untouched; upper dir orphaned |

---

## Error Handling

| Situation | Behavior |
|---|---|
| Snapshot dir creation fails | Abort before spawn, print path and error |
| SIGKILL to `inbox` (macOS) | Snapshot preserved; warn on next invocation |
| Restore fails | Print error + snapshot path for manual recovery, exit non-zero |
| SBPL generation error | Error at rule-build time, before spawn |
| Landlock unavailable (< 5.13) | Startup error with kernel version |
| Profile cycle | Startup error naming the cycle |
| Unknown profile name | Startup error |
| Profile escalation conflict | Warning on stderr, continue with derived rule |

---

## Crates

| Crate | Purpose |
|---|---|
| `clap` | CLI parsing |
| `serde` + `serde_yaml` | Config parsing |
| `globset` + `walkdir` | Glob expansion and directory traversal |
| `nix` | PTY, signals, fork/spawn, namespaces |
| `ratatui` + `crossterm` | Review TUI |
| `tempfile` | Temp dirs for snapshots |
| `similar` | Diff computation for TUI display |

---

## README Structure

The project README covers:

### Installation
```bash
# Homebrew (macOS and Linux)
brew install avbel/tap/inbox

# Direct download (GitHub releases)
# See releases page for pre-built binaries

# From source
cargo install --path .
```

### All flags

Full table of `--ro`, `--rw`, `--ephemeral`, `--hide`, `--profile`, `--review-ephemeral`, `--snapshot-dir`, `--restore`, `--discard` with descriptions and examples.

### Config file reference

Full `~/.config/inbox.yaml` schema: `settings` block, profile definition, `based_on` inheritance, all rule types with examples.

### Profile inheritance and conflict rules

The conflict resolution table from the design spec, simplified for users.

### Usage examples (see next section)

### Linux requirements

> **Linux kernel 5.13 or later is required.**

---

## Usage Examples

### 1. Running an app from an unknown source

You downloaded a binary or script from the internet and want to run it without letting it touch your dotfiles, SSH keys, or cloud credentials.

```bash
# Protect sensitive files; let it write anywhere else
inbox \
  --ro ~/.ssh \
  --ro ~/.gnupg \
  --hide ~/.aws \
  --hide "**/.env" \
  --ephemeral ~/.zshrc \
  --ephemeral ~/.bashrc \
  "./downloaded-installer.sh"
```

Or use a profile for reuse:

```yaml
# ~/.config/inbox.yaml
untrusted:
  ro:
    - ~/.ssh
    - ~/.gnupg
  hide:
    - ~/.aws
    - ~/.config/gcloud
    - "**/.env"
  ephemeral:
    - ~/.zshrc
    - ~/.bashrc
    - ~/.profile
```

```bash
inbox --profile untrusted "./sketchy-tool"
```

### 2. Running an AI coding agent (e.g. Claude Code)

Let the agent work freely inside your project, but prevent it from reading secrets or touching configuration outside the project directory.

```bash
inbox \
  --rw ~/project \
  --ro ~/.ssh \
  --hide ~/.aws \
  --hide "**/.env" \
  --hide ~/.config/gcloud \
  --ephemeral ~/.gitconfig \
  "claude"
```

Profile variant:

```yaml
ai-agent:
  rw:
    - ~/project
  ro:
    - ~/.ssh
  hide:
    - ~/.aws
    - ~/.config/gcloud
    - "**/.env"
  ephemeral:
    - ~/.gitconfig
    - ~/.config/gh
```

```bash
inbox --profile ai-agent "claude"
inbox --profile ai-agent "aider"
```

### 3. Installing a global npm/pip/gem package without polluting dotfiles

Package managers often write aggressively to `~/.config`, `~/.local`, and rc files.

```bash
# See what it would change, then decide what to keep
inbox --review-ephemeral --rw ~/.local/bin "npm install -g some-cli"
```

### 4. Running a build or test suite that shouldn't touch your home directory

```bash
inbox --ro ~/ --rw ~/project --rw /tmp "make all test"
```

### 5. Auditing a setup script before running it for real

```bash
# Run in full review mode: nothing persists unless you choose it
inbox --review-ephemeral --rw ~/project "bash setup.sh"
```

After the script exits, a TUI shows every file it would have created or modified. You select what to actually keep.

### 6. Protecting credentials during a deploy

```bash
inbox \
  --hide ~/.aws \
  --hide ~/.config/gcloud \
  --hide "**/.env" \
  --ro ~/.ssh \
  "make deploy"
```

The deploy script can use SSH normally (read-only), but cannot read or exfiltrate cloud credentials.

---

## CLAUDE.md

A `CLAUDE.md` at the repo root gives Claude Code context about the project:

```markdown
# inbox

Rust CLI utility that runs commands in a sandboxed filesystem environment.
Targets macOS (sandbox-exec + SBPL) and Linux 5.13+ (Landlock + overlayfs).

## Stack
- Language: Rust (edition 2024)
- CLI: clap
- Config: serde_yaml
- Globs: globset + walkdir
- PTY / syscalls: nix
- TUI: ratatui + crossterm

## After every code change
Run:
  cargo fmt --all
  cargo clippy --all-targets --all-features -- -D warnings

## Tests
  cargo test --all-features
```

### PostToolUse hook (`.claude/settings.json`)

Auto-runs `cargo fmt` and `cargo clippy` after every Write or Edit tool call:

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Write|Edit",
        "hooks": [
          {
            "type": "command",
            "command": "cargo fmt --all 2>&1 && cargo clippy --all-targets --all-features -- -D warnings 2>&1"
          }
        ]
      }
    ]
  }
}
```

---

## Code Quality

### cargo fmt + clippy

Applied on every code change:

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
```

Clippy runs with `-D warnings` — any warning is a build failure.

### Pre-commit git hook

`.git/hooks/pre-commit` (installed via `cargo make setup-hooks` or a setup script):

```bash
#!/usr/bin/env bash
set -e
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

Committed as `.githooks/pre-commit`; developers run `git config core.hooksPath .githooks` once after cloning (or the setup script does it).

---

## Unit Tests

Each module has a `#[cfg(test)] mod tests` block. Key test areas:

### RuleSet / glob expansion
- `~` expands to `$HOME`
- `**/.env` matches nested files
- Non-matching globs produce no entries (no error)
- Duplicate paths with same mode deduplicated
- Duplicate paths with different modes follow conflict table

### Profile loader
- Simple profile loads correctly
- `based_on` single-level inheritance merges rules
- `based_on` chain (a → b → c) resolves in order
- Cycle detection returns error naming the cycle
- Unknown `based_on` returns error
- Escalation (ro → rw) emits warning
- Restriction (rw → ro) is silent

### Conflict resolution
- All 8 combinations in the conflict table tested explicitly

### Snapshot directory resolution
- CLI flag wins over config wins over default
- Default is `temp_dir()/.inbox/snapshots`
- `create_dir_all` called on resolved path

### EphemeralManager
- `before()` creates manifest and snapshot correctly
- `after()` silent mode restores original files and removes new files
- `Drop` triggers restore if `after()` was not called
- Orphaned manifest detected and warning emitted
- `--restore <uuid>` restores from manifest
- `--discard <uuid>` removes snapshot dir

### SBPL generation (macOS, unit-tested with string matching)
- `--ro` path produces `deny file-write*` rule
- `--hide` path produces three deny rules including `file-read-metadata`
- Allow-all mode starts with `(allow default)`
- Deny-all mode starts with `(deny default)` plus minimum system allows
- `--ephemeral` paths absent from SBPL output

### Version
- `env!("CARGO_PKG_VERSION")` is non-empty

---

## CI Workflows

### `ci.yml` — runs on every push and pull request

```yaml
on:
  push:
  pull_request:

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - run: cargo fmt --all -- --check
      - run: cargo clippy --all-targets --all-features -- -D warnings
      - run: cargo test --all-features
```

Also runs on `macos-latest` (parallel job) to catch macOS-specific code paths.

### `release.yml` — runs on tag push `v*.*`

1. **Build jobs (parallel):**

   | Job | Runner | Target | Artifact |
   |---|---|---|---|
   | macOS arm64 | `macos-14` | `aarch64-apple-darwin` | `.zip` |
   | Linux x64 | `ubuntu-latest` | `x86_64-unknown-linux-gnu` | `.tar.gz` |
   | Linux arm64 | `ubuntu-latest` + `cross` | `aarch64-unknown-linux-gnu` | `.tar.gz` |

   Each job:
   - Extracts `VERSION=${GITHUB_REF_NAME#v}`
   - Runs `cargo set-version $VERSION`
   - Runs `cargo fmt --check` + `cargo clippy -D warnings` + `cargo test`
   - Builds with `--release`
   - Packages artifact

2. **Release job** (after all build jobs pass):
   - Creates GitHub release via `softprops/action-gh-release`
   - Uploads all three artifacts

3. **Homebrew tap job** (after release job):
   - Checks out `avbel/homebrew-tap` using `HOMEBREW_TAP_TOKEN` secret
   - Computes SHA256 for each artifact
   - Renders `Formula/inbox.rb` with new version and hashes
   - Commits and pushes: `chore: release inbox vVERSION`

---

## Linux Requirements (README note)

> **Linux kernel 5.13 or later is required.** `inbox` uses Landlock for filesystem access control and overlayfs in user namespaces for ephemeral mounts. Earlier kernels are not supported and will produce a clear startup error.
