//! Rule evaluation for libraries and conditional arguments.
//!
//! Mojang's rules start from "disallow" when any rule is present. Each rule
//! that matches the current OS and feature set replaces the result with that
//! rule's action. The last match wins. An empty rule list means the entry is
//! included.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::platform::OsInfo;

/// Whether a matching rule includes or excludes its library or argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    /// Include the library or argument.
    Allow,
    /// Exclude the library or argument.
    Disallow,
}

/// Optional OS constraint inside a rule.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct OsConstraint {
    /// `windows`, `osx`, or `linux`.
    pub name: Option<String>,
    /// `x86`, `x64`, `arm64`, and the other spellings Mojang has used.
    pub arch: Option<String>,
    /// Regular expression matched against the OS version string.
    pub version: Option<String>,
}

/// One allow or disallow rule.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Rule {
    /// Action applied when the rule matches.
    pub action: RuleAction,
    /// When set, the OS must match as well.
    #[serde(default)]
    pub os: Option<OsConstraint>,
    /// When set, every feature must equal the current feature state.
    #[serde(default)]
    pub features: Option<HashMap<String, Value>>,
}

/// Feature flags substituted into conditional game arguments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeatureSet {
    /// Launch the demo world.
    pub is_demo_user: bool,
    /// Pass a custom width and height.
    pub has_custom_resolution: bool,
    /// Quick Play path support.
    pub has_quick_plays_support: bool,
    /// Open a singleplayer world immediately.
    pub is_quick_play_singleplayer: bool,
    /// Join a multiplayer server immediately.
    pub is_quick_play_multiplayer: bool,
    /// Join a Realms world immediately.
    pub is_quick_play_realms: bool,
}

impl FeatureSet {
    /// Current state of a named feature. Unknown names are off.
    pub fn get(&self, name: &str) -> bool {
        match name {
            "is_demo_user" => self.is_demo_user,
            "has_custom_resolution" => self.has_custom_resolution,
            "has_quick_plays_support" => self.has_quick_plays_support,
            "is_quick_play_singleplayer" => self.is_quick_play_singleplayer,
            "is_quick_play_multiplayer" => self.is_quick_play_multiplayer,
            "is_quick_play_realms" => self.is_quick_play_realms,
            _ => false,
        }
    }
}

/// Decide whether a library or conditional argument applies.
pub fn rules_allow(rules: &[Rule], os: &OsInfo, features: &FeatureSet) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for rule in rules {
        if rule_matches(rule, os, features) {
            allowed = rule.action == RuleAction::Allow;
        }
    }
    allowed
}

fn rule_matches(rule: &Rule, os: &OsInfo, features: &FeatureSet) -> bool {
    if let Some(constraint) = &rule.os
        && !os_matches(constraint, os)
    {
        return false;
    }
    if let Some(required) = &rule.features {
        for (name, expected) in required {
            if !feature_matches(expected, features.get(name)) {
                return false;
            }
        }
    }
    true
}

fn os_matches(constraint: &OsConstraint, os: &OsInfo) -> bool {
    if let Some(name) = &constraint.name
        && !name.eq_ignore_ascii_case(os.name.mojang())
    {
        return false;
    }
    if let Some(arch) = &constraint.arch
        && !os.arch.matches_mojang(arch)
    {
        return false;
    }
    if let Some(pattern) = &constraint.version {
        let Ok(expression) = regex::Regex::new(pattern) else {
            return false;
        };
        if !expression.is_match(&os.version) {
            return false;
        }
    }
    true
}

