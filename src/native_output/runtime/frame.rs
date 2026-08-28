use super::*;
use std::borrow::Cow;

use oblivion_one::compositor::{
    DecorationRenderInstance, DecorationSceneSnapshot, FullscreenRenderPlanMetrics,
};

#[derive(Debug)]
pub(crate) struct ResolvedNativeFrameScene<'a> {
    pub(crate) surfaces: Cow<'a, [RenderableSurface]>,
    pub(crate) decorations: Vec<DecorationRenderInstance>,
    pub(crate) popup_surface_ids: Cow<'a, [u32]>,
    pub(crate) external_overlay_surface_ids: Vec<u32>,
    pub(crate) render_generation: u64,
    pub(crate) visibility: FullscreenRenderPlanMetrics,
    pub(crate) snapshot: NativeSceneSnapshot,
}

impl<'a> ResolvedNativeFrameScene<'a> {
    pub(crate) fn from_server(server: &'a OwnCompositorServer) -> Self {
        let surfaces = server.native_frame_renderable_surfaces();
        let decorations =
            server.native_decoration_render_instances_for_scale(surfaces.as_ref(), 1.0);
        let popup_surface_ids = Cow::Borrowed(server.popup_surface_ids());
        let external_overlay_surface_ids = server.external_overlay_surface_ids();
        let render_generation = server.scene_render_generation();
        let snapshot = NativeSceneSnapshot::from_surfaces(
            surfaces.as_ref(),
            decorations
                .iter()
                .map(DecorationRenderInstance::scene_snapshot)
                .collect(),
        );
        Self {
            surfaces,
            decorations,
            popup_surface_ids,
            external_overlay_surface_ids,
            render_generation,
            visibility: server.fullscreen_render_plan_metrics(),
            snapshot,
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

    pub(crate) fn visibility_signature(&self) -> u64 {
        let metrics = self.visibility;
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

    pub(crate) fn snapshot(&self) -> NativeSceneSnapshot {
        let snapshot_surface_ids = self
            .snapshot
            .surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        debug_assert_eq!(
            self.surface_ids().collect::<Vec<_>>(),
            snapshot_surface_ids,
            "resolved frame scene and its snapshot diverged on surfaces"
        );
        let snapshot_decoration_ids = self
            .snapshot
            .decorations
            .iter()
            .map(DecorationSceneSnapshot::identity)
            .collect::<Vec<_>>();
        debug_assert_eq!(
            self.decoration_identities().collect::<Vec<_>>(),
            snapshot_decoration_ids,
            "resolved frame scene and its snapshot diverged on decorations"
        );
        let mut snapshot = self.snapshot.clone();
        snapshot.popup_surface_ids = self.popup_surface_ids.to_vec();
        snapshot.external_overlay_surface_ids = self.external_overlay_surface_ids.clone();
        snapshot.visibility_signature = self.visibility_signature();
        snapshot
    }

    pub(crate) fn scene_identity_signature(&self) -> u64 {
        self.snapshot().identity_signature()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeRepaintInputs {
    // Retained for cycle diagnostics only; accepting a socket is not visual work.
    pub(crate) accepted_clients: bool,
    pub(crate) render_generation_changed: bool,
    pub(crate) pending_frame_work: bool,
    pub(crate) only_pending_surface_frame_callbacks: bool,
    pub(crate) redraw_requested: bool,
    pub(crate) cursor_work_pending: bool,
    pub(crate) page_flip_pending: bool,
}

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

    let protocol_only_present = inputs.pending_frame_work
        && inputs.only_pending_surface_frame_callbacks
        && !inputs.render_generation_changed
        && !inputs.redraw_requested;
    NativeRepaintDecision {
        // Accepting a socket is protocol progress, not visual work.  The
        // accepted-client bit is retained in the input for diagnostics, but
        // a client must create visible scene or protocol-owned output work
        // before the primary renderer is scheduled.
        repaint: inputs.render_generation_changed
            || inputs.redraw_requested
            || inputs.cursor_work_pending
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
    use super::NativeCursorOutputArbitration;

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
    pub(crate) failed: Option<(PointerConstraintBackendId, &'static str)>,
    pub(crate) restore_position: Option<CompositorOutputPosition>,
    pub(crate) cursor_position: Option<CompositorOutputPosition>,
    pub(crate) cursor_visibility_changed: Option<bool>,
}

impl NativePointerConstraintBackend {
    pub(crate) fn new() -> Self {
        Self {
            active: None,
            cursor_visible: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn active_locked(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|constraint| constraint.mode == PointerConstraintMode::Locked)
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
            PointerConstraintBackendRequest::ActivateLocked { id, anchor } => {
                self.activate_locked(id, anchor)
            }
            PointerConstraintBackendRequest::ActivateConfined { id, region } => {
                self.activate_confined(id, cursor_position, region)
            }
            PointerConstraintBackendRequest::UpdateConfinedRegion { id, region } => {
                self.update_confined_region(id, cursor_position, region)
            }
            PointerConstraintBackendRequest::Deactivate {
                id,
                restore_position,
            } => self.deactivate(id, restore_position),
            PointerConstraintBackendRequest::WarpPointer { position } => {
                native_pointer_debug_log_lazy(|| {
                    format!(
                        "backend warp requested position=({},{})",
                        position.x, position.y
                    )
                });
                NativePointerConstraintBackendAction {
                    cursor_position: Some(position),
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
            ..NativePointerConstraintBackendAction::default()
        }
    }

    pub(crate) fn deactivate(
        &mut self,
        id: PointerConstraintBackendId,
        restore_position: Option<CompositorOutputPosition>,
    ) -> NativePointerConstraintBackendAction {
        let Some(active) = self.active.as_ref().cloned() else {
            return NativePointerConstraintBackendAction::default();
        };
        if active.id != id {
            return NativePointerConstraintBackendAction::default();
        }
        self.active = None;
        let restore_position = (active.mode == PointerConstraintMode::Locked)
            .then(|| restore_position.unwrap_or(active.anchor));
        NativePointerConstraintBackendAction {
            deactivated: Some(id),
            restore_position,
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
            client_cursor: cursor_mode
                .is_software()
                .then(|| server.client_cursor_render_state())
                .flatten(),
            current_damage,
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
