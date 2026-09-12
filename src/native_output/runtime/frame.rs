use super::*;
use oblivion_one::effects::EffectRegion;
use std::borrow::Cow;

use oblivion_one::compositor::{
    AnimationTime, DecorationRenderInstance, DecorationSceneSnapshot, FullscreenRenderPlanMetrics,
    PointerWarpOrigin, PresentationFrameSnapshot, PresentationSceneSample, ResolvedEffectScene,
};
use oblivion_one::window_lifecycle_animation::{LifecycleFrameSnapshot, LifecycleSceneSample};

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SnapshotWorkCounters {
    snapshot_finalizations: usize,
    snapshot_owned_clones: usize,
    identity_computations: usize,
}

#[cfg(test)]
thread_local! {
    static SNAPSHOT_WORK_COUNTERS: std::cell::Cell<SnapshotWorkCounters> =
        const {
            std::cell::Cell::new(SnapshotWorkCounters {
                snapshot_finalizations: 0,
                snapshot_owned_clones: 0,
                identity_computations: 0,
            })
        };
}

#[cfg(test)]
fn reset_snapshot_work_counters() {
    SNAPSHOT_WORK_COUNTERS.with(|counters| counters.set(SnapshotWorkCounters::default()));
}

#[cfg(test)]
fn snapshot_work_counters() -> SnapshotWorkCounters {
    SNAPSHOT_WORK_COUNTERS.with(std::cell::Cell::get)
}

#[cfg(test)]
fn note_snapshot_finalization() {
    SNAPSHOT_WORK_COUNTERS.with(|counters| {
        let mut value = counters.get();
        value.snapshot_finalizations += 1;
        counters.set(value);
    });
}

#[cfg(test)]
fn note_snapshot_owned_clone() {
    SNAPSHOT_WORK_COUNTERS.with(|counters| {
        let mut value = counters.get();
        value.snapshot_owned_clones += 1;
        counters.set(value);
    });
}

#[cfg(test)]
fn note_identity_computation() {
    SNAPSHOT_WORK_COUNTERS.with(|counters| {
        let mut value = counters.get();
        value.identity_computations += 1;
        counters.set(value);
    });
}

#[derive(Debug)]
pub(crate) struct ResolvedNativeFrameScene<'a> {
    pub(crate) surfaces: Cow<'a, [RenderableSurface]>,
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

impl<'a> ResolvedNativeFrameScene<'a> {
    pub(crate) fn from_server(server: &'a OwnCompositorServer) -> Self {
        let at = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        Self::from_server_at(server, at)
    }

