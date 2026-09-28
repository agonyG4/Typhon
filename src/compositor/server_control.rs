use super::OwnCompositorServer;
use crate::compositor::window_state::ToplevelMode;
use crate::compositor::{WindowBackend, WindowId};
use crate::control_snapshots::{
    ControlWindowId, GeometrySnapshot, WindowKindSnapshot, WindowListSnapshot, WindowSnapshot,
    bounded_window_list,
};
use crate::wm::WindowManagementState;

#[derive(Debug)]
pub enum MaterialSetError {
    Invalid,
    UnsupportedCapability(crate::material::MaterialDimension),
    Persistence(crate::material::MaterialPersistenceError),
}

#[derive(Debug)]
pub enum MaterialProgramSetError {
    InvalidConfiguration,
    UnknownProgram,
    Unqualified,
    Persistence(crate::material_program::MaterialProgramPersistenceError),
}

impl std::fmt::Display for MaterialProgramSetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfiguration => "material program configuration is invalid",
            Self::UnknownProgram => "requested material program is not registered",
            Self::Unqualified => "requested program is not qualified as a global material",
            Self::Persistence(error) => return error.fmt(formatter),
        })
    }
}

impl std::fmt::Display for MaterialSetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid => formatter.write_str("material configuration is invalid"),
            Self::UnsupportedCapability(_) => {
                formatter.write_str("material override is unsupported by the active renderer")
            }
            Self::Persistence(error) => formatter.write_str(&error.to_string()),
        }
    }
}

impl crate::compositor::CompositorState {
    pub(crate) fn material_snapshot(&self) -> crate::material::MaterialSnapshot {
        self.material_control
            .snapshot(self.material_runtime_capabilities)
    }

    pub(crate) fn set_material_runtime_capabilities(
        &mut self,
        capabilities: crate::material::MaterialCapabilities,
    ) {
        self.material_runtime_capabilities = capabilities;
    }

    pub(crate) fn material_program_catalog_snapshot(
        &self,
    ) -> crate::material_program::MaterialProgramCatalogSnapshot {
        let generation = self.trusted_effect_registry.current();
        crate::material_program::MaterialProgramCatalogSnapshot::from_registry_generation(
            &generation,
            self.material_program_rendering_available,
        )
    }

    pub(crate) fn material_program_selection_snapshot(
        &self,
    ) -> crate::material_program::MaterialProgramSelectionSnapshot {
        let generation = self.trusted_effect_registry.current();
        self.material_program_control
            .snapshot(&generation, self.material_program_rendering_available)
    }

    pub(crate) fn material_program_state_snapshot(
        &self,
    ) -> crate::material_program::MaterialProgramStateSnapshot {
        let generation = self.trusted_effect_registry.current();
        crate::material_program::MaterialProgramStateSnapshot::from_registry_generation(
            &generation,
            &self.material_program_control,
            self.material_program_rendering_available,
        )
    }

    pub(crate) fn material_program_description_snapshot(
        &self,
        name: &str,
    ) -> Result<
        crate::material_program::MaterialProgramDescriptionSnapshot,
        crate::material_program::MaterialProgramParameterDescriptionError,
    > {
        let generation = self.trusted_effect_registry.current();
        self.material_program_parameter_control
            .describe(name, &generation)
    }

    pub(crate) fn set_material_program_rendering_available(&mut self, available: bool) {
        self.material_program_rendering_available = available;
    }

    pub(crate) fn note_trusted_effect_registry_reload(&mut self) {
        self.advance_render_generation_with_scene_effect(
            super::RenderGenerationCause::EffectBinding,
            true,
        );
        self.refresh_effect_scene_summary();
    }

    pub(crate) fn set_material_program_configuration(
        &mut self,
        configuration: crate::material_program::MaterialProgramConfiguration,
    ) -> Result<crate::material_program::MaterialProgramSelectionUpdate, MaterialProgramSetError>
    {
        let generation = self.trusted_effect_registry.current();
        let update = self
            .material_program_control
            .set_configuration(
                configuration,
                &generation,
                self.material_program_rendering_available,
            )
            .map_err(map_material_program_mutation_error)?;
        if update.changed {
            self.advance_render_generation_with_scene_effect(
                super::RenderGenerationCause::EffectBinding,
                true,
            );
            self.refresh_effect_scene_summary();
        }
        Ok(update)
    }

    pub(crate) fn set_material_program_parameter_configuration(
        &mut self,
        configuration: crate::material_program::MaterialProgramParameterConfiguration,
    ) -> Result<
        crate::material_program::MaterialProgramParameterUpdate,
        crate::material_program::MaterialProgramParameterMutationError,
    > {
        let generation = self.trusted_effect_registry.current();
        let update = self
            .material_program_parameter_control
            .set_configuration(configuration, &generation)?;
        if update.changed {
            self.advance_render_generation_with_scene_effect(
                super::RenderGenerationCause::EffectBinding,
                true,
            );
            self.refresh_effect_scene_summary();
        }
        Ok(update)
    }

