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

    // Sort by path component count ascending so parent paths come before child paths.
    // SBPL is last-match-wins: a more-specific rule (deeper path) must appear AFTER
    // the less-specific rule it overrides (e.g. --hide /dir before --rw /dir/sub).
    let mut sorted_rules: Vec<_> = rules.rules().iter().collect();
    sorted_rules.sort_by_key(|r| r.path.components().count());

    for rule in sorted_rules {
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
            //
            // Use explicit operation names rather than file-read*/file-write* wildcards:
            // SBPL wildcards in allow rules do NOT override specific-operation deny rules
            // (e.g. (allow file-read* ...) does not override (deny file-read-metadata ...)).
            Mode::Rw | Mode::Ephemeral => {
                lines.push(format!(
                    "(allow file-read-metadata file-read-data file-write* (subpath \"{path}\"))"
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
        assert!(sbpl.contains("(allow file-read-metadata file-read-data file-write* (subpath \"/home/user/tmp\"))"));
    }

    #[test]
    fn rw_in_deny_all_produces_allow() {
        let rs = ruleset(vec![rule("/home/user/project", Mode::Rw)]);
        let sbpl = generate_sbpl(&rs, true);
        assert!(sbpl.contains("(allow file-read-metadata file-read-data file-write* (subpath \"/home/user/project\"))"));
    }

    #[test]
    fn parent_rule_always_precedes_child_rule_regardless_of_insertion_order() {
        // Regardless of which order rules are inserted, the parent path's rule must
        // appear before the child path's rule in the SBPL output (SBPL last-match-wins).
        // Case: Hide parent inserted AFTER Rw child — sorting must fix the order.
        let rs = ruleset(vec![
            rule("/home/user/dir/sub", Mode::Rw), // inserted first (deeper)
            rule("/home/user/dir", Mode::Hide),   // inserted second (shallower)
        ]);
        let sbpl = generate_sbpl(&rs, false);
        let hide_pos = sbpl
            .find("(deny file-read-data (subpath \"/home/user/dir\"))")
            .expect("missing hide deny for parent");
        let allow_pos = sbpl
            .find("(allow file-read-metadata file-read-data file-write* (subpath \"/home/user/dir/sub\"))")
            .expect("missing allow rule for subdir");
        assert!(
            hide_pos < allow_pos,
            "parent deny must appear before child allow so child allow wins"
        );
    }
}
