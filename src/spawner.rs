use crate::error::Result;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
pub fn spawn_sandboxed_macos(_sbpl_profile: &str, _cmd: &str) -> Result<i32> {
    Ok(0) // stub — implemented in Task 7
}
