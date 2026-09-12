//! Typhon-owned animation configuration, resolution, and runtime policy.

mod catalog;
mod config;
mod persistence;
mod snapshot;

pub use catalog::{AnimationEffect, AnimationPreset, AnimationSlot};
pub use config::{
    ANIMATION_CONFIGURATION_VERSION, AnimationConfiguration, AnimationConfigurationDocument,
    AnimationConfigurationError, MAX_ANIMATION_SPEED, MIN_ANIMATION_SPEED,
};
pub use persistence::{AnimationConfigurationStore, AnimationPersistenceError, MAX_DOCUMENT_BYTES};
pub use snapshot::{
    AnimationCatalogSnapshot, AnimationConfigurationSnapshot, AnimationControlSnapshot,
};

/// Runtime capabilities confirmed by the compositor renderer boundary.
///
/// This intentionally contains no renderer objects or GL state. A missing
/// confirmation is represented by `false`, so an animation cannot start on an
/// optimistic capability assumption.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnimationRuntimeCapabilities {
    pub lamp_renderer: bool,
}

use crate::presentation_animation::{AnimationCurve, SpringSpec};
use crate::presentation_animation_policy::{
    PresentationAnimationKind, PresentationAnimationPolicy, PresentationAnimationStyle,
    presentation_animation_style_from_env,
};
use catalog::effect_for_request;
use std::{collections::BTreeMap, time::Duration};

const MIN_DURATION: Duration = Duration::from_nanos(1);

#[derive(Debug, Clone, PartialEq)]
pub struct AnimationControlState {
    configuration: AnimationConfiguration,
    store: AnimationConfigurationStore,
    generation: u64,
    source: String,
    startup_override: bool,
    legacy_style: Option<PresentationAnimationStyle>,
}

impl Default for AnimationControlState {
    fn default() -> Self {
        Self::from_environment()
    }
}

impl AnimationControlState {
    pub fn from_environment() -> Self {
        let store = AnimationConfigurationStore::from_environment()
            .unwrap_or_else(AnimationConfigurationStore::unavailable);
        Self::from_store(store)
    }

    pub fn from_store(store: AnimationConfigurationStore) -> Self {
        let (mut configuration, mut source) = match store.read() {
            Ok(configuration) => (configuration, "persisted".to_string()),
            Err(AnimationPersistenceError::Missing)
            | Err(AnimationPersistenceError::Invalid)
            | Err(AnimationPersistenceError::Insecure) => {
                (AnimationConfiguration::default(), "default".to_string())
            }
            Err(_) => (AnimationConfiguration::default(), "default".to_string()),
        };
        let mut startup_override = false;
        if let Ok(value) = std::env::var("OBLIVION_ONE_ANIMATIONS") {
            match value.as_str() {
                "on" => {
                    configuration.enabled = true;
                    startup_override = true;
                }
                "off" => {
                    configuration.enabled = false;
                    startup_override = true;
                }
                _ => {}
            }
        }
        let legacy_style = std::env::var("OBLIVION_ONE_ANIMATION_STYLE")
            .ok()
            .filter(|value| !value.is_empty())
            .map(|value| presentation_animation_style_from_env(Some(&value)));
        if legacy_style.is_some() {
            startup_override = true;
        }
        if startup_override {
            source = "environment".to_string();
        }
        Self {
            configuration,
            store,
            generation: 0,
            source,
            startup_override,
            legacy_style,
        }
    }

    pub fn configuration(&self) -> &AnimationConfiguration {
        &self.configuration
    }
    pub fn enabled(&self) -> bool {
        self.configuration.enabled
    }

    pub fn effective_effect(
        &self,
        slot: AnimationSlot,
        runtime_capabilities: AnimationRuntimeCapabilities,
    ) -> AnimationEffect {
        let mut requested = self.configuration.requested_effect(slot).0;
        if !self.configuration.overrides.contains_key(&slot)
            && let (Some(style), Some(_)) = (self.legacy_style, slot.geometry_kind())
        {
            requested = match style {
                PresentationAnimationStyle::Macos => AnimationEffect::GeometryMacos,
                PresentationAnimationStyle::Kde => AnimationEffect::GeometryKde,
            };
        }
        effect_for_request(
            slot,
            requested,
            self.configuration.enabled,
            runtime_capabilities,
        )
    }