    pub(crate) fn set_material_configuration(
        &mut self,
        configuration: crate::material::MaterialConfiguration,
    ) -> Result<crate::material::MaterialSnapshot, MaterialSetError> {
        self.material_control
            .validate_candidate(&configuration, self.material_runtime_capabilities)
            .map_err(map_material_mutation_error)?;
        let effective = configuration
            .effective()
            .map_err(|_| MaterialSetError::Invalid)?;
        let generation = self
            .trusted_effect_registry
            .prepare_material_generation(effective)
            .map_err(|_| MaterialSetError::Invalid)?;
        let snapshot = self
            .material_control
            .set_configuration(configuration, self.material_runtime_capabilities)
            .map_err(map_material_mutation_error)?;
        self.trusted_effect_registry
            .publish_material_generation(generation);
        self.advance_render_generation_with_scene_effect(
            super::RenderGenerationCause::EffectBinding,
            true,
        );
        let summary = self.resolved_effect_scene().summary;
        self.set_effect_scene_summary(summary);
        Ok(snapshot)
    }

    pub(crate) fn animation_control_snapshot(
        &self,
    ) -> crate::animation_control::AnimationControlSnapshot {
        self.animation_control
            .snapshot(self.animation_runtime_capabilities())
    }

    pub(crate) fn set_animation_configuration(
        &mut self,
        configuration: crate::animation_control::AnimationConfiguration,
    ) -> Result<
        crate::animation_control::AnimationControlSnapshot,
        crate::animation_control::AnimationPersistenceError,
    > {
        let enabled = configuration.enabled;
        self.animation_control
            .set_configuration(configuration, self.animation_runtime_capabilities())?;
        self.presentation_animator.set_enabled(enabled);
        self.set_lifecycle_animation_enabled(enabled);
        self.reconcile_lifecycle_animation_policy();
        Ok(self.animation_control_snapshot())
    }
}

fn map_material_mutation_error(error: crate::material::MaterialMutationError) -> MaterialSetError {
    match error {
        crate::material::MaterialMutationError::Invalid => MaterialSetError::Invalid,
        crate::material::MaterialMutationError::UnsupportedCapability(dimension) => {
            MaterialSetError::UnsupportedCapability(dimension)
        }
        crate::material::MaterialMutationError::Persistence(error) => {
            MaterialSetError::Persistence(error)
        }
    }
}

fn map_material_program_mutation_error(
    error: crate::material_program::MaterialProgramMutationError,
) -> MaterialProgramSetError {
    match error {
        crate::material_program::MaterialProgramMutationError::InvalidConfiguration => {
            MaterialProgramSetError::InvalidConfiguration
        }
        crate::material_program::MaterialProgramMutationError::UnknownProgram => {
            MaterialProgramSetError::UnknownProgram
        }
        crate::material_program::MaterialProgramMutationError::Unqualified => {
            MaterialProgramSetError::Unqualified
        }
        crate::material_program::MaterialProgramMutationError::Persistence(error) => {
            MaterialProgramSetError::Persistence(error)
        }
    }
}

impl OwnCompositorServer {
    pub fn material_snapshot(&self) -> crate::material::MaterialSnapshot {
        self.state.material_snapshot()
    }

    pub fn set_material_runtime_capabilities(
        &mut self,
        capabilities: crate::material::MaterialCapabilities,
    ) {
        self.state.set_material_runtime_capabilities(capabilities);
    }

    pub fn material_program_catalog_snapshot(
        &self,
    ) -> crate::material_program::MaterialProgramCatalogSnapshot {
        self.state.material_program_catalog_snapshot()
    }

    pub fn material_program_selection_snapshot(
        &self,
    ) -> crate::material_program::MaterialProgramSelectionSnapshot {
        self.state.material_program_selection_snapshot()
    }

    pub fn material_program_state_snapshot(
        &self,
    ) -> crate::material_program::MaterialProgramStateSnapshot {
        self.state.material_program_state_snapshot()
    }

    pub fn material_program_description_snapshot(
        &self,
        name: &str,
    ) -> Result<
        crate::material_program::MaterialProgramDescriptionSnapshot,
        crate::material_program::MaterialProgramParameterDescriptionError,
    > {
        self.state.material_program_description_snapshot(name)
    }

    pub fn set_material_program_rendering_available(&mut self, available: bool) {
        self.state
            .set_material_program_rendering_available(available);
    }

    #[doc(hidden)]
    pub fn note_trusted_effect_registry_reload(&mut self) {
        self.state.note_trusted_effect_registry_reload();
    }

    pub fn set_material_program_configuration(
        &mut self,
        configuration: crate::material_program::MaterialProgramConfiguration,
    ) -> Result<crate::material_program::MaterialProgramSelectionUpdate, MaterialProgramSetError>
    {
        self.state.set_material_program_configuration(configuration)
    }

    pub fn set_material_program_parameter_configuration(
        &mut self,
        configuration: crate::material_program::MaterialProgramParameterConfiguration,
    ) -> Result<
        crate::material_program::MaterialProgramParameterUpdate,
        crate::material_program::MaterialProgramParameterMutationError,
    > {
        self.state
            .set_material_program_parameter_configuration(configuration)
    }

    pub fn set_material_configuration(
        &mut self,
        configuration: crate::material::MaterialConfiguration,
    ) -> Result<crate::material::MaterialSnapshot, MaterialSetError> {
        self.state.set_material_configuration(configuration)
    }

