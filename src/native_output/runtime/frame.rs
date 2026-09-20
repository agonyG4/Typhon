use super::*;
use oblivion_one::effects::{
    EffectRect, EffectRegion, EffectRegistryGeneration, effect_output_influence_region,
};
use std::borrow::Cow;

use oblivion_one::compositor::{
    AnimationTime, DecorationRenderInstance, DecorationSceneSnapshot, FullscreenRenderPlanMetrics,
    PointerWarpOrigin, PresentationFrameSnapshot, PresentationSceneSample, ResolvedEffectScene,
    SceneNodeId,
};
use oblivion_one::window_lifecycle_animation::{LifecycleFrameSnapshot, LifecycleSceneSample};

use super::frame_scene_identity::{
    assert_surface_owner_alignment, assert_surface_scene_node_alignment,
    filter_surface_scene_nodes_with_owners, finalize_snapshot,
};

#[cfg(test)]
use super::frame_scene_identity::{
    note_identity_computation, note_snapshot_finalization, note_snapshot_owned_clone,
};

#[derive(Debug)]
pub(crate) struct ResolvedNativeFrameScene<'a> {
    pub(crate) surfaces: Cow<'a, [RenderableSurface]>,
    pub(crate) surface_scene_node_ids: Cow<'a, [SceneNodeId]>,
    pub(crate) presentation_owner_root_surface_ids: Cow<'a, [u32]>,
    pub(crate) decorations: Vec<DecorationRenderInstance>,
    pub(crate) popup_surface_ids: Cow<'a, [u32]>,
    pub(crate) external_overlay_surface_ids: Vec<u32>,
    pub(crate) render_generation: u64,
    pub(crate) visibility: FullscreenRenderPlanMetrics,
    pub(crate) snapshot: NativeSceneSnapshot,
    pub(crate) scene_identity_signature: u64,
    pub(crate) effects: ResolvedEffectScene,
    pub(crate) presentation: PresentationSceneSample,
    pub(crate) presentation_snapshot: PresentationFrameSnapshot,
    pub(crate) lifecycle: LifecycleSceneSample,
    pub(crate) lifecycle_surfaces: Vec<RenderableSurface>,
    pub(crate) lifecycle_decorations: Vec<DecorationRenderInstance>,
    pub(crate) lifecycle_snapshot: LifecycleFrameSnapshot,
}

fn freeze_presentation_effect_influences(
    effects: &ResolvedEffectScene,
    registry_generation: &EffectRegistryGeneration,
    output_bounds: EffectRect,
    mut owner_for_surface: impl FnMut(u32) -> Option<(SceneNodeId, u32)>,
) -> Vec<crate::native_output::output::NativePresentationEffectInfluenceSnapshot> {
    let mut influences =
        Vec::<crate::native_output::output::NativePresentationEffectInfluenceSnapshot>::new();

    for instance in &effects.instances {
        let surface_id = match instance.anchor {
            oblivion_one::compositor::EffectAnchor::BeforeSurface(surface_id)
            | oblivion_one::compositor::EffectAnchor::ReplaceSurface(surface_id)
            | oblivion_one::compositor::EffectAnchor::AfterSurface(surface_id) => surface_id,
            oblivion_one::compositor::EffectAnchor::OutputPostProcess => continue,
        };
        let Some((scene_node_id, root_surface_id)) = owner_for_surface(surface_id) else {
            eprintln!(
                "native effects: anchor surface={surface_id} has no presentation owner SceneNode; skipping owner damage evidence"
            );
            continue;
        };
        let region = match registry_generation.effect_for_program(instance.program) {
            Some(effect) => effect_output_influence_region(
                effect.program.aggregate_footprint,
                &instance.region,
                output_bounds,
            ),
            None => {
                eprintln!(
                    "native effects: resolved program {:?} for owner {:?} is missing from the captured registry generation; using output bounds for presentation damage",
                    instance.program, scene_node_id
                );
                EffectRegion::from_rect(output_bounds)
            }
        };

        if let Some(existing) = influences
            .iter_mut()
            .find(|influence| influence.scene_node_id == scene_node_id)
        {
            debug_assert_eq!(existing.presentation_owner_root_surface_id, root_surface_id);
            existing.region = existing.region.union(&region);
        } else {
            influences.push(
                crate::native_output::output::NativePresentationEffectInfluenceSnapshot {
                    scene_node_id,
                    presentation_owner_root_surface_id: root_surface_id,
                    region,
                },
            );
        }
    }

    influences.sort_unstable_by_key(|influence| influence.scene_node_id);
    influences
}

#[cfg(test)]
mod presentation_effect_influence_tests {
    use super::*;
    use oblivion_one::compositor::{
        EffectAnchor, EffectAnchorScope, EffectSceneOrder, ResolvedEffectInstance,
    };
    use oblivion_one::effects::{
        EffectFrameDemand, EffectInstanceId, EffectParameterBlock, EffectProgramId,
        EffectRegistryGeneration,
    };

    fn instance(
        id: u64,
        program: EffectProgramId,
        anchor: EffectAnchor,
        rect: EffectRect,
    ) -> ResolvedEffectInstance {
        ResolvedEffectInstance {
            id: EffectInstanceId::new(id).expect("test instance id"),
            program,
            anchor,
            region: EffectRegion::from_rect(rect),
            target_bounds: rect,
            parameter_block: EffectParameterBlock::default(),
            signature: id,
            frame_demand: EffectFrameDemand::OnDamage,
            visual_group: None,
            anchor_scope: EffectAnchorScope::Surface,
            scene_order: EffectSceneOrder::for_anchor(anchor),
        }
    }