fn feature_matches(expected: &Value, actual: bool) -> bool {
    match expected {
        Value::Bool(value) => *value == actual,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Arch, OsName};

    fn linux() -> OsInfo {
        OsInfo::new(OsName::Linux, Arch::X86_64, "6.8.0")
    }

    fn windows(version: &str, arch: Arch) -> OsInfo {
        OsInfo::new(OsName::Windows, arch, version)
    }

    fn osx(arch: Arch) -> OsInfo {
        OsInfo::new(OsName::Osx, arch, "14.5")
    }

    fn allow_os(name: &str) -> Rule {
        Rule {
            action: RuleAction::Allow,
            os: Some(OsConstraint {
                name: Some(name.into()),
                arch: None,
                version: None,
            }),
            features: None,
        }
    }

    #[test]
    fn empty_rules_allow_the_entry() {
        assert!(rules_allow(&[], &linux(), &FeatureSet::default()));
    }

    #[test]
    fn last_matching_rule_wins() {
        let rules = vec![
            Rule {
                action: RuleAction::Allow,
                os: None,
                features: None,
            },
            Rule {
                action: RuleAction::Disallow,
                os: Some(OsConstraint {
                    name: Some("osx".into()),
                    arch: None,
                    version: None,
                }),
                features: None,
            },
        ];
        assert!(rules_allow(&rules, &linux(), &FeatureSet::default()));
        assert!(!rules_allow(
            &rules,
            &osx(Arch::X86_64),
            &FeatureSet::default()
        ));
    }

    #[test]
    fn os_allow_does_not_match_other_systems() {
        let rules = vec![allow_os("windows")];
        assert!(!rules_allow(&rules, &linux(), &FeatureSet::default()));
        assert!(rules_allow(
            &rules,
            &windows("10.0.19045", Arch::X86_64),
            &FeatureSet::default()
        ));
    }

    #[test]
    fn arch_and_windows_version_rules() {
        let x86_only = vec![Rule {
            action: RuleAction::Allow,
            os: Some(OsConstraint {
                name: None,
                arch: Some("x86".into()),
                version: None,
            }),
            features: None,
        }];
        assert!(!rules_allow(
            &x86_only,
            &windows("10.0.19045", Arch::X86_64),
            &FeatureSet::default()
        ));
        assert!(rules_allow(
            &x86_only,
            &windows("10.0.19045", Arch::X86),
            &FeatureSet::default()
        ));

        let windows_10 = vec![Rule {
            action: RuleAction::Allow,
            os: Some(OsConstraint {
                name: Some("windows".into()),
                arch: None,
                version: Some("^10\\.".into()),
            }),
            features: None,
        }];
        assert!(rules_allow(
            &windows_10,
            &windows("10.0.19045.3803", Arch::X86_64),
            &FeatureSet::default()
        ));
        assert!(!rules_allow(
            &windows_10,
            &windows("6.1.7601", Arch::X86_64),
            &FeatureSet::default()
        ));
        assert!(!rules_allow(&windows_10, &linux(), &FeatureSet::default()));
    }

    #[test]
    fn invalid_version_regex_does_not_match() {
        let rules = vec![Rule {
            action: RuleAction::Allow,
            os: Some(OsConstraint {
                name: Some("windows".into()),
                arch: None,
                version: Some("[".into()),
            }),
            features: None,
        }];
        assert!(!rules_allow(
            &rules,
            &windows("10.0", Arch::X86_64),
            &FeatureSet::default()
        ));
    }

    #[test]
    fn feature_rules_require_an_exact_bool() {
        let rules = vec![Rule {
            action: RuleAction::Allow,
            os: None,
            features: Some(HashMap::from([("is_demo_user".into(), Value::Bool(true))])),
        }];
        assert!(!rules_allow(&rules, &linux(), &FeatureSet::default()));
        let demo = FeatureSet {
            is_demo_user: true,
            ..FeatureSet::default()
        };
        assert!(rules_allow(&rules, &linux(), &demo));
    }

    #[test]
    fn os_and_features_must_both_match() {
        let rules = vec![Rule {
            action: RuleAction::Allow,
            os: Some(OsConstraint {
                name: Some("osx".into()),
                arch: None,
                version: None,
            }),
            features: Some(HashMap::from([(
                "has_custom_resolution".into(),
                Value::Bool(true),
            )])),
        }];
        let features = FeatureSet {
            has_custom_resolution: true,
            ..FeatureSet::default()
        };
        assert!(rules_allow(&rules, &osx(Arch::Arm64), &features));
        assert!(!rules_allow(&rules, &linux(), &features));
        assert!(!rules_allow(
            &rules,
            &osx(Arch::Arm64),
            &FeatureSet::default()
        ));
    }
}