    pub fn animation_control_snapshot(&self) -> crate::animation_control::AnimationControlSnapshot {
        self.state.animation_control_snapshot()
    }

    pub fn set_animation_configuration(
        &mut self,
        configuration: crate::animation_control::AnimationConfiguration,
    ) -> Result<
        crate::animation_control::AnimationControlSnapshot,
        crate::animation_control::AnimationPersistenceError,
    > {
        self.state.set_animation_configuration(configuration)
    }
}

impl OwnCompositorServer {
    pub fn revoke_astrea_shell_pid(&mut self, pid: u32) {
        self.state.revoke_astrea_shell_pid(pid);
    }

    pub fn control_window_snapshot(&self, id: WindowId) -> Option<WindowSnapshot> {
        let window = self.state.window(id)?;
        let (kind, x11) = match window.backend {
            WindowBackend::Xdg(_) => (WindowKindSnapshot::XdgToplevel, false),
            WindowBackend::X11(_) => (WindowKindSnapshot::X11, true),
        };
        let geometry = self
            .state
            .current_root_window_geometry(window.root_surface_id)
            .map(|geometry| GeometrySnapshot {
                x: geometry.placement.local_x,
                y: geometry.placement.local_y,
                width: geometry.width,
                height: geometry.height,
            });
        let mode = window.state.mode();
        Some(WindowSnapshot {
            id: ControlWindowId(window.id.get()),
            app_id: window.metadata.app_id.as_deref().map(|app_id| {
                crate::control_snapshots::truncate_utf8(
                    app_id,
                    crate::control_snapshots::MAX_CONTROL_NAME_BYTES,
                )
            }),
            title: crate::control_snapshots::truncate_utf8(
                window.metadata.title.as_deref().unwrap_or(""),
                crate::control_snapshots::MAX_CONTROL_TITLE_BYTES,
            ),
            pid: window.metadata.pid,
            kind,
            mapped: geometry.is_some(),
            active: self.state.focused_window_id == Some(id),
            minimized: window.state.is_minimized(),
            maximized: matches!(mode, ToplevelMode::Maximized),
            fullscreen: matches!(mode, ToplevelMode::Fullscreen),
            urgent: None,
            skip_taskbar: x11 && window.is_auxiliary_x11_role(),
            workspace: control_workspace_label(window.management),
            output: Some("oblivion-1".to_string()),
            geometry,
            focus_serial: None,
        })
    }

    pub fn control_window_list_snapshot(&self) -> Result<WindowListSnapshot, serde_json::Error> {
        let total = u32::try_from(self.state.desktop_windows.len()).unwrap_or(u32::MAX);
        bounded_window_list(
            total,
            self.state
                .window_stacking
                .iter()
                .rev()
                .filter_map(|id| self.control_window_snapshot(*id)),
        )
    }

    pub fn control_window_counts(&self) -> (u32, u32, u32) {
        let total = u32::try_from(self.state.desktop_windows.len()).unwrap_or(u32::MAX);
        let mapped = self
            .state
            .desktop_windows
            .values()
            .filter(|window| {
                self.state
                    .current_root_window_geometry(window.root_surface_id)
                    .is_some()
            })
            .count();
        let minimized = self
            .state
            .desktop_windows
            .values()
            .filter(|window| window.state.is_minimized())
            .count();
        (
            total,
            u32::try_from(mapped).unwrap_or(u32::MAX),
            u32::try_from(minimized).unwrap_or(u32::MAX),
        )
    }

    pub fn control_active_window_snapshot(&self) -> Option<WindowSnapshot> {
        self.state
            .focused_window_id
            .and_then(|id| self.control_window_snapshot(id))
    }

    /// Returns the focused normal desktop application's in-memory identity.
    ///
    /// This deliberately exposes no cgroup or filesystem details. Native
    /// runtime code submits the identity to its asynchronous dmem worker.
    pub fn dmem_foreground_target(&self) -> Option<(WindowId, u32)> {
        let window_id = self.state.focused_window_id?;
        let window = self.state.window(window_id)?;
        if !window.is_workspace_managed() {
            return None;
        }
        let pid = window.metadata.pid.filter(|pid| *pid != 0)?;
        Some((window_id, pid))
    }
}

