pub mod sbpl;

use crate::error::Result;
use crate::rules::RuleSet;

#[allow(dead_code)]
pub struct MacOsBackend;

impl MacOsBackend {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self
    }

    #[allow(dead_code)]
    pub fn run(&self, rules: &RuleSet, cmd: &str, deny_all: bool) -> Result<i32> {
        let profile = sbpl::generate_sbpl(rules, deny_all);
        crate::spawner::spawn_sandboxed_macos(&profile, cmd)
    }
}

impl Default for MacOsBackend {
    fn default() -> Self {
        Self::new()
    }
}