    pub fn curve_for(&self, kind: PresentationAnimationKind) -> Option<AnimationCurve> {
        let slot = AnimationSlot::from_geometry_kind(kind);
        match self.effective_effect(slot, AnimationRuntimeCapabilities::default()) {
            AnimationEffect::GeometryKde => Some(scale_curve(
                PresentationAnimationPolicy::kde().curve_for(kind),
                self.configuration.speed,
            )),
            AnimationEffect::GeometryMacos => Some(scale_curve(
                PresentationAnimationPolicy::macos().curve_for(kind),
                self.configuration.speed,
            )),
            _ => None,
        }
    }

    pub fn snapshot(
        &self,
        runtime_capabilities: AnimationRuntimeCapabilities,
    ) -> AnimationControlSnapshot {
        let mut requested = BTreeMap::new();
        let mut effective = BTreeMap::new();
        for slot in AnimationSlot::ALL {
            requested.insert(
                slot.id().to_string(),
                self.configuration.requested_effect(slot).0.id().to_string(),
            );
            effective.insert(
                slot.id().to_string(),
                self.effective_effect(slot, runtime_capabilities)
                    .id()
                    .to_string(),
            );
        }
        AnimationControlSnapshot {
            generation: self.generation,
            source: self.source.clone(),
            startup_override: self.startup_override,
            config: (&self.configuration).into(),
            effective,
            requested,
            catalog: AnimationCatalogSnapshot::for_runtime_capabilities(runtime_capabilities),
        }
    }

    pub fn set_configuration(
        &mut self,
        candidate: AnimationConfiguration,
        runtime_capabilities: AnimationRuntimeCapabilities,
    ) -> Result<AnimationControlSnapshot, AnimationPersistenceError> {
        candidate
            .validate()
            .map_err(|_| AnimationPersistenceError::Invalid)?;
        candidate
            .validate_runtime_mutation(&self.configuration, runtime_capabilities)
            .map_err(|_| AnimationPersistenceError::Invalid)?;
        self.store.write(&candidate)?;
        self.configuration = candidate;
        self.source = "runtime".to_string();
        self.startup_override = false;
        self.legacy_style = None;
        self.generation = self.generation.saturating_add(1);
        Ok(self.snapshot(runtime_capabilities))
    }
}

