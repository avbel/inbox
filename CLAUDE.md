# inbox

Rust CLI that runs commands in a sandboxed filesystem environment.
Targets macOS (sandbox-exec + SBPL) and Linux 5.13+ (Landlock + overlayfs in user namespaces).

## Stack
- Language: Rust (edition 2024)
- CLI: clap
- Config: serde_yaml
- Globs: globset + walkdir
- PTY / syscalls: nix
- TUI: ratatui + crossterm

## After every code change
```
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
```

## Tests
```
cargo test --all-features
```

## Git hooks
Run once after cloning:
```
git config core.hooksPath .githooks
```
