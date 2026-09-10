use super::{RenderableSurface, SurfaceData, SurfaceOpaqueRegion, WindowBackend};
use crate::blur_policy::{
    BlurApplicationMode, BlurAssignment, BlurBackend, BlurLayerMode, BlurPolicySnapshot,
    BlurRuleAction, BlurTargetKind, BlurWindowTarget, CompiledBlurRules, SurfaceAlphaCapability,
};

#[derive(Debug)]
pub struct BlurAssignmentResolver {
    snapshot: BlurPolicySnapshot,
    rules: CompiledBlurRules,
    renderer_supported: bool,
}

impl Default for BlurAssignmentResolver {
    fn default() -> Self {
        Self::from_snapshot(BlurPolicySnapshot::default(), false)
            .expect("the built-in blur policy must compile")
    }
}

impl BlurAssignmentResolver {
    pub fn from_snapshot(
        mut snapshot: BlurPolicySnapshot,
        renderer_supported: bool,
    ) -> Result<Self, crate::blur_policy::BlurRuleCompileError> {
        let rules = CompiledBlurRules::compile(&snapshot.window_rules, &snapshot.layer_rules)?;
        snapshot.renderer_supported = renderer_supported;
        Ok(Self {
            snapshot,
            rules,
            renderer_supported,
        })
    }

    pub fn snapshot(&self) -> BlurPolicySnapshot {
        let mut snapshot = self.snapshot.clone();
        snapshot.renderer_supported = self.renderer_supported;
        snapshot
    }

    pub fn set_renderer_supported(&mut self, supported: bool) -> bool {
        if self.renderer_supported == supported {
            return false;
        }
        self.renderer_supported = supported;
        self.snapshot.renderer_supported = supported;
        true
    }

    pub fn replace_snapshot(
        &mut self,
        mut snapshot: BlurPolicySnapshot,
    ) -> Result<bool, crate::blur_policy::BlurRuleCompileError> {
        let rules = CompiledBlurRules::compile(&snapshot.window_rules, &snapshot.layer_rules)?;
        let changed = self.snapshot.version != snapshot.version
            || self.snapshot.enabled != snapshot.enabled
            || self.snapshot.applications != snapshot.applications
            || self.snapshot.layers != snapshot.layers
            || self.snapshot.window_rules != snapshot.window_rules
            || self.snapshot.layer_rules != snapshot.layer_rules;
        snapshot.renderer_supported = self.renderer_supported;
        snapshot.generation = if changed {
            self.snapshot.generation.saturating_add(1).max(1)
        } else {
            self.snapshot.generation
        };
        snapshot.last_error = None;
        self.snapshot = snapshot;
        self.rules = rules;
        Ok(changed)
    }

    pub fn set_last_error(&mut self, error: Option<String>) {
        self.snapshot.last_error = error;
    }

