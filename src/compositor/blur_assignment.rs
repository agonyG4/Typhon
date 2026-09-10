use super::{RenderableSurface, SurfaceData, SurfaceOpaqueRegion, WindowBackend};
use crate::blur_policy::{
    BlurApplicationMode, BlurAssignment, BlurAssignmentCounts, BlurBackend, BlurPolicyConfig,
    BlurPolicySnapshot, BlurRuleAction, BlurTargetKind, BlurWindowTarget, CompiledBlurRules,
    SurfaceAlphaCapability,
};
use crate::compositor::effects::EffectAnchorScope;
use crate::effects::{EffectRect, EffectRegion};
use std::path::Path;
use wayland_server::Resource;

const MAX_CONTROL_DETAIL_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlurAssignmentSource {
    Client,
    WaylandAuto,
    WindowRule,
    LayerRule,
}

impl BlurAssignmentSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::WaylandAuto => "wayland_auto",
            Self::WindowRule => "window_rule",
            Self::LayerRule => "layer_rule",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedBlurAssignment {
    pub source: BlurAssignmentSource,
    pub anchor_scope: EffectAnchorScope,
    pub target_surface_id: u32,
    pub region: EffectRegion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BlurAssignmentDecision {
    source: BlurAssignmentSource,
    anchor_scope: EffectAnchorScope,
}

#[derive(Debug)]
pub struct BlurAssignmentResolver {
    config: BlurPolicyConfig,
    rules: CompiledBlurRules,
    renderer_supported: bool,
    generation: u64,
    config_path: String,
    last_reload_error: Option<String>,
}

impl Default for BlurAssignmentResolver {
    fn default() -> Self {
        Self::from_config(BlurPolicyConfig::default(), false).expect("built-in blur policy")
    }
}

impl BlurAssignmentResolver {
    pub fn from_config(
        config: BlurPolicyConfig,
        renderer_supported: bool,
    ) -> Result<Self, crate::blur_policy::BlurRuleCompileError> {
        let rules = CompiledBlurRules::compile(&config.window_rules, &config.layer_rules)?;
        Ok(Self {
            config,
            rules,
            renderer_supported,
            generation: 1,
            config_path: String::new(),
            last_reload_error: None,
        })
    }

    pub fn from_snapshot(
        config: BlurPolicyConfig,
        renderer_supported: bool,
    ) -> Result<Self, crate::blur_policy::BlurRuleCompileError> {
        Self::from_config(config, renderer_supported)
    }

    pub fn config(&self) -> BlurPolicyConfig {
        self.config.clone()
    }

    pub fn snapshot(&self) -> BlurPolicySnapshot {
        self.status(BlurAssignmentCounts::default())
    }

    pub fn status(&self, counts: BlurAssignmentCounts) -> BlurPolicySnapshot {
        BlurPolicySnapshot {
            enabled: self.config.enabled,
            renderer_supported: self.renderer_supported,
            generation: self.generation,
            wayland_mode: self.config.applications.wayland,
            xwayland_mode: self.config.applications.xwayland,
            auto_fullscreen: self.config.applications.auto_fullscreen,
            layer_default: self.config.layers.default,
            window_rule_count: self.config.window_rules.len() as u32,
            layer_rule_count: self.config.layer_rules.len() as u32,
            active_assignment_count: counts.client
                + counts.wayland_auto
                + counts.window_rule
                + counts.layer_rule,
            assignment_counts_by_source: counts,
            config_path: self.config_path.clone(),
            last_reload_error: self.last_reload_error.clone(),
        }
    }

    pub fn set_renderer_supported(&mut self, supported: bool) -> bool {
        if self.renderer_supported == supported {
            return false;
        }
        self.renderer_supported = supported;
        true
    }

    pub fn replace_config(
        &mut self,
        config: BlurPolicyConfig,
    ) -> Result<bool, crate::blur_policy::BlurRuleCompileError> {
        let rules = CompiledBlurRules::compile(&config.window_rules, &config.layer_rules)?;
        let changed = self.config != config;
        if changed {
            self.generation = self.generation.saturating_add(1).max(1);
        }
        self.config = config;
        self.rules = rules;
        self.last_reload_error = None;
        Ok(changed)
    }

    pub fn replace_snapshot(
        &mut self,
        config: BlurPolicyConfig,
    ) -> Result<bool, crate::blur_policy::BlurRuleCompileError> {
        self.replace_config(config)
    }

    pub fn set_config_path(&mut self, path: &Path) {
        self.config_path = bounded_text(path.display().to_string());
    }
    pub fn set_last_error(&mut self, error: Option<String>) {
        self.last_reload_error = error.map(bounded_text);
    }

    pub fn resolve_client_request(&self, committed: bool) -> BlurAssignment {
        if self.renderer_supported && self.config.enabled && committed {
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
        self.resolve_window_decision(
            app_id,
            title,
            backend,
            alpha_capability,
            full_opaque,
            fullscreen,
            client_request_committed,
        )
        .map_or(BlurAssignment::None, |decision| {
            if decision.source == BlurAssignmentSource::Client {
                BlurAssignment::ClientExact
            } else {
                BlurAssignment::CompositorSynthesized
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn resolve_window_assignment(
        &self,
        target_surface_id: u32,
        client_region: EffectRegion,
        candidate_region: EffectRegion,
        opaque_region: &EffectRegion,
        full_opaque: bool,
        app_id: Option<&str>,
        title: Option<&str>,
        backend: BlurBackend,
        alpha_capability: SurfaceAlphaCapability,
        fullscreen: bool,
        client_request_committed: bool,
    ) -> Option<ResolvedBlurAssignment> {
        let decision = self.resolve_window_decision(
            app_id,
            title,
            backend,
            alpha_capability,
            full_opaque,
            fullscreen,
            client_request_committed,
        )?;
        self.materialize_assignment(
            target_surface_id,
            client_region,
            candidate_region,
            opaque_region,
            decision,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn resolve_layer_assignment(
        &self,
        target_surface_id: u32,
        client_region: EffectRegion,
        candidate_region: EffectRegion,
        opaque_region: &EffectRegion,
        full_opaque: bool,
        namespace: &str,
        alpha_capability: SurfaceAlphaCapability,
        fullscreen: bool,
        client_request_committed: bool,
    ) -> Option<ResolvedBlurAssignment> {
        let decision = self.resolve_decision(
            BlurTargetKind::Layer,
            self.rules
                .last_layer_action(crate::blur_policy::BlurLayerTarget { namespace }),
            alpha_capability,
            full_opaque,
            fullscreen,
            client_request_committed,
            false,
        )?;
        self.materialize_assignment(
            target_surface_id,
            client_region,
            candidate_region,
            opaque_region,
            decision,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_window_decision(
        &self,
        app_id: Option<&str>,
        title: Option<&str>,
        backend: BlurBackend,
        alpha_capability: SurfaceAlphaCapability,
        full_opaque: bool,
        fullscreen: bool,
        client_request_committed: bool,
    ) -> Option<BlurAssignmentDecision> {
        self.resolve_decision(
            BlurTargetKind::Window,
            self.rules.last_window_action(BlurWindowTarget {
                app_id,
                title,
                backend,
            }),
            alpha_capability,
            full_opaque,
            fullscreen,
            client_request_committed,
            backend == BlurBackend::Wayland
                && self.config.applications.wayland == BlurApplicationMode::Auto,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_decision(
        &self,
        kind: BlurTargetKind,
        matching_action: Option<BlurRuleAction>,
        alpha_capability: SurfaceAlphaCapability,
        full_opaque: bool,
        fullscreen: bool,
        client_request_committed: bool,
        automatic_eligible: bool,
    ) -> Option<BlurAssignmentDecision> {
        if !self.renderer_supported || !self.config.enabled {
            return None;
        }
        if matching_action == Some(BlurRuleAction::Disable) {
            return None;
        }
        if client_request_committed {
            return Some(BlurAssignmentDecision {
                source: BlurAssignmentSource::Client,
                anchor_scope: EffectAnchorScope::Surface,
            });
        }
        if matching_action == Some(BlurRuleAction::Enable) {
            return Some(BlurAssignmentDecision {
                source: if kind == BlurTargetKind::Window {
                    BlurAssignmentSource::WindowRule
                } else {
                    BlurAssignmentSource::LayerRule
                },
                anchor_scope: if kind == BlurTargetKind::Window {
                    EffectAnchorScope::VisualGroup
                } else {
                    EffectAnchorScope::Surface
                },
            });
        }
        if kind == BlurTargetKind::Layer || !automatic_eligible {
            return None;
        }
        (alpha_capability == SurfaceAlphaCapability::AlphaCapable
            && !full_opaque
            && (!fullscreen || self.config.applications.auto_fullscreen))
            .then_some(BlurAssignmentDecision {
                source: BlurAssignmentSource::WaylandAuto,
                anchor_scope: EffectAnchorScope::VisualGroup,
            })
    }

    fn materialize_assignment(
        &self,
        target_surface_id: u32,
        client_region: EffectRegion,
        candidate_region: EffectRegion,
        opaque_region: &EffectRegion,
        decision: BlurAssignmentDecision,
    ) -> Option<ResolvedBlurAssignment> {
        let region = if decision.source == BlurAssignmentSource::Client {
            client_region
        } else {
            candidate_region.subtract(opaque_region)
        };
        (!region.is_empty()).then_some(ResolvedBlurAssignment {
            source: decision.source,
            anchor_scope: decision.anchor_scope,
            target_surface_id,
            region,
        })
    }
}

fn bounded_text(text: String) -> String {
    if text.len() <= MAX_CONTROL_DETAIL_BYTES {
        return text;
    }
    let mut end = MAX_CONTROL_DETAIL_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

impl super::CompositorState {
    pub(in crate::compositor) fn blur_assignment_for_surface(
        &self,
        surface: &RenderableSurface,
        surface_data: &SurfaceData,
        origin: (i32, i32),
    ) -> Option<ResolvedBlurAssignment> {
        let root_surface_id = self.root_surface_id_for_surface(surface.surface_id);
        let opaque = surface.opaque_region();
        let full_opaque = matches!(opaque, SurfaceOpaqueRegion::Full);
        let fullscreen = self
            .fullscreen_presentation
            .is_some_and(|state| state.owner_root_surface_id == root_surface_id);
        let client_request_committed = !surface_data.committed_background_effect().ops().is_empty();
        let candidate = EffectRect::new(origin.0, origin.1, surface.width, surface.height)
            .map(EffectRegion::from_rect)?;
        let client_region = crate::compositor::effects::background_effect_output_region(
            &surface_data.committed_background_effect(),
            origin,
            surface.width,
            surface.height,
        );
        let opaque_region = opaque_effect_region(&opaque, origin);
        if let Some(layer) = self.layer_surfaces.get(&root_surface_id) {
            return self.blur_assignment.resolve_layer_assignment(
                surface.surface_id,
                client_region,
                candidate,
                &opaque_region,
                full_opaque,
                &layer.namespace,
                surface.buffer.alpha_capability(),
                fullscreen,
                client_request_committed,
            );
        }
        let Some(window) = self
            .window_id_for_surface(root_surface_id)
            .and_then(|window_id| self.desktop_windows.get(&window_id))
        else {
            return (self
                .blur_assignment
                .resolve_client_request(client_request_committed)
                == BlurAssignment::ClientExact
                && !client_region.is_empty())
            .then_some(ResolvedBlurAssignment {
                source: BlurAssignmentSource::Client,
                anchor_scope: EffectAnchorScope::Surface,
                target_surface_id: surface.surface_id,
                region: client_region,
            });
        };
        let backend = match window.backend {
            WindowBackend::Xdg(_) => BlurBackend::Wayland,
            WindowBackend::X11(_) => BlurBackend::Xwayland,
        };
        self.blur_assignment.resolve_window_assignment(
            surface.surface_id,
            client_region,
            candidate,
            &opaque_region,
            full_opaque,
            window.metadata.app_id.as_deref(),
            window.metadata.title.as_deref(),
            backend,
            surface.buffer.alpha_capability(),
            fullscreen,
            client_request_committed,
        )
    }

    pub(in crate::compositor) fn active_blur_assignment_counts(&self) -> BlurAssignmentCounts {
        let mut counts = BlurAssignmentCounts::default();
        for (surface, origin) in self
            .active_scene_surfaces()
            .iter()
            .zip(self.active_scene_surface_origins())
        {
            let Some(surface_resource) = self.surface_resource_by_id(surface.surface_id) else {
                continue;
            };
            let Some(surface_data) = surface_resource.data::<SurfaceData>() else {
                continue;
            };
            let client_request_committed =
                !surface_data.committed_background_effect().ops().is_empty();
            if self.root_surface_id_for_surface(surface.surface_id) != surface.surface_id
                && !client_request_committed
            {
                continue;
            }
            let Some(assignment) = self.blur_assignment_for_surface(surface, surface_data, *origin)
            else {
                continue;
            };
            match assignment.source {
                BlurAssignmentSource::Client => counts.client += 1,
                BlurAssignmentSource::WaylandAuto => counts.wayland_auto += 1,
                BlurAssignmentSource::WindowRule => counts.window_rule += 1,
                BlurAssignmentSource::LayerRule => counts.layer_rule += 1,
            }
        }
        counts
    }

    pub(in crate::compositor) fn blur_policy_snapshot(
        &self,
    ) -> crate::blur_policy::BlurPolicySnapshot {
        self.blur_assignment
            .status(self.active_blur_assignment_counts())
    }

    pub(in crate::compositor) fn reload_blur_policy_from_disk(
        &mut self,
    ) -> Result<crate::blur_policy::BlurPolicySnapshot, String> {
        let fallback_path = crate::blur_policy::config_path();
        let (path, config) = match crate::blur_policy::load() {
            Ok(value) => value,
            Err(error) => {
                let error = error.to_string();
                self.blur_assignment.set_config_path(&fallback_path);
                self.blur_assignment.set_last_error(Some(error.clone()));
                return Err(error);
            }
        };
        self.blur_assignment.set_config_path(&path);
        let changed = match self.blur_assignment.replace_config(config) {
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
        Ok(self.blur_policy_snapshot())
    }
}

fn opaque_effect_region(region: &SurfaceOpaqueRegion, origin: (i32, i32)) -> EffectRegion {
    let SurfaceOpaqueRegion::Partial(rects) = region else {
        return EffectRegion::empty();
    };
    let mut output = EffectRegion::empty();
    for rect in rects {
        let Some(x) = i64::from(origin.0).checked_add(i64::from(rect.x())) else {
            continue;
        };
        let Some(y) = i64::from(origin.1).checked_add(i64::from(rect.y())) else {
            continue;
        };
        let Some(rect) = EffectRect::new(x as i32, y as i32, rect.width(), rect.height()) else {
            continue;
        };
        output.push(rect);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blur_policy::{BlurLayerMatch, BlurLayerRule, BlurWindowMatch, BlurWindowRule};

    fn resolver() -> BlurAssignmentResolver {
        BlurAssignmentResolver::from_config(BlurPolicyConfig::default(), true).unwrap()
    }
    fn window_rule(action: BlurRuleAction) -> BlurWindowRule {
        BlurWindowRule {
            name: format!("rule-{action:?}"),
            matcher: BlurWindowMatch {
                app_id: Some("org.example.app".to_string()),
                title: None,
                backend: None,
            },
            action,
        }
    }

    #[test]
    fn precedence_and_v1_modes_are_enforced() {
        let r = resolver();
        assert_eq!(
            r.resolve_window(
                None,
                None,
                BlurBackend::Wayland,
                SurfaceAlphaCapability::AlphaCapable,
                false,
                false,
                false
            ),
            BlurAssignment::CompositorSynthesized
        );
        assert_eq!(
            r.resolve_window(
                None,
                None,
                BlurBackend::Xwayland,
                SurfaceAlphaCapability::AlphaCapable,
                false,
                false,
                false
            ),
            BlurAssignment::None
        );
        assert_eq!(
            r.resolve_window(
                None,
                None,
                BlurBackend::Wayland,
                SurfaceAlphaCapability::Unknown,
                true,
                true,
                true
            ),
            BlurAssignment::ClientExact
        );
        assert_eq!(
            r.resolve_window(
                None,
                None,
                BlurBackend::Wayland,
                SurfaceAlphaCapability::AlphaCapable,
                false,
                true,
                false
            ),
            BlurAssignment::None
        );
    }

    #[test]
    fn explicit_rules_preserve_window_and_layer_scopes() {
        let mut r = resolver();
        let mut config = r.config();
        config.window_rules = vec![window_rule(BlurRuleAction::Enable)];
        config.layer_rules = vec![BlurLayerRule {
            name: "waybar".to_string(),
            matcher: BlurLayerMatch {
                namespace: Some("^waybar$".to_string()),
            },
            action: BlurRuleAction::Enable,
        }];
        r.replace_config(config).unwrap();
        let candidate = EffectRegion::from_rect(EffectRect::new(0, 0, 100, 20).unwrap());
        let empty = EffectRegion::empty();
        let window = r
            .resolve_window_assignment(
                1,
                empty.clone(),
                candidate.clone(),
                &empty,
                false,
                Some("org.example.app"),
                None,
                BlurBackend::Wayland,
                SurfaceAlphaCapability::Opaque,
                true,
                false,
            )
            .unwrap();
        assert_eq!(window.source, BlurAssignmentSource::WindowRule);
        assert_eq!(window.anchor_scope, EffectAnchorScope::VisualGroup);
        let layer = r
            .resolve_layer_assignment(
                2,
                empty,
                candidate,
                &EffectRegion::empty(),
                false,
                "waybar",
                SurfaceAlphaCapability::Opaque,
                false,
                false,
            )
            .unwrap();
        assert_eq!(layer.source, BlurAssignmentSource::LayerRule);
        assert_eq!(layer.anchor_scope, EffectAnchorScope::Surface);
    }

    #[test]
    fn client_is_surface_scoped_and_synthetic_region_subtracts_opaque() {
        let r = resolver();
        let candidate = EffectRegion::from_rect(EffectRect::new(0, 0, 100, 100).unwrap());
        let opaque = EffectRegion::from_rect(EffectRect::new(0, 0, 80, 100).unwrap());
        let auto = r
            .resolve_window_assignment(
                3,
                EffectRegion::empty(),
                candidate.clone(),
                &opaque,
                false,
                None,
                None,
                BlurBackend::Wayland,
                SurfaceAlphaCapability::AlphaCapable,
                false,
                false,
            )
            .unwrap();
        assert_eq!(auto.source, BlurAssignmentSource::WaylandAuto);
        assert!(!auto.region.contains_point(10, 50));
        assert!(auto.region.contains_point(90, 50));
        let client = r
            .resolve_window_assignment(
                4,
                candidate.clone(),
                candidate,
                &opaque,
                true,
                None,
                None,
                BlurBackend::Wayland,
                SurfaceAlphaCapability::Unknown,
                false,
                true,
            )
            .unwrap();
        assert_eq!(client.source, BlurAssignmentSource::Client);
        assert_eq!(client.anchor_scope, EffectAnchorScope::Surface);
        assert!(client.region.contains_point(10, 50));
    }

    #[test]
    fn invalid_replacement_is_transactional_and_status_is_runtime_only() {
        let mut r = resolver();
        let generation = r.snapshot().generation;
        let mut invalid = r.config();
        invalid.window_rules = vec![
            window_rule(BlurRuleAction::Enable),
            window_rule(BlurRuleAction::Disable),
        ];
        assert!(r.replace_config(invalid).is_err());
        assert_eq!(r.snapshot().generation, generation);
        let status = r.status(BlurAssignmentCounts {
            client: 1,
            wayland_auto: 2,
            window_rule: 3,
            layer_rule: 4,
        });
        let value = serde_json::to_value(status).unwrap();
        assert_eq!(value["active_assignment_count"], 10);
        assert!(value.get("window_rules").is_none());
    }
}
