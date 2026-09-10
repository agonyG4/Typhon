use super::model::{BlurBackend, BlurLayerRule, BlurRuleAction, BlurWindowRule};
use regex::{Regex, RegexBuilder};
use std::{collections::HashSet, fmt};

const REGEX_SIZE_LIMIT: usize = 64 * 1024;
const REGEX_DFA_SIZE_LIMIT: usize = 64 * 1024;
pub const MAX_PATTERN_BYTES: usize = 256;
pub const MAX_RULE_NAME_BYTES: usize = 128;

#[derive(Debug)]
pub enum BlurRuleCompileError {
    EmptyName,
    NameTooLarge,
    DuplicateName {
        name: String,
    },
    PatternTooLarge {
        field: &'static str,
    },
    InvalidPattern {
        field: &'static str,
        error: regex::Error,
    },
}

impl fmt::Display for BlurRuleCompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyName => write!(formatter, "blur rule name must not be empty"),
            Self::NameTooLarge => write!(formatter, "blur rule name exceeds 128 bytes"),
            Self::DuplicateName { name } => write!(formatter, "duplicate blur rule name: {name}"),
            Self::PatternTooLarge { field } => {
                write!(formatter, "blur {field} pattern exceeds 256 bytes")
            }
            Self::InvalidPattern { field, error } => {
                write!(formatter, "invalid blur {field} pattern: {error}")
            }
        }
    }
}

impl std::error::Error for BlurRuleCompileError {}

#[derive(Debug, Clone, Copy)]
pub struct BlurWindowTarget<'a> {
    pub app_id: Option<&'a str>,
    pub title: Option<&'a str>,
    pub backend: BlurBackend,
}

#[derive(Debug, Clone, Copy)]
pub struct BlurLayerTarget<'a> {
    pub namespace: &'a str,
}

#[derive(Debug, Clone)]
struct CompiledWindowRule {
    name: String,
    app_id: Option<Regex>,
    title: Option<Regex>,
    backend: Option<Regex>,
    action: BlurRuleAction,
}

#[derive(Debug, Clone)]
struct CompiledLayerRule {
    name: String,
    namespace: Option<Regex>,
    action: BlurRuleAction,
}

#[derive(Debug, Clone)]
pub struct CompiledBlurRules {
    window: Vec<CompiledWindowRule>,
    layer: Vec<CompiledLayerRule>,
}

impl CompiledBlurRules {
    pub fn compile(
        window_rules: &[BlurWindowRule],
        layer_rules: &[BlurLayerRule],
    ) -> Result<Self, BlurRuleCompileError> {
        let mut names = HashSet::with_capacity(window_rules.len() + layer_rules.len());
        let window = window_rules
            .iter()
            .map(|rule| {
                validate_name(&mut names, &rule.name)?;
                Ok(CompiledWindowRule {
                    name: rule.name.clone(),
                    app_id: compile_optional(rule.matcher.app_id.as_deref(), "app_id")?,
                    title: compile_optional(rule.matcher.title.as_deref(), "title")?,
                    backend: compile_optional(rule.matcher.backend.as_deref(), "backend")?,
                    action: rule.action,
                })
            })
            .collect::<Result<Vec<_>, BlurRuleCompileError>>()?;
        let layer = layer_rules
            .iter()
            .map(|rule| {
                validate_name(&mut names, &rule.name)?;
                Ok(CompiledLayerRule {
                    name: rule.name.clone(),
                    namespace: compile_optional(rule.matcher.namespace.as_deref(), "namespace")?,
                    action: rule.action,
                })
            })
            .collect::<Result<Vec<_>, BlurRuleCompileError>>()?;
        Ok(Self { window, layer })
    }

    pub fn last_window_action(&self, target: BlurWindowTarget<'_>) -> Option<BlurRuleAction> {
        self.last_window_match(target).map(|(_, action)| action)
    }

    pub fn last_window_match(
        &self,
        target: BlurWindowTarget<'_>,
    ) -> Option<(&str, BlurRuleAction)> {
        self.window
            .iter()
            .filter(|rule| {
                matches_optional(&rule.app_id, target.app_id)
                    && matches_optional(&rule.title, target.title)
                    && rule
                        .backend
                        .as_ref()
                        .is_none_or(|pattern| pattern.is_match(target.backend.as_str()))
            })
            .map(|rule| (rule.name.as_str(), rule.action))
            .next_back()
    }

    pub fn last_layer_action(&self, target: BlurLayerTarget<'_>) -> Option<BlurRuleAction> {
        self.last_layer_match(target).map(|(_, action)| action)
    }

    pub fn last_layer_match(&self, target: BlurLayerTarget<'_>) -> Option<(&str, BlurRuleAction)> {
        self.layer
            .iter()
            .filter(|rule| {
                rule.namespace
                    .as_ref()
                    .is_none_or(|pattern| pattern.is_match(target.namespace))
            })
            .map(|rule| (rule.name.as_str(), rule.action))
            .next_back()
    }
}

