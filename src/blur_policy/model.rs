use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceAlphaCapability {
    Opaque,
    AlphaCapable,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlurAssignment {
    None,
    ClientExact,
    CompositorSynthesized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlurTargetKind {
    Window,
    Layer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlurBackend {
    Wayland,
    Xwayland,
}

impl BlurBackend {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wayland => "wayland",
            Self::Xwayland => "xwayland",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlurApplicationMode {
    Auto,
    RulesOnly,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlurXwaylandMode {
    RulesOnly,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlurLayerMode {
    ClientOnly,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlurRuleAction {
    Enable,
    Disable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub struct BlurApplicationPolicy {
    pub wayland: BlurApplicationMode,
    pub xwayland: BlurXwaylandMode,
    pub auto_fullscreen: bool,
}

impl Default for BlurApplicationPolicy {
    fn default() -> Self {
        Self {
            wayland: BlurApplicationMode::Auto,
            xwayland: BlurXwaylandMode::RulesOnly,
            auto_fullscreen: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub struct BlurLayerPolicy {
    pub default: BlurLayerMode,
}

impl Default for BlurLayerPolicy {
    fn default() -> Self {
        Self {
            default: BlurLayerMode::ClientOnly,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub struct BlurWindowRule {
    pub name: String,
    #[serde(rename = "match")]
    pub matcher: BlurWindowMatch,
    #[serde(rename = "blur")]
    pub action: BlurRuleAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub struct BlurWindowMatch {
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub backend: Option<BlurBackend>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub struct BlurLayerRule {
    pub name: String,
    #[serde(rename = "match")]
    pub matcher: BlurLayerMatch,
    #[serde(rename = "blur")]
    pub action: BlurRuleAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub struct BlurLayerMatch {
    #[serde(default)]
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub struct BlurPolicyConfig {
    pub version: u32,
    pub enabled: bool,
    pub applications: BlurApplicationPolicy,
    pub layers: BlurLayerPolicy,
    #[serde(default)]
    pub window_rules: Vec<BlurWindowRule>,
    #[serde(default)]
    pub layer_rules: Vec<BlurLayerRule>,
}

impl Default for BlurPolicyConfig {
    fn default() -> Self {
        Self {
            version: 1,
            enabled: true,
            applications: BlurApplicationPolicy::default(),
            layers: BlurLayerPolicy::default(),
            window_rules: Vec::new(),
            layer_rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub struct BlurAssignmentCounts {
    pub client: u32,
    pub wayland_auto: u32,
    pub window_rule: u32,
    pub layer_rule: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub struct BlurPolicySnapshot {
    pub enabled: bool,
    #[serde(default)]
    pub renderer_supported: bool,
    pub generation: u64,
    pub wayland_mode: BlurApplicationMode,
    pub xwayland_mode: BlurXwaylandMode,
    pub auto_fullscreen: bool,
    pub layer_default: BlurLayerMode,
    pub window_rule_count: u32,
    pub layer_rule_count: u32,
    pub active_assignment_count: u32,
    pub assignment_counts_by_source: BlurAssignmentCounts,
    pub config_path: String,
    #[serde(default)]
    pub last_reload_error: Option<String>,
}

impl Default for BlurPolicySnapshot {
    fn default() -> Self {
        Self {
            enabled: true,
            renderer_supported: false,
            generation: 1,
            wayland_mode: BlurApplicationMode::Auto,
            xwayland_mode: BlurXwaylandMode::RulesOnly,
            auto_fullscreen: false,
            layer_default: BlurLayerMode::ClientOnly,
            window_rule_count: 0,
            layer_rule_count: 0,
            active_assignment_count: 0,
            assignment_counts_by_source: BlurAssignmentCounts::default(),
            config_path: String::new(),
            last_reload_error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_matches_the_v1_contract() {
        let policy = BlurPolicyConfig::default();
        assert!(policy.enabled);
        assert_eq!(policy.applications.wayland, BlurApplicationMode::Auto);
        assert_eq!(policy.applications.xwayland, BlurXwaylandMode::RulesOnly);
        assert!(!policy.applications.auto_fullscreen);
        assert_eq!(policy.layers.default, BlurLayerMode::ClientOnly);
        assert!(policy.window_rules.is_empty());
        assert!(policy.layer_rules.is_empty());
    }

    #[test]
    fn public_config_schema_is_nested_and_named() {
        let config: BlurPolicyConfig = serde_json::from_str(
            r#"{"version":1,"enabled":true,"applications":{"wayland":"auto","xwayland":"rules_only","auto_fullscreen":false},"layers":{"default":"client_only"},"window_rules":[{"name":"force-terminal-blur","match":{"app_id":"^kitty$","backend":"wayland"},"blur":"enable"}],"layer_rules":[{"name":"third-party-bar","match":{"namespace":"^waybar$"},"blur":"enable"}]}"#,
        )
        .expect("approved schema");
        assert_eq!(config.window_rules[0].name, "force-terminal-blur");
        assert_eq!(
            config.window_rules[0].matcher.app_id.as_deref(),
            Some("^kitty$")
        );
        assert_eq!(
            config.window_rules[0].matcher.backend,
            Some(BlurBackend::Wayland)
        );
        assert_eq!(
            config.layer_rules[0].matcher.namespace.as_deref(),
            Some("^waybar$")
        );
    }

    #[test]
    fn runtime_fields_and_unapproved_modes_are_rejected() {
        let runtime = r#"{"version":1,"enabled":true,"applications":{"wayland":"auto","xwayland":"rules_only","auto_fullscreen":false},"layers":{"default":"client_only"},"window_rules":[],"layer_rules":[],"renderer_supported":true}"#;
        assert!(serde_json::from_str::<BlurPolicyConfig>(runtime).is_err());
        assert!(
            serde_json::from_str::<BlurPolicyConfig>(
                &runtime.replace("\"rules_only\"", "\"auto\"")
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<BlurPolicyConfig>(
                &runtime.replace("\"client_only\"", "\"auto\"")
            )
            .is_err()
        );
    }
}
