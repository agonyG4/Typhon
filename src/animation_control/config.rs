//! Validated user intent for the animation control plane.

use super::catalog::{AnimationEffect, AnimationPreset, AnimationSlot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const ANIMATION_CONFIGURATION_VERSION: u32 = 1;
pub const MIN_ANIMATION_SPEED: f64 = 0.5;
pub const MAX_ANIMATION_SPEED: f64 = 2.0;
pub const MAX_ANIMATION_OVERRIDES: usize = AnimationSlot::ALL.len();

#[derive(Debug, Clone, PartialEq)]
pub struct AnimationConfiguration {
    pub enabled: bool,
    pub preset: AnimationPreset,
    pub speed: f64,
    pub overrides: BTreeMap<AnimationSlot, AnimationEffect>,
}

impl Default for AnimationConfiguration {
    fn default() -> Self {
        Self {
            enabled: true,
            preset: AnimationPreset::Astrea,
            speed: 1.0,
            overrides: BTreeMap::new(),
        }
    }
}

impl AnimationConfiguration {
    pub fn validate(&self) -> Result<(), AnimationConfigurationError> {
        if !self.speed.is_finite()
            || !(MIN_ANIMATION_SPEED..=MAX_ANIMATION_SPEED).contains(&self.speed)
        {
            return Err(AnimationConfigurationError::InvalidSpeed);
        }
        if self.overrides.len() > MAX_ANIMATION_OVERRIDES {
            return Err(AnimationConfigurationError::TooManyOverrides);
        }
        for (&slot, &effect) in &self.overrides {
            if !effect.is_available() {
                return Err(AnimationConfigurationError::PlannedEffect { slot, effect });
            }
            if !effect.compatible_with(slot) {
                return Err(AnimationConfigurationError::IncompatibleEffect { slot, effect });
            }
        }
        Ok(())
    }

    pub fn requested_effect(&self, slot: AnimationSlot) -> (AnimationEffect, bool) {
        match self.overrides.get(&slot) {
            Some(effect) => (*effect, true),
            None => (self.preset.requested_effect(slot), false),
        }
    }

    pub fn clear_overrides(&mut self) {
        self.overrides.clear();
    }

    pub fn to_document(&self) -> AnimationConfigurationDocument {
        AnimationConfigurationDocument {
            version: ANIMATION_CONFIGURATION_VERSION,
            enabled: self.enabled,
            preset: self.preset.id().to_string(),
            speed: self.speed,
            overrides: self
                .overrides
                .iter()
                .map(|(slot, effect)| (slot.id().to_string(), effect.id().to_string()))
                .collect(),
        }
    }

    pub fn from_document(
        document: AnimationConfigurationDocument,
    ) -> Result<Self, AnimationConfigurationError> {
        if document.version != ANIMATION_CONFIGURATION_VERSION {
            return Err(AnimationConfigurationError::UnsupportedVersion(
                document.version,
            ));
        }
        if document.overrides.len() > MAX_ANIMATION_OVERRIDES {
            return Err(AnimationConfigurationError::TooManyOverrides);
        }
        let preset = AnimationPreset::parse(&document.preset)
            .ok_or_else(|| AnimationConfigurationError::UnknownPreset(document.preset.clone()))?;
        let mut overrides = BTreeMap::new();
        for (slot_id, effect_id) in document.overrides {
            let slot = AnimationSlot::parse(&slot_id)
                .ok_or(AnimationConfigurationError::UnknownSlot(slot_id))?;
            let effect = AnimationEffect::parse(&effect_id)
                .ok_or(AnimationConfigurationError::UnknownEffect(effect_id))?;
            if overrides.insert(slot, effect).is_some() {
                return Err(AnimationConfigurationError::DuplicateSlot(slot));
            }
        }
        let configuration = Self {
            enabled: document.enabled,
            preset,
            speed: document.speed,
            overrides,
        };
        configuration.validate()?;
        Ok(configuration)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnimationConfigurationDocument {
    pub version: u32,
    pub enabled: bool,
    pub preset: String,
    pub speed: f64,
    #[serde(default)]
    pub overrides: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AnimationConfigurationError {
    UnsupportedVersion(u32),
    UnknownPreset(String),
    UnknownSlot(String),
    UnknownEffect(String),
    DuplicateSlot(AnimationSlot),
    InvalidSpeed,
    TooManyOverrides,
    PlannedEffect {
        slot: AnimationSlot,
        effect: AnimationEffect,
    },
    IncompatibleEffect {
        slot: AnimationSlot,
        effect: AnimationEffect,
    },
}

impl std::fmt::Display for AnimationConfigurationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => write!(formatter, "unsupported version {version}"),
            Self::UnknownPreset(value) => write!(formatter, "unknown preset {value}"),
            Self::UnknownSlot(value) => write!(formatter, "unknown slot {value}"),
            Self::UnknownEffect(value) => write!(formatter, "unknown effect {value}"),
            Self::DuplicateSlot(slot) => write!(formatter, "duplicate slot {}", slot.id()),
            Self::InvalidSpeed => formatter.write_str("animation speed is outside 0.5..=2.0"),
            Self::TooManyOverrides => formatter.write_str("too many animation overrides"),
            Self::PlannedEffect { slot, effect } => write!(
                formatter,
                "effect {} for slot {} is planned",
                effect.id(),
                slot.id()
            ),
            Self::IncompatibleEffect { slot, effect } => write!(
                formatter,
                "effect {} is incompatible with slot {}",
                effect.id(),
                slot.id()
            ),
        }
    }
}

impl std::error::Error for AnimationConfigurationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_default_is_astrea_at_normal_speed() {
        let config = AnimationConfiguration::default();
        assert!(config.enabled);
        assert_eq!(config.preset, AnimationPreset::Astrea);
        assert_eq!(config.speed, 1.0);
        assert!(config.overrides.is_empty());
    }

    #[test]
    fn override_wins_and_clearing_restores_the_preset() {
        let mut config = AnimationConfiguration::default();
        config
            .overrides
            .insert(AnimationSlot::WindowMove, AnimationEffect::GeometryMacos);
        assert_eq!(
            config.requested_effect(AnimationSlot::WindowMove),
            (AnimationEffect::GeometryMacos, true)
        );
        config.clear_overrides();
        assert_eq!(
            config.requested_effect(AnimationSlot::WindowMove),
            (AnimationEffect::GeometryKde, false)
        );
    }

    #[test]
    fn invalid_speed_and_planned_manual_override_are_rejected() {
        let mut document = AnimationConfiguration::default().to_document();
        document.speed = 3.0;
        assert_eq!(
            AnimationConfiguration::from_document(document),
            Err(AnimationConfigurationError::InvalidSpeed)
        );
        let mut document = AnimationConfiguration::default().to_document();
        document
            .overrides
            .insert("window.minimize".into(), "minimize.lamp".into());
        assert!(matches!(
            AnimationConfiguration::from_document(document),
            Err(AnimationConfigurationError::PlannedEffect { .. })
        ));
    }

    #[test]
    fn document_round_trips() {
        let mut config = AnimationConfiguration::default();
        config
            .overrides
            .insert(AnimationSlot::WindowMove, AnimationEffect::GeometryMacos);
        assert_eq!(
            AnimationConfiguration::from_document(config.to_document()).unwrap(),
            config
        );
    }
}