    pub fn resolve_client_request(&self, client_request_committed: bool) -> BlurAssignment {
        if self.renderer_supported && self.snapshot.enabled && client_request_committed {
            BlurAssignment::ClientExact
        } else {
            BlurAssignment::None
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn resolve_window(
        &self,
        app_id: Option<&str>,
        title: Option<&str>,
        backend: BlurBackend,
        alpha_capability: SurfaceAlphaCapability,
        full_opaque: bool,
        fullscreen: bool,
        client_request_committed: bool,
    ) -> BlurAssignment {
        let target = BlurWindowTarget {
            app_id,
            title,
            backend,
        };
        self.resolve_with_action(
            BlurTargetKind::Window,
            self.rules.last_window_action(target),
            alpha_capability,
            full_opaque,
            fullscreen,
            client_request_committed,
            self.automatic_window_eligible(backend),
        )
    }

    pub fn resolve_layer(
        &self,
        namespace: &str,
        alpha_capability: SurfaceAlphaCapability,
        full_opaque: bool,
        fullscreen: bool,
        client_request_committed: bool,
    ) -> BlurAssignment {
        let matching_action = self
            .rules
            .last_layer_action(crate::blur_policy::BlurLayerTarget { namespace });
        self.resolve_with_action(
            BlurTargetKind::Layer,
            matching_action,
            alpha_capability,
            full_opaque,
            fullscreen,
            client_request_committed,
            self.snapshot.layers.default == BlurLayerMode::Auto,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_with_action(
        &self,
        kind: BlurTargetKind,
        matching_action: Option<BlurRuleAction>,
        alpha_capability: SurfaceAlphaCapability,
        full_opaque: bool,
        fullscreen: bool,
        client_request_committed: bool,
        automatic_eligible: bool,
    ) -> BlurAssignment {
        if !self.renderer_supported || !self.snapshot.enabled {
            return BlurAssignment::None;
        }
        if matching_action == Some(BlurRuleAction::Disable) {
            return BlurAssignment::None;
        }
        if client_request_committed {
            return BlurAssignment::ClientExact;
        }
        if matching_action == Some(BlurRuleAction::Enable) {
            return BlurAssignment::CompositorSynthesized;
        }
        if kind == BlurTargetKind::Layer && self.snapshot.layers.default != BlurLayerMode::Auto {
            return BlurAssignment::None;
        }
        if automatic_eligible
            && alpha_capability == SurfaceAlphaCapability::AlphaCapable
            && !full_opaque
            && (!fullscreen || self.snapshot.applications.auto_fullscreen)
        {
            BlurAssignment::CompositorSynthesized
        } else {
            BlurAssignment::None
        }
    }

    fn automatic_window_eligible(&self, backend: BlurBackend) -> bool {
        match backend {
            BlurBackend::Wayland => self.snapshot.applications.wayland == BlurApplicationMode::Auto,
            BlurBackend::Xwayland => {
                self.snapshot.applications.xwayland == BlurApplicationMode::Auto
            }
        }
    }
}

impl super::CompositorState {
    pub(in crate::compositor) fn blur_assignment_for_surface(
        &self,
        surface: &RenderableSurface,
        surface_data: &SurfaceData,
    ) -> BlurAssignment {
        let root_surface_id = self.root_surface_id_for_surface(surface.surface_id);
        let alpha_capability = surface.buffer.alpha_capability();
        let full_opaque = matches!(surface.opaque_region(), SurfaceOpaqueRegion::Full);
        let fullscreen = self
            .fullscreen_presentation
            .is_some_and(|state| state.owner_root_surface_id == root_surface_id);
        let client_request_committed = !surface_data.committed_background_effect().ops().is_empty();

        if let Some(layer) = self.layer_surfaces.get(&root_surface_id) {
            return self.blur_assignment.resolve_layer(
                &layer.namespace,
                alpha_capability,
                full_opaque,
                fullscreen,
                client_request_committed,
            );
        }

        let Some(window) = self
            .window_id_for_surface(root_surface_id)
            .and_then(|window_id| self.desktop_windows.get(&window_id))
        else {
            return self
                .blur_assignment
                .resolve_client_request(client_request_committed);
        };
        let backend = match window.backend {
            WindowBackend::Xdg(_) => BlurBackend::Wayland,
            WindowBackend::X11(_) => BlurBackend::Xwayland,
        };
        self.blur_assignment.resolve_window(
            window.metadata.app_id.as_deref(),
            window.metadata.title.as_deref(),
            backend,
            alpha_capability,
            full_opaque,
            fullscreen,
            client_request_committed,
        )
    }

    pub(in crate::compositor) fn blur_policy_snapshot(
        &self,
    ) -> crate::blur_policy::BlurPolicySnapshot {
        self.blur_assignment.snapshot()
    }

    pub(in crate::compositor) fn reload_blur_policy_from_disk(
        &mut self,
    ) -> Result<crate::blur_policy::BlurPolicySnapshot, String> {
        let (path, mut snapshot) = match crate::blur_policy::load() {
            Ok(value) => value,
            Err(error) => {
                let error = error.to_string();
                self.blur_assignment.set_last_error(Some(error.clone()));
                return Err(error);
            }
        };
        snapshot.config_path = path.display().to_string();
        let changed = match self.blur_assignment.replace_snapshot(snapshot) {
            Ok(changed) => changed,
            Err(error) => {
                let error = error.to_string();
                self.blur_assignment.set_last_error(Some(error.clone()));
                return Err(error);
            }
        };
        if changed {
            self.advance_render_generation_with_scene_effect(
                super::RenderGenerationCause::EffectBinding,
                true,
            );
            self.refresh_effect_scene_summary();
        }
        Ok(self.blur_assignment.snapshot())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blur_policy::{BlurLayerPolicy, BlurLayerRule, BlurWindowRule};

    fn make_resolver() -> BlurAssignmentResolver {
        BlurAssignmentResolver::from_snapshot(BlurPolicySnapshot::default(), true)
            .expect("default policy")
    }

    fn app_rule(action: BlurRuleAction) -> BlurWindowRule {
        BlurWindowRule {
            app_id: Some("org.example.app".to_string()),
            title: None,
            backend: None,
            action,
        }
    }

    fn resolve_fullscreen(resolver: &BlurAssignmentResolver) -> BlurAssignment {
        resolver.resolve_window(
            Some("org.example.app"),
            None,
            BlurBackend::Wayland,
            SurfaceAlphaCapability::AlphaCapable,
            false,
            true,
            false,
        )
    }

    #[test]
    fn precedence_is_renderer_policy_disable_client_enable_auto_none() {
        let mut resolver = make_resolver();
        resolver.set_renderer_supported(false);
        assert_eq!(
            resolver.resolve_window(
                Some("org.example.app"),
                None,
                BlurBackend::Wayland,
                SurfaceAlphaCapability::AlphaCapable,
                false,
                false,
                true,
            ),
            BlurAssignment::None
        );

        let mut resolver = make_resolver();
        let mut snapshot = resolver.snapshot();
        snapshot.enabled = false;
        resolver.replace_snapshot(snapshot).expect("policy");
        assert_eq!(
            resolver.resolve_window(
                None,
                None,
                BlurBackend::Wayland,
                SurfaceAlphaCapability::AlphaCapable,
                false,
                false,
                true,
            ),
            BlurAssignment::None
        );

        let mut resolver = make_resolver();
        let mut snapshot = resolver.snapshot();
        snapshot.window_rules = vec![app_rule(BlurRuleAction::Disable)];
        resolver.replace_snapshot(snapshot).expect("policy");
        assert_eq!(
            resolver.resolve_window(
                Some("org.example.app"),
                None,
                BlurBackend::Wayland,
                SurfaceAlphaCapability::AlphaCapable,
                false,
                false,
                true,
            ),
            BlurAssignment::None
        );

        let resolver = make_resolver();
        assert_eq!(
            resolver.resolve_window(
                None,
                None,
                BlurBackend::Wayland,
                SurfaceAlphaCapability::Unknown,
                true,
                true,
                true,
            ),
            BlurAssignment::ClientExact
        );

        let mut resolver = make_resolver();
        let mut snapshot = resolver.snapshot();
        snapshot.window_rules = vec![app_rule(BlurRuleAction::Enable)];
        resolver.replace_snapshot(snapshot).expect("policy");
        assert_eq!(
            resolver.resolve_window(
                Some("org.example.app"),
                None,
                BlurBackend::Wayland,
                SurfaceAlphaCapability::Opaque,
                true,
                true,
                false,
            ),
            BlurAssignment::CompositorSynthesized
        );

        assert_eq!(
            make_resolver().resolve_window(
                None,
                None,
                BlurBackend::Wayland,
                SurfaceAlphaCapability::AlphaCapable,
                false,
                false,
                false,
            ),
            BlurAssignment::CompositorSynthesized
        );
        assert_eq!(
            make_resolver().resolve_window(
                None,
                None,
                BlurBackend::Xwayland,
                SurfaceAlphaCapability::AlphaCapable,
                false,
                false,
                false,
            ),
            BlurAssignment::None
        );
    }

    #[test]
    fn fullscreen_auto_is_off_but_explicit_enable_overrides_it() {
        assert_eq!(
            make_resolver().resolve_window(
                None,
                None,
                BlurBackend::Wayland,
                SurfaceAlphaCapability::AlphaCapable,
                false,
                true,
                false,
            ),
            BlurAssignment::None
        );
        let mut resolver = make_resolver();
        let mut snapshot = resolver.snapshot();
        snapshot.window_rules = vec![app_rule(BlurRuleAction::Enable)];
        resolver.replace_snapshot(snapshot).expect("policy");
        assert_eq!(
            resolver.resolve_window(
                Some("org.example.app"),
                None,
                BlurBackend::Wayland,
                SurfaceAlphaCapability::AlphaCapable,
                false,
                true,
                false,
            ),
            BlurAssignment::CompositorSynthesized
        );
    }

    #[test]
    fn fullscreen_assignment_transition_matches_the_existing_scanout_blocker() {
        let mut resolver = make_resolver();

        assert_eq!(resolve_fullscreen(&resolver), BlurAssignment::None);
        assert_eq!(
            crate::compositor::direct_scanout_scene_rejection_for_effects(
                crate::compositor::EffectSceneSummary::default()
            ),
            None
        );

        let mut snapshot = resolver.snapshot();
        snapshot.window_rules = vec![app_rule(BlurRuleAction::Enable)];
        resolver.replace_snapshot(snapshot).expect("policy");
        assert_eq!(
            resolve_fullscreen(&resolver),
            BlurAssignment::CompositorSynthesized
        );
        assert_eq!(
            crate::compositor::direct_scanout_scene_rejection_for_effects(
                crate::compositor::EffectSceneSummary {
                    visible_instance_count: 1,
                    requires_composition: true,
                    ..Default::default()
                }
            ),
            Some(crate::compositor::DirectScanoutSceneRejection::EffectRequiresComposition)
        );

        let mut snapshot = resolver.snapshot();
        snapshot.window_rules.clear();
        resolver.replace_snapshot(snapshot).expect("policy");
        assert_eq!(resolve_fullscreen(&resolver), BlurAssignment::None);
        assert_eq!(
            crate::compositor::direct_scanout_scene_rejection_for_effects(
                crate::compositor::EffectSceneSummary::default()
            ),
            None
        );
    }

    #[test]
    fn layer_rules_are_independent_and_last_match_wins() {
        let mut resolver = make_resolver();
        let mut snapshot = resolver.snapshot();
        snapshot.layers = BlurLayerPolicy {
            default: BlurLayerMode::ClientOnly,
        };
        snapshot.layer_rules = vec![
            BlurLayerRule {
                namespace: Some("astrea-.*".to_string()),
                action: BlurRuleAction::Enable,
            },
            BlurLayerRule {
                namespace: Some("astrea-dock".to_string()),
                action: BlurRuleAction::Disable,
            },
        ];
        resolver.replace_snapshot(snapshot).expect("policy");
        assert_eq!(
            resolver.resolve_layer(
                "astrea-dock",
                SurfaceAlphaCapability::Unknown,
                true,
                false,
                false,
            ),
            BlurAssignment::None
        );
        assert_eq!(
            resolver.resolve_layer(
                "astrea-spotlight",
                SurfaceAlphaCapability::Unknown,
                true,
                false,
                false,
            ),
            BlurAssignment::CompositorSynthesized
        );
    }

    #[test]
    fn policy_snapshot_is_serializable_for_control_status() {
        let snapshot = make_resolver().snapshot();
        let value = serde_json::to_value(snapshot).expect("serialize snapshot");
        assert_eq!(value["enabled"], true);
        assert_eq!(value["renderer_supported"], true);
    }
}