fn scale_curve(curve: AnimationCurve, speed: f64) -> AnimationCurve {
    debug_assert!(
        speed.is_finite() && (MIN_ANIMATION_SPEED..=MAX_ANIMATION_SPEED).contains(&speed)
    );
    if speed == 1.0 {
        return curve;
    }
    match curve {
        AnimationCurve::Easing { duration, curve } => {
            AnimationCurve::easing(duration.mul_f64(1.0 / speed).max(MIN_DURATION), curve)
        }
        AnimationCurve::Spring(spec) => AnimationCurve::spring(
            SpringSpec::new(spec.stiffness() * speed * speed, spec.damping() * speed)
                .with_settlement(spec.displacement_epsilon(), spec.velocity_epsilon() * speed),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation_animation::{AnimationTime, EasingCurve, PresentationRect};
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_directory() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "typhon-animation-state-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn speed_one_is_exactly_the_existing_kde_curve() {
        let directory = temp_directory();
        let state = AnimationControlState::from_store(
            AnimationConfigurationStore::new(directory.clone()).unwrap(),
        );
        assert_eq!(
            state.curve_for(PresentationAnimationKind::ProgrammaticMove),
            Some(
                PresentationAnimationPolicy::kde()
                    .curve_for(PresentationAnimationKind::ProgrammaticMove)
            )
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn speed_scales_easing_and_spring_time_domains() {
        assert_eq!(
            scale_curve(
                AnimationCurve::easing(Duration::from_millis(200), EasingCurve::EaseOutCubic),
                2.0
            ),
            AnimationCurve::easing(Duration::from_millis(100), EasingCurve::EaseOutCubic)
        );
        let AnimationCurve::Spring(spec) = scale_curve(
            AnimationCurve::spring(SpringSpec::new(240.0, 32.0).with_settlement(0.5, 8.0)),
            0.5,
        ) else {
            panic!("expected spring")
        };
        assert_eq!(spec.stiffness(), 60.0);
        assert_eq!(spec.damping(), 16.0);
        assert_eq!(spec.displacement_epsilon(), 0.5);
        assert_eq!(spec.velocity_epsilon(), 4.0);
    }

    #[test]
    fn persistence_failure_keeps_runtime_and_generation_unchanged() {
        let store =
            AnimationConfigurationStore::unavailable(AnimationPersistenceError::WriteFailed);
        let mut state = AnimationControlState::from_store(store);
        let before = state.snapshot(AnimationRuntimeCapabilities::default());
        let candidate = AnimationConfiguration {
            preset: AnimationPreset::Macos,
            ..AnimationConfiguration::default()
        };
        assert_eq!(
            state.set_configuration(candidate, AnimationRuntimeCapabilities::default()),
            Err(AnimationPersistenceError::WriteFailed)
        );
        assert_eq!(
            state.snapshot(AnimationRuntimeCapabilities::default()),
            before
        );
    }

    #[test]
    fn snapshots_project_runtime_lamp_truth_without_rewriting_requested_intent() {
        let directory = temp_directory();
        let state = AnimationControlState::from_store(
            AnimationConfigurationStore::new(directory.clone()).unwrap(),
        );
        let available = state.snapshot(AnimationRuntimeCapabilities {
            lamp_renderer: true,
        });
        assert_eq!(available.requested["window.minimize"], "minimize.lamp");
        assert_eq!(available.effective["window.minimize"], "minimize.lamp");
        assert_eq!(
            available
                .catalog
                .effects
                .iter()
                .find(|effect| effect.id == "minimize.lamp")
                .unwrap()
                .availability,
            "available"
        );

        let unavailable = state.snapshot(AnimationRuntimeCapabilities::default());
        assert_eq!(unavailable.requested["window.minimize"], "minimize.lamp");
        assert_eq!(unavailable.effective["window.minimize"], "none");
        assert_eq!(
            unavailable
                .catalog
                .effects
                .iter()
                .find(|effect| effect.id == "minimize.lamp")
                .unwrap()
                .availability,
            "unavailable"
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn existing_lamp_override_survives_capability_loss_for_unrelated_mutations() {
        let directory = temp_directory();
        let mut state = AnimationControlState::from_store(
            AnimationConfigurationStore::new(directory.clone()).unwrap(),
        );
        let mut lamp = AnimationConfiguration::default();
        lamp.overrides
            .insert(AnimationSlot::WindowMinimize, AnimationEffect::MinimizeLamp);
        state
            .set_configuration(
                lamp.clone(),
                AnimationRuntimeCapabilities {
                    lamp_renderer: true,
                },
            )
            .unwrap();
        let mut speed = lamp.clone();
        speed.speed = 1.25;
        let snapshot = state
            .set_configuration(speed, AnimationRuntimeCapabilities::default())
            .unwrap();
        assert_eq!(snapshot.requested["window.minimize"], "minimize.lamp");
        assert_eq!(snapshot.effective["window.minimize"], "none");

        let mut removed = lamp;
        removed.overrides.clear();
        assert!(
            state
                .set_configuration(removed, AnimationRuntimeCapabilities::default())
                .is_ok()
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn disabling_animator_cancels_active_transition_without_synthesizing_frames() {
        let mut animator = crate::presentation_animation::PresentationAnimator::enabled();
        let start = PresentationRect::new(0.0, 0.0, 1.0, 1.0).unwrap();
        let target = PresentationRect::new(10.0, 0.0, 1.0, 1.0).unwrap();
        animator.start(
            1,
            start,
            target,
            AnimationTime::from_nanos(0),
            PresentationAnimationPolicy::kde()
                .curve_for(PresentationAnimationKind::ProgrammaticMove),
        );
        animator.set_enabled(false);
        assert_eq!(animator.active_count(), 0);
    }
}