fn control_workspace_label(management: Option<WindowManagementState>) -> Option<String> {
    management.map(|management| match management.location() {
        crate::wm::WorkspaceLocation::Regular(workspace) => workspace.to_string(),
        crate::wm::WorkspaceLocation::Special(_) => "special".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::{MaterialSetError, control_workspace_label};
    use crate::material::{
        MaterialConfiguration, MaterialConfigurationStore, MaterialControlState, MaterialDimension,
        MaterialOverrides,
    };
    use crate::wm::{WindowManagementState, WorkspaceId};
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn material_config_home() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "typhon-material-server-control-{}-{}",
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

    fn state_with_material_store(
        store: MaterialConfigurationStore,
    ) -> crate::compositor::CompositorState {
        let mut state = crate::compositor::CompositorState::new(None);
        state.material_control = MaterialControlState::from_store(store);
        state
    }

    #[test]
    fn material_program_selection_changes_only_scene_selection_not_registry() {
        let directory = material_config_home();
        let mut state = crate::compositor::CompositorState::new(None);
        state.material_control = MaterialControlState::from_store(
            MaterialConfigurationStore::new(directory.clone()).unwrap(),
        );
        state.set_material_runtime_capabilities(crate::material::MaterialCapabilities::full());
        state.material_program_control =
            crate::material_program::MaterialProgramControlState::from_store(
                crate::material_program::MaterialProgramConfigurationStore::new(directory.clone())
                    .unwrap(),
            );
        let source = crate::effects::EffectNodeId::new(1).unwrap();
        let program = crate::effects::EffectProgram {
            id: crate::effects::EffectProgramId::new(79).unwrap(),
            nodes: vec![crate::effects::EffectNode::source(
                source,
                crate::effects::EffectSource::Backdrop,
            )],
            output: source,
            working_space: crate::effects::EffectWorkingSpace::LinearSrgb,
            alpha_mode: crate::effects::EffectAlphaMode::Opaque,
            outsets: crate::effects::EffectOutsets::ZERO,
            frame_demand: crate::effects::EffectFrameDemand::OnDamage,
            failure_policy: crate::effects::EffectFailurePolicy::Passthrough,
        };
        state
            .trusted_effect_registry
            .reload(
                crate::effects::EffectManifest {
                    version: 1,
                    effects: [(
                        "glass.liquid".to_owned(),
                        crate::effects::EffectDefinition {
                            name: "glass.liquid".to_owned(),
                            program,
                            parameters: Default::default(),
                            shader_assets: Vec::new(),
                        },
                    )]
                    .into_iter()
                    .collect(),
                },
                |_| Ok(()),
            )
            .unwrap();
        let previous_registry = state.trusted_effect_registry.current();
        let previous_scene_generation = state.scene_render_generation;

        let update = state
            .set_material_program_configuration(
                crate::material_program::MaterialProgramConfiguration {
                    version: 1,
                    requested_program: "glass.liquid".to_owned(),
                },
            )
            .unwrap();

        assert!(update.changed);
        assert_eq!(update.snapshot.effective_program, "glass.liquid");
        assert_eq!(state.scene_render_generation, previous_scene_generation + 1);
        assert!(std::sync::Arc::ptr_eq(
            &state.trusted_effect_registry.current(),
            &previous_registry
        ));
        assert_eq!(
            state.resolved_effect_scene().summary.visible_instance_count,
            0
        );
        assert!(!state.resolved_effect_scene().summary.requires_composition);
        let catalog = state.material_program_catalog_snapshot();
        assert_eq!(
            catalog.registry_generation,
            state.trusted_effect_registry.current().generation
        );
        assert_eq!(
            catalog.programs.first().unwrap().name,
            crate::effects::BUILTIN_BACKGROUND_BLUR_NAME
        );
        assert!(!catalog.rendering_available);

        let previous_builtin_radius = state
            .trusted_effect_registry
            .current()
            .program(crate::effects::BUILTIN_BACKGROUND_BLUR_NAME)
            .unwrap()
            .aggregate_footprint
            .sample_radius_x;
        state
            .set_material_configuration(MaterialConfiguration {
                position: 1.0,
                ..MaterialConfiguration::default()
            })
            .unwrap();
        let material_registry = state.trusted_effect_registry.current();
        let current_builtin_radius = material_registry
            .program(crate::effects::BUILTIN_BACKGROUND_BLUR_NAME)
            .unwrap()
            .aggregate_footprint
            .sample_radius_x;
        assert!(current_builtin_radius > previous_builtin_radius);
        assert!(material_registry.effects.contains_key("glass.liquid"));
        let selected_after_material_update = state.material_program_selection_snapshot();
        assert_eq!(
            selected_after_material_update
                .configuration
                .requested_program,
            "glass.liquid"
        );
        assert_eq!(
            selected_after_material_update.effective_program,
            "glass.liquid"
        );

        let after_change = state.scene_render_generation;
        let idempotent = state
            .set_material_program_configuration(
                crate::material_program::MaterialProgramConfiguration {
                    version: 1,
                    requested_program: "glass.liquid".to_owned(),
                },
            )
            .unwrap();
        assert!(!idempotent.changed);
        assert_eq!(state.scene_render_generation, after_change);
        assert!(std::sync::Arc::ptr_eq(
            &state.trusted_effect_registry.current(),
            &material_registry
        ));

        let selection_generation = state.material_program_selection_snapshot().generation;
        let scene_generation = state.scene_render_generation;
        state
            .trusted_effect_registry
            .reload(
                crate::effects::EffectManifest {
                    version: 1,
                    effects: Default::default(),
                },
                |_| Ok(()),
            )
            .unwrap();
        state.note_trusted_effect_registry_reload();
        let fallback = state.material_program_selection_snapshot();
        assert_eq!(fallback.configuration.requested_program, "glass.liquid");
        assert_eq!(
            fallback.effective_program,
            crate::effects::BUILTIN_BACKGROUND_BLUR_NAME
        );
        assert_eq!(
            fallback.fallback_reason,
            Some(crate::material_program::MaterialProgramFallbackReason::Missing)
        );
        assert_eq!(fallback.generation, selection_generation);
        assert_eq!(state.scene_render_generation, scene_generation + 1);

        let scene_generation_after_fallback = state.scene_render_generation;
        let same_requested_program = state
            .set_material_program_configuration(
                crate::material_program::MaterialProgramConfiguration {
                    version: 1,
                    requested_program: "glass.liquid".to_owned(),
                },
            )
            .unwrap();
        assert!(!same_requested_program.changed);
        assert_eq!(same_requested_program.snapshot, fallback);
        assert_eq!(
            state.scene_render_generation,
            scene_generation_after_fallback
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn unsupported_material_override_is_rejected_before_transaction_publication() {
        let directory = material_config_home();
        let store = MaterialConfigurationStore::new(directory.clone()).unwrap();
        let mut state = state_with_material_store(store.clone());
        let previous = state.material_snapshot();
        let registry_generation = state.trusted_effect_registry.current().generation;
        let scene_generation = state.scene_render_generation;
        let candidate = MaterialConfiguration {
            overrides: MaterialOverrides {
                blur: Some(0.9),
                ..MaterialOverrides::default()
            },
            ..previous.configuration.clone()
        };

        let result = state.set_material_configuration(candidate);
        let no_persisted_candidate = store.read().is_err();
        let current = state.material_snapshot();
        let current_registry_generation = state.trusted_effect_registry.current().generation;
        let current_scene_generation = state.scene_render_generation;
        let _ = fs::remove_dir_all(directory);

        assert!(matches!(
            result,
            Err(MaterialSetError::UnsupportedCapability(
                MaterialDimension::Blur
            ))
        ));
        assert_eq!(current, previous);
        assert_eq!(current_registry_generation, registry_generation);
        assert_eq!(current_scene_generation, scene_generation);
        assert!(no_persisted_candidate);
    }

    #[test]
    fn material_server_transaction_publishes_configuration_registry_and_scene_once() {
        let directory = material_config_home();
        let store = MaterialConfigurationStore::new(directory.clone()).unwrap();
        let mut state = state_with_material_store(store.clone());
        state.set_material_runtime_capabilities(crate::material::MaterialCapabilities::full());
        let before = state.material_snapshot();
        let previous_registry = state.trusted_effect_registry.current();
        let previous_program = previous_registry
            .program("system.background_blur")
            .expect("canonical material program")
            .clone();
        let previous_scene_generation = state.scene_render_generation;

        let candidate = MaterialConfiguration {
            position: 1.0,
            ..before.configuration.clone()
        };
        let snapshot = state
            .set_material_configuration(candidate.clone())
            .expect("position-only material update is always allowed");
        let current_registry = state.trusted_effect_registry.current();
        let current_program = current_registry
            .program("system.background_blur")
            .expect("canonical material program remains registered");

        assert_eq!(snapshot.generation, before.generation + 1);
        assert_eq!(snapshot.configuration, candidate);
        assert_eq!(
            snapshot.capabilities,
            crate::material::MaterialCapabilities::full()
        );
        assert_eq!(
            current_registry.generation,
            previous_registry.generation + 1
        );
        assert!(
            current_program.aggregate_footprint.sample_radius_x
                > previous_program.aggregate_footprint.sample_radius_x
        );
        assert_eq!(state.scene_render_generation, previous_scene_generation + 1);
        assert_eq!(store.read().unwrap(), candidate);

        let material_only_scene = state.resolved_effect_scene();
        assert_eq!(material_only_scene.summary.visible_instance_count, 0);
        assert!(!material_only_scene.summary.requires_composition);
        assert!(material_only_scene.instances.iter().all(
            |instance| instance.program != crate::effects::builtin_background_blur_program_id()
        ));
        assert_eq!(
            crate::compositor::direct_scanout_scene_rejection_for_effects(
                state.effect_scene_summary()
            ),
            None
        );
        let plan = state.fullscreen_composition_plan();
        let direct_scanout_analysis = state.direct_scanout_effect_analysis(
            &plan,
            crate::render_backend::buffer::BufferSize::new(
                state.output_size.width,
                state.output_size.height,
            )
            .expect("test output size"),
            None,
        );
        assert!(!direct_scanout_analysis.requires_composition);
        assert_eq!(direct_scanout_analysis.contributing_instance_count, 0);

        let blur_region = crate::effects::EffectRegion::from_rect(
            crate::effects::EffectRect::new(0, 0, 320, 180).unwrap(),
        );
        assert!(state.set_internal_surface_effect(
            42,
            crate::compositor::EffectAnchor::BeforeSurface(42),
            crate::effects::builtin_background_blur_program_id(),
            blur_region,
        ));
        let visible_material_blur = state.resolved_effect_scene();
        assert_eq!(visible_material_blur.summary.visible_instance_count, 1);
        assert!(visible_material_blur.summary.requires_composition);
        assert_eq!(
            crate::compositor::direct_scanout_scene_rejection_for_effects(
                state.effect_scene_summary()
            ),
            Some(crate::compositor::DirectScanoutSceneRejection::EffectRequiresComposition)
        );
        assert!(state.clear_internal_surface_effect(42));
        assert!(state.resolved_effect_scene().is_empty());
        assert_eq!(
            crate::compositor::direct_scanout_scene_rejection_for_effects(
                state.effect_scene_summary()
            ),
            None
        );

        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn material_server_persistence_failure_publishes_no_candidate_generation() {
        let mut state = state_with_material_store(MaterialConfigurationStore::unavailable());
        let previous = state.material_snapshot();
        let registry_generation = state.trusted_effect_registry.current().generation;
        let scene_generation = state.scene_render_generation;
        let candidate = MaterialConfiguration {
            position: 1.0,
            ..previous.configuration.clone()
        };

        let result = state.set_material_configuration(candidate);

        assert!(result.is_err());
        assert_eq!(state.material_snapshot(), previous);
        assert_eq!(
            state.trusted_effect_registry.current().generation,
            registry_generation
        );
        assert_eq!(state.scene_render_generation, scene_generation);
    }

    #[test]
    fn material_snapshot_uses_runtime_capabilities_without_changing_wire_shape_or_persistence() {
        let directory = material_config_home();
        let store = MaterialConfigurationStore::new(directory.clone()).unwrap();
        let persisted = MaterialConfiguration {
            overrides: MaterialOverrides {
                blur: Some(0.42),
                ..MaterialOverrides::default()
            },
            ..MaterialConfiguration::default()
        };
        store.write(&persisted).unwrap();
        let mut state = state_with_material_store(store.clone());
        let material_generation = state
            .material_control
            .snapshot(crate::material::MaterialCapabilities::unavailable())
            .generation;
        let registry_generation = state.trusted_effect_registry.current().generation;
        let scene_generation = state.scene_render_generation;

        state.set_material_runtime_capabilities(crate::material::MaterialCapabilities::full());
        let full = state.material_snapshot();
        assert_eq!(
            full.capabilities,
            crate::material::MaterialCapabilities::full()
        );
        let encoded = serde_json::to_value(&full).unwrap();
        let snapshot_fields = encoded
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            snapshot_fields,
            [
                "capabilities",
                "configuration",
                "effective",
                "generation",
                "source"
            ]
            .into_iter()
            .collect()
        );
        let capability_fields = encoded["capabilities"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            capability_fields,
            ["blurOverride", "noiseOverride", "saturationOverride"]
                .into_iter()
                .collect()
        );

        state.set_material_runtime_capabilities(crate::material::MaterialCapabilities::unavailable());
        let unavailable = state.material_snapshot();
        assert_eq!(
            unavailable.capabilities,
            crate::material::MaterialCapabilities::unavailable()
        );
        assert_eq!(unavailable.generation, material_generation);
        assert_eq!(store.read().unwrap(), persisted);
        assert_eq!(
            state.trusted_effect_registry.current().generation,
            registry_generation
        );
        assert_eq!(state.scene_render_generation, scene_generation);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn unsupported_persisted_overrides_can_be_preserved_cleared_and_reject_new_values() {
        let directory = material_config_home();
        let store = MaterialConfigurationStore::new(directory.clone()).unwrap();
        let persisted = MaterialConfiguration {
            position: 0.5,
            overrides: MaterialOverrides {
                blur: Some(0.42),
                saturation: Some(0.31),
                noise: Some(0.27),
            },
            ..MaterialConfiguration::default()
        };
        store.write(&persisted).unwrap();
        let mut state = state_with_material_store(store.clone());

        let preserved = MaterialConfiguration {
            position: 0.8,
            ..persisted.clone()
        };
        let preserved_snapshot = state
            .set_material_configuration(preserved.clone())
            .expect("position edits may preserve unsupported stored overrides");
        assert_eq!(preserved_snapshot.configuration, preserved);

        let cleared = MaterialConfiguration {
            overrides: MaterialOverrides::default(),
            ..preserved_snapshot.configuration
        };
        let cleared_snapshot = state
            .set_material_configuration(cleared.clone())
            .expect("clearing unsupported overrides is allowed");
        assert_eq!(cleared_snapshot.configuration, cleared);

        let mut accepted_unsupported = Vec::new();
        for (name, override_value) in [
            (
                "blur",
                MaterialOverrides {
                    blur: Some(0.9),
                    ..MaterialOverrides::default()
                },
            ),
            (
                "saturation",
                MaterialOverrides {
                    saturation: Some(0.9),
                    ..MaterialOverrides::default()
                },
            ),
            (
                "noise",
                MaterialOverrides {
                    noise: Some(0.9),
                    ..MaterialOverrides::default()
                },
            ),
        ] {
            let candidate = MaterialConfiguration {
                overrides: override_value,
                ..cleared.clone()
            };
            if state.set_material_configuration(candidate).is_ok() {
                accepted_unsupported.push(name);
            }
        }

        assert!(
            accepted_unsupported.is_empty(),
            "unsupported mutations were accepted: {accepted_unsupported:?}"
        );
        assert_eq!(state.material_snapshot().configuration, cleared);
        assert_eq!(store.read().unwrap(), cleared);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn control_snapshot_publishes_numeric_workspace_or_none() {
        assert_eq!(
            control_workspace_label(Some(WindowManagementState::new(
                crate::wm::WorkspaceLocation::Regular(WorkspaceId::new(1).unwrap()),
            ))),
            Some("1".to_string())
        );
        assert_eq!(control_workspace_label(None), None);
    }

    #[test]
    fn control_snapshot_labels_special_without_reinterpreting_it_as_regular() {
        assert_eq!(
            control_workspace_label(Some(WindowManagementState::new(
                crate::wm::WorkspaceLocation::Special(crate::wm::SpecialWorkspaceId::DEFAULT),
            ))),
            Some("special".to_string())
        );
    }

    #[test]
    fn material_program_state_snapshot_has_coherent_catalog_and_selection() {
        let state = crate::compositor::CompositorState::new(None);
        let snapshot = state.material_program_state_snapshot();

        assert_eq!(
            snapshot.catalog.registry_generation,
            snapshot.selection.registry_generation
        );
        assert_eq!(
            snapshot.catalog.rendering_available,
            snapshot.selection.rendering_available
        );
    }

    #[test]
    fn parameter_mutation_does_not_rebuild_selection_or_create_an_effect() {
        let directory = material_config_home();
        let mut state = crate::compositor::CompositorState::new(None);
        state.material_program_control =
            crate::material_program::MaterialProgramControlState::from_store(
                crate::material_program::MaterialProgramConfigurationStore::new(directory.clone())
                    .unwrap(),
            );
        let parameter_store =
            crate::material_program::MaterialProgramParameterConfigurationStore::new(
                directory.clone(),
            )
            .unwrap();
        state.material_program_parameter_control =
            crate::material_program::MaterialProgramParameterControlState::from_store(
                parameter_store.clone(),
            );

        let source = crate::effects::EffectNodeId::new(1).unwrap();
        let program = crate::effects::EffectProgram {
            id: crate::effects::EffectProgramId::new(80).unwrap(),
            nodes: vec![crate::effects::EffectNode::source(
                source,
                crate::effects::EffectSource::Backdrop,
            )],
            output: source,
            working_space: crate::effects::EffectWorkingSpace::LinearSrgb,
            alpha_mode: crate::effects::EffectAlphaMode::Opaque,
            outsets: crate::effects::EffectOutsets::ZERO,
            frame_demand: crate::effects::EffectFrameDemand::OnDamage,
            failure_policy: crate::effects::EffectFailurePolicy::Passthrough,
        };
        let parameter_definition = crate::effects::EffectParameterDefinition {
            spec: crate::effects::EffectParameterSpec {
                id: crate::effects::EffectParameterId::new(1).unwrap(),
                name: "intensity".to_owned(),
                ty: crate::effects::EffectParameterType::Float,
                range: Some(crate::effects::EffectParameterRange::Float { min: 0.0, max: 1.0 }),
                impact: crate::effects::EffectParameterImpact::UniformOnly,
            },
            default: crate::effects::EffectUniformValue::Float(0.65),
        };
        let mut continuous_program = program.clone();
        continuous_program.id = crate::effects::EffectProgramId::new(81).unwrap();
        continuous_program.frame_demand = crate::effects::EffectFrameDemand::Continuous;
        state
            .trusted_effect_registry
            .reload(
                crate::effects::EffectManifest {
                    version: 1,
                    effects: [
                        (
                            "glass.liquid".to_owned(),
                            crate::effects::EffectDefinition {
                                name: "glass.liquid".to_owned(),
                                program,
                                parameters: [(
                                    "intensity".to_owned(),
                                    parameter_definition.clone(),
                                )]
                                .into_iter()
                                .collect(),
                                shader_assets: Vec::new(),
                            },
                        ),
                        (
                            "glass.continuous".to_owned(),
                            crate::effects::EffectDefinition {
                                name: "glass.continuous".to_owned(),
                                program: continuous_program,
                                parameters: [("intensity".to_owned(), parameter_definition)]
                                    .into_iter()
                                    .collect(),
                                shader_assets: Vec::new(),
                            },
                        ),
                    ]
                    .into_iter()
                    .collect(),
                },
                |_| Ok(()),
            )
            .unwrap();

        let registry = state.trusted_effect_registry.current();
        let effect = registry.effects.get("glass.liquid").unwrap();
        let candidate = crate::material_program::MaterialProgramParameterConfiguration {
            version: 1,
            program: "glass.liquid".to_owned(),
            schema_signature: effect.parameter_schema_signature(),
            overrides: [(
                "intensity".to_owned(),
                crate::material_program::MaterialProgramParameterValue::Float(0.82),
            )]
            .into_iter()
            .collect(),
        };
        let before_registry = registry.clone();
        let before_selection_generation = state.material_program_selection_snapshot().generation;
        let before_scene_generation = state.scene_render_generation;

        let update = state
            .set_material_program_parameter_configuration(candidate.clone())
            .unwrap();

        assert!(update.changed);
        assert_eq!(update.snapshot.parameter_generation, 1);
        assert_eq!(state.material_program_parameter_control.generation(), 1);
        assert_eq!(
            state.material_program_selection_snapshot().generation,
            before_selection_generation
        );
        assert_eq!(state.scene_render_generation, before_scene_generation + 1);
        assert!(std::sync::Arc::ptr_eq(
            &state.trusted_effect_registry.current(),
            &before_registry
        ));
        assert_eq!(parameter_store.read().unwrap().programs.len(), 1);
        let scene = state.resolved_effect_scene();
        assert_eq!(scene.summary.visible_instance_count, 0);
        assert!(!scene.summary.requires_composition);

        let scene_generation = state.scene_render_generation;
        let replay = state
            .set_material_program_parameter_configuration(candidate)
            .unwrap();
        assert!(!replay.changed);
        assert_eq!(state.material_program_parameter_control.generation(), 1);
        assert_eq!(state.scene_render_generation, scene_generation);

        use crate::material_program::MaterialProgramParameterMutationError as MutationError;
        let mut type_mismatch = update.snapshot.configuration.clone();
        type_mismatch.overrides.insert(
            "intensity".to_owned(),
            crate::material_program::MaterialProgramParameterValue::Int(1),
        );
        let mut unknown_parameter = update.snapshot.configuration.clone();
        unknown_parameter.overrides.clear();
        unknown_parameter.overrides.insert(
            "undeclared".to_owned(),
            crate::material_program::MaterialProgramParameterValue::Float(0.4),
        );
        let mut out_of_range = update.snapshot.configuration.clone();
        out_of_range.overrides.insert(
            "intensity".to_owned(),
            crate::material_program::MaterialProgramParameterValue::Float(1.2),
        );
        let mut stale_schema = update.snapshot.configuration.clone();
        stale_schema.schema_signature = stale_schema.schema_signature.wrapping_add(1);
        let mut unknown_program = update.snapshot.configuration.clone();
        unknown_program.program = "missing.program".to_owned();
        let mut non_finite = update.snapshot.configuration.clone();
        non_finite.overrides.insert(
            "intensity".to_owned(),
            crate::material_program::MaterialProgramParameterValue::Float(f64::NAN),
        );
        let continuous = registry.effects.get("glass.continuous").unwrap();
        let unqualified = crate::material_program::MaterialProgramParameterConfiguration {
            version: 1,
            program: continuous.name.clone(),
            schema_signature: continuous.parameter_schema_signature(),
            overrides: [(
                "intensity".to_owned(),
                crate::material_program::MaterialProgramParameterValue::Float(0.8),
            )]
            .into_iter()
            .collect(),
        };
        for (candidate, expected) in [
            (type_mismatch, "invalid_value"),
            (unknown_parameter, "unknown_parameter"),
            (out_of_range, "invalid_value"),
            (stale_schema, "stale_schema"),
            (unknown_program, "unknown_program"),
            (unqualified, "unqualified"),
            (non_finite, "invalid_configuration"),
        ] {
            let persistent_before_rejection = parameter_store.read().unwrap();
            let registry_before_rejection = state.trusted_effect_registry.current();
            let selection_generation_before_rejection =
                state.material_program_selection_snapshot().generation;
            let scene_generation_before_rejection = state.scene_render_generation;
            let error = state
                .set_material_program_parameter_configuration(candidate)
                .unwrap_err();
            let error_kind = match error {
                MutationError::InvalidConfiguration => "invalid_configuration",
                MutationError::UnknownProgram => "unknown_program",
                MutationError::Unqualified => "unqualified",
                MutationError::StaleSchema => "stale_schema",
                MutationError::UnknownParameter => "unknown_parameter",
                MutationError::InvalidParameterValue => "invalid_value",
                MutationError::GenerationExhausted => "generation_exhausted",
                MutationError::Persistence(_) => "persistence",
            };
            assert_eq!(error_kind, expected);
            assert_eq!(parameter_store.read().unwrap(), persistent_before_rejection);
            assert_eq!(state.material_program_parameter_control.generation(), 1);
            assert_eq!(
                state.material_program_selection_snapshot().generation,
                selection_generation_before_rejection
            );
            assert_eq!(
                state.scene_render_generation,
                scene_generation_before_rejection
            );
            assert_eq!(
                state.trusted_effect_registry.current().generation,
                registry_before_rejection.generation
            );
            assert!(std::sync::Arc::ptr_eq(
                &state.trusted_effect_registry.current(),
                &registry_before_rejection
            ));
        }

        state.material_program_parameter_control =
            crate::material_program::MaterialProgramParameterControlState::from_store(
                crate::material_program::MaterialProgramParameterConfigurationStore::unavailable(),
            );
        let failure_before_registry = state.trusted_effect_registry.current();
        let failure_before_selection = state.material_program_selection_snapshot().generation;
        let failure_before_scene = state.scene_render_generation;
        let mut failing_candidate = update.snapshot.configuration;
        failing_candidate.overrides.insert(
            "intensity".to_owned(),
            crate::material_program::MaterialProgramParameterValue::Float(0.9),
        );
        assert!(matches!(
            state.set_material_program_parameter_configuration(failing_candidate),
            Err(crate::material_program::MaterialProgramParameterMutationError::Persistence(_))
        ));
        assert_eq!(state.material_program_parameter_control.generation(), 0);
        assert_eq!(
            state.material_program_selection_snapshot().generation,
            failure_before_selection
        );
        assert_eq!(state.scene_render_generation, failure_before_scene);
        assert!(std::sync::Arc::ptr_eq(
            &state.trusted_effect_registry.current(),
            &failure_before_registry
        ));
        assert_eq!(
            state
                .material_program_description_snapshot("glass.liquid")
                .unwrap()
                .parameters[0]
                .effective,
            crate::material_program::MaterialProgramParameterValue::Float(0.65)
        );
        let _ = fs::remove_dir_all(directory);
    }
}
