use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "inbox",
    version,
    about = "Run commands in a sandboxed filesystem environment"
)]
pub struct Cli {
    /// Command to run in the sandbox
    #[arg(last = true)]
    pub command: Vec<String>,

    /// Read-only path (EPERM on write). Accepts globs and ~.
    #[arg(long, value_name = "PATH", action = clap::ArgAction::Append)]
    pub ro: Vec<String>,

    /// Explicitly writable path (punch hole in a parent --ro).
    #[arg(long, value_name = "PATH", action = clap::ArgAction::Append)]
    pub rw: Vec<String>,

    /// Fake writable path — writes captured and discarded on exit.
    #[arg(long, value_name = "PATH", action = clap::ArgAction::Append)]
    pub ephemeral: Vec<String>,

    /// Hidden path — appears as ENOENT.
    #[arg(long, value_name = "PATH", action = clap::ArgAction::Append)]
    pub hide: Vec<String>,

    /// Load profile from ~/.config/inbox.yaml.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Deny-all mode: all unlisted paths ephemeral; interactive TUI at exit.
    #[arg(long)]
    pub review_ephemeral: bool,

    /// Override snapshot directory.
    #[arg(long, value_name = "PATH")]
    pub snapshot_dir: Option<PathBuf>,

    #[command(subcommand)]
    pub recovery: Option<RecoveryCmd>,
}

#[derive(Subcommand, Debug)]
pub enum RecoveryCmd {
    /// Restore files from an orphaned snapshot.
    Restore { uuid: String },
    /// Delete an orphaned snapshot without restoring.
    Discard { uuid: String },
}