    pub(crate) fn from_server_at(server: &'a OwnCompositorServer, at: AnimationTime) -> Self {
        let (canonical_surfaces, visibility) =
            server.native_frame_renderable_surfaces_with_metrics();
        let lifecycle = server.lifecycle_scene_sample_at(at);
        let lifecycle_surfaces = server.lifecycle_renderable_surfaces(&lifecycle);
        let lifecycle_decorations =
            server.native_decoration_render_instances_for_scale(&lifecycle_surfaces, 1.0);
        let canonical_surfaces = if server.lifecycle_render_suppressed_roots().is_empty() {
            canonical_surfaces
        } else {
            Cow::Owned(
                canonical_surfaces
                    .iter()
                    .filter(|surface| !server.lifecycle_surface_is_suppressed(surface.surface_id))
                    .cloned()
                    .collect(),
            )
        };
        let targets = server.native_frame_presentation_targets(canonical_surfaces.as_ref());
        let presentation = server.presentation_scene_sample_for_targets_at(at, &targets);
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
        let external_overlay_surface_ids = server.external_overlay_surface_ids();
        let render_generation = server.scene_render_generation();
        let effects = server.resolved_effect_scene_for_presentation(&presentation);
        let snapshot = NativeSceneSnapshot::from_surfaces_with_popup_ids(
            surfaces.as_ref(),
            decorations
                .iter()
                .map(DecorationRenderInstance::scene_snapshot)
                .collect(),
            popup_surface_ids.as_ref(),
        );
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

fn visibility_signature(metrics: FullscreenRenderPlanMetrics) -> u64 {
    let mut signature = 0xcbf2_9ce4_8422_2325_u64;
    for value in [
            metrics.fullscreen_active as u64,
            u64::from(metrics.owner_root_surface_id.unwrap_or(0)),
            metrics.solitary_tree_active as u64,
            metrics.culled_surface_count as u64,
            metrics.wallpaper_culled as u64,
            metrics.visible_overlay_count as u64,
            metrics.rejection.map_or(0, |rejection| {
                match rejection {
                    oblivion_one::compositor::FullscreenPresentationRejection::NoFullscreenOwner => 1,
                    oblivion_one::compositor::FullscreenPresentationRejection::OwnerMissing => 2,
                    oblivion_one::compositor::FullscreenPresentationRejection::OwnerMinimized => 3,
                    oblivion_one::compositor::FullscreenPresentationRejection::OwnerDoesNotCoverOutput => 4,
                    oblivion_one::compositor::FullscreenPresentationRejection::OwnerOpacityUnknown => 5,
                    oblivion_one::compositor::FullscreenPresentationRejection::OverlayVisible => 6,
                    oblivion_one::compositor::FullscreenPresentationRejection::SoftwareCursorVisible => 7,
                    oblivion_one::compositor::FullscreenPresentationRejection::TransformOrScaleIncompatible => 8,
                }
            }),
        ] {
            signature ^= value;
            signature = signature.wrapping_mul(0x1000_0000_01b3);
        }
    signature
}

fn finalize_snapshot(
    mut snapshot: NativeSceneSnapshot,
    external_overlay_surface_ids: &[u32],
    visibility: FullscreenRenderPlanMetrics,
    effects: &ResolvedEffectScene,
) -> (NativeSceneSnapshot, u64) {
    snapshot.external_overlay_surface_ids = external_overlay_surface_ids.to_vec();
    snapshot.visibility_signature = visibility_signature(visibility);
    snapshot.effect_damage = effects
        .instances
        .iter()
        .fold(EffectRegion::empty(), |damage, instance| {
            damage.union(&instance.region)
        });
    snapshot.effect_identity_signature = effects.signature;
    let mut scene_identity_signature = snapshot.identity_signature();
    scene_identity_signature ^= effects.signature;
    scene_identity_signature = scene_identity_signature.wrapping_mul(0x1000_0000_01b3);
    (snapshot, scene_identity_signature)
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
mod tests {
    use super::{
        NativeCursorOutputArbitration, NativeCursorRenderMode, NativeFrameRenderer,
        NativeInputState, NativeSceneSnapshot, ResolvedNativeFrameScene, finalize_snapshot,
        reset_snapshot_work_counters, snapshot_work_counters, visibility_signature,
    };
    use oblivion_one::compositor::{
        AnimationTime, FullscreenRenderPlanMetrics, PresentationRect, RenderableSurface,
        RenderableSurfaceDamage, ResolvedEffectScene, SurfaceCommitSequence, SurfaceOpaqueRegion,
        SurfacePlacement, SurfaceRenderBackend, WindowId,
    };
    use oblivion_one::compositor::{
        EffectAnchor, EffectAnchorScope, EffectSceneOrder, OwnCompositorServer,
        ResolvedEffectInstance,
    };
    use oblivion_one::compositor::{PresentationFrameSnapshot, PresentationSceneSample};
    use oblivion_one::effects::{
        EffectFrameDemand, EffectInstanceId, EffectParameterBlock, EffectProgramId, EffectRect,
        EffectRegion,
    };
    use oblivion_one::render_backend::buffer::{
        BufferIdAllocator, BufferSize, CommittedSurfaceBuffer,
    };
    use oblivion_one::window_lifecycle_animation::{LifecycleFrameSnapshot, LifecycleSceneSample};
    use std::borrow::Cow;
    use std::process;
    use wayland_server::protocol::wl_output;

    fn test_surface(
        surface_id: u32,
        width: u32,
        height: u32,
        placement: SurfacePlacement,
    ) -> RenderableSurface {
        let identity = BufferIdAllocator::default()
            .allocate()
            .expect("test buffer identity");
        RenderableSurface {
            surface_id,
            x: 0,
            y: 0,
            width,
            height,
            placement,
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: CommittedSurfaceBuffer::shm_snapshot(
                identity,
                BufferSize::new(width, height).expect("test surface size"),
                vec![0; width as usize * height as usize],
            ),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            opaque_region: SurfaceOpaqueRegion::None,
            damage: RenderableSurfaceDamage::Full,
        }
    }

    #[test]
    fn resolved_native_frame_scene_excludes_culled_transition_owner() {
        let socket_name = format!("typhon-frame-membership-{}", process::id());
        let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("bind compositor for frame-membership regression");
        let rear = test_surface(601, 320, 200, SurfacePlacement::root_at(100, 100));
        let owner = test_surface(602, 1280, 800, SurfacePlacement::absolute_root_at(0, 0));
        let rear_rect =
            PresentationRect::new(100.0, 100.0, 320.0, 200.0).expect("rear presentation rect");
        let settled_rear_rect = PresentationRect::new(140.0, 100.0, 320.0, 200.0)
            .expect("settled rear presentation rect");
        server.install_native_frame_test_scene(
            vec![rear, owner],
            &[
                (601, WindowId::from_raw(1).expect("rear window id")),
                (602, WindowId::from_raw(2).expect("fullscreen window id")),
            ],
            Some(602),
        );
        server.start_test_presentation_transition(
            601,
            rear_rect,
            settled_rear_rect,
            AnimationTime::from_nanos(0),
        );

        let resolved =
            ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(2_000_000));

        assert_eq!(resolved.surface_ids().collect::<Vec<_>>(), [602]);
        assert!(resolved.presentation.transform_for_root(601).is_none());
        assert_eq!(
            resolved
                .presentation_snapshot
                .presented_windows
                .iter()
                .map(|window| window.root_surface_id())
                .collect::<Vec<_>>(),
            [602]
        );
    }

    #[test]
    fn resolved_native_frame_scene_and_egl_request_retain_floating_ssd() {
        let socket_name = format!("typhon-floating-ssd-frame-{}", process::id());
        let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("bind compositor for Floating SSD frame regression");
        server.install_native_frame_test_scene_with_server_decorations(
            vec![test_surface(
                603,
                320,
                200,
                SurfacePlacement::root_at(40, 50),
            )],
            &[(603, WindowId::from_raw(3).expect("test window id"))],
            None,
        );

        let resolved =
            ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(0));

        assert_eq!(resolved.surface_ids().collect::<Vec<_>>(), [603]);
        assert_eq!(resolved.decorations.len(), 1);
        let decoration = &resolved.decorations[0];
        assert_eq!(decoration.root_surface_id(), 603);
        let (_, _, width, height) = decoration.scene_snapshot().bounds();
        assert_eq!(width, 320);
        assert!(height > 200, "Floating SSD must add visible chrome height");
        assert!(resolved.lifecycle_decorations.is_empty());

        let mut renderer = NativeFrameRenderer::default();
        let input_state = NativeInputState::new(1280, 800);
        let request = renderer.egl_scene_draw_request(
            1280,
            800,
            &resolved,
            &server,
            &input_state,
            NativeCursorRenderMode::Hardware,
            None,
        );
        assert_eq!(request.decoration_instances.len(), 1);
        assert_eq!(request.decoration_instances[0].root_surface_id(), 603);
    }

    #[test]
    fn scene_identity_and_damage_reuse_finalized_snapshot() {
        let socket_name = format!("typhon-c2a-snapshot-{}", process::id());
        let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("bind compositor for C2a snapshot regression");
        server.install_native_frame_test_scene(
            vec![test_surface(701, 320, 200, SurfacePlacement::root_at(0, 0))],
            &[(701, WindowId::from_raw(1).expect("test window id"))],
            None,
        );
        reset_snapshot_work_counters();
        let resolved =
            ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(0));
        let construction_counters = snapshot_work_counters();
        assert_eq!(construction_counters.snapshot_finalizations, 1);
        assert_eq!(construction_counters.identity_computations, 1);
        let mut expected_signature = resolved.snapshot_ref().identity_signature();
        expected_signature ^= resolved.effects.signature;
        expected_signature = expected_signature.wrapping_mul(0x1000_0000_01b3);
        assert_eq!(resolved.scene_identity_signature(), expected_signature);
        reset_snapshot_work_counters();

        let _current = resolved.snapshot_ref();
        let _identity = resolved.scene_identity_signature();

        let counters = snapshot_work_counters();
        assert_eq!(counters.snapshot_finalizations, 0);
        assert_eq!(counters.snapshot_owned_clones, 0);
        assert_eq!(counters.identity_computations, 0);

        let owned = resolved.snapshot_owned();
        assert_eq!(owned, *resolved.snapshot_ref());
        assert_eq!(snapshot_work_counters().snapshot_owned_clones, 1);
    }

