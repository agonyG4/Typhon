use super::planner::NativePresentationPath;
use super::*;
use oblivion_one::native::kms::KmsBackendKind;

pub(super) struct DirectPresentationInspection {
    pub(super) cursor_direct_compatible: bool,
    pub(super) atomic_primary_commit_pending: bool,
    pub(super) direct_candidate_changed: bool,
    pub(super) direct_candidate_eligible: bool,
    pub(super) primary_visual_work_pending: bool,
    pub(super) composition_required: bool,
}

pub(super) struct DirectPresentationInputs<'a> {
    pub(super) server: &'a OwnCompositorServer,
    pub(super) kms_kind: KmsBackendKind,
    pub(super) atomic_cursor: Option<&'a NativeAtomicCursor>,
    pub(super) cursor_render_mode: NativeCursorRenderMode,
    pub(super) cursor_visible: bool,
    pub(super) client_cursor_active: bool,
    pub(super) client_cursor_hardware_usable: bool,
    pub(super) legacy_cursor_available: bool,
    pub(super) page_flip_pending: bool,
    pub(super) atomic_commit_pending: bool,
    pub(super) drm_file_generation: u64,
    pub(super) effective_cursor: Option<&'a AtomicCursorVisualState>,
    pub(super) last_direct_candidate_key: &'a mut Option<DirectScanoutCandidateKey>,
    pub(super) scene_changed: bool,
    pub(super) pending_frame_work: bool,
    pub(super) primary_redraw_requested: bool,
    pub(super) direct_active: bool,
}

pub(super) fn inspect_direct_presentation(
    inputs: DirectPresentationInputs<'_>,
) -> DirectPresentationInspection {
    let cursor_direct_compatible = if inputs.kms_kind == KmsBackendKind::Atomic {
        if inputs.client_cursor_active {
            !inputs.cursor_visible || inputs.client_cursor_hardware_usable
        } else {
            inputs.atomic_cursor.as_ref().is_some_and(|cursor| {
                atomic_cursor_visibility_policy(
                    cursor.desired().visible,
                    cursor.failure_latched(),
                    inputs.cursor_render_mode,
                    inputs.cursor_visible,
                )
                .direct_compatible(inputs.cursor_visible)
            }) || !inputs.cursor_visible
        }
    } else {
        true
    };
    let atomic_primary_commit_pending = inputs.page_flip_pending || inputs.atomic_commit_pending;
    let direct_candidate = inputs.server.direct_scanout_scene_candidate().ok();
    let direct_candidate_eligible = direct_candidate.is_some();
    let direct_candidate_key = direct_candidate.as_ref().and_then(|candidate| {
        DirectScanoutCandidateKey::from_candidate(
            candidate,
            inputs.drm_file_generation,
            super::scanout::direct_cursor_plan_key(
                inputs.effective_cursor,
                cursor_direct_compatible,
            ),
            0,
        )
    });
    let direct_candidate_changed = direct_candidate_key != *inputs.last_direct_candidate_key;
    *inputs.last_direct_candidate_key = direct_candidate_key;
    let primary_visual_work_pending =
        inputs.scene_changed || inputs.pending_frame_work || inputs.primary_redraw_requested;
    let composition_required = (inputs.client_cursor_active
        && !inputs.client_cursor_hardware_usable)
        || (inputs.cursor_render_mode.is_software() && inputs.cursor_visible)
        || (inputs.cursor_visible
            && inputs.cursor_render_mode == NativeCursorRenderMode::Hardware
            && inputs.atomic_cursor.is_none()
            && !inputs.legacy_cursor_available)
        || !cursor_direct_compatible
        || (inputs.direct_active && !direct_candidate_eligible);
    DirectPresentationInspection {
        cursor_direct_compatible,
        atomic_primary_commit_pending,
        direct_candidate_changed,
        direct_candidate_eligible,
        primary_visual_work_pending,
        composition_required,
    }
}

pub(super) fn suppress_direct_render_ahead(
    presentation_path: NativePresentationPath,
    scheduler_decision: &mut SchedulerDecision,
    scanout: &mut NativeScanoutBackend,
    perf: NativePerfLogger,
) {
    if presentation_path == NativePresentationPath::DirectPrimary
        && *scheduler_decision == SchedulerDecision::RenderAhead
    {
        *scheduler_decision = SchedulerDecision::Render;
        scanout.note_direct_composited_render_ahead_suppressed();
        perf.log("direct_scanout", || {
            vec![NativePerfField::str(
                "event",
                "suppressed_composited_render_ahead",
            )]
        });
    }
    if presentation_path == NativePresentationPath::IdleDirect
        && matches!(
            *scheduler_decision,
            SchedulerDecision::Render | SchedulerDecision::RenderAhead
        )
    {
        *scheduler_decision = SchedulerDecision::WaitForPageFlip;
        scanout.note_direct_composited_render_ahead_suppressed();
        perf.log("direct_scanout", || {
            vec![NativePerfField::str(
                "event",
                "suppressed_composited_render_ahead",
            )]
        });
    }
}
