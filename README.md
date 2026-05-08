# inbox

<p align="center"><img src="assets/logo.png" width="600" alt="inbox logo"></p>

Run any command in a sandboxed filesystem environment. Control exactly which paths the command can read, write, or even see — without root privileges.

## Installation

### Homebrew (macOS and Linux)

```bash
brew tap avbel/tap
brew install inbox
```

### Direct download

See [GitHub Releases](https://github.com/avbel/inbox/releases) for pre-built binaries.

### From source

```bash
cargo install --path .
```

## Requirements

- **macOS**: 10.15 or later (uses `sandbox-exec`)
- **Linux**: kernel **5.13 or later** (uses Landlock + overlayfs in user namespaces)

## Usage

```
inbox [OPTIONS] "<command>"
```

## Flags

### Restriction flags (all accept globs and `~`)

| Flag | Behavior | Process sees on write |
|---|---|---|
| `--ro <path>` | True read-only | EPERM |
| `--rw <path>` | Explicitly writable | normal |
| `--ephemeral <path>` | Fake writable — writes discarded on exit | success |
| `--hide <path>` | Not visible | ENOENT |

### Mode flags

| Flag | Behavior |
|---|---|
| `--profile <name>` | Load profile from `~/.config/inbox.yaml` |
| `--review-ephemeral` | Deny-all mode; interactive TUI at exit to keep changes |
| `--snapshot-dir <path>` | Override snapshot directory |

### Recovery subcommands

| Command | Behavior |
|---|---|
| `inbox restore <uuid>` | Restore an orphaned snapshot |
| `inbox discard <uuid>` | Delete an orphaned snapshot |

## Config file: `~/.config/inbox.yaml`

```yaml
settings:
  snapshot_dir: /custom/path   # optional

untrusted:
  ro:
    - ~/
  hide:
    - ~/.ssh
    - ~/.config
    - ~/.aws
    - "**/.env"
    - "**/.env.*"
    - "**/*.config"
  ephemeral:
    - ~/.zshrc

ai-agent:
  based_on: untrusted
  rw:
    - ~/projects
```

## Profile inheritance

Profiles can inherit from a base with `based_on`. Derived rules override base rules.
Escalation (e.g. `ro` → `rw`) emits a warning. Restriction (`rw` → `ro`) is silent.

## Examples

### Run an app from an unknown source

```bash
inbox --ro ~/.ssh --hide ~/.aws --hide "**/.env" --ephemeral ~/.zshrc "./installer.sh"
```

### Run an AI coding agent

```bash
inbox --profile ai-agent "claude"
```

### Audit a setup script — review what it changes before applying

```bash
inbox --review-ephemeral --rw ~/project "bash setup.sh"
```

### Install a global package without touching your dotfiles

```bash
inbox --review-ephemeral --rw ~/.local/bin "npm install -g some-tool"
```

### Protect credentials during a deploy

```bash
inbox --ro ~/.ssh --hide ~/.aws --hide "**/.env" "make deploy"
```

## License

MIT