    #[test]
    fn snapshot_ref_preserves_constructor_popup_ids_and_order() {
        fn assert_popup_ids(popup_surface_ids: &[u32]) {
            let surfaces: &[RenderableSurface] = &[];
            let resolved = ResolvedNativeFrameScene {
                surfaces: Cow::Borrowed(surfaces),
                decorations: Vec::new(),
                popup_surface_ids: Cow::Borrowed(popup_surface_ids),
                external_overlay_surface_ids: Vec::new(),
                render_generation: 1,
                visibility: FullscreenRenderPlanMetrics::default(),
                snapshot: NativeSceneSnapshot::from_surfaces_with_popup_ids(
                    surfaces,
                    Vec::new(),
                    popup_surface_ids,
                ),
                scene_identity_signature: 0,
                effects: ResolvedEffectScene::default(),
                presentation: PresentationSceneSample::empty(AnimationTime::from_nanos(0)),
                presentation_snapshot: PresentationFrameSnapshot::empty(),
                lifecycle: LifecycleSceneSample {
                    sampled_at: AnimationTime::from_nanos(0),
                    lamps: Vec::new(),
                    visual_sources: Vec::new(),
                },
                lifecycle_surfaces: Vec::new(),
                lifecycle_decorations: Vec::new(),
                lifecycle_snapshot: LifecycleFrameSnapshot::default(),
            };
            assert_eq!(resolved.snapshot_ref().popup_surface_ids, popup_surface_ids);
        }

        assert_popup_ids(&[]);
        assert_popup_ids(&[701, 702, 701]);
    }

