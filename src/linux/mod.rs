use crate::error::Result;
use crate::rules::RuleSet;

#[allow(dead_code)]
pub struct LinuxBackend;

#[allow(dead_code)]
impl LinuxBackend {
    pub fn new() -> Self {
        Self
    }

    pub fn run(&self, _rules: &RuleSet, _cmd: &str, _deny_all: bool) -> Result<i32> {
        Err(crate::error::InboxError::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Linux backend not yet implemented",
        )))
    }
}
