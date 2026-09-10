use super::model::{BlurBackend, BlurLayerRule, BlurRuleAction, BlurWindowRule};
use regex::{Regex, RegexBuilder};
use std::fmt;

const REGEX_SIZE_LIMIT: usize = 64 * 1024;
const REGEX_DFA_SIZE_LIMIT: usize = 64 * 1024;
const MAX_PATTERN_BYTES: usize = 4096;

#[derive(Debug)]
pub enum BlurRuleCompileError {
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
            Self::PatternTooLarge { field } => {
                write!(formatter, "blur {field} pattern exceeds 4096 bytes")
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
    app_id: Option<Regex>,
    title: Option<Regex>,
    backend: Option<Regex>,
    action: BlurRuleAction,
}

#[derive(Debug, Clone)]
struct CompiledLayerRule {
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
        let window = window_rules
            .iter()
            .map(|rule| {
                Ok(CompiledWindowRule {
                    app_id: compile_optional(rule.app_id.as_deref(), "app_id")?,
                    title: compile_optional(rule.title.as_deref(), "title")?,
                    backend: compile_optional(rule.backend.as_deref(), "backend")?,
                    action: rule.action,
                })
            })
            .collect::<Result<Vec<_>, BlurRuleCompileError>>()?;
        let layer = layer_rules
            .iter()
            .map(|rule| {
                Ok(CompiledLayerRule {
                    namespace: compile_optional(rule.namespace.as_deref(), "namespace")?,
                    action: rule.action,
                })
            })
            .collect::<Result<Vec<_>, BlurRuleCompileError>>()?;
        Ok(Self { window, layer })
    }

    pub fn last_window_action(&self, target: BlurWindowTarget<'_>) -> Option<BlurRuleAction> {
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
            .map(|rule| rule.action)
            .next_back()
    }

    pub fn last_layer_action(&self, target: BlurLayerTarget<'_>) -> Option<BlurRuleAction> {
        self.layer
            .iter()
            .filter(|rule| {
                rule.namespace
                    .as_ref()
                    .is_none_or(|pattern| pattern.is_match(target.namespace))
            })
            .map(|rule| rule.action)
            .next_back()
    }
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
        app_id: Option<&str>,
        title: Option<&str>,
        backend: Option<&str>,
        action: BlurRuleAction,
    ) -> BlurWindowRule {
        BlurWindowRule {
            app_id: app_id.map(str::to_string),
            title: title.map(str::to_string),
            backend: backend.map(str::to_string),
            action,
        }
    }

    #[test]
    fn all_window_fields_are_anded_and_last_matching_rule_wins() {
        let rules = CompiledBlurRules::compile(
            &[
                window_rule(
                    Some("org.example.*"),
                    Some("Editor.*"),
                    Some("wayland"),
                    BlurRuleAction::Enable,
                ),
                window_rule(Some("org.example.app"), None, None, BlurRuleAction::Disable),
            ],
            &[],
        )
        .expect("compile rules");
        assert_eq!(
            rules.last_window_action(BlurWindowTarget {
                app_id: Some("org.example.app"),
                title: Some("Editor — file"),
                backend: BlurBackend::Wayland,
            }),
            Some(BlurRuleAction::Disable)
        );
        assert_eq!(
            rules.last_window_action(BlurWindowTarget {
                app_id: Some("org.example.other"),
                title: Some("Editor — file"),
                backend: BlurBackend::Wayland,
            }),
            Some(BlurRuleAction::Enable)
        );
        assert_eq!(
            rules.last_window_action(BlurWindowTarget {
                app_id: Some("org.example.app"),
                title: Some("Terminal"),
                backend: BlurBackend::Wayland,
            }),
            Some(BlurRuleAction::Disable)
        );
    }

    #[test]
    fn malformed_or_unbounded_patterns_are_rejected_at_compile_time() {
        let invalid = window_rule(Some("["), None, None, BlurRuleAction::Enable);
        assert!(matches!(
            CompiledBlurRules::compile(&[invalid], &[]),
            Err(BlurRuleCompileError::InvalidPattern {
                field: "app_id",
                ..
            })
        ));
        let too_large = "a".repeat(MAX_PATTERN_BYTES + 1);
        let too_large = window_rule(Some(too_large.as_str()), None, None, BlurRuleAction::Enable);
        assert!(matches!(
            CompiledBlurRules::compile(&[too_large], &[]),
            Err(BlurRuleCompileError::PatternTooLarge { field: "app_id" })
        ));
    }
}
