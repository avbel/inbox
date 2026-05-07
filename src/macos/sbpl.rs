use crate::rules::{Mode, RuleSet};

/// Generate an SBPL profile string from the given rule set.
/// `deny_all`: if true, start with `(deny default)` (--review-ephemeral mode).
///             if false, start with `(allow default)` (normal mode).
/// Note: paths with non-UTF-8 bytes are replaced with U+FFFD by to_string_lossy;
/// in practice macOS paths are always valid UTF-8.
#[must_use = "SBPL profile must be passed to sandbox-exec"]
pub fn generate_sbpl(rules: &RuleSet, deny_all: bool) -> String {
    let mut lines = vec!["(version 1)".to_string()];

    if deny_all {
        lines.push("(deny default)".to_string());
        lines.push("(allow file-read* (subpath \"/usr\"))".to_string());
        lines.push("(allow file-read* (subpath \"/Library\"))".to_string());
        lines.push("(allow file-read* (subpath \"/System\"))".to_string());
        lines.push("(allow process*)".to_string());
    } else {
        lines.push("(allow default)".to_string());
    }

    for rule in rules.rules() {
        let path = rule.path.to_string_lossy();
        match &rule.mode {
            Mode::Ro => {
                lines.push(format!("(deny file-write* (subpath \"{path}\"))"));
            }
            Mode::Hide => {
                lines.push(format!("(deny file-read-metadata (subpath \"{path}\"))"));
                lines.push(format!("(deny file-read-data (subpath \"{path}\"))"));
                lines.push(format!("(deny file-write* (subpath \"{path}\"))"));
            }
            Mode::Rw if deny_all => {
                lines.push(format!(
                    "(allow file-read* file-write* (subpath \"{path}\"))"
                ));
            }
            Mode::Ephemeral if deny_all => {
                // Ephemeral needs full access in SBPL; snapshot-restore handles protection
                lines.push(format!(
                    "(allow file-read* file-write* (subpath \"{path}\"))"
                ));
            }
            Mode::Rw | Mode::Ephemeral => {
                // In allow-all mode: rw and ephemeral need no SBPL rule
            }
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Rule;
    use std::path::PathBuf;

    fn rule(path: &str, mode: Mode) -> Rule {
        Rule {
            path: PathBuf::from(path),
            mode,
        }
    }

    fn ruleset(rules: Vec<Rule>) -> RuleSet {
        RuleSet::from_rules(rules)
    }

    #[test]
    fn allow_all_mode_starts_with_allow_default() {
        let rs = ruleset(vec![]);
        let sbpl = generate_sbpl(&rs, false);
        assert!(sbpl.contains("(allow default)"));
        assert!(!sbpl.contains("(deny default)"));
    }

    #[test]
    fn deny_all_mode_starts_with_deny_default() {
        let rs = ruleset(vec![]);
        let sbpl = generate_sbpl(&rs, true);
        assert!(sbpl.contains("(deny default)"));
        assert!(sbpl.contains("(allow file-read* (subpath \"/usr\"))"));
    }

    #[test]
    fn ro_path_produces_deny_write() {
        let rs = ruleset(vec![rule("/home/user/.zshrc", Mode::Ro)]);
        let sbpl = generate_sbpl(&rs, false);
        assert!(sbpl.contains("(deny file-write* (subpath \"/home/user/.zshrc\"))"));
    }

    #[test]
    fn hide_path_produces_three_deny_rules() {
        let rs = ruleset(vec![rule("/home/user/.env", Mode::Hide)]);
        let sbpl = generate_sbpl(&rs, false);
        assert!(sbpl.contains("(deny file-read-metadata (subpath \"/home/user/.env\"))"));
        assert!(sbpl.contains("(deny file-read-data (subpath \"/home/user/.env\"))"));
        assert!(sbpl.contains("(deny file-write* (subpath \"/home/user/.env\"))"));
    }

    #[test]
    fn ephemeral_path_absent_from_sbpl() {
        let rs = ruleset(vec![rule("/home/user/tmp", Mode::Ephemeral)]);
        let sbpl = generate_sbpl(&rs, false);
        assert!(!sbpl.contains("/home/user/tmp"));
    }

    #[test]
    fn rw_in_deny_all_produces_allow() {
        let rs = ruleset(vec![rule("/home/user/project", Mode::Rw)]);
        let sbpl = generate_sbpl(&rs, true);
        assert!(sbpl.contains("(allow file-read* file-write* (subpath \"/home/user/project\"))"));
    }
}
