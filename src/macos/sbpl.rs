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
            // Always emit explicit allow for Rw and Ephemeral so they can override a
            // parent Ro or Hide rule. SBPL is last-match-wins: the deny from a parent
            // path comes first in the rule list, then this allow overrides it for the
            // subpath. In allow-all mode the allow is redundant when no parent restriction
            // exists, but harmless.
            Mode::Rw | Mode::Ephemeral => {
                lines.push(format!(
                    "(allow file-read* file-write* (subpath \"{path}\"))"
                ));
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
    fn ephemeral_path_produces_allow_rule() {
        // Ephemeral always gets an explicit allow so it can override a parent Ro rule.
        let rs = ruleset(vec![rule("/home/user/tmp", Mode::Ephemeral)]);
        let sbpl = generate_sbpl(&rs, false);
        assert!(sbpl.contains("(allow file-read* file-write* (subpath \"/home/user/tmp\"))"));
    }

    #[test]
    fn rw_in_deny_all_produces_allow() {
        let rs = ruleset(vec![rule("/home/user/project", Mode::Rw)]);
        let sbpl = generate_sbpl(&rs, true);
        assert!(sbpl.contains("(allow file-read* file-write* (subpath \"/home/user/project\"))"));
    }

    #[test]
    fn ro_parent_rw_subdir_allows_subdir_writes() {
        // --ro /dir --rw /dir/sub: deny on /dir must come before allow on /dir/sub
        // so the allow wins for /dir/sub (SBPL last-match-wins).
        let rs = ruleset(vec![
            rule("/home/user/dir", Mode::Ro),
            rule("/home/user/dir/sub", Mode::Rw),
        ]);
        let sbpl = generate_sbpl(&rs, false);
        let deny_pos = sbpl
            .find("(deny file-write* (subpath \"/home/user/dir\"))")
            .expect("missing deny rule for parent");
        let allow_pos = sbpl
            .find("(allow file-read* file-write* (subpath \"/home/user/dir/sub\"))")
            .expect("missing allow rule for subdir");
        assert!(
            deny_pos < allow_pos,
            "deny rule must appear before allow rule so allow wins for subdir"
        );
    }
}
