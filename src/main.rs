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

use error::Result;

fn main() -> Result<()> {
    Ok(())
}