    #[test]
    fn owned_effects_merge_by_scene_node_and_postprocess_is_excluded() {
        let generation = EffectRegistryGeneration::with_builtin_background_blur();
        let program = oblivion_one::effects::builtin_background_blur_program_id();
        let owner = SceneNodeId::from_raw(7).expect("WindowGroup node");
        let output_bounds = EffectRect::new(0, 0, 400, 300).expect("output bounds");
        let first_rect = EffectRect::new(50, 50, 30, 30).expect("first effect");
        let second_rect = EffectRect::new(200, 100, 20, 20).expect("second effect");
        let post_rect = EffectRect::new(300, 200, 40, 30).expect("postprocess effect");
        let effects = ResolvedEffectScene::new(
            1,
            vec![
                instance(1, program, EffectAnchor::BeforeSurface(10), first_rect),
                instance(2, program, EffectAnchor::AfterSurface(11), second_rect),
                instance(3, program, EffectAnchor::OutputPostProcess, post_rect),
            ],
        );

        let mut resolved_surfaces = Vec::new();
        let influences = freeze_presentation_effect_influences(
            &effects,
            &generation,
            output_bounds,
            |surface_id| {
                resolved_surfaces.push(surface_id);
                match surface_id {
                    10 | 11 => Some((owner, 70)),
                    _ => None,
                }
            },
        );

        let footprint = generation
            .effect_for_program(program)
            .expect("registered builtin effect")
            .program
            .aggregate_footprint;
        let expected = effect_output_influence_region(
            footprint,
            &EffectRegion::from_rect(first_rect),
            output_bounds,
        )
        .union(&effect_output_influence_region(
            footprint,
            &EffectRegion::from_rect(second_rect),
            output_bounds,
        ));
        assert_eq!(influences.len(), 1);
        assert_eq!(influences[0].scene_node_id, owner);
        assert_eq!(influences[0].presentation_owner_root_surface_id, 70);
        assert_eq!(influences[0].region, expected);
        assert_eq!(resolved_surfaces, [10, 11]);
    }

    #[test]
    fn missing_program_uses_bounded_output_influence_for_its_owner() {
        let generation = EffectRegistryGeneration::empty();
        let owner = SceneNodeId::from_raw(7).expect("WindowGroup node");
        let output_bounds = EffectRect::new(0, 0, 400, 300).expect("output bounds");
        let effects = ResolvedEffectScene::new(
            1,
            vec![instance(
                1,
                EffectProgramId::new(777).expect("missing program id"),
                EffectAnchor::ReplaceSurface(10),
                EffectRect::new(50, 50, 30, 30).expect("effect"),
            )],
        );

        let influences =
            freeze_presentation_effect_influences(&effects, &generation, output_bounds, |_| {
                Some((owner, 70))
            });

        assert_eq!(influences.len(), 1);
        assert_eq!(influences[0].region, EffectRegion::from_rect(output_bounds));
        assert_eq!(influences[0].presentation_owner_root_surface_id, 70);
    }
}

impl<'a> ResolvedNativeFrameScene<'a> {
    pub(crate) fn from_server(server: &'a OwnCompositorServer) -> Self {
        let (at, source) = AnimationTime::monotonic_now().map_or(
            (
                AnimationTime::from_nanos(0),
                oblivion_one::compositor::PresentationSampleTimeSource::ZeroFallback,
            ),
            |at| {
                (
                    at,
                    oblivion_one::compositor::PresentationSampleTimeSource::MonotonicFallback,
                )
            },
        );
        Self::from_server_at_with_source(server, at, source)
    }

    #[allow(dead_code)]
    pub(crate) fn from_server_at(server: &'a OwnCompositorServer, at: AnimationTime) -> Self {
        Self::from_server_at_with_source(
            server,
            at,
            oblivion_one::compositor::PresentationSampleTimeSource::MonotonicFallback,
        )
    }

