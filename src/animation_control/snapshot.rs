//! Bounded, deterministic control-socket representation of animation state.

use super::{catalog::*, config::AnimationConfiguration};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnimationConfigurationSnapshot {
    pub enabled: bool,
    pub preset: String,
    pub speed: f64,
    pub overrides: BTreeMap<String, String>,
}

impl From<&AnimationConfiguration> for AnimationConfigurationSnapshot {
    fn from(configuration: &AnimationConfiguration) -> Self {
        Self {
            enabled: configuration.enabled,
            preset: configuration.preset.id().to_string(),
            speed: configuration.speed,
            overrides: configuration
                .overrides
                .iter()
                .map(|(slot, effect)| (slot.id().to_string(), effect.id().to_string()))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnimationPresetCapability {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnimationSlotCapability {
    pub id: String,
    pub compatible_effects: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnimationEffectCapability {
    pub id: String,
    pub availability: String,
    pub compatible_slots: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnimationCatalogSnapshot {
    pub presets: Vec<AnimationPresetCapability>,
    pub slots: Vec<AnimationSlotCapability>,
    pub effects: Vec<AnimationEffectCapability>,
}

impl Default for AnimationCatalogSnapshot {
    fn default() -> Self {
        Self {
            presets: AnimationPreset::ALL
                .into_iter()
                .map(|preset| AnimationPresetCapability {
                    id: preset.id().to_string(),
                })
                .collect(),
            slots: AnimationSlot::ALL
                .into_iter()
                .map(|slot| AnimationSlotCapability {
                    id: slot.id().to_string(),
                    compatible_effects: AnimationEffect::ALL
                        .into_iter()
                        .filter(|effect| effect.compatible_with(slot))
                        .map(|effect| effect.id().to_string())
                        .collect(),
                })
                .collect(),
            effects: AnimationEffect::ALL
                .into_iter()
                .map(|effect| AnimationEffectCapability {
                    id: effect.id().to_string(),
                    availability: effect.availability().to_string(),
                    compatible_slots: AnimationSlot::ALL
                        .into_iter()
                        .filter(|slot| effect.compatible_with(*slot))
                        .map(|slot| slot.id().to_string())
                        .collect(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnimationControlSnapshot {
    pub generation: u64,
    pub source: String,
    pub startup_override: bool,
    pub config: AnimationConfigurationSnapshot,
    pub effective: BTreeMap<String, String>,
    pub requested: BTreeMap<String, String>,
    pub catalog: AnimationCatalogSnapshot,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation_control::catalog::effect_for_request;

    #[test]
    fn snapshot_explicitly_reports_astrea_lamp_as_requested_but_none_effective() {
        let configuration = AnimationConfiguration::default();
        let mut requested = BTreeMap::new();
        let mut effective = BTreeMap::new();
        for slot in AnimationSlot::ALL {
            let effect = configuration.requested_effect(slot).0;
            requested.insert(slot.id().to_string(), effect.id().to_string());
            effective.insert(
                slot.id().to_string(),
                effect_for_request(slot, effect, configuration.enabled)
                    .id()
                    .to_string(),
            );
        }
        let snapshot = AnimationControlSnapshot {
            generation: 0,
            source: "default".into(),
            startup_override: false,
            config: (&configuration).into(),
            requested,
            effective,
            catalog: AnimationCatalogSnapshot::default(),
        };
        assert_eq!(snapshot.requested["window.minimize"], "minimize.lamp");
        assert_eq!(snapshot.effective["window.minimize"], "none");
        assert!(serde_json::to_vec(&snapshot).unwrap().len() < 16 * 1024);
    }
}
