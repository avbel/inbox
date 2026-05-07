use crate::config::Config;
use crate::error::{InboxError, Result};
use crate::rules::Mode;

#[allow(dead_code)]
pub fn resolve_profile(
    name: &str,
    config: &Config,
    visiting: &mut Vec<String>,
) -> Result<Vec<(String, Mode)>> {
    let warnings = std::cell::RefCell::new(vec![]);
    let patterns = resolve_profile_with_warnings(name, config, visiting, &warnings)?;
    for w in warnings.borrow().iter() {
        eprintln!("warning: {w}");
    }
    Ok(patterns)
}

#[allow(dead_code)]
pub fn resolve_profile_with_warnings(
    name: &str,
    config: &Config,
    visiting: &mut Vec<String>,
    warnings: &std::cell::RefCell<Vec<String>>,
) -> Result<Vec<(String, Mode)>> {
    if visiting.contains(&name.to_string()) {
        return Err(InboxError::ProfileCycle(visiting.join(" → ")));
    }

    let profile = config
        .profiles
        .get(name)
        .ok_or_else(|| InboxError::ProfileNotFound(name.to_string()))?;

    visiting.push(name.to_string());

    // Ordered map: path → mode (preserves insertion order)
    let mut patterns: indexmap::IndexMap<String, (Mode, String)> = indexmap::IndexMap::new();

    // Load base first
    if let Some(base_name) = &profile.based_on {
        let base = resolve_profile_with_warnings(base_name, config, visiting, warnings)?;
        for (path, mode) in base {
            patterns.insert(path, (mode, base_name.clone()));
        }
    }

    // Apply this profile's rules
    let this_name = name.to_string();
    let rules: &[(&[String], Mode)] = &[
        (&profile.ro, Mode::Ro),
        (&profile.rw, Mode::Rw),
        (&profile.ephemeral, Mode::Ephemeral),
        (&profile.hide, Mode::Hide),
    ];

    for (paths, new_mode) in rules {
        for path in paths.iter() {
            #[allow(clippy::collapsible_if)]
            if let Some((old_mode, from_profile)) = patterns.get(path) {
                if is_escalation(old_mode, new_mode) {
                    warnings.borrow_mut().push(format!(
                        "{this_name} escalates '{old_mode:?}' → '{new_mode:?}' for {path} (inherited from {from_profile})"
                    ));
                }
            }
            patterns.insert(path.clone(), (new_mode.clone(), this_name.clone()));
        }
    }

    visiting.pop();
    Ok(patterns
        .into_iter()
        .map(|(path, (mode, _))| (path, mode))
        .collect())
}

#[allow(dead_code)]
fn is_escalation(base: &Mode, derived: &Mode) -> bool {
    matches!(
        (base, derived),
        (Mode::Ro, Mode::Rw)
            | (Mode::Hide, Mode::Rw)
            | (Mode::Hide, Mode::Ro)
            | (Mode::Ro, Mode::Ephemeral)
            | (Mode::Hide, Mode::Ephemeral)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ProfileDef, Settings};

    fn make_config(profiles: Vec<(&str, ProfileDef)>) -> Config {
        Config {
            settings: Settings::default(),
            profiles: profiles
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        }
    }

    fn make_profile(
        based_on: Option<&str>,
        ro: &[&str],
        rw: &[&str],
        hide: &[&str],
        ephemeral: &[&str],
    ) -> ProfileDef {
        ProfileDef {
            based_on: based_on.map(str::to_string),
            ro: ro.iter().map(|s| s.to_string()).collect(),
            rw: rw.iter().map(|s| s.to_string()).collect(),
            hide: hide.iter().map(|s| s.to_string()).collect(),
            ephemeral: ephemeral.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn simple_profile_loads() {
        let config = make_config(vec![("p1", make_profile(None, &["/tmp/a"], &[], &[], &[]))]);
        let patterns = resolve_profile("p1", &config, &mut vec![]).unwrap();
        assert!(
            patterns
                .iter()
                .any(|(p, m)| p == "/tmp/a" && *m == crate::rules::Mode::Ro)
        );
    }

    #[test]
    fn based_on_merges_base_first() {
        let config = make_config(vec![
            ("base", make_profile(None, &["/tmp/base"], &[], &[], &[])),
            (
                "derived",
                make_profile(Some("base"), &["/tmp/derived"], &[], &[], &[]),
            ),
        ]);
        let patterns = resolve_profile("derived", &config, &mut vec![]).unwrap();
        assert_eq!(patterns.len(), 2);
    }

    #[test]
    fn cycle_detection() {
        let config = make_config(vec![
            ("a", make_profile(Some("b"), &[], &[], &[], &[])),
            ("b", make_profile(Some("a"), &[], &[], &[], &[])),
        ]);
        let err = resolve_profile("a", &config, &mut vec![]).unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn unknown_profile_error() {
        let config = make_config(vec![]);
        let err = resolve_profile("missing", &config, &mut vec![]).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn escalation_ro_to_rw_overwrites() {
        // base has ro, derived has rw → result is rw
        let config = make_config(vec![
            ("base", make_profile(None, &["/tmp/x"], &[], &[], &[])),
            (
                "derived",
                make_profile(Some("base"), &[], &["/tmp/x"], &[], &[]),
            ),
        ]);
        let warnings = std::cell::RefCell::new(vec![]);
        let patterns =
            resolve_profile_with_warnings("derived", &config, &mut vec![], &warnings).unwrap();
        let mode = patterns
            .iter()
            .find(|(p, _)| p == "/tmp/x")
            .map(|(_, m)| m)
            .unwrap();
        assert_eq!(*mode, crate::rules::Mode::Rw);
        assert!(!warnings.borrow().is_empty());
    }

    #[test]
    fn restriction_rw_to_ro_silent() {
        // base has rw, derived has ro → result is ro, no warning
        let config = make_config(vec![
            ("base", make_profile(None, &[], &["/tmp/x"], &[], &[])),
            (
                "derived",
                make_profile(Some("base"), &["/tmp/x"], &[], &[], &[]),
            ),
        ]);
        let warnings = std::cell::RefCell::new(vec![]);
        let patterns =
            resolve_profile_with_warnings("derived", &config, &mut vec![], &warnings).unwrap();
        let mode = patterns
            .iter()
            .find(|(p, _)| p == "/tmp/x")
            .map(|(_, m)| m)
            .unwrap();
        assert_eq!(*mode, crate::rules::Mode::Ro);
        assert!(warnings.borrow().is_empty());
    }
}