    pub(crate) fn from_server_at_with_source(
        server: &'a OwnCompositorServer,
        at: AnimationTime,
        sample_time_source: oblivion_one::compositor::PresentationSampleTimeSource,
    ) -> Self {
        let (canonical_surfaces, canonical_scene_nodes, fullscreen_plan, visibility) =
            server.native_frame_renderable_surfaces_with_scene_nodes_and_composition_plan();
        let canonical_owner_roots = Cow::Owned(
            canonical_surfaces
                .iter()
                .map(|surface| server.presentation_owner_root_for_surface(surface.surface_id))
                .collect::<Vec<_>>(),
        );
        let lifecycle = server.lifecycle_scene_sample_at(at);
        let lifecycle_surfaces = server.lifecycle_renderable_surfaces(&lifecycle);
        let lifecycle_decorations =
            server.lifecycle_decoration_render_instances(&lifecycle, &lifecycle_surfaces);
        let (canonical_surfaces, canonical_scene_nodes, canonical_owner_roots) =
            if server.lifecycle_render_suppressed_roots().is_empty() {
                (
                    canonical_surfaces,
                    canonical_scene_nodes,
                    canonical_owner_roots,
                )
            } else {
                filter_surface_scene_nodes_with_owners(
                    canonical_surfaces,
                    canonical_scene_nodes,
                    canonical_owner_roots,
                    |surface| !server.lifecycle_surface_is_suppressed(surface.surface_id),
                )
            };
        assert_surface_scene_node_alignment(
            canonical_surfaces.as_ref(),
            canonical_scene_nodes.as_ref(),
        );
        assert_surface_owner_alignment(canonical_surfaces.as_ref(), canonical_owner_roots.as_ref());
        let targets = server.native_frame_presentation_targets(canonical_surfaces.as_ref());
        let presentation = server.presentation_scene_sample_for_targets_at_with_source(
            at,
            sample_time_source,
            &targets,
        );
        let decorations = server
            .native_decoration_render_instances_for_scale(canonical_surfaces.as_ref(), 1.0)
            .into_iter()
            .map(|decoration| {
                presentation
                    .transform_for_root(decoration.root_surface_id())
                    .and_then(|transform| decoration.with_presentation_transform(transform))
                    .unwrap_or(decoration)
            })
            .collect::<Vec<_>>();
        let surfaces =
            server.apply_presentation_to_native_frame_surfaces(canonical_surfaces, &presentation);
        let presentation_snapshot = PresentationFrameSnapshot::from_sample_with_presented_windows(
            &presentation,
            server.presented_window_geometries_for_targets(&presentation, &targets),
        );
        let lifecycle_snapshot = LifecycleFrameSnapshot::from_sample(&lifecycle);
        let popup_surface_ids = Cow::Borrowed(server.popup_surface_ids());
        let external_overlay_surface_ids = server.external_overlay_surface_ids(&lifecycle);
        let render_generation = server.scene_render_generation();
        let effect_registry_generation = server.trusted_effect_registry().current();
        let effects =
            server.resolved_effect_scene_for_presentation(&presentation, &fullscreen_plan);
        let mut snapshot =
            NativeSceneSnapshot::from_surfaces_with_scene_nodes_and_presentation_owners(
                surfaces.as_ref(),
                canonical_scene_nodes.as_ref(),
                canonical_owner_roots.as_ref(),
                decorations
                    .iter()
                    .map(DecorationRenderInstance::scene_snapshot)
                    .collect(),
                popup_surface_ids.as_ref(),
            );
        let (output_width, output_height) = server.output_dimensions();
        let physical_effect_damage = EffectRect::new(0, 0, output_width, output_height)
            .map(|output_bounds| {
                freeze_native_effect_damage(&effects, &effect_registry_generation, output_bounds)
            })
            .unwrap_or_else(|| {
                NativeEffectDamageFrameSnapshot::conservative_full(
                    effects.frame_demand_snapshot().dirty_region,
                )
            });
        snapshot.physical_effect_damage = physical_effect_damage;
        if let Some(output_bounds) = EffectRect::new(0, 0, output_width, output_height) {
            snapshot.presentation_effect_influences = freeze_presentation_effect_influences(
                &effects,
                &effect_registry_generation,
                output_bounds,
                |surface_id| {
                    let root_surface_id = server.presentation_owner_root_for_surface(surface_id);
                    server
                        .presentation_scene_node_id_for_root(root_surface_id)
                        .map(|scene_node_id| (scene_node_id, root_surface_id))
                },
            );
        }
        let (snapshot, scene_identity_signature) = finalize_snapshot(
            snapshot,
            &external_overlay_surface_ids,
            visibility,
            &effects,
        );
        #[cfg(test)]
        {
            note_snapshot_finalization();
            note_identity_computation();
        }
        Self {
            surfaces,
            surface_scene_node_ids: canonical_scene_nodes,
            presentation_owner_root_surface_ids: canonical_owner_roots,
            decorations,
            popup_surface_ids,
            external_overlay_surface_ids,
            render_generation,
            visibility,
            snapshot,
            scene_identity_signature,
            effects,
            presentation,
            presentation_snapshot,
            lifecycle,
            lifecycle_surfaces,
            lifecycle_decorations,
            lifecycle_snapshot,
        }
    }
    pub(crate) fn into_owned(self) -> ResolvedNativeFrameScene<'static> {
        ResolvedNativeFrameScene {
            surfaces: Cow::Owned(self.surfaces.into_owned()),
            surface_scene_node_ids: Cow::Owned(self.surface_scene_node_ids.into_owned()),
            presentation_owner_root_surface_ids: Cow::Owned(
                self.presentation_owner_root_surface_ids.into_owned(),
            ),
            decorations: self.decorations,
            popup_surface_ids: Cow::Owned(self.popup_surface_ids.into_owned()),
            external_overlay_surface_ids: self.external_overlay_surface_ids,
            render_generation: self.render_generation,
            visibility: self.visibility,
            snapshot: self.snapshot,
            scene_identity_signature: self.scene_identity_signature,
            effects: self.effects,
            presentation: self.presentation,
            presentation_snapshot: self.presentation_snapshot,
            lifecycle: self.lifecycle,
            lifecycle_surfaces: self.lifecycle_surfaces,
            lifecycle_decorations: self.lifecycle_decorations,
            lifecycle_snapshot: self.lifecycle_snapshot,
        }
    }

    pub(crate) fn surface_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.surfaces.iter().map(|surface| surface.surface_id)
    }

    pub(crate) fn decoration_identities(&self) -> impl Iterator<Item = (WindowId, u32)> + '_ {
        self.decorations
            .iter()
            .map(DecorationRenderInstance::scene_snapshot)
            .map(|snapshot| snapshot.identity())
    }

    pub(crate) fn snapshot_ref(&self) -> &NativeSceneSnapshot {
        self.debug_assert_snapshot_consistency();
        &self.snapshot
    }

    pub(crate) fn snapshot_owned(&self) -> NativeSceneSnapshot {
        self.debug_assert_snapshot_consistency();
        #[cfg(test)]
        note_snapshot_owned_clone();
        self.snapshot.clone()
    }

    fn debug_assert_snapshot_consistency(&self) {
        assert_surface_scene_node_alignment(
            self.surfaces.as_ref(),
            self.surface_scene_node_ids.as_ref(),
        );
        debug_assert!(
            self.surface_scene_node_ids.iter().copied().eq(self
                .snapshot
                .surfaces
                .iter()
                .map(|surface| surface.scene_node_id)),
            "resolved frame scene and its snapshot diverged on SceneNode identity"
        );
        debug_assert!(
            self.surface_ids().eq(self
                .snapshot
                .surfaces
                .iter()
                .map(|surface| surface.surface_id)),
            "resolved frame scene and its snapshot diverged on surfaces"
        );
        debug_assert!(
            self.decoration_identities().eq(self
                .snapshot
                .decorations
                .iter()
                .map(DecorationSceneSnapshot::identity)),
            "resolved frame scene and its snapshot diverged on decorations"
        );
    }

    pub(crate) fn scene_identity_signature(&self) -> u64 {
        self.scene_identity_signature
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeRepaintInputs {
    // Retained for cycle diagnostics only; accepting a socket is not visual work.
    pub(crate) accepted_clients: bool,
    pub(crate) render_generation_changed: bool,
    pub(crate) pending_frame_work: bool,
    pub(crate) only_pending_surface_frame_callbacks: bool,
    pub(crate) redraw_requested: bool,
    pub(crate) cursor_work_pending: bool,
    pub(crate) effect_work_pending: bool,
    pub(crate) effect_dirty_region: EffectRegion,
    pub(crate) page_flip_pending: bool,
}

#[cfg(test)]
pub(crate) fn earliest_native_deadline(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
        (None, None) => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeRepaintDecision {
    pub(crate) repaint: bool,
    pub(crate) protocol_only_present: bool,
}

pub(crate) fn native_repaint_decision(inputs: NativeRepaintInputs) -> NativeRepaintDecision {
    if inputs.page_flip_pending {
        return NativeRepaintDecision {
            repaint: false,
            protocol_only_present: false,
        };
    }

    let effect_work_pending = inputs.effect_work_pending || !inputs.effect_dirty_region.is_empty();
    let protocol_only_present = inputs.pending_frame_work
        && inputs.only_pending_surface_frame_callbacks
        && !inputs.render_generation_changed
        && !inputs.redraw_requested
        && !effect_work_pending;
    NativeRepaintDecision {
        // Accepting a socket is protocol progress, not visual work.  The
        // accepted-client bit is retained in the input for diagnostics, but
        // a client must create visible scene or protocol-owned output work
        // before the primary renderer is scheduled.
        repaint: inputs.render_generation_changed
            || inputs.redraw_requested
            || inputs.cursor_work_pending
            || effect_work_pending
            || (inputs.pending_frame_work && !protocol_only_present),
        protocol_only_present,
    }
}

pub(crate) fn normalize_refresh_hz(refresh_hz: u32) -> u32 {
    if refresh_hz == 0 {
        60
    } else {
        refresh_hz.clamp(30, 360)
    }
}

#[derive(Debug, Default)]
pub(crate) struct NativeFrameRenderer {
    pub(crate) scene_renderer: DesktopSceneRenderer,
    pub(crate) frame: Vec<u32>,
}

impl NativeFrameRenderer {
    pub(crate) fn with_cursor_image(
        cursor_image: std::sync::Arc<oblivion_one::cursor_theme::CompositorCursorImage>,
    ) -> Self {
        Self {
            scene_renderer: DesktopSceneRenderer::with_cursor_image(cursor_image),
            frame: Vec::new(),
        }
    }

    pub(crate) fn set_cursor_image(
        &mut self,
        cursor_image: std::sync::Arc<oblivion_one::cursor_theme::CompositorCursorImage>,
    ) {
        self.scene_renderer.set_cursor_image(cursor_image);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCursorRenderMode {
    Software,
    SoftwareClient,
    Hardware,
}

impl NativeCursorRenderMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Software => "software",
            Self::SoftwareClient => "software_client",
            Self::Hardware => "hardware",
        }
    }

    pub(crate) const fn is_software(self) -> bool {
        matches!(self, Self::Software | Self::SoftwareClient)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCursorPreference {
    Auto,
    Hardware,
    Software,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCursorSchedulingPolicy {
    Auto,
    Piggyback,
    Software,
}

impl NativeCursorSchedulingPolicy {
    pub(crate) fn from_env() -> Self {
        match std::env::var("OBLIVION_ONE_CURSOR_SCHEDULING") {
            Ok(value) if value == "piggyback" => Self::Piggyback,
            Ok(value) if value == "software" => Self::Software,
            Ok(value) if value == "auto" => Self::Auto,
            Ok(value) => {
                eprintln!(
                    "native cursor: unknown OBLIVION_ONE_CURSOR_SCHEDULING={value:?}; using auto"
                );
                Self::Auto
            }
            Err(_) => Self::Auto,
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Piggyback => "piggyback",
            Self::Software => "software",
        }
    }
}

/// Cursor output is lower-priority work than a primary scene transaction.  A
/// request opens one output opportunity for the client to respond; it does
/// not reserve the Atomic commit arbiter.  The desired epoch is replaced by
/// newer input while that opportunity is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeCursorOutputDisposition {
    PiggybackPrimary,
    DeferForPrimary,
    SubmitPlaneDelta,
    SoftwareOverlay,
    Noop,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeCursorOutputArbitration {
    pending_since_ns: Option<u64>,
    deadline_ns: Option<u64>,
    desired_epoch: u64,
    primary_response_window_open: bool,
    hardware_cursor_pending: bool,
    software_overlay_pending: bool,
    pub(crate) response_windows_opened: u64,
    pub(crate) changes_coalesced: u64,
    pub(crate) plane_delta_plans: u64,
    pub(crate) plane_delta_submissions: u64,
    pub(crate) plane_delta_deferred_for_primary: u64,
    pub(crate) cursor_state_piggybacked: u64,
    pub(crate) idle_hardware_updates: u64,
    pub(crate) idle_software_updates: u64,
}

impl NativeCursorOutputArbitration {
    #[cfg(test)]
    pub(crate) fn request(&mut self, epoch: u64, now_ns: u64, deadline_ns: u64) {
        self.request_with_source(epoch, now_ns, deadline_ns, false);
    }

    pub(crate) fn request_hardware(&mut self, epoch: u64, now_ns: u64, deadline_ns: u64) {
        self.request_with_source(epoch, now_ns, deadline_ns, true);
    }

    fn request_with_source(
        &mut self,
        epoch: u64,
        now_ns: u64,
        deadline_ns: u64,
        hardware_cursor: bool,
    ) {
        if self.pending_since_ns.is_none() {
            self.pending_since_ns = Some(now_ns);
            self.deadline_ns = Some(deadline_ns);
            self.primary_response_window_open = true;
            self.response_windows_opened = self.response_windows_opened.saturating_add(1);
        } else if epoch != self.desired_epoch {
            self.changes_coalesced = self.changes_coalesced.saturating_add(1);
        }
        self.desired_epoch = epoch;
        self.hardware_cursor_pending |= hardware_cursor;
    }

    pub(crate) const fn deadline_ns(&self) -> Option<u64> {
        self.deadline_ns
    }

    /// Return the cursor response deadline only while it is still a future
    /// temporal boundary.  Matured debt remains pending for disposition and
    /// exact epoch settlement, but its old timestamp must not poll the owner
    /// that now controls progress.
    pub(crate) fn wake_deadline_ns(&self, now_ns: u64) -> Option<u64> {
        self.deadline_ns.filter(|deadline_ns| *deadline_ns > now_ns)
    }

    #[cfg(test)]
    pub(crate) const fn desired_epoch(&self) -> u64 {
        self.desired_epoch
    }

    pub(crate) const fn pending(&self) -> bool {
        self.deadline_ns.is_some()
    }

    pub(crate) fn due(&self, now_ns: u64) -> bool {
        self.deadline_ns.is_some_and(|deadline| now_ns >= deadline)
    }

    pub(crate) fn set_software_overlay_pending(&mut self, pending: bool) {
        self.software_overlay_pending = pending;
    }

    pub(crate) fn reconcile_hardware_cursor_liveness(&mut self, needed: bool) {
        if needed {
            self.hardware_cursor_pending = true;
        } else if self.hardware_cursor_pending {
            self.hardware_cursor_pending = false;
            if !self.software_overlay_pending {
                self.clear_pending();
            }
        }
    }

    pub(crate) fn note_disposition(&mut self, disposition: NativeCursorOutputDisposition) {
        match disposition {
            NativeCursorOutputDisposition::PiggybackPrimary => {
                self.cursor_state_piggybacked = self.cursor_state_piggybacked.saturating_add(1);
            }
            NativeCursorOutputDisposition::DeferForPrimary => {
                self.plane_delta_deferred_for_primary =
                    self.plane_delta_deferred_for_primary.saturating_add(1);
            }
            NativeCursorOutputDisposition::SubmitPlaneDelta => {
                self.plane_delta_plans = self.plane_delta_plans.saturating_add(1);
                self.idle_hardware_updates = self.idle_hardware_updates.saturating_add(1);
            }
            NativeCursorOutputDisposition::SoftwareOverlay => {
                self.idle_software_updates = self.idle_software_updates.saturating_add(1);
            }
            NativeCursorOutputDisposition::Noop => {}
        }
    }

    pub(crate) fn note_plane_delta_submission(&mut self) {
        self.plane_delta_submissions = self.plane_delta_submissions.saturating_add(1);
    }

    pub(crate) const fn response_windows_opened(&self) -> u64 {
        self.response_windows_opened
    }

    pub(crate) const fn changes_coalesced(&self) -> u64 {
        self.changes_coalesced
    }

    pub(crate) const fn plane_delta_plans(&self) -> u64 {
        self.plane_delta_plans
    }

    pub(crate) const fn plane_delta_submissions(&self) -> u64 {
        self.plane_delta_submissions
    }

    pub(crate) const fn plane_delta_deferred_for_primary(&self) -> u64 {
        self.plane_delta_deferred_for_primary
    }

    pub(crate) const fn cursor_state_piggybacked(&self) -> u64 {
        self.cursor_state_piggybacked
    }

    pub(crate) const fn idle_hardware_updates(&self) -> u64 {
        self.idle_hardware_updates
    }

    pub(crate) const fn idle_software_updates(&self) -> u64 {
        self.idle_software_updates
    }

    pub(crate) fn disposition(
        &self,
        now_ns: u64,
        primary_work_pending: bool,
        hardware_usable: bool,
    ) -> NativeCursorOutputDisposition {
        if !self.pending() {
            return NativeCursorOutputDisposition::Noop;
        }
        if primary_work_pending {
            return NativeCursorOutputDisposition::PiggybackPrimary;
        }
        if !self.due(now_ns) {
            return if self.primary_response_window_open {
                NativeCursorOutputDisposition::DeferForPrimary
            } else {
                NativeCursorOutputDisposition::Noop
            };
        }
        if hardware_usable && !self.software_overlay_pending {
            NativeCursorOutputDisposition::SubmitPlaneDelta
        } else {
            NativeCursorOutputDisposition::SoftwareOverlay
        }
    }

    pub(crate) fn consume(&mut self, epoch: u64) {
        if self.pending() && epoch == self.desired_epoch {
            self.clear_pending();
        }
    }

    pub(crate) fn consume_submitted_epoch(
        &mut self,
        submitted_epoch: u64,
        now_ns: u64,
        next_deadline_ns: u64,
    ) {
        if !self.pending() {
            return;
        }
        if submitted_epoch == self.desired_epoch {
            self.clear_pending();
            return;
        }
        // A newer epoch was coalesced while the submitted job was waiting in
        // the worker.  The old epoch is consumed, while the newer epoch gets
        // a fresh response window and refresh-boundary deadline.
        self.pending_since_ns = Some(now_ns);
        self.deadline_ns = Some(next_deadline_ns);
        self.primary_response_window_open = true;
        self.response_windows_opened = self.response_windows_opened.saturating_add(1);
    }

    pub(crate) fn defer_after_busy(&mut self, now_ns: u64, next_deadline_ns: u64) {
        if self.pending() {
            self.deadline_ns = Some(next_deadline_ns.max(now_ns.saturating_add(1)));
        }
    }

    pub(crate) fn clear_pending(&mut self) {
        self.pending_since_ns = None;
        self.deadline_ns = None;
        self.desired_epoch = 0;
        self.primary_response_window_open = false;
        self.hardware_cursor_pending = false;
        self.software_overlay_pending = false;
    }
}

pub(crate) fn update_cursor_output_arbitration(
    arbitration: &mut NativeCursorOutputArbitration,
    cursor_epoch: u64,
    last_submitted_cursor_epoch: u64,
    now_ns: u64,
    frame_scheduler: &NativeFrameScheduler,
    software_overlay_pending: bool,
    hardware_cursor_work_pending: bool,
) -> (bool, bool, bool) {
    arbitration.set_software_overlay_pending(software_overlay_pending);
    let hardware_cursor_changed =
        hardware_cursor_work_pending && cursor_epoch != last_submitted_cursor_epoch;
    let output_cursor_work_changed = hardware_cursor_changed || software_overlay_pending;
    if output_cursor_work_changed {
        arbitration.request_with_source(
            cursor_epoch,
            now_ns,
            frame_scheduler.next_refresh_deadline_ns(now_ns),
            hardware_cursor_changed,
        );
    }
    let deadline_due = arbitration.due(now_ns);
    (
        output_cursor_work_changed,
        deadline_due,
        deadline_due && output_cursor_work_changed,
    )
}

#[cfg(test)]
#[path = "frame_tests.rs"]
mod tests;

pub(crate) fn plane_delta_allowed_at_deadline(
    arbitration: &mut NativeCursorOutputArbitration,
    _policy: NativeCursorSchedulingPolicy,
    now_ns: u64,
    primary_work_pending: bool,
    cursor_state_changed: bool,
    hardware_usable: bool,
) -> bool {
    let disposition = arbitration.disposition(now_ns, primary_work_pending, hardware_usable);
    arbitration.note_disposition(disposition);
    // Piggyback is a primary-priority policy, not a permanent prohibition on
    // idle cursor updates.  `disposition` has already selected
    // `PiggybackPrimary` whenever primary work is ready.  Once the output
    // opportunity matures with no primary work, allowing the one queued cursor
    // update prevents a permanently armed deadline in the diagnostic mode.
    cursor_state_changed && disposition == NativeCursorOutputDisposition::SubmitPlaneDelta
}

impl NativeCursorPreference {
    pub(crate) fn from_env() -> Self {
        match std::env::var("OBLIVION_ONE_CURSOR") {
            Ok(value) if matches!(value.as_str(), "hardware" | "hw" | "drm") => Self::Hardware,
            Ok(value) if matches!(value.as_str(), "software" | "sw" | "cpu") => Self::Software,
            Ok(value) if value == "auto" => Self::Auto,
            Ok(value) => {
                eprintln!("native cursor: unknown OBLIVION_ONE_CURSOR={value:?}; using auto");
                Self::Auto
            }
            Err(_) => Self::Auto,
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Hardware => "hardware",
            Self::Software => "software",
        }
    }
}

#[derive(Debug)]
pub(crate) struct NativePointerConstraintBackend {
    pub(crate) active: Option<NativePointerConstraint>,
    pub(crate) cursor_visible: bool,
}

pub(crate) fn native_pointer_debug_log_lazy(message: impl FnOnce() -> String) {
    crate::pointer_debug::log_lazy(message);
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NativePointerConstraint {
    pub(crate) id: PointerConstraintBackendId,
    pub(crate) mode: PointerConstraintMode,
    pub(crate) anchor: CompositorOutputPosition,
    pub(crate) region: Option<OutputRegion>,
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct NativePointerConstraintBackendAction {
    pub(crate) activated: Option<NativePointerConstraint>,
    pub(crate) deactivated: Option<PointerConstraintBackendId>,
    pub(crate) deactivated_mode: Option<PointerConstraintMode>,
    pub(crate) failed: Option<(PointerConstraintBackendId, &'static str)>,
    pub(crate) restore_position: Option<CompositorOutputPosition>,
    pub(crate) restore_origin: Option<PointerWarpOrigin>,
    pub(crate) cursor_position: Option<CompositorOutputPosition>,
    pub(crate) cursor_position_origin: Option<PointerWarpOrigin>,
    pub(crate) cursor_visibility_changed: Option<bool>,
    pub(crate) region_resolution_timing: Option<PointerConstraintRegionResolutionTiming>,
}

impl NativePointerConstraintBackend {
    pub(crate) fn new() -> Self {
        Self {
            active: None,
            cursor_visible: true,
        }
    }

    pub(crate) fn active_locked(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|constraint| constraint.mode == PointerConstraintMode::Locked)
    }

    pub(crate) fn active_backend_id(&self) -> Option<PointerConstraintBackendId> {
        self.active.as_ref().map(|constraint| constraint.id)
    }

    pub(crate) fn active_constraint_state(&self) -> NativePointerConstraintState {
        match self.active.as_ref() {
            Some(NativePointerConstraint {
                mode: PointerConstraintMode::Locked,
                anchor,
                ..
            }) => NativePointerConstraintState::Locked { anchor: *anchor },
            Some(NativePointerConstraint {
                mode: PointerConstraintMode::Confined,
                region: Some(region),
                ..
            }) => NativePointerConstraintState::Confined {
                region: region.clone(),
            },
            _ => NativePointerConstraintState::None,
        }
    }

    pub(crate) fn handle_request(
        &mut self,
        request: PointerConstraintBackendRequest,
        cursor_position: CompositorOutputPosition,
    ) -> NativePointerConstraintBackendAction {
        match request {
            PointerConstraintBackendRequest::ActivateLocked { id } => {
                self.activate_locked(id, cursor_position)
            }
            PointerConstraintBackendRequest::ActivateConfined {
                id,
                region,
                region_resolution_timing,
            } => self.activate_confined(id, cursor_position, region, region_resolution_timing),
            PointerConstraintBackendRequest::UpdateConfinedRegion { id, region } => {
                self.update_confined_region(id, cursor_position, region)
            }
            PointerConstraintBackendRequest::Deactivate {
                id,
                restore_position,
                restore_origin,
            } => self.deactivate(id, restore_position, restore_origin),
            PointerConstraintBackendRequest::WarpPointer { position, origin } => {
                if self.active_locked() {
                    native_pointer_debug_log_lazy(|| {
                        format!(
                            "backend warp ignored reason=active_lock position=({},{})",
                            position.x, position.y
                        )
                    });
                    return NativePointerConstraintBackendAction::default();
                }
                let constraint = self.active_constraint_state();
                let final_position = constraint.constrain_position(position);
                native_pointer_debug_log_lazy(|| {
                    format!(
                        "backend warp constraint={:?} requested=({},{}) final=({},{})",
                        constraint, position.x, position.y, final_position.x, final_position.y
                    )
                });
                NativePointerConstraintBackendAction {
                    cursor_position: Some(final_position),
                    cursor_position_origin: Some(origin),
                    ..NativePointerConstraintBackendAction::default()
                }
            }
            PointerConstraintBackendRequest::ApplyCursorVisibility { visible } => {
                if self.cursor_visible == visible {
                    NativePointerConstraintBackendAction::default()
                } else {
                    self.cursor_visible = visible;
                    NativePointerConstraintBackendAction {
                        cursor_visibility_changed: Some(visible),
                        ..NativePointerConstraintBackendAction::default()
                    }
                }
            }
        }
    }

    pub(crate) fn handle_resolved_request(
        &mut self,
        request: PointerConstraintBackendRequest,
        cursor_position: CompositorOutputPosition,
        locked_anchor: Option<CompositorOutputPosition>,
    ) -> NativePointerConstraintBackendAction {
        match request {
            PointerConstraintBackendRequest::ActivateLocked { id } => {
                self.activate_locked(id, locked_anchor.unwrap_or(cursor_position))
            }
            request => self.handle_request(request, cursor_position),
        }
    }

    pub(crate) fn activate_locked(
        &mut self,
        id: PointerConstraintBackendId,
        anchor: CompositorOutputPosition,
    ) -> NativePointerConstraintBackendAction {
        if let Some(active) = self.active.as_ref() {
            if active.id == id {
                return NativePointerConstraintBackendAction::default();
            }
            return NativePointerConstraintBackendAction {
                failed: Some((id, "native pointer constraint already active")),
                ..NativePointerConstraintBackendAction::default()
            };
        }
        let constraint = NativePointerConstraint {
            id,
            mode: PointerConstraintMode::Locked,
            anchor,
            region: None,
        };
        self.active = Some(constraint.clone());
        NativePointerConstraintBackendAction {
            activated: Some(constraint),
            ..NativePointerConstraintBackendAction::default()
        }
    }

    pub(crate) fn activate_confined(
        &mut self,
        id: PointerConstraintBackendId,
        anchor: CompositorOutputPosition,
        region: OutputRegion,
        region_resolution_timing: Option<PointerConstraintRegionResolutionTiming>,
    ) -> NativePointerConstraintBackendAction {
        if let Some(active) = self.active.as_ref() {
            if active.id == id {
                return NativePointerConstraintBackendAction::default();
            }
            return NativePointerConstraintBackendAction {
                failed: Some((id, "native pointer constraint already active")),
                ..NativePointerConstraintBackendAction::default()
            };
        }
        let constraint = NativePointerConstraint {
            id,
            mode: PointerConstraintMode::Confined,
            anchor,
            region: Some(region),
        };
        self.active = Some(constraint.clone());
        NativePointerConstraintBackendAction {
            activated: Some(constraint),
            region_resolution_timing,
            ..NativePointerConstraintBackendAction::default()
        }
    }

    pub(crate) fn deactivate(
        &mut self,
        id: PointerConstraintBackendId,
        restore_position: Option<CompositorOutputPosition>,
        restore_origin: Option<PointerWarpOrigin>,
    ) -> NativePointerConstraintBackendAction {
        let Some(active) = self.active.as_ref().cloned() else {
            return NativePointerConstraintBackendAction::default();
        };
        if active.id != id {
            return NativePointerConstraintBackendAction::default();
        }
        self.active = None;
        let restore_position = (active.mode == PointerConstraintMode::Locked)
            .then_some(restore_position)
            .flatten();
        let restore_origin = restore_position
            .is_some()
            .then_some(restore_origin)
            .flatten();
        NativePointerConstraintBackendAction {
            deactivated: Some(id),
            deactivated_mode: Some(active.mode),
            restore_position,
            restore_origin,
            ..NativePointerConstraintBackendAction::default()
        }
    }

    pub(crate) fn update_confined_region(
        &mut self,
        id: PointerConstraintBackendId,
        cursor_position: CompositorOutputPosition,
        region: OutputRegion,
    ) -> NativePointerConstraintBackendAction {
        let Some(active) = self.active.as_mut() else {
            return NativePointerConstraintBackendAction::default();
        };
        if active.id != id || active.mode != PointerConstraintMode::Confined {
            return NativePointerConstraintBackendAction::default();
        }
        active.region = Some(region.clone());
        let constrained = region.closest_point(cursor_position);
        NativePointerConstraintBackendAction {
            cursor_position: (constrained != cursor_position).then_some(constrained),
            cursor_position_origin: (constrained != cursor_position)
                .then_some(PointerWarpOrigin::ConfinedRegionCorrection),
            ..NativePointerConstraintBackendAction::default()
        }
    }
}

impl Default for NativePointerConstraintBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeFrameRenderer {
    pub(crate) fn render_server_frame(
        &mut self,
        width: u32,
        height: u32,
        resolved_scene: &ResolvedNativeFrameScene<'_>,
        server: &OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
    ) -> NativeRenderedFrame<'_> {
        self.scene_renderer
            .set_decoration_instances(&resolved_scene.decorations);
        self.scene_renderer
            .set_popup_surface_ids(&resolved_scene.popup_surface_ids);
        self.scene_renderer.set_presentation_projection_with_clips(
            &resolved_scene.presentation.opacities,
            &resolved_scene.presentation.clips,
            resolved_scene
                .surfaces
                .iter()
                .map(|surface| surface.surface_id)
                .zip(
                    resolved_scene
                        .presentation_owner_root_surface_ids
                        .iter()
                        .copied(),
                ),
        );
        let cursor_visible = resolve_native_cursor_for_server(server, input_state).visible;
        self.render_frame(NativeFrameRequest {
            width,
            height,
            surfaces: resolved_scene.surfaces.as_ref(),
            external_overlay_surface_ids: resolved_scene.external_overlay_surface_ids.clone(),
            visual_state: input_state.desktop_visual_state(cursor_mode, cursor_visible),
            render_generation: resolved_scene.render_generation,
            client_cursor: cursor_mode
                .is_software()
                .then(|| server.client_cursor_render_state())
                .flatten(),
        })
    }

    pub(crate) fn render_frame(
        &mut self,
        request: NativeFrameRequest<'_>,
    ) -> NativeRenderedFrame<'_> {
        let NativeFrameRequest {
            width,
            height,
            surfaces,
            visual_state,
            render_generation,
            client_cursor,
            external_overlay_surface_ids,
        } = request;
        let pixel_count = width.saturating_mul(height) as usize;
        self.frame.resize(pixel_count, 0);
        self.scene_renderer
            .compose_reusing_frame(DesktopComposeRequest {
                frame: &mut self.frame,
                frame_width: width,
                frame_height: height,
                output_scale: 1.0,
                surfaces,
                external_overlay_surface_ids,
                content_generation: native_scene_content_generation(render_generation),
                visual_state,
                client_cursor,
            });
        NativeRenderedFrame {
            pixels: &self.frame,
            scene_rebuild_kind: self.scene_renderer.last_rebuild_kind(),
            frame_copy_kind: self.scene_renderer.last_frame_copy_kind(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn egl_scene_draw_request<'a>(
        &mut self,
        width: u32,
        height: u32,
        resolved_scene: &'a ResolvedNativeFrameScene<'_>,
        server: &'a OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
        current_damage: Option<OutputDamage>,
    ) -> EglSceneDrawRequest<'a> {
        // The resolved scene and this authority lookup are borrowed from the
        // same immutable compositor state on the native compositor thread.
        // Keeping the lookup at this synchronous request boundary prevents a
        // deferred renderer from pairing frozen surfaces with newer journal
        // metadata; such a renderer would need an explicit snapshot identity.
        let cursor_visible = resolve_native_cursor_for_server(server, input_state).visible;
        let surface_resource_sync_states = server.surface_resource_sync_states(
            resolved_scene
                .surfaces
                .iter()
                .map(|surface| surface.surface_id)
                .chain(
                    cursor_mode
                        .is_software()
                        .then(|| server.client_cursor_render_state())
                        .flatten()
                        .map(|cursor| cursor.surface.surface_id),
                ),
        );
        let lifecycle_surface_resource_sync_states = server.surface_resource_sync_states(
            resolved_scene
                .lifecycle_surfaces
                .iter()
                .map(|surface| surface.surface_id),
        );
        let mut surface_resource_sync_states = surface_resource_sync_states;
        surface_resource_sync_states.extend(lifecycle_surface_resource_sync_states);
        EglSceneDrawRequest {
            width,
            height,
            surfaces: resolved_scene.surfaces.as_ref(),
            external_overlay_surface_ids: &resolved_scene.external_overlay_surface_ids,
            popup_surface_ids: &resolved_scene.popup_surface_ids,
            content_generation: native_scene_content_generation(resolved_scene.render_generation),
            frame_id: None,
            render_generation: Some(resolved_scene.render_generation),
            scene_generation: resolved_scene.effects.generation,
            scene_signature: resolved_scene.scene_identity_signature(),
            visual_state: input_state.desktop_visual_state(cursor_mode, cursor_visible),
            output_scale: 1.0,
            decoration_instances: &resolved_scene.decorations,
            presentation_visual_signature: resolved_scene
                .presentation
                .presentation_visual_signature(),
            presentation_opacities: &resolved_scene.presentation.opacities,
            presentation_clips: &resolved_scene.presentation.clips,
            presentation_owner_root_surface_ids: resolved_scene
                .presentation_owner_root_surface_ids
                .as_ref(),
            client_cursor: cursor_mode
                .is_software()
                .then(|| server.client_cursor_render_state())
                .flatten(),
            current_damage,
            effects: &resolved_scene.effects,
            surface_resource_sync_states,
            lifecycle: &resolved_scene.lifecycle,
            lifecycle_surfaces: &resolved_scene.lifecycle_surfaces,
            lifecycle_decorations: &resolved_scene.lifecycle_decorations,
        }
    }
}

pub(crate) struct NativeRenderedFrame<'a> {
    pub(crate) pixels: &'a [u32],
    pub(crate) scene_rebuild_kind: DesktopSceneRebuildKind,
    pub(crate) frame_copy_kind: DesktopFrameCopyKind,
}

pub(crate) struct NativeFrameRequest<'a> {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) surfaces: &'a [RenderableSurface],
    pub(crate) external_overlay_surface_ids: Vec<u32>,
    pub(crate) visual_state: DesktopVisualState,
    pub(crate) render_generation: u64,
    pub(crate) client_cursor: Option<oblivion_one::compositor::ClientCursorRenderState<'a>>,
}

pub(crate) const fn native_scene_content_generation(render_generation: u64) -> u64 {
    render_generation
}