fn validate_name(names: &mut HashSet<String>, name: &str) -> Result<(), BlurRuleCompileError> {
    if name.is_empty() {
        return Err(BlurRuleCompileError::EmptyName);
    }
    if name.len() > MAX_RULE_NAME_BYTES {
        return Err(BlurRuleCompileError::NameTooLarge);
    }
    if !names.insert(name.to_string()) {
        return Err(BlurRuleCompileError::DuplicateName {
            name: name.to_string(),
        });
    }
    Ok(())
}

fn compile_optional(
    pattern: Option<&str>,
    field: &'static str,
) -> Result<Option<Regex>, BlurRuleCompileError> {
    let Some(pattern) = pattern else {
        return Ok(None);
    };
    if pattern.len() > MAX_PATTERN_BYTES {
        return Err(BlurRuleCompileError::PatternTooLarge { field });
    }
    RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_DFA_SIZE_LIMIT)
        .build()
        .map(Some)
        .map_err(|error| BlurRuleCompileError::InvalidPattern { field, error })
}

fn matches_optional(pattern: &Option<Regex>, value: Option<&str>) -> bool {
    pattern
        .as_ref()
        .is_none_or(|pattern| value.is_some_and(|value| pattern.is_match(value)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window_rule(
        name: &str,
        app_id: Option<&str>,
        title: Option<&str>,
        backend: Option<&str>,
        action: BlurRuleAction,
    ) -> BlurWindowRule {
        BlurWindowRule {
            name: name.to_string(),
            matcher: super::super::model::BlurWindowMatch {
                app_id: app_id.map(str::to_string),
                title: title.map(str::to_string),
                backend: backend.map(str::to_string),
            },
            action,
        }
    }

    #[test]
    fn all_window_fields_are_anded_and_last_matching_rule_wins() {
        let rules = CompiledBlurRules::compile(
            &[
                window_rule(
                    "enable-editor",
                    Some("org.example.*"),
                    Some("Editor.*"),
                    Some("wayland"),
                    BlurRuleAction::Enable,
                ),
                window_rule(
                    "disable-app",
                    Some("org.example.app"),
                    None,
                    None,
                    BlurRuleAction::Disable,
                ),
            ],
            &[],
        )
        .expect("compile rules");
        assert_eq!(
            rules.last_window_action(BlurWindowTarget {
                app_id: Some("org.example.app"),
                title: Some("Editor — file"),
                backend: BlurBackend::Wayland
            }),
            Some(BlurRuleAction::Disable)
        );
        assert_eq!(
            rules.last_window_action(BlurWindowTarget {
                app_id: Some("org.example.other"),
                title: Some("Editor — file"),
                backend: BlurBackend::Wayland
            }),
            Some(BlurRuleAction::Enable)
        );
        assert_eq!(
            rules.last_window_action(BlurWindowTarget {
                app_id: Some("org.example.app"),
                title: Some("Terminal"),
                backend: BlurBackend::Wayland
            }),
            Some(BlurRuleAction::Disable)
        );
    }

    #[test]
    fn malformed_or_unbounded_patterns_are_rejected_at_compile_time() {
        let invalid = window_rule("invalid", Some("["), None, None, BlurRuleAction::Enable);
        assert!(matches!(
            CompiledBlurRules::compile(&[invalid], &[]),
            Err(BlurRuleCompileError::InvalidPattern {
                field: "app_id",
                ..
            })
        ));
        let too_large = "a".repeat(MAX_PATTERN_BYTES + 1);
        let too_large = window_rule(
            "large",
            Some(&too_large),
            None,
            None,
            BlurRuleAction::Enable,
        );
        assert!(matches!(
            CompiledBlurRules::compile(&[too_large], &[]),
            Err(BlurRuleCompileError::PatternTooLarge { field: "app_id" })
        ));
    }

    #[test]
    fn rule_names_are_nonempty_bounded_and_unique_across_kinds() {
        let mut empty = window_rule("", None, None, None, BlurRuleAction::Enable);
        empty.name.clear();
        assert!(matches!(
            CompiledBlurRules::compile(&[empty], &[]),
            Err(BlurRuleCompileError::EmptyName)
        ));
        let long = window_rule(
            &"n".repeat(MAX_RULE_NAME_BYTES + 1),
            None,
            None,
            None,
            BlurRuleAction::Enable,
        );
        assert!(matches!(
            CompiledBlurRules::compile(&[long], &[]),
            Err(BlurRuleCompileError::NameTooLarge)
        ));
        let duplicate = super::super::model::BlurLayerRule {
            name: "enable".to_string(),
            matcher: super::super::model::BlurLayerMatch { namespace: None },
            action: BlurRuleAction::Enable,
        };
        let first = window_rule("enable", None, None, None, BlurRuleAction::Enable);
        assert!(matches!(
            CompiledBlurRules::compile(&[first], &[duplicate]),
            Err(BlurRuleCompileError::DuplicateName { .. })
        ));
    }
}
