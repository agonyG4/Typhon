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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlurLayerMode {
    ClientOnly,
    Auto,
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
    pub xwayland: BlurApplicationMode,
    pub auto_fullscreen: bool,
}

impl Default for BlurApplicationPolicy {
    fn default() -> Self {
        Self {
            wayland: BlurApplicationMode::Auto,
            xwayland: BlurApplicationMode::RulesOnly,
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
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub backend: Option<String>,
    pub action: BlurRuleAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub struct BlurLayerRule {
    #[serde(default)]
    pub namespace: Option<String>,
    pub action: BlurRuleAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub struct BlurPolicySnapshot {
    pub version: u32,
    pub enabled: bool,
    pub applications: BlurApplicationPolicy,
    pub layers: BlurLayerPolicy,
    #[serde(default)]
    pub window_rules: Vec<BlurWindowRule>,
    #[serde(default)]
    pub layer_rules: Vec<BlurLayerRule>,
    #[serde(default)]
    pub renderer_supported: bool,
    #[serde(default)]
    pub generation: u64,
    #[serde(default)]
    pub config_path: String,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl Default for BlurPolicySnapshot {
    fn default() -> Self {
        Self {
            version: 1,
            enabled: true,
            applications: BlurApplicationPolicy::default(),
            layers: BlurLayerPolicy::default(),
            window_rules: Vec::new(),
            layer_rules: Vec::new(),
            renderer_supported: false,
            generation: 1,
            config_path: String::new(),
            last_error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_matches_the_v1_contract() {
        let policy = BlurPolicySnapshot::default();
        assert!(policy.enabled);
        assert_eq!(policy.applications.wayland, BlurApplicationMode::Auto);
        assert_eq!(policy.applications.xwayland, BlurApplicationMode::RulesOnly);
        assert!(!policy.applications.auto_fullscreen);
        assert_eq!(policy.layers.default, BlurLayerMode::ClientOnly);
        assert!(policy.window_rules.is_empty());
        assert!(policy.layer_rules.is_empty());
    }
}