    #[test]
    fn finalized_snapshot_contains_all_dynamic_metadata_fields() {
        let effect_region = EffectRegion::from_rect(EffectRect::new(20, 30, 40, 50).unwrap());
        let effects = ResolvedEffectScene::new(
            9,
            vec![ResolvedEffectInstance {
                id: EffectInstanceId::new(1).unwrap(),
                program: EffectProgramId::new(2).unwrap(),
                anchor: EffectAnchor::BeforeSurface(701),
                region: effect_region.clone(),
                target_bounds: effect_region.bounding_rect().unwrap(),
                parameter_block: EffectParameterBlock::default(),
                signature: 17,
                frame_demand: EffectFrameDemand::OnDamage,
                visual_group: None,
                anchor_scope: EffectAnchorScope::VisualGroup,
                scene_order: EffectSceneOrder::for_anchor(EffectAnchor::BeforeSurface(701)),
            }],
        );
        let visibility = FullscreenRenderPlanMetrics {
            fullscreen_active: true,
            owner_root_surface_id: Some(701),
            solitary_tree_active: false,
            culled_surface_count: 3,
            wallpaper_culled: true,
            visible_overlay_count: 2,
            rejection: None,
        };

        let (snapshot, cached_signature) = finalize_snapshot(
            NativeSceneSnapshot::from_surfaces_with_popup_ids(&[], Vec::new(), &[701, 702]),
            &[703],
            visibility,
            &effects,
        );

        assert_eq!(snapshot.popup_surface_ids, [701, 702]);
        assert_eq!(snapshot.external_overlay_surface_ids, [703]);
        assert_eq!(
            snapshot.visibility_signature,
            visibility_signature(visibility)
        );
        assert_eq!(snapshot.effect_damage, effect_region);
        assert_eq!(snapshot.effect_identity_signature, effects.signature);
        let mut expected_signature = snapshot.identity_signature();
        expected_signature ^= effects.signature;
        expected_signature = expected_signature.wrapping_mul(0x1000_0000_01b3);
        assert_eq!(cached_signature, expected_signature);
    }

    #[test]
    fn presentation_animation_samples_keep_distinct_frame_local_snapshots() {
        let socket_name = format!("typhon-c2a-animation-{}", process::id());
        let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name)
            .expect("bind compositor for C2a animation regression");
        server.install_native_frame_test_scene(
            vec![test_surface(702, 320, 200, SurfacePlacement::root_at(0, 0))],
            &[(702, WindowId::from_raw(2).expect("test window id"))],
            None,
        );
        server.start_test_presentation_transition(
            702,
            PresentationRect::new(0.0, 0.0, 320.0, 200.0).expect("animation start"),
            PresentationRect::new(100.0, 0.0, 320.0, 200.0).expect("animation target"),
            AnimationTime::from_nanos(0),
        );

        reset_snapshot_work_counters();
        let first =
            ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(250_000));
        let first_counters = snapshot_work_counters();
        reset_snapshot_work_counters();
        let second =
            ResolvedNativeFrameScene::from_server_at(&server, AnimationTime::from_nanos(750_000));
        let second_counters = snapshot_work_counters();

        assert_eq!(first.render_generation, second.render_generation);
        assert_ne!(
            first.snapshot_ref().surfaces[0].bounds,
            second.snapshot_ref().surfaces[0].bounds
        );
        assert_ne!(
            first.scene_identity_signature(),
            second.scene_identity_signature()
        );
        assert_eq!(first_counters.snapshot_finalizations, 1);
        assert_eq!(second_counters.snapshot_finalizations, 1);
        assert_eq!(first_counters.identity_computations, 1);
        assert_eq!(second_counters.identity_computations, 1);
    }

    #[test]
    fn stale_atomic_cursor_debt_is_cleared_without_clearing_software_work() {
        let mut arbitration = NativeCursorOutputArbitration::default();

        arbitration.request_hardware(7, 1_000, 2_000);
        arbitration.set_software_overlay_pending(true);
        arbitration.reconcile_hardware_cursor_liveness(false);

        assert!(arbitration.pending());
        assert_eq!(
            arbitration.disposition(2_000, false, false),
            super::NativeCursorOutputDisposition::SoftwareOverlay
        );
    }

    #[test]
    fn cursor_submit_consumes_only_exact_queued_epoch() {
        let mut arbitration = NativeCursorOutputArbitration::default();
        arbitration.request(10, 1, 100);
        arbitration.request(11, 2, 100);

        arbitration.consume_submitted_epoch(10, 120, 200);

        assert!(arbitration.pending());
        assert_eq!(arbitration.desired_epoch(), 11);
        assert_eq!(arbitration.deadline_ns(), Some(200));

        arbitration.consume_submitted_epoch(11, 220, 300);
        assert!(!arbitration.pending());
    }
}

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
        self.render_frame(NativeFrameRequest {
            width,
            height,
            surfaces: resolved_scene.surfaces.as_ref(),
            external_overlay_surface_ids: resolved_scene.external_overlay_surface_ids.clone(),
            visual_state: input_state.desktop_visual_state(cursor_mode),
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
            visual_state: input_state.desktop_visual_state(cursor_mode),
            output_scale: 1.0,
            decoration_instances: &resolved_scene.decorations,
            presentation_geometry_signature: resolved_scene.presentation.geometry_signature(),
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
