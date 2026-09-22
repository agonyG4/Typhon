#![allow(unused_imports)]
use super::super::*;
use super::{
    client_setup::*, clipboard_dmabuf::*, frame_buffer_client::*, input_client::*,
    locked_relative::*, output_bindings::*, registry_state::*, subsurface_client::*, window_ops::*,
};
use crate::compositor::layer_shell::KeyboardInteractivity;
use crate::render_backend::buffer::DrmFormat;
use crate::wm::{LayoutMembership, WorkspaceId, WorkspaceLocation};

type PendingSurfaceTreeTransactionsSnapshot = Vec<(u64, Vec<(u32, u64)>)>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::compositor::tests) struct SubsurfaceStackStateSnapshot {
    pub(in crate::compositor::tests) committed: Option<Vec<u32>>,
    pub(in crate::compositor::tests) latched: Option<Vec<u32>>,
    pub(in crate::compositor::tests) pending: Option<Vec<u32>>,
}

#[derive(Debug)]
pub(in crate::compositor::tests) struct DelayedParentSubsurfaceStackSnapshots {
    pub(in crate::compositor::tests) after_first_parent_commit: SubsurfaceStackStateSnapshot,
    pub(in crate::compositor::tests) after_child_creation: SubsurfaceStackStateSnapshot,
    pub(in crate::compositor::tests) after_second_restack: SubsurfaceStackStateSnapshot,
    pub(in crate::compositor::tests) after_first_publication: SubsurfaceStackStateSnapshot,
    pub(in crate::compositor::tests) after_second_publication: SubsurfaceStackStateSnapshot,
}

pub(in crate::compositor::tests) fn create_test_shm_file(
    pixels: &[u32],
) -> Result<File, Box<dyn std::error::Error>> {
    let path = runtime_socket_path(&format!("oblivion-one-shm-{}", unique_socket_name()));
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    fs::remove_file(path)?;
    for pixel in pixels {
        file.write_all(&pixel.to_ne_bytes())?;
    }
    file.flush()?;
    Ok(file)
}

pub(in crate::compositor::tests) fn open_fd_count() -> usize {
    fs::read_dir("/proc/self/fd")
        .expect("test process must expose /proc/self/fd")
        .count()
}

pub(in crate::compositor::tests) fn spawn_test_server(
    mut server: OwnCompositorServer,
) -> (Arc<AtomicBool>, JoinHandle<OwnCompositorServer>) {
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let server_thread = thread::spawn(move || {
        while server_running.load(Ordering::Relaxed) {
            let _ = server.tick();
            thread::sleep(Duration::from_millis(2));
        }
        server
    });

    (running, server_thread)
}

pub(in crate::compositor::tests) fn stop_test_server(
    running: Arc<AtomicBool>,
    server_thread: JoinHandle<OwnCompositorServer>,
) -> OwnCompositorServer {
    running.store(false, Ordering::Relaxed);
    server_thread.join().unwrap()
}

#[derive(Clone)]
pub(in crate::compositor::tests) enum ServerCommand {
    ApplyXwaylandSelectionEvent(crate::xwayland::XwaylandSelectionEvent),
    KeyboardKey {
        key: u32,
        pressed: bool,
    },
    SetKeyboardLayout {
        index: u32,
        reply: Sender<bool>,
    },
    NextKeyboardLayout {
        reply: Sender<bool>,
    },
    PreviousKeyboardLayout {
        reply: Sender<bool>,
    },
    SetShortcutInhibitorPolicy {
        surface_id: u32,
        enabled: bool,
    },
    PointerMotion {
        x: f64,
        y: f64,
    },
    PointerMotionSample(PointerMotionSample),
    ActivatePointerConstraint(PointerConstraintMode),
    PointerButton {
        button: u32,
        pressed: bool,
    },
    PointerAxis {
        horizontal: f64,
        vertical: f64,
    },
    PointerAxisFrame(PointerAxisFrame),
    BeginFrameAction {
        x: f64,
        y: f64,
    },
    BeginMove {
        x: f64,
        y: f64,
    },
    BeginResize {
        x: f64,
        y: f64,
    },
    UpdateInteraction {
        x: f64,
        y: f64,
    },
    UpdateInteractionResult {
        x: f64,
        y: f64,
        reply: Sender<bool>,
    },
    SendWindowInteractionPointerMotion {
        timestamp_usec: u64,
        x: f64,
        y: f64,
        reply: Sender<usize>,
    },
    EndInteraction,
    ResizeFocusedTo {
        width: u32,
        height: u32,
    },
    SetOutputSize {
        width: u32,
        height: u32,
    },
    SetOutputRefresh {
        refresh_hz: u32,
    },
    SetOutputScale {
        scale_factor: f64,
    },
    SetOutputPreferredTransform(u32),
    CapturePendingAstreaScreenCapture(Sender<bool>),
    UnregisterOutputResources,
    MinimizeFocused,
    RestoreNextMinimized,
    FocusRootWindow(u32),
    RaiseRootWindow(u32),
    ActivateRootWindow(u32),
    ToggleMaximizeFocused,
    ToggleFocusedWindowLayout,
    ToggleFullscreenFocused,
    ActivateWorkspace {
        workspace: u32,
    },
    ToggleDefaultSpecialWorkspace,
    MoveFocusedWindowToOrFromSpecialWorkspace,
    MoveFocusedWindowToWorkspace {
        workspace: u32,
    },
    MoveWindowToWorkspace {
        window_id: WindowId,
        workspace: u32,
        reply: Sender<bool>,
    },
    SetFocusedRootVisualGeometry {
        placement: SurfacePlacement,
        width: u32,
        height: u32,
    },
    SetPointerHitInstrumentationEnabled(bool),
    CaptureRenderGeneration(Sender<u64>),
    CaptureSceneRenderGeneration(Sender<u64>),
    CaptureCoreComplianceMetrics(Sender<CoreComplianceMetrics>),
    CapturePointerHitGeneration(Sender<u64>),
    CapturePointerSceneHit {
        x: f64,
        y: f64,
        reply: Sender<(Option<u32>, Option<(f64, f64)>)>,
    },
    CaptureResolvedEffectScene(Sender<ResolvedEffectScene>),
    CaptureLifecycleEffectPath(Sender<LifecycleEffectPathSnapshot>),
    ReplaceBlurPolicyConfig {
        config: crate::blur_policy::BlurPolicyConfig,
        reply: Sender<bool>,
    },
    CaptureFocusGeneration(Sender<u64>),
    CapturePointerInputMetrics(Sender<PointerInputMetrics>),
    CaptureRenderGenerationCause(Sender<RenderGenerationCause>),
    CaptureRenderableSurfaceCount(Sender<usize>),
    CaptureNativeDecorationCount(Sender<usize>),
    CaptureNativeFrameSurfaceIds(Sender<Vec<u32>>),
    CaptureSurfaceResourceCount(Sender<usize>),
    CaptureShmResourceCounts(Sender<(usize, usize, usize)>),
    CaptureRenderableSurfaceSnapshot(Sender<Vec<RenderableSurfaceSnapshot>>),
    CaptureSurfaceBufferOwnership {
        surface_id: u32,
        reply: Sender<SurfaceBufferOwnershipSnapshot>,
    },
    CaptureSubsurfaceStackState {
        parent_id: u32,
        reply: Sender<SubsurfaceStackStateSnapshot>,
    },
    CaptureLayerSurfaceCommitState {
        surface_id: u32,
        reply: Sender<Option<(i32, u8)>>,
    },
    CaptureLayerSurfaceLifecycleState {
        surface_id: u32,
        reply: Sender<Option<LayerSurfaceLifecycleSnapshot>>,
    },
    CaptureLayerSurfacePendingAck {
        surface_id: u32,
        reply: Sender<Option<u32>>,
    },
    CaptureCommittedWindowGeometry(Sender<Option<XdgWindowGeometry>>),
    CaptureToplevelVisualGeometry(Sender<Option<ToplevelVisualGeometrySnapshot>>),
    CaptureRootWindowGeometry {
        root_surface_id: u32,
        reply: Sender<Option<WindowGeometry>>,
    },
    CaptureMinimizeAnchor {
        window_id: WindowId,
        reply: Sender<Option<MinimizeAnchorRect>>,
    },
    CapturePresentationTransitionCurve(Sender<Option<AnimationCurve>>),
    CapturePresentationTransitionCurveForRoot {
        root_surface_id: u32,
        reply: Sender<Option<AnimationCurve>>,
    },
    CapturePresentationTransitionStart {
        root_surface_id: u32,
        reply: Sender<Option<PresentationWindowSample>>,
    },
    CaptureFocusedPresentationAfter {
        elapsed_nanos: u64,
        reply: Sender<Option<PresentationWindowSample>>,
    },
    CapturePresentedPresentation {
        root_surface_id: u32,
        reply: Sender<(
            u64,
            Option<PresentedWindowGeometry>,
            Option<PresentationGroupTransform>,
        )>,
    },
    DropFocusedToplevelVisualGeometry,
    CancelFocusedPresentationTransition,
    CaptureFullscreenPresentationEligibility(Sender<FullscreenPresentationEligibility>),
    CaptureFullscreenRenderPlanMetrics(Sender<FullscreenRenderPlanMetrics>),
    CaptureConfigureSerial(Sender<u32>),
    CaptureSurfacePresentationMetadata {
        surface_id: u32,
        reply: Sender<Option<(SurfacePresentationMetadata, SurfacePresentationMetadata)>>,
    },
    CaptureSurfacePresentationLineage {
        surface_id: u32,
        reply: Sender<Option<(u64, SurfaceCommitSequence)>>,
    },
    CaptureDirectScanoutCandidate(
        Sender<Result<DirectScanoutCandidateSnapshot, DirectScanoutSceneRejection>>,
    ),
    CaptureClientCursorSnapshot(Sender<Option<ClientCursorSnapshot>>),
    CaptureInteractionCursorState(Sender<InteractionCursorStateSnapshot>),
    CaptureCursorHiddenByPointerLock(Sender<bool>),
    CaptureClipboardState(Sender<ClipboardStateSnapshot>),
    CaptureXdgRoleSnapshot {
        surface_id: u32,
        reply: Sender<XdgRoleSnapshot>,
    },
    CapturePendingSurfaceTreeTransactions(Sender<PendingSurfaceTreeTransactionsSnapshot>),
    CapturePendingFrameCallbacks(Sender<bool>),
    CaptureOnlyPendingSurfaceFrameCallbacks(Sender<bool>),
    CapturePendingFrameWork(Sender<bool>),
    CaptureFrameEligiblePresentationFeedbackWork(Sender<bool>),
    CaptureFrameCallbackMetrics(Sender<FrameCallbackMetrics>),
    CompleteProtocolOnlyFrameTick(Sender<ProtocolOnlyCompletion>),
    CaptureIdleInhibited(Sender<bool>),
    CapturePointerConstraintBackendRequests(Sender<Vec<PointerConstraintBackendRequest>>),
    SettlePointerConstraintBackendRequests,
    CapturePendingLockedPointerReveal(Sender<bool>),
    CapturePendingLockedPointerRevealAndBackendRequests(
        Sender<(bool, Vec<PointerConstraintBackendRequest>)>,
    ),
    CapturePointerConstraintIds(Sender<Vec<u64>>),
    CaptureTerminalClientCount(Sender<usize>),
    CapturePointerConstraintSnapshot {
        constraint_id: u64,
        reply: Sender<Option<PointerConstraintSurfaceSnapshot>>,
    },
    CaptureLastPointerPosition(Sender<(f64, f64)>),
    CapturePointerFocusSurfaceId(Sender<Option<u32>>),
    CaptureActiveLockedPointerAnchor(Sender<Option<(f64, f64)>>),
    CaptureFocusedSurfaceId(Sender<Option<u32>>),
    CaptureKeyboardFocusSurfaceId(Sender<Option<u32>>),
    CaptureFocusedWindowId(Sender<Option<WindowId>>),
    CaptureFocusedToplevelMode(Sender<Option<ToplevelMode>>),
    CaptureWindowIdForSurface {
        surface_id: u32,
        reply: Sender<Option<WindowId>>,
    },
    CaptureWindowManagement {
        window_id: WindowId,
        reply: Sender<Option<(LayoutMembership, WorkspaceLocation, bool)>>,
    },
    CaptureWindowInteractionDebugSnapshot(Sender<Option<WindowInteractionDebugSnapshot>>),
    CapturePointerOwnershipIsClear(Sender<bool>),
    CaptureWindowInteractionReleaseMetrics(Sender<WindowInteractionReleaseMetrics>),
    CaptureUsableOutputGeometry(Sender<OutputRect>),
    ApplyXwaylandWindowEvent {
        event: Box<crate::xwayland::xwm::XwmEvent>,
        reply: Sender<Vec<crate::xwayland::xwm::XwmCommand>>,
    },
    ApplyXwaylandAssociationEvent(crate::xwayland::xwm::XwmAssociationEvent),
    CaptureXwaylandAssociationEvents(Sender<Vec<crate::xwayland::XwaylandAssociationEvent>>),
    CaptureXwaylandBackendCommands(Sender<Vec<crate::xwayland::xwm::XwmCommand>>),
    CaptureX11WindowState {
        window_id: WindowId,
        reply: Sender<Option<(Option<u32>, bool)>>,
    },
    AuthorizeAstreaShellPid(u32),
    ClearAstreaShellAuthorization,
    EmitAstreaShortcut {
        namespace: String,
        name: String,
        phase: AstreaShortcutPhase,
        timestamp: u32,
        reply: Sender<usize>,
    },
    UpdatePointerPositionWithoutClientDispatch {
        x: f64,
        y: f64,
        reply: Sender<bool>,
    },
    BeginNativeInputBatch,
    ProgressWaylandDuringNativeInputBatch,
    EndNativeInputBatch,
    PointerConstraintBackendActivated(PointerConstraintBackendId),
    PointerConstraintBackendFailed(PointerConstraintBackendId),
    #[allow(dead_code)]
    PointerConstraintBackendDeactivated(PointerConstraintBackendId),
    ClearPointerEnterTracking,
    Barrier(Sender<()>),
    PrepareFrame,
    AdmitInteractiveVisualState {
        render_ahead: bool,
    },
    CaptureLegacyPreparedFrame,
    MarkPreparedFrameCallbacksRendered,
    CaptureAndFinishLegacyPreparedFrame,
    CaptureAndCompleteRenderedLegacyPreparedFrame,
    CaptureLegacySubmittedAndPreparedFrames,
    CapturePreparedFrame(Sender<bool>),
    PublishTestPresentationAt {
        frame_id: u64,
        at: AnimationTime,
    },
    PublishFocusedPresentationAfter {
        frame_id: u64,
        elapsed_nanos: u64,
    },
    SettleNoVisualChangeWork {
        owns_frame_batch: bool,
        reply: Sender<bool>,
    },
    FinishPreparedFrame,
    FinishFrame,
    FinishFrameWithPresentation(FramePresentation),
    CaptureFrameBatch {
        frame_id: u64,
        reply: Sender<CompositorFrameBatchId>,
    },
    CaptureNativeFrameBatch {
        frame_id: u64,
        reply: Sender<CompositorFrameBatchId>,
    },
    CaptureFrameBatchSurfaceIds {
        batch_id: CompositorFrameBatchId,
        reply: Sender<Vec<u32>>,
    },
    CapturePresentationFeedbackBatch {
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
        reply: Sender<Option<PresentationFeedbackBatchId>>,
    },
    DiscardAllPresentationFeedbacks,
    CompletePresentationFeedbackBatch {
        batch_id: PresentationFeedbackBatchId,
        presentation: FramePresentation,
    },
    CaptureFrameCallbackPacingCompleted {
        batch_id: CompositorFrameBatchId,
        reply: Sender<bool>,
    },
    PrepareTerminalCallbackOwnership {
        batch_id: CompositorFrameBatchId,
        disposition: TerminalCallbackDisposition,
        reply: Sender<TerminalCallbackOwnership>,
    },
    CompleteFrameBatch {
        frame_id: u64,
        batch_id: CompositorFrameBatchId,
        presentation: FramePresentation,
    },
    CompleteFrameBatchNow {
        frame_id: u64,
        batch_id: CompositorFrameBatchId,
    },
    MarkFrameCallbacksRendered(CompositorFrameBatchId),
    CompleteFrameCallbacksAfterAdmission {
        batch_id: CompositorFrameBatchId,
        admission: FrameCallbackAdmission,
    },
    NoteFrameCallbacksDeferredReady(CompositorFrameBatchId),
    NoteFrameCallbackAdmissionFailure(CompositorFrameBatchId),
    RestoreFrameBatchAfterRenderFailure(CompositorFrameBatchId),
    CompleteNoVisualChangeFrameBatch(CompositorFrameBatchId),
    CompleteDirectFrameBatch {
        frame_id: u64,
        batch_id: CompositorFrameBatchId,
        direct_surface_id: u32,
        presentation: FramePresentation,
    },
    CompleteDirectFrameBatchWithLineage {
        frame_id: u64,
        batch_id: CompositorFrameBatchId,
        direct_surface_id: u32,
        direct_lineage: (u64, SurfaceCommitSequence),
        presentation: FramePresentation,
    },
    DiscardFrameBatch {
        batch_id: CompositorFrameBatchId,
        reason: FrameBatchDiscardReason,
    },
    PresentFrame,
    MarkRenderDamagePresented,
    Stop,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::compositor::tests) struct LifecycleEffectPathSnapshot {
    pub lifecycle_surface_ids: Vec<u32>,
    pub raw_lamp_surface_ids: Vec<u32>,
    pub presentation_effect_instance_count: usize,
    pub lifecycle_resolved_effect_instance_count: usize,
    pub canonical_surface_count: usize,
    pub retained_surface_count: usize,
    pub lifecycle_transition_count: usize,
    pub restore_suppression_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor::tests) struct DirectScanoutCandidateSnapshot {
    pub(in crate::compositor::tests) surface_id: u32,
    pub(in crate::compositor::tests) root_surface_id: u32,
    pub(in crate::compositor::tests) generation: u64,
    pub(in crate::compositor::tests) commit_sequence: SurfaceCommitSequence,
    pub(in crate::compositor::tests) buffer_size: BufferSize,
    pub(in crate::compositor::tests) output_size: BufferSize,
    pub(in crate::compositor::tests) format: DrmFormat,
    pub(in crate::compositor::tests) viewport_identity_metadata_present: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::compositor::tests) struct PointerConstraintSurfaceSnapshot {
    pub(in crate::compositor::tests) committed: bool,
    pub(in crate::compositor::tests) active: bool,
    pub(in crate::compositor::tests) protocol_resource_alive: bool,
    pub(in crate::compositor::tests) backend_pending: bool,
    pub(in crate::compositor::tests) surface_constraint_pending: bool,
    pub(in crate::compositor::tests) lifecycle_removal_pending: bool,
    pub(in crate::compositor::tests) defunct: bool,
    pub(in crate::compositor::tests) committed_region: SurfaceInputRegion,
    pub(in crate::compositor::tests) committed_cursor_position_hint: Option<(f64, f64)>,
}

pub(in crate::compositor::tests) fn spawn_controllable_test_server(
    mut server: OwnCompositorServer,
) -> (Sender<ServerCommand>, JoinHandle<OwnCompositorServer>) {
    let (commands, receiver) = mpsc::channel();
    let server_thread = thread::spawn(move || {
        let mut running = true;
        while running {
            let mut barriers = Vec::new();
            while let Ok(command) = receiver.try_recv() {
                match command {
                    ServerCommand::ApplyXwaylandSelectionEvent(event) => {
                        server.apply_xwayland_selection_event(event);
                    }
                    ServerCommand::KeyboardKey { key, pressed } => {
                        server.send_keyboard_key(key, pressed);
                    }
                    ServerCommand::SetKeyboardLayout { index, reply } => {
                        let _ = reply.send(server.set_keyboard_layout(index).is_ok());
                    }
                    ServerCommand::NextKeyboardLayout { reply } => {
                        let _ = reply.send(server.next_keyboard_layout().is_ok());
                    }
                    ServerCommand::PreviousKeyboardLayout { reply } => {
                        let _ = reply.send(server.previous_keyboard_layout().is_ok());
                    }
                    ServerCommand::SetShortcutInhibitorPolicy {
                        surface_id,
                        enabled,
                    } => {
                        server
                            .state
                            .set_shortcut_inhibitor_policy_enabled_for_surface(surface_id, enabled);
                    }
                    ServerCommand::PointerMotion { x, y } => {
                        server.send_pointer_motion(x, y);
                    }
                    ServerCommand::PointerMotionSample(sample) => {
                        server.send_pointer_motion_sample(sample);
                    }
                    ServerCommand::ActivatePointerConstraint(mode) => {
                        server
                            .state
                            .activate_pointer_constraint_for_focused_surface(mode);
                    }
                    ServerCommand::PointerButton { button, pressed } => {
                        server.send_pointer_button(button, pressed);
                    }
                    ServerCommand::PointerAxis {
                        horizontal,
                        vertical,
                    } => {
                        server.send_pointer_axis(horizontal, vertical);
                    }
                    ServerCommand::PointerAxisFrame(frame) => {
                        server.send_pointer_axis_frame(frame);
                    }
                    ServerCommand::BeginFrameAction { x, y } => {
                        server.begin_window_frame_action_at(x, y);
                    }
                    ServerCommand::BeginMove { x, y } => {
                        server.begin_window_move_at(x, y);
                    }
                    ServerCommand::BeginResize { x, y } => {
                        server.begin_window_resize_at(x, y);
                    }
                    ServerCommand::UpdateInteraction { x, y } => {
                        server.update_window_interaction(x, y);
                    }
                    ServerCommand::UpdateInteractionResult { x, y, reply } => {
                        let _ = reply.send(server.update_window_interaction(x, y));
                    }
                    ServerCommand::SendWindowInteractionPointerMotion {
                        timestamp_usec,
                        x,
                        y,
                        reply,
                    } => {
                        let _ = reply.send(server.send_window_interaction_pointer_motion(
                            timestamp_usec,
                            x,
                            y,
                        ));
                    }
                    ServerCommand::EndInteraction => {
                        server.end_window_interaction();
                    }
                    ServerCommand::ResizeFocusedTo { width, height } => {
                        server.resize_focused_window_to(width, height);
                    }
                    ServerCommand::SetOutputSize { width, height } => {
                        server.set_output_size(width, height);
                    }
                    ServerCommand::SetOutputRefresh { refresh_hz } => {
                        server.set_output_refresh_hz(refresh_hz);
                    }
                    ServerCommand::SetOutputScale { scale_factor } => {
                        server.set_output_scale_factor(scale_factor);
                    }
                    ServerCommand::SetOutputPreferredTransform(transform) => {
                        let transform = match transform {
                            0 => wayland_server::protocol::wl_output::Transform::Normal,
                            1 => wayland_server::protocol::wl_output::Transform::_90,
                            2 => wayland_server::protocol::wl_output::Transform::_180,
                            3 => wayland_server::protocol::wl_output::Transform::_270,
                            4 => wayland_server::protocol::wl_output::Transform::Flipped,
                            5 => wayland_server::protocol::wl_output::Transform::Flipped90,
                            6 => wayland_server::protocol::wl_output::Transform::Flipped180,
                            7 => wayland_server::protocol::wl_output::Transform::Flipped270,
                            _ => continue,
                        };
                        server.set_output_preferred_transform(transform);
                    }
                    ServerCommand::CapturePendingAstreaScreenCapture(reply) => {
                        let _ = reply.send(server.has_pending_astrea_screen_capture());
                    }
                    ServerCommand::UnregisterOutputResources => {
                        let outputs = server.state.output_resources.clone();
                        for output in outputs {
                            server.state.unregister_output_resource(&output);
                        }
                    }
                    ServerCommand::MinimizeFocused => {
                        server.minimize_focused_window();
                    }
                    ServerCommand::RestoreNextMinimized => {
                        server.restore_next_minimized_window();
                    }
                    ServerCommand::FocusRootWindow(surface_id) => {
                        if let Some(surface) = server.state.surface_resource_by_id(surface_id) {
                            server.state.focus_surface(surface);
                            server.publish_astrea_toplevel_updates();
                        }
                    }
                    ServerCommand::RaiseRootWindow(surface_id) => {
                        server.state.raise_root_window(surface_id);
                    }
                    ServerCommand::ActivateRootWindow(surface_id) => {
                        server.activate_window(surface_id);
                    }
                    ServerCommand::ToggleMaximizeFocused => {
                        server.toggle_maximize_focused_window();
                    }
                    ServerCommand::ToggleFocusedWindowLayout => {
                        server.toggle_focused_window_layout();
                    }
                    ServerCommand::ToggleFullscreenFocused => {
                        server.toggle_fullscreen_focused_window();
                    }
                    ServerCommand::ActivateWorkspace { workspace } => {
                        if let Some(workspace) = WorkspaceId::new(workspace) {
                            server.activate_workspace(workspace);
                        }
                    }
                    ServerCommand::ToggleDefaultSpecialWorkspace => {
                        server.toggle_default_special_workspace();
                    }
                    ServerCommand::MoveFocusedWindowToOrFromSpecialWorkspace => {
                        server.move_focused_window_to_or_from_special_workspace();
                    }
                    ServerCommand::MoveFocusedWindowToWorkspace { workspace } => {
                        let Some(workspace) = WorkspaceId::new(workspace) else {
                            continue;
                        };
                        server.move_focused_window_to_workspace(workspace);
                    }
                    ServerCommand::MoveWindowToWorkspace {
                        window_id,
                        workspace,
                        reply,
                    } => {
                        let moved = WorkspaceId::new(workspace).is_some_and(|workspace| {
                            server
                                .state
                                .move_window_family_to_workspace(window_id, workspace)
                        });
                        let _ = reply.send(moved);
                    }
                    ServerCommand::SetFocusedRootVisualGeometry {
                        placement,
                        width,
                        height,
                    } => {
                        if let Some(surface_id) = server.state.focused_root_surface_id() {
                            server.state.set_surface_placement(surface_id, placement);
                            let has_visual = if let Some(visual) =
                                server.state.toplevel_visual_geometries.get_mut(&surface_id)
                            {
                                visual.placement = placement;
                                visual.width = width;
                                visual.height = height;
                                true
                            } else {
                                false
                            };
                            if !has_visual {
                                if let Some(surface) = server
                                    .state
                                    .renderable_surfaces
                                    .iter_mut()
                                    .find(|surface| surface.surface_id == surface_id)
                                {
                                    surface.width = width;
                                    surface.height = height;
                                }
                                if let Some(geometry) =
                                    server.state.surface_window_geometries.get_mut(&surface_id)
                                {
                                    geometry.width = width as i32;
                                    geometry.height = height as i32;
                                }
                            }
                            server.state.reconcile_all_surface_output_memberships();
                            server
                                .state
                                .update_toplevel_visual_render_assignment(surface_id);
                        }
                    }
                    ServerCommand::SetPointerHitInstrumentationEnabled(enabled) => {
                        server.state.pointer_hit_instrumentation_enabled = enabled;
                    }
                    ServerCommand::CaptureRenderGeneration(reply) => {
                        let _ = reply.send(server.render_generation());
                    }
                    ServerCommand::CaptureSceneRenderGeneration(reply) => {
                        let _ = reply.send(server.scene_render_generation());
                    }
                    ServerCommand::CaptureCoreComplianceMetrics(reply) => {
                        let _ = reply.send(server.core_compliance_metrics());
                    }
                    ServerCommand::CapturePointerHitGeneration(reply) => {
                        let _ = reply.send(server.state.pointer_hit_generation);
                    }
                    ServerCommand::CapturePointerSceneHit { x, y, reply } => {
                        let hit = server.state.pointer_scene_hit_at(x, y);
                        let snapshot = match hit {
                            PointerSceneHit::Client { target } => (
                                Some(compositor_surface_id(&target.surface)),
                                Some((target.surface_x, target.surface_y)),
                            ),
                            PointerSceneHit::Decoration { .. } | PointerSceneHit::None => {
                                (None, None)
                            }
                        };
                        let _ = reply.send(snapshot);
                    }
                    ServerCommand::CaptureResolvedEffectScene(reply) => {
                        let _ = reply.send(server.resolved_effect_scene());
                    }
                    ServerCommand::CaptureLifecycleEffectPath(reply) => {
                        let at =
                            AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
                        let (canonical_surfaces, fullscreen_plan, _) =
                            server.native_frame_renderable_surfaces_with_composition_plan();
                        let targets =
                            server.native_frame_presentation_targets(canonical_surfaces.as_ref());
                        let presentation =
                            server.presentation_scene_sample_for_targets_at(at, &targets);
                        let presentation_effects = server.resolved_effect_scene_for_presentation(
                            &presentation,
                            &fullscreen_plan,
                        );
                        let lifecycle = server.lifecycle_scene_sample_at(at);
                        let lifecycle_surfaces = server.lifecycle_renderable_surfaces(&lifecycle);
                        let lifecycle_surface_ids = lifecycle_surfaces
                            .iter()
                            .map(|surface| surface.surface_id)
                            .collect::<Vec<_>>();
                        let (canonical_surface_count, retained_surface_count) = lifecycle
                            .lamps
                            .first()
                            .and_then(|lamp| server.state.window(lamp.window_id))
                            .map(|window| {
                                (
                                    server
                                        .state
                                        .renderable_surfaces
                                        .iter()
                                        .filter(|surface| {
                                            server
                                                .state
                                                .root_surface_id_for_surface(surface.surface_id)
                                                == window.root_surface_id
                                        })
                                        .count(),
                                    window.state.minimized_surfaces().len(),
                                )
                            })
                            .unwrap_or_default();
                        let _ = reply.send(LifecycleEffectPathSnapshot {
                            raw_lamp_surface_ids: lifecycle_surface_ids.clone(),
                            lifecycle_surface_ids,
                            presentation_effect_instance_count: presentation_effects
                                .instances
                                .len(),
                            lifecycle_resolved_effect_instance_count: lifecycle
                                .lamps
                                .first()
                                .and_then(|lamp| lifecycle.visual_source_for_window(lamp.window_id))
                                .map_or(0, |source| source.effect_scene.instances.len()),
                            canonical_surface_count,
                            retained_surface_count,
                            lifecycle_transition_count: server
                                .state
                                .window_lifecycle_animator
                                .active_count(),
                            restore_suppression_active: !server
                                .state
                                .lifecycle_render_suppressed_roots()
                                .is_empty(),
                        });
                    }
                    ServerCommand::ReplaceBlurPolicyConfig { config, reply } => {
                        let changed = server.state.blur_assignment.replace_config(config).is_ok();
                        if changed {
                            server.state.advance_render_generation_with_scene_effect(
                                RenderGenerationCause::EffectBinding,
                                true,
                            );
                            server.state.refresh_effect_scene_summary();
                        }
                        let _ = reply.send(changed);
                    }
                    ServerCommand::CaptureFocusGeneration(reply) => {
                        let _ = reply.send(server.state.focus_generation);
                    }
                    ServerCommand::CapturePointerInputMetrics(reply) => {
                        let _ = reply.send(server.state.pointer_hit_metrics);
                    }
                    ServerCommand::CaptureRenderGenerationCause(reply) => {
                        let _ = reply.send(server.render_generation_cause());
                    }
                    ServerCommand::CaptureRenderableSurfaceCount(reply) => {
                        let _ = reply.send(server.renderable_surfaces().len());
                    }
                    ServerCommand::CaptureNativeDecorationCount(reply) => {
                        let count = server
                            .native_decoration_render_instances(server.renderable_surfaces())
                            .len();
                        let _ = reply.send(count);
                    }
                    ServerCommand::CaptureNativeFrameSurfaceIds(reply) => {
                        let _ = reply.send(
                            server
                                .native_frame_renderable_surfaces()
                                .iter()
                                .map(|surface| surface.surface_id)
                                .collect(),
                        );
                    }
                    ServerCommand::CaptureSurfaceResourceCount(reply) => {
                        let _ = reply.send(server.state.surface_resources.len());
                    }
                    ServerCommand::CaptureShmResourceCounts(reply) => {
                        let _ = reply.send((
                            server.state.current_surface_buffers.len(),
                            0,
                            server.state.frame_batches.len()
                                + server.state.retired_frame_batches.len(),
                        ));
                    }
                    ServerCommand::CaptureRenderableSurfaceSnapshot(reply) => {
                        let surfaces = server.renderable_surfaces();
                        let origins = render::surface_origins(surfaces);
                        let _ = reply.send(
                            surfaces
                                .iter()
                                .zip(origins)
                                .map(
                                    |(surface, (origin_x, origin_y))| RenderableSurfaceSnapshot {
                                        surface_id: surface.surface_id,
                                        width: surface.width,
                                        height: surface.height,
                                        parent_surface_id: surface.placement.parent_surface_id,
                                        local_x: surface.placement.local_x,
                                        local_y: surface.placement.local_y,
                                        content_x: surface.x,
                                        content_y: surface.y,
                                        origin_x,
                                        origin_y,
                                        buffer_id: surface.buffer_id().get(),
                                        pixel_checksum: surface.cpu_pixels().map(|pixels| {
                                            pixels.iter().fold(0_u64, |checksum, pixel| {
                                                checksum.rotate_left(5) ^ u64::from(*pixel)
                                            })
                                        }),
                                        buffer_scale: surface.buffer_scale,
                                        buffer_transform: surface.buffer_transform,
                                        viewport_source: surface.viewport_source.map(|source| {
                                            (
                                                (source.x * 256.0).round() as i64,
                                                (source.y * 256.0).round() as i64,
                                                (source.width * 256.0).round() as i64,
                                                (source.height * 256.0).round() as i64,
                                            )
                                        }),
                                        viewport_destination: surface
                                            .viewport_destination
                                            .map(|size| (size.width, size.height)),
                                        generation: surface.generation,
                                        resize_preview_active: surface.visual_clip.is_some(),
                                    },
                                )
                                .collect(),
                        );
                    }
                    ServerCommand::CaptureSurfaceBufferOwnership { surface_id, reply } => {
                        let tracked_surface_id = server
                            .state
                            .surface_resources
                            .iter()
                            .find(|(_, surface)| surface.id().protocol_id() == surface_id)
                            .map_or(surface_id, |(tracked_id, _)| *tracked_id);
                        let _ = reply.send(SurfaceBufferOwnershipSnapshot {
                            current_surface_buffer: server
                                .state
                                .current_surface_buffers
                                .contains_key(&tracked_surface_id),
                            active_dmabuf: server
                                .state
                                .active_dmabuf_buffers
                                .contains_key(&tracked_surface_id),
                            pending_dmabuf_releases: server
                                .state
                                .pending_dmabuf_buffer_releases
                                .len(),
                        });
                    }
                    ServerCommand::CaptureSubsurfaceStackState { parent_id, reply } => {
                        let _ = reply.send(SubsurfaceStackStateSnapshot {
                            committed: server
                                .state
                                .committed_subsurface_stacks
                                .get(&parent_id)
                                .cloned(),
                            latched: server
                                .state
                                .latched_subsurface_stacks
                                .get(&parent_id)
                                .cloned(),
                            pending: server
                                .state
                                .pending_subsurface_stacks
                                .get(&parent_id)
                                .cloned(),
                        });
                    }
                    ServerCommand::CaptureLayerSurfaceCommitState { surface_id, reply } => {
                        let state = server
                            .state
                            .layer_surfaces
                            .values()
                            .find(|role| role.surface.id().protocol_id() == surface_id)
                            .map(|role| {
                                (
                                    role.committed.exclusive_zone,
                                    match role.committed.keyboard_interactivity {
                                        KeyboardInteractivity::None => 0,
                                        KeyboardInteractivity::Exclusive => 1,
                                        KeyboardInteractivity::OnDemand => 2,
                                    },
                                )
                            });
                        let _ = reply.send(state);
                    }
                    ServerCommand::CaptureLayerSurfaceLifecycleState { surface_id, reply } => {
                        let state = server
                            .state
                            .layer_surfaces
                            .values()
                            .find(|role| role.surface.id().protocol_id() == surface_id)
                            .map(|role| LayerSurfaceLifecycleSnapshot {
                                mapped: role.mapped,
                                order: role.order,
                                layer_rank: role.committed.layer.scene_rank(),
                                geometry: role.geometry.map(|geometry| {
                                    (geometry.x, geometry.y, geometry.width, geometry.height)
                                }),
                            });
                        let _ = reply.send(state);
                    }
                    ServerCommand::CaptureLayerSurfacePendingAck { surface_id, reply } => {
                        let pending_ack = server
                            .state
                            .layer_surfaces
                            .values()
                            .find(|role| role.surface.id().protocol_id() == surface_id)
                            .and_then(|role| role.pending_ack_for_next_surface_commit)
                            .map(|configure| configure.serial);
                        let _ = reply.send(pending_ack);
                    }
                    ServerCommand::CaptureCommittedWindowGeometry(reply) => {
                        let geometry =
                            if server.state.toplevel_surfaces.len() == 1 {
                                server.state.toplevel_surfaces.keys().next().and_then(
                                    |surface_id| {
                                        server
                                            .state
                                            .surface_window_geometries
                                            .get(surface_id)
                                            .copied()
                                    },
                                )
                            } else {
                                None
                            };
                        let _ = reply.send(geometry);
                    }
                    ServerCommand::CaptureToplevelVisualGeometry(reply) => {
                        let visual =
                            if server.state.toplevel_surfaces.len() == 1 {
                                server.state.toplevel_surfaces.keys().next().and_then(
                                    |surface_id| {
                                        server.state.toplevel_visual_geometries.get(surface_id).map(
                                            |visual| ToplevelVisualGeometrySnapshot {
                                                local_x: visual.placement.local_x,
                                                local_y: visual.placement.local_y,
                                                width: visual.width,
                                                height: visual.height,
                                                active_resize: visual.active_resize.is_some(),
                                            },
                                        )
                                    },
                                )
                            } else {
                                None
                            };
                        let _ = reply.send(visual);
                    }
                    ServerCommand::CaptureRootWindowGeometry {
                        root_surface_id,
                        reply,
                    } => {
                        let geometry = server
                            .state
                            .current_visual_root_window_geometry(root_surface_id)
                            .or_else(|| server.state.current_root_window_geometry(root_surface_id));
                        let _ = reply.send(geometry);
                    }
                    ServerCommand::CaptureMinimizeAnchor { window_id, reply } => {
                        let _ = reply.send(
                            server
                                .state
                                .astrea_toplevel_publisher
                                .minimize_anchor_for_test(window_id),
                        );
                    }
                    ServerCommand::CapturePresentationTransitionCurve(reply) => {
                        let curve =
                            if server.state.toplevel_surfaces.len() == 1 {
                                server.state.toplevel_surfaces.keys().next().and_then(
                                    |surface_id| {
                                        let scene_node_id = server
                                            .state
                                            .presentation_scene_node_id_for_root(*surface_id)?;
                                        server
                                            .state
                                            .presentation_animator
                                            .track_curve(scene_node_id)
                                    },
                                )
                            } else {
                                None
                            };
                        let _ = reply.send(curve);
                    }
                    ServerCommand::CapturePresentationTransitionCurveForRoot {
                        root_surface_id,
                        reply,
                    } => {
                        let curve = server
                            .state
                            .presentation_scene_node_id_for_root(root_surface_id)
                            .and_then(|scene_node_id| {
                                server
                                    .state
                                    .presentation_animator
                                    .track_curve(scene_node_id)
                            });
                        let _ = reply.send(curve);
                    }
                    ServerCommand::CapturePresentationTransitionStart {
                        root_surface_id,
                        reply,
                    } => {
                        let sample = server
                            .state
                            .presentation_scene_node_id_for_root(root_surface_id)
                            .and_then(|scene_node_id| {
                                server
                                    .state
                                    .presentation_animator
                                    .sample_at_transition_start_for_scene_node(scene_node_id)
                            });
                        let _ = reply.send(sample);
                    }
                    ServerCommand::CaptureFocusedPresentationAfter {
                        elapsed_nanos,
                        reply,
                    } => {
                        let sample =
                            server
                                .state
                                .focused_root_surface_id()
                                .and_then(|root_surface_id| {
                                    let scene_node_id = server
                                        .state
                                        .presentation_scene_node_id_for_root(root_surface_id)?;
                                    server
                                        .state
                                        .presentation_animator
                                        .transition_started_at_for_scene_node(scene_node_id)
                                        .and_then(|started_at| {
                                            server
                                                .state
                                                .presentation_animator
                                                .sample_for_scene_node(
                                                    scene_node_id,
                                                    AnimationTime::from_nanos(
                                                        started_at
                                                            .as_nanos()
                                                            .saturating_add(elapsed_nanos),
                                                    ),
                                                )
                                        })
                                });
                        let _ = reply.send(sample);
                    }
                    ServerCommand::CapturePresentedPresentation {
                        root_surface_id,
                        reply,
                    } => {
                        let _ = reply.send((
                            server.state.presented_presentation_frame_id(),
                            server.state.presented_window_geometry(root_surface_id),
                            server
                                .state
                                .presented_presentation_transform(root_surface_id),
                        ));
                    }
                    ServerCommand::DropFocusedToplevelVisualGeometry => {
                        if let Some(surface_id) = server.state.focused_root_surface_id() {
                            server.state.toplevel_visual_geometries.remove(&surface_id);
                        }
                    }
                    ServerCommand::CancelFocusedPresentationTransition => {
                        if let Some(surface_id) = server.state.focused_root_surface_id() {
                            server
                                .state
                                .cancel_presentation_geometry_for_root(surface_id);
                        }
                    }
                    ServerCommand::CaptureFullscreenPresentationEligibility(reply) => {
                        let _ = reply.send(server.state.fullscreen_presentation_eligibility());
                    }
                    ServerCommand::CaptureFullscreenRenderPlanMetrics(reply) => {
                        let _ = reply.send(server.fullscreen_render_plan_metrics());
                    }
                    ServerCommand::CaptureConfigureSerial(reply) => {
                        let _ = reply.send(server.state.next_configure_serial);
                    }
                    ServerCommand::CaptureSurfacePresentationMetadata { surface_id, reply } => {
                        let metadata = server
                            .state
                            .surface_resources
                            .values()
                            .find(|surface| surface.id().protocol_id() == surface_id)
                            .and_then(|surface| {
                                surface.data::<SurfaceData>().map(|data| {
                                    (data.current_presentation(), data.pending_presentation())
                                })
                            });
                        let _ = reply.send(metadata);
                    }
                    ServerCommand::CaptureSurfacePresentationLineage { surface_id, reply } => {
                        let lineage = server
                            .state
                            .surface_presentation_generations
                            .get(&surface_id)
                            .copied()
                            .and_then(|generation| {
                                server
                                    .state
                                    .surface_publications
                                    .get(&surface_id)
                                    .and_then(|publication| publication.latest_published)
                                    .map(|commit_sequence| (generation, commit_sequence))
                            });
                        let _ = reply.send(lineage);
                    }
                    ServerCommand::CaptureDirectScanoutCandidate(reply) => {
                        let candidate = server.direct_scanout_scene_candidate().map(|candidate| {
                            DirectScanoutCandidateSnapshot {
                                surface_id: candidate.surface_id,
                                root_surface_id: candidate.root_surface_id,
                                generation: candidate.generation,
                                commit_sequence: candidate.commit_sequence,
                                buffer_size: candidate.buffer_size,
                                output_size: candidate.output_size,
                                format: candidate.buffer.format(),
                                viewport_identity_metadata_present: candidate
                                    .viewport_identity_metadata_present,
                            }
                        });
                        let _ = reply.send(candidate);
                    }
                    ServerCommand::CaptureClientCursorSnapshot(reply) => {
                        let snapshot = server.client_cursor_render_state().map(|cursor| {
                            let surface_id = cursor.surface.surface_id;
                            let commit_sequence = cursor.surface.commit_sequence;
                            ClientCursorSnapshot {
                                surface_id,
                                buffer_id: cursor.surface.buffer_id().get(),
                                commit_sequence: commit_sequence.get(),
                                first_pixel: cursor
                                    .surface
                                    .cpu_pixels()
                                    .and_then(|pixels| pixels.first().copied()),
                                journal_contains_commit_sequence: server
                                    .state
                                    .surface_damage_journals
                                    .get(&surface_id)
                                    .is_some_and(|journal| {
                                        journal
                                            .commit_counter_for_sequence(commit_sequence)
                                            .is_some()
                                    }),
                                logical_x: cursor.logical_x,
                                logical_y: cursor.logical_y,
                                width: cursor.surface.width,
                                height: cursor.surface.height,
                                buffer_scale: cursor.surface.buffer_scale,
                                buffer_transform: cursor.surface.buffer_transform,
                            }
                        });
                        let _ = reply.send(snapshot);
                    }
                    ServerCommand::CaptureInteractionCursorState(reply) => {
                        let (pointer_x, pointer_y) = server.last_pointer_position();
                        let _ = reply.send(InteractionCursorStateSnapshot {
                            override_active: server.interaction_cursor_override_active(),
                            visible: server.cursor_visibility_requested(),
                            pointer_x,
                            pointer_y,
                        });
                    }
                    ServerCommand::CaptureCursorHiddenByPointerLock(reply) => {
                        let _ = reply.send(server.cursor_hidden_by_pointer_lock());
                    }
                    ServerCommand::CaptureClipboardState(reply) => {
                        let _ = reply.send(ClipboardStateSnapshot {
                            active_source: server
                                .state
                                .selection_state
                                .active_selection(SelectionKind::Clipboard)
                                .is_some(),
                            source_count: server.state.data_sources.len(),
                            offer_count: server.state.data_offers.len(),
                        });
                    }
                    ServerCommand::CaptureXdgRoleSnapshot { surface_id, reply } => {
                        let tracked_surface_id = if let Some((tracked_id, _)) = server
                            .state
                            .surface_resources
                            .iter()
                            .find(|(_, surface)| surface.id().protocol_id() == surface_id)
                        {
                            *tracked_id
                        } else if server.state.popup_surfaces.len() == 1 {
                            *server.state.popup_surfaces.keys().next().unwrap()
                        } else if server.state.toplevel_surfaces.len() == 1 {
                            *server.state.toplevel_surfaces.keys().next().unwrap()
                        } else if server.state.surface_resources.len() == 1 {
                            *server.state.surface_resources.keys().next().unwrap()
                        } else {
                            surface_id
                        };
                        let toplevel = server.state.toplevel_surfaces.get(&tracked_surface_id);
                        let desktop_window =
                            toplevel.and_then(|toplevel| server.state.window(toplevel.window_id));
                        let _ = reply.send(XdgRoleSnapshot {
                            surface_id: tracked_surface_id,
                            surface_registered: server
                                .state
                                .surface_resources
                                .contains_key(&tracked_surface_id),
                            configured: server.state.xdg_surface_is_configured(tracked_surface_id),
                            toplevel_count: server.state.toplevel_surfaces.len(),
                            toplevel_registered: server
                                .state
                                .toplevel_surfaces
                                .contains_key(&tracked_surface_id),
                            popup_count: server.state.popup_surfaces.len(),
                            popup_node_count: server.state.popup_nodes.len(),
                            popup_grab_active: server.state.popup_grab.is_some(),
                            window_geometry_present: server
                                .state
                                .surface_window_geometries
                                .contains_key(&tracked_surface_id),
                            placement: server
                                .state
                                .surface_placements
                                .get(&tracked_surface_id)
                                .copied(),
                            permanent_role: server.state.permanent_surface_role(tracked_surface_id),
                            xdg_association: server
                                .state
                                .xdg_association_exists(tracked_surface_id),
                            toplevel_has_app_id: desktop_window
                                .is_some_and(|window| window.metadata.app_id.is_some()),
                            toplevel_has_title: desktop_window
                                .is_some_and(|window| window.metadata.title.is_some()),
                            toplevel_has_non_default_constraints: desktop_window.is_some_and(
                                |window| {
                                    window.constraints != WindowConstraints::default()
                                        || toplevel.is_some_and(|toplevel| {
                                            toplevel.pending_constraints.is_some()
                                        })
                                },
                            ),
                            toplevel_mode: desktop_window.map(|window| window.state.mode()),
                            popup_parent_surface_id: server
                                .state
                                .popup_surfaces
                                .get(&tracked_surface_id)
                                .and_then(|popup| popup.parent_surface_id),
                            pending_explicit_sync_commits: server
                                .state
                                .pending_explicit_sync_commits
                                .iter()
                                .filter(|commit| commit.surface_id == tracked_surface_id)
                                .count(),
                            pending_surface_tree_transactions: server
                                .state
                                .pending_surface_tree_transactions
                                .iter()
                                .filter(|transaction| {
                                    transaction
                                        .nodes
                                        .iter()
                                        .any(|(surface_id, _)| *surface_id == tracked_surface_id)
                                })
                                .count(),
                            current_surface_buffer: server
                                .state
                                .current_surface_buffers
                                .contains_key(&tracked_surface_id),
                            renderable_surface: server
                                .state
                                .renderable_surfaces
                                .iter()
                                .any(|surface| surface.surface_id == tracked_surface_id),
                            role_destroyed_pending_commits_retired: server
                                .state
                                .compliance_metrics
                                .xdg_role_destroyed_pending_commits_retired,
                            role_destroyed_pending_trees_retired: server
                                .state
                                .compliance_metrics
                                .xdg_role_destroyed_pending_trees_retired,
                            role_destroyed_acquire_watches_cancelled: server
                                .state
                                .compliance_metrics
                                .xdg_role_destroyed_acquire_watches_cancelled,
                            reassociation_blocked_stale_work: server
                                .state
                                .compliance_metrics
                                .xdg_reassociation_blocked_stale_unpublished_work,
                            subsurface_relationship_phase: server
                                .state
                                .subsurface_transactions
                                .relationship_phase(tracked_surface_id),
                            subsurface_parent_is_mapped: server
                                .state
                                .subsurface_parent_is_mapped(tracked_surface_id),
                            subsurface_can_map: server.state.subsurface_can_map(tracked_surface_id),
                            subsurface_content_is_inactive: server
                                .state
                                .subsurface_content_is_inactive(tracked_surface_id),
                        });
                    }
                    ServerCommand::CapturePendingSurfaceTreeTransactions(reply) => {
                        let transactions = server
                            .state
                            .pending_surface_tree_transactions
                            .iter()
                            .map(|transaction| {
                                (
                                    transaction.id.get(),
                                    transaction
                                        .nodes
                                        .iter()
                                        .map(|(surface_id, commit)| {
                                            (*surface_id, commit.commit_id.get())
                                        })
                                        .collect(),
                                )
                            })
                            .collect();
                        let _ = reply.send(transactions);
                    }
                    ServerCommand::CapturePendingFrameCallbacks(reply) => {
                        let _ = reply.send(server.has_pending_frame_callbacks());
                    }
                    ServerCommand::CaptureOnlyPendingSurfaceFrameCallbacks(reply) => {
                        let _ = reply.send(server.has_only_pending_surface_frame_callbacks());
                    }
                    ServerCommand::CapturePendingFrameWork(reply) => {
                        let _ = reply.send(server.has_unowned_frame_work());
                    }
                    ServerCommand::CaptureFrameEligiblePresentationFeedbackWork(reply) => {
                        let _ = reply.send(
                            server
                                .state
                                .has_frame_eligible_pending_presentation_feedbacks(),
                        );
                    }
                    ServerCommand::CaptureFrameCallbackMetrics(reply) => {
                        let _ = reply.send(server.frame_callback_metrics());
                    }
                    ServerCommand::CompleteProtocolOnlyFrameTick(reply) => {
                        let output_time = server.frame_callback_time_for_output();
                        let _ = reply.send(server.complete_protocol_only_frame_tick(output_time));
                    }
                    ServerCommand::CaptureIdleInhibited(reply) => {
                        let _ = reply.send(server.state.idle_inhibited());
                    }
                    ServerCommand::CapturePointerConstraintBackendRequests(reply) => {
                        let _ = reply.send(server.take_pointer_constraint_backend_requests());
                    }
                    ServerCommand::SettlePointerConstraintBackendRequests => {
                        let (x, y) = server.last_pointer_position();
                        let current_position = OutputPosition { x, y };
                        for request in server.take_pointer_constraint_backend_requests() {
                            let Some(resolved) = server.resolve_pointer_constraint_backend_request(
                                request,
                                current_position,
                            ) else {
                                continue;
                            };
                            if let PointerConstraintBackendRequest::ActivateLocked { id } =
                                resolved.request
                                && let Some(anchor) = resolved.locked_anchor
                            {
                                server.pointer_constraint_backend_activated_at(id, anchor);
                            }
                        }
                    }
                    ServerCommand::CapturePendingLockedPointerReveal(reply) => {
                        let _ = reply.send(server.state.pending_locked_pointer_reveal.is_some());
                    }
                    ServerCommand::CapturePendingLockedPointerRevealAndBackendRequests(reply) => {
                        let pending = server.state.pending_locked_pointer_reveal.is_some();
                        let requests = server.take_pointer_constraint_backend_requests();
                        let _ = reply.send((pending, requests));
                    }
                    ServerCommand::CapturePointerConstraintIds(reply) => {
                        let ids = server.state.pointer_constraints.keys().copied().collect();
                        let _ = reply.send(ids);
                    }
                    ServerCommand::CaptureTerminalClientCount(reply) => {
                        let _ = reply.send(server.state.terminal_client_ids.len());
                    }
                    ServerCommand::CapturePointerConstraintSnapshot {
                        constraint_id,
                        reply,
                    } => {
                        let snapshot = server.state.pointer_constraints.get(&constraint_id).map(
                            |constraint| PointerConstraintSurfaceSnapshot {
                                committed: constraint.committed,
                                active: constraint.active,
                                protocol_resource_alive: constraint.protocol_resource_alive,
                                backend_pending: constraint.backend_pending,
                                surface_constraint_pending: constraint.surface_constraint_pending,
                                lifecycle_removal_pending: constraint.lifecycle_removal_pending,
                                defunct: constraint.defunct,
                                committed_region: constraint.committed_region.clone(),
                                committed_cursor_position_hint: constraint
                                    .committed_cursor_position_hint,
                            },
                        );
                        let _ = reply.send(snapshot);
                    }
                    ServerCommand::CaptureLastPointerPosition(reply) => {
                        let _ =
                            reply.send((server.state.last_pointer_x, server.state.last_pointer_y));
                    }
                    ServerCommand::CapturePointerFocusSurfaceId(reply) => {
                        let _ = reply.send(
                            server
                                .state
                                .pointer_surface
                                .as_ref()
                                .map(compositor_surface_id),
                        );
                    }
                    ServerCommand::CaptureActiveLockedPointerAnchor(reply) => {
                        let anchor =
                            server
                                .state
                                .active_locked_pointer_routing
                                .as_ref()
                                .map(|routing| {
                                    (routing.activation_anchor.x, routing.activation_anchor.y)
                                });
                        let _ = reply.send(anchor);
                    }
                    ServerCommand::CaptureFocusedSurfaceId(reply) => {
                        let _ = reply.send(
                            server
                                .state
                                .focused_surface
                                .as_ref()
                                .map(compositor_surface_id),
                        );
                    }
                    ServerCommand::CaptureKeyboardFocusSurfaceId(reply) => {
                        let _ = reply.send(
                            server
                                .state
                                .keyboard_surface
                                .as_ref()
                                .map(compositor_surface_id),
                        );
                    }
                    ServerCommand::CaptureFocusedWindowId(reply) => {
                        let _ = reply.send(server.state.focused_window_id);
                    }
                    ServerCommand::CaptureFocusedToplevelMode(reply) => {
                        let mode = server
                            .state
                            .focused_root_surface_id()
                            .and_then(|surface_id| server.state.window_id_for_surface(surface_id))
                            .and_then(|window_id| server.state.window(window_id))
                            .map(|window| window.state.mode());
                        let _ = reply.send(mode);
                    }
                    ServerCommand::CaptureWindowIdForSurface { surface_id, reply } => {
                        let _ = reply.send(server.state.window_id_for_surface(surface_id));
                    }
                    ServerCommand::CaptureWindowManagement { window_id, reply } => {
                        let management = server.state.window(window_id).and_then(|window| {
                            let management = window.management?;
                            let location = management.location();
                            let in_tree = server
                                .state
                                .tiled_layout
                                .tree(location)
                                .is_some_and(|tree| tree.contains_window(window_id));
                            Some((management.layout(), location, in_tree))
                        });
                        let _ = reply.send(management);
                    }
                    ServerCommand::CaptureWindowInteractionDebugSnapshot(reply) => {
                        let _ = reply.send(server.window_interaction_debug_snapshot());
                    }
                    ServerCommand::CapturePointerOwnershipIsClear(reply) => {
                        let _ = reply.send(server.pointer_ownership_is_clear());
                    }
                    ServerCommand::CaptureWindowInteractionReleaseMetrics(reply) => {
                        let _ = reply.send(server.window_interaction_release_metrics());
                    }
                    ServerCommand::CaptureUsableOutputGeometry(reply) => {
                        let _ = reply.send(server.state.usable_output_geometry());
                    }
                    ServerCommand::ApplyXwaylandWindowEvent { event, reply } => {
                        let _ = reply.send(server.apply_xwayland_window_event(*event));
                    }
                    ServerCommand::ApplyXwaylandAssociationEvent(event) => {
                        server.apply_xwayland_association_event(event);
                    }
                    ServerCommand::CaptureXwaylandAssociationEvents(reply) => {
                        let _ = reply.send(server.take_xwayland_association_events());
                    }
                    ServerCommand::CaptureXwaylandBackendCommands(reply) => {
                        let _ = reply.send(server.take_xwayland_backend_commands(0));
                    }
                    ServerCommand::CaptureX11WindowState { window_id, reply } => {
                        let state = server
                            .state
                            .window(window_id)
                            .map(|window| (window.x11_surface_id, window.state.is_minimized()));
                        let _ = reply.send(state);
                    }
                    ServerCommand::AuthorizeAstreaShellPid(pid) => {
                        server.authorize_astrea_shell_pid(pid);
                    }
                    ServerCommand::ClearAstreaShellAuthorization => {
                        server.clear_astrea_shell_authorization();
                    }
                    ServerCommand::EmitAstreaShortcut {
                        namespace,
                        name,
                        phase,
                        timestamp,
                        reply,
                    } => {
                        let _ = reply
                            .send(server.emit_astrea_shortcut(&namespace, &name, phase, timestamp));
                    }
                    ServerCommand::UpdatePointerPositionWithoutClientDispatch { x, y, reply } => {
                        let _ = reply
                            .send(server.update_pointer_position_without_client_dispatch(x, y));
                    }
                    ServerCommand::BeginNativeInputBatch => {
                        server.begin_native_input_batch();
                    }
                    ServerCommand::ProgressWaylandDuringNativeInputBatch => {
                        let _ = server.tick();
                    }
                    ServerCommand::EndNativeInputBatch => {
                        let _ = server.end_native_input_batch();
                    }
                    ServerCommand::PointerConstraintBackendActivated(id) => {
                        server.pointer_constraint_backend_activated(id);
                    }
                    ServerCommand::PointerConstraintBackendFailed(id) => {
                        server.pointer_constraint_backend_failed(id, "test failure");
                    }
                    ServerCommand::PointerConstraintBackendDeactivated(id) => {
                        server.pointer_constraint_backend_deactivated(id);
                    }
                    ServerCommand::ClearPointerEnterTracking => {
                        server.state.pointer_entered_surfaces.clear();
                    }
                    ServerCommand::Barrier(reply) => barriers.push(reply),
                    ServerCommand::PrepareFrame => {
                        server.prepare_frame();
                    }
                    ServerCommand::AdmitInteractiveVisualState { render_ahead } => {
                        let _ = server.flush_pending_interactive_visual_state_for_render_admission(
                            render_ahead,
                        );
                    }
                    ServerCommand::CaptureLegacyPreparedFrame => {
                        server.capture_frame_callbacks_for_render();
                    }
                    ServerCommand::MarkPreparedFrameCallbacksRendered => {
                        server.mark_frame_callbacks_rendered_for_prepared();
                    }
                    ServerCommand::CaptureAndFinishLegacyPreparedFrame => {
                        server.capture_frame_callbacks_for_render();
                        server.finish_prepared_frame();
                    }
                    ServerCommand::CaptureAndCompleteRenderedLegacyPreparedFrame => {
                        server.capture_frame_callbacks_for_render();
                        server.finish_prepared_frame();
                    }
                    ServerCommand::CaptureLegacySubmittedAndPreparedFrames => {
                        server.capture_frame_callbacks_for_render();
                        server.mark_prepared_frame_submitted();
                        server.capture_frame_callbacks_for_render();
                    }
                    ServerCommand::CapturePreparedFrame(reply) => {
                        let _ = reply.send(server.has_prepared_frame_batch());
                    }
                    ServerCommand::PublishTestPresentationAt { frame_id, at } => {
                        server.publish_test_presentation_at(frame_id, at);
                    }
                    ServerCommand::PublishFocusedPresentationAfter {
                        frame_id,
                        elapsed_nanos,
                    } => {
                        if let Some(root_surface_id) = server.state.focused_root_surface_id()
                            && let Some(scene_node_id) = server
                                .state
                                .presentation_scene_node_id_for_root(root_surface_id)
                            && let Some(started_at) = server
                                .state
                                .presentation_animator
                                .transition_started_at_for_scene_node(scene_node_id)
                        {
                            server.publish_test_presentation_at(
                                frame_id,
                                AnimationTime::from_nanos(
                                    started_at.as_nanos().saturating_add(elapsed_nanos),
                                ),
                            );
                        }
                    }
                    ServerCommand::SettleNoVisualChangeWork {
                        owns_frame_batch,
                        reply,
                    } => {
                        let _ =
                            reply.send(server.settle_no_visual_change_work(None, owns_frame_batch));
                    }
                    ServerCommand::FinishFrame => {
                        server.finish_frame();
                    }
                    ServerCommand::FinishPreparedFrame => {
                        server.finish_prepared_frame();
                    }
                    ServerCommand::FinishFrameWithPresentation(presentation) => {
                        server.finish_frame_with_presentation(presentation);
                    }
                    ServerCommand::CaptureFrameBatch { frame_id, reply } => {
                        let _ = reply.send(server.take_frame_batch_for_render(frame_id));
                    }
                    ServerCommand::CaptureNativeFrameBatch { frame_id, reply } => {
                        let _ =
                            reply.send(server.test_take_native_frame_batch_for_render(frame_id));
                    }
                    ServerCommand::CaptureFrameBatchSurfaceIds { batch_id, reply } => {
                        let _ =
                            reply.send(server.test_frame_batch_presentation_surface_ids(batch_id));
                    }
                    ServerCommand::CapturePresentationFeedbackBatch {
                        surface_id,
                        commit_sequence,
                        reply,
                    } => {
                        let batch_id = server
                            .presentation_commit_key_for_surface_commit(surface_id, commit_sequence)
                            .and_then(|key| {
                                server.take_presentation_feedback_batch_for_samples([key])
                            });
                        let _ = reply.send(batch_id);
                    }
                    ServerCommand::DiscardAllPresentationFeedbacks => {
                        server.state.discard_all_pending_presentation_feedbacks();
                    }
                    ServerCommand::CompletePresentationFeedbackBatch {
                        batch_id,
                        presentation,
                    } => {
                        server.complete_presentation_feedback_batch(batch_id, presentation);
                    }
                    ServerCommand::CaptureFrameCallbackPacingCompleted { batch_id, reply } => {
                        let _ =
                            reply.send(server.test_frame_callback_pacing_is_completed(batch_id));
                    }
                    ServerCommand::PrepareTerminalCallbackOwnership {
                        batch_id,
                        disposition,
                        reply,
                    } => {
                        let _ = reply.send(
                            server.prepare_terminal_callback_ownership(batch_id, disposition),
                        );
                    }
                    ServerCommand::CompleteFrameBatch {
                        frame_id,
                        batch_id,
                        presentation,
                    } => {
                        server.complete_presented_frame_batch(frame_id, batch_id, presentation);
                    }
                    ServerCommand::CompleteFrameBatchNow { frame_id, batch_id } => {
                        let presentation =
                            FramePresentation::software_now(server.state.presentation_clock)
                                .expect("test presentation clock should be usable");
                        server.complete_presented_frame_batch(frame_id, batch_id, presentation);
                    }
                    ServerCommand::MarkFrameCallbacksRendered(batch_id) => {
                        server.mark_frame_callbacks_rendered(batch_id);
                    }
                    ServerCommand::CompleteFrameCallbacksAfterAdmission {
                        batch_id,
                        admission,
                    } => {
                        server.complete_frame_callbacks_after_admission(batch_id, admission);
                    }
                    ServerCommand::NoteFrameCallbacksDeferredReady(batch_id) => {
                        server.note_frame_callbacks_deferred_ready(batch_id);
                    }
                    ServerCommand::NoteFrameCallbackAdmissionFailure(batch_id) => {
                        server.note_frame_callback_admission_failure(batch_id);
                    }
                    ServerCommand::RestoreFrameBatchAfterRenderFailure(batch_id) => {
                        server.restore_frame_batch_after_render_failure(batch_id);
                    }
                    ServerCommand::CompleteNoVisualChangeFrameBatch(batch_id) => {
                        server.complete_no_visual_change_frame_batch(batch_id);
                    }
                    ServerCommand::CompleteDirectFrameBatch {
                        frame_id,
                        batch_id,
                        direct_surface_id,
                        presentation,
                    } => {
                        server.complete_direct_presented_frame_batch(
                            frame_id,
                            batch_id,
                            direct_surface_id,
                            presentation,
                        );
                    }
                    ServerCommand::CompleteDirectFrameBatchWithLineage {
                        frame_id,
                        batch_id,
                        direct_surface_id,
                        direct_lineage,
                        presentation,
                    } => {
                        let prepared = server
                            .prepare_direct_presented_frame_batch_with_lineage(
                                frame_id,
                                batch_id,
                                direct_surface_id,
                                Some(direct_lineage),
                            )
                            .expect("test direct frame batch should be owned");
                        server.commit_prepared_direct_presented_frame_batch(prepared, presentation);
                    }
                    ServerCommand::DiscardFrameBatch { batch_id, reason } => {
                        server.discard_frame_batch(batch_id, reason);
                    }
                    ServerCommand::PresentFrame => {
                        server.present_frame();
                    }
                    ServerCommand::MarkRenderDamagePresented => {
                        server.mark_render_damage_presented();
                    }
                    ServerCommand::Stop => running = false,
                }
            }
            let _ = server.tick();
            for barrier in barriers {
                let _ = barrier.send(());
            }
            thread::sleep(Duration::from_millis(2));
        }
        server
    });

    (commands, server_thread)
}

pub(in crate::compositor::tests) fn stop_controllable_test_server(
    commands: Sender<ServerCommand>,
    server_thread: JoinHandle<OwnCompositorServer>,
) -> OwnCompositorServer {
    let _ = commands.send(ServerCommand::Stop);
    server_thread.join().unwrap()
}

pub(in crate::compositor::tests) fn wait_for_server_commands(commands: &Sender<ServerCommand>) {
    let (reply, receiver) = mpsc::channel();
    commands.send(ServerCommand::Barrier(reply)).unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should process command barrier");
}

pub(in crate::compositor::tests) fn capture_surface_presentation_metadata(
    commands: &Sender<ServerCommand>,
    surface_id: u32,
) -> Option<(SurfacePresentationMetadata, SurfacePresentationMetadata)> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureSurfacePresentationMetadata { surface_id, reply })
        .unwrap();
    wait_for_server_commands(commands);
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should return surface presentation metadata")
}

pub(in crate::compositor::tests) fn capture_surface_presentation_lineage(
    commands: &Sender<ServerCommand>,
    surface_id: u32,
) -> Option<(u64, SurfaceCommitSequence)> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureSurfacePresentationLineage { surface_id, reply })
        .unwrap();
    wait_for_server_commands(commands);
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should return surface presentation lineage")
}

pub(in crate::compositor::tests) fn capture_clipboard_state(
    commands: &Sender<ServerCommand>,
) -> ClipboardStateSnapshot {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureClipboardState(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report clipboard state")
}

pub(in crate::compositor::tests) fn capture_render_generation(
    commands: &Sender<ServerCommand>,
) -> u64 {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureRenderGeneration(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report render generation")
}

pub(in crate::compositor::tests) fn capture_scene_render_generation(
    commands: &Sender<ServerCommand>,
) -> u64 {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureSceneRenderGeneration(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report scene render generation")
}

pub(in crate::compositor::tests) fn capture_core_compliance_metrics(
    commands: &Sender<ServerCommand>,
) -> CoreComplianceMetrics {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureCoreComplianceMetrics(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report core compliance metrics")
}

pub(in crate::compositor::tests) fn capture_pointer_hit_generation(
    commands: &Sender<ServerCommand>,
) -> u64 {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePointerHitGeneration(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report pointer hit generation")
}

pub(in crate::compositor::tests) fn capture_pointer_scene_hit(
    commands: &Sender<ServerCommand>,
    x: f64,
    y: f64,
) -> (Option<u32>, Option<(f64, f64)>) {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePointerSceneHit { x, y, reply })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report pointer scene hit")
}

pub(in crate::compositor::tests) fn capture_last_pointer_position(
    commands: &Sender<ServerCommand>,
) -> (f64, f64) {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLastPointerPosition(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report last pointer position")
}

pub(in crate::compositor::tests) fn capture_focus_generation(
    commands: &Sender<ServerCommand>,
) -> u64 {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureFocusGeneration(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report focus generation")
}

pub(in crate::compositor::tests) fn set_pointer_hit_instrumentation_enabled(
    commands: &Sender<ServerCommand>,
    enabled: bool,
) {
    commands
        .send(ServerCommand::SetPointerHitInstrumentationEnabled(enabled))
        .unwrap();
    wait_for_server_commands(commands);
}

pub(in crate::compositor::tests) fn capture_pointer_input_metrics(
    commands: &Sender<ServerCommand>,
) -> PointerInputMetrics {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePointerInputMetrics(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report pointer input metrics")
}

pub(in crate::compositor::tests) fn capture_pointer_focus_surface_id(
    commands: &Sender<ServerCommand>,
) -> Option<u32> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePointerFocusSurfaceId(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report pointer focus surface")
}

pub(in crate::compositor::tests) fn capture_render_generation_cause(
    commands: &Sender<ServerCommand>,
) -> RenderGenerationCause {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureRenderGenerationCause(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report render generation cause")
}

pub(in crate::compositor::tests) fn capture_renderable_surface_count(
    commands: &Sender<ServerCommand>,
) -> usize {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureRenderableSurfaceCount(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report renderable surface count")
}

pub(in crate::compositor::tests) fn capture_native_decoration_count(
    commands: &Sender<ServerCommand>,
) -> usize {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureNativeDecorationCount(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report native decoration count")
}

pub(in crate::compositor::tests) fn capture_surface_resource_count(
    commands: &Sender<ServerCommand>,
) -> usize {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureSurfaceResourceCount(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report surface resource count")
}

pub(in crate::compositor::tests) fn capture_shm_resource_counts(
    commands: &Sender<ServerCommand>,
) -> (usize, usize, usize) {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureShmResourceCounts(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report SHM resource counts")
}

pub(in crate::compositor::tests) fn capture_renderable_surface_snapshot(
    commands: &Sender<ServerCommand>,
) -> Vec<RenderableSurfaceSnapshot> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureRenderableSurfaceSnapshot(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report renderable surface snapshot")
}

pub(in crate::compositor::tests) fn capture_surface_buffer_ownership(
    commands: &Sender<ServerCommand>,
    surface_id: u32,
) -> SurfaceBufferOwnershipSnapshot {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureSurfaceBufferOwnership { surface_id, reply })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report surface buffer ownership")
}

pub(in crate::compositor::tests) fn capture_layer_surface_commit_state(
    commands: &Sender<ServerCommand>,
    surface_id: u32,
) -> Option<(i32, u8)> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLayerSurfaceCommitState { surface_id, reply })
        .unwrap();
    wait_for_server_commands(commands);
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should return layer surface state")
}

pub(in crate::compositor::tests) fn capture_layer_surface_lifecycle_state(
    commands: &Sender<ServerCommand>,
    surface_id: u32,
) -> Option<LayerSurfaceLifecycleSnapshot> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLayerSurfaceLifecycleState { surface_id, reply })
        .unwrap();
    wait_for_server_commands(commands);
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should return layer surface lifecycle state")
}

pub(in crate::compositor::tests) fn capture_layer_surface_pending_ack(
    commands: &Sender<ServerCommand>,
    surface_id: u32,
) -> Option<u32> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLayerSurfacePendingAck { surface_id, reply })
        .unwrap();
    wait_for_server_commands(commands);
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should return layer surface pending acknowledgement")
}

pub(in crate::compositor::tests) fn capture_native_frame_surface_ids(
    commands: &Sender<ServerCommand>,
) -> Vec<u32> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureNativeFrameSurfaceIds(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report native frame surface ids")
}

pub(in crate::compositor::tests) fn capture_focused_surface_id(
    commands: &Sender<ServerCommand>,
) -> Option<u32> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureFocusedSurfaceId(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report keyboard focus surface")
}

pub(in crate::compositor::tests) fn capture_keyboard_focus_surface_id(
    commands: &Sender<ServerCommand>,
) -> Option<u32> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureKeyboardFocusSurfaceId(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report keyboard protocol focus surface")
}

pub(in crate::compositor::tests) fn capture_focused_window_id(
    commands: &Sender<ServerCommand>,
) -> Option<WindowId> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureFocusedWindowId(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report focused window")
}

pub(in crate::compositor::tests) fn capture_focused_toplevel_mode(
    commands: &Sender<ServerCommand>,
) -> Option<ToplevelMode> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureFocusedToplevelMode(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report focused toplevel mode")
}

pub(in crate::compositor::tests) fn capture_window_id_for_surface(
    commands: &Sender<ServerCommand>,
    surface_id: u32,
) -> Option<WindowId> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureWindowIdForSurface { surface_id, reply })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report window for surface")
}

pub(in crate::compositor::tests) fn apply_xwayland_association_event(
    commands: &Sender<ServerCommand>,
    event: crate::xwayland::xwm::XwmAssociationEvent,
) {
    commands
        .send(ServerCommand::ApplyXwaylandAssociationEvent(event))
        .unwrap();
    wait_for_server_commands(commands);
}

pub(in crate::compositor::tests) fn capture_x11_window_state(
    commands: &Sender<ServerCommand>,
    window_id: WindowId,
) -> Option<(Option<u32>, bool)> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureX11WindowState { window_id, reply })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report X11 window state")
}

pub(in crate::compositor::tests) fn capture_window_management(
    commands: &Sender<ServerCommand>,
    window_id: WindowId,
) -> Option<(LayoutMembership, WorkspaceLocation, bool)> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureWindowManagement { window_id, reply })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report window management")
}

pub(in crate::compositor::tests) fn capture_window_interaction_debug_snapshot(
    commands: &Sender<ServerCommand>,
) -> Option<WindowInteractionDebugSnapshot> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureWindowInteractionDebugSnapshot(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report window interaction snapshot")
}

pub(in crate::compositor::tests) fn capture_pointer_ownership_is_clear(
    commands: &Sender<ServerCommand>,
) -> bool {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePointerOwnershipIsClear(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report pointer ownership")
}

pub(in crate::compositor::tests) fn capture_window_interaction_release_metrics(
    commands: &Sender<ServerCommand>,
) -> WindowInteractionReleaseMetrics {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureWindowInteractionReleaseMetrics(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report window interaction release metrics")
}

pub(in crate::compositor::tests) fn capture_usable_output_geometry(
    commands: &Sender<ServerCommand>,
) -> OutputRect {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureUsableOutputGeometry(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report usable output geometry")
}

pub(in crate::compositor::tests) fn capture_committed_window_geometry(
    commands: &Sender<ServerCommand>,
) -> Option<XdgWindowGeometry> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureCommittedWindowGeometry(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report committed window geometry")
}

pub(in crate::compositor::tests) fn capture_toplevel_visual_geometry(
    commands: &Sender<ServerCommand>,
) -> Option<ToplevelVisualGeometrySnapshot> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureToplevelVisualGeometry(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report toplevel visual geometry")
}

pub(in crate::compositor::tests) fn capture_root_window_geometry(
    commands: &Sender<ServerCommand>,
    root_surface_id: u32,
) -> Option<WindowGeometry> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureRootWindowGeometry {
            root_surface_id,
            reply,
        })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report root window geometry")
}

pub(in crate::compositor::tests) fn capture_minimize_anchor(
    commands: &Sender<ServerCommand>,
    window_id: WindowId,
) -> Option<MinimizeAnchorRect> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureMinimizeAnchor { window_id, reply })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report minimize anchor")
}

pub(in crate::compositor::tests) fn capture_presentation_transition_curve(
    commands: &Sender<ServerCommand>,
) -> Option<AnimationCurve> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePresentationTransitionCurve(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report presentation transition curve")
}

pub(in crate::compositor::tests) fn capture_presentation_transition_curve_for_root(
    commands: &Sender<ServerCommand>,
    root_surface_id: u32,
) -> Option<AnimationCurve> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePresentationTransitionCurveForRoot {
            root_surface_id,
            reply,
        })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report root presentation transition curve")
}

pub(in crate::compositor::tests) fn capture_presented_presentation(
    commands: &Sender<ServerCommand>,
    root_surface_id: u32,
) -> (
    u64,
    Option<PresentedWindowGeometry>,
    Option<PresentationGroupTransform>,
) {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePresentedPresentation {
            root_surface_id,
            reply,
        })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report presented presentation")
}

pub(in crate::compositor::tests) fn capture_presentation_transition_start(
    commands: &Sender<ServerCommand>,
    root_surface_id: u32,
) -> Option<PresentationWindowSample> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePresentationTransitionStart {
            root_surface_id,
            reply,
        })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report presentation transition start")
}

pub(in crate::compositor::tests) fn capture_focused_presentation_after(
    commands: &Sender<ServerCommand>,
    elapsed_nanos: u64,
) -> Option<PresentationWindowSample> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureFocusedPresentationAfter {
            elapsed_nanos,
            reply,
        })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report presentation sample")
}

pub(in crate::compositor::tests) fn drop_focused_toplevel_visual_geometry(
    commands: &Sender<ServerCommand>,
) {
    commands
        .send(ServerCommand::DropFocusedToplevelVisualGeometry)
        .unwrap();
    wait_for_server_commands(commands);
}

pub(in crate::compositor::tests) fn capture_fullscreen_presentation_eligibility(
    commands: &Sender<ServerCommand>,
) -> FullscreenPresentationEligibility {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureFullscreenPresentationEligibility(
            reply,
        ))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report fullscreen presentation eligibility")
}

pub(in crate::compositor::tests) fn capture_fullscreen_render_plan_metrics(
    commands: &Sender<ServerCommand>,
) -> FullscreenRenderPlanMetrics {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureFullscreenRenderPlanMetrics(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report fullscreen render plan metrics")
}

pub(in crate::compositor::tests) fn capture_configure_serial(
    commands: &Sender<ServerCommand>,
) -> u32 {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureConfigureSerial(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report configure serial")
}

pub(in crate::compositor::tests) fn capture_direct_scanout_candidate(
    commands: &Sender<ServerCommand>,
) -> Result<DirectScanoutCandidateSnapshot, DirectScanoutSceneRejection> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureDirectScanoutCandidate(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report direct scanout candidate")
}

pub(in crate::compositor::tests) fn capture_resolved_effect_scene(
    commands: &Sender<ServerCommand>,
) -> ResolvedEffectScene {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureResolvedEffectScene(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report resolved effect scene")
}

pub(in crate::compositor::tests) fn capture_lifecycle_effect_path(
    commands: &Sender<ServerCommand>,
) -> LifecycleEffectPathSnapshot {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureLifecycleEffectPath(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report lifecycle effect path")
}

pub(in crate::compositor::tests) fn replace_blur_policy_config(
    commands: &Sender<ServerCommand>,
    config: crate::blur_policy::BlurPolicyConfig,
) {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::ReplaceBlurPolicyConfig { config, reply })
        .unwrap();
    assert!(
        receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("server should replace blur policy config")
    );
}

pub(in crate::compositor::tests) fn focus_root_window(
    commands: &Sender<ServerCommand>,
    surface_id: u32,
) {
    commands
        .send(ServerCommand::FocusRootWindow(surface_id))
        .unwrap();
    wait_for_server_commands(commands);
}

pub(in crate::compositor::tests) fn raise_root_window(
    commands: &Sender<ServerCommand>,
    surface_id: u32,
) {
    commands
        .send(ServerCommand::RaiseRootWindow(surface_id))
        .unwrap();
    wait_for_server_commands(commands);
}

pub(in crate::compositor::tests) fn set_focused_root_visual_geometry(
    commands: &Sender<ServerCommand>,
    placement: SurfacePlacement,
    width: u32,
    height: u32,
) {
    commands
        .send(ServerCommand::SetFocusedRootVisualGeometry {
            placement,
            width,
            height,
        })
        .unwrap();
    wait_for_server_commands(commands);
}

pub(in crate::compositor::tests) fn capture_client_cursor_snapshot(
    commands: &Sender<ServerCommand>,
) -> Option<ClientCursorSnapshot> {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureClientCursorSnapshot(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report client cursor snapshot")
}

pub(in crate::compositor::tests) fn capture_interaction_cursor_state(
    commands: &Sender<ServerCommand>,
) -> InteractionCursorStateSnapshot {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureInteractionCursorState(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report interaction cursor state")
}

pub(in crate::compositor::tests) fn capture_cursor_hidden_by_pointer_lock(
    commands: &Sender<ServerCommand>,
) -> bool {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureCursorHiddenByPointerLock(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report effective pointer-lock cursor hiding")
}

pub(in crate::compositor::tests) fn capture_xdg_role_snapshot(
    commands: &Sender<ServerCommand>,
    surface_id: u32,
) -> XdgRoleSnapshot {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureXdgRoleSnapshot { surface_id, reply })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report XDG role snapshot")
}

pub(in crate::compositor::tests) fn capture_pending_surface_tree_transactions(
    commands: &Sender<ServerCommand>,
) -> PendingSurfaceTreeTransactionsSnapshot {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePendingSurfaceTreeTransactions(reply))
        .unwrap();
    wait_for_server_commands(commands);
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report pending surface-tree transactions")
}

pub(in crate::compositor::tests) fn capture_subsurface_stack_state(
    commands: &Sender<ServerCommand>,
    parent_id: u32,
) -> SubsurfaceStackStateSnapshot {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureSubsurfaceStackState { parent_id, reply })
        .unwrap();
    wait_for_server_commands(commands);
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report subsurface stack state")
}

pub(in crate::compositor::tests) fn capture_pending_frame_callbacks(
    commands: &Sender<ServerCommand>,
) -> bool {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePendingFrameCallbacks(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report pending frame callbacks")
}

pub(in crate::compositor::tests) fn capture_only_pending_surface_frame_callbacks(
    commands: &Sender<ServerCommand>,
) -> bool {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureOnlyPendingSurfaceFrameCallbacks(
            reply,
        ))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report pending surface frame callback state")
}

pub(in crate::compositor::tests) fn capture_pending_frame_work(
    commands: &Sender<ServerCommand>,
) -> bool {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePendingFrameWork(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report pending frame work")
}

pub(in crate::compositor::tests) fn capture_frame_eligible_presentation_feedback_work(
    commands: &Sender<ServerCommand>,
) -> bool {
    let (reply, receiver) = std::sync::mpsc::channel();
    commands
        .send(ServerCommand::CaptureFrameEligiblePresentationFeedbackWork(
            reply,
        ))
        .unwrap();
    receiver.recv_timeout(Duration::from_secs(1)).unwrap()
}

pub(in crate::compositor::tests) fn capture_frame_callback_metrics(
    commands: &Sender<ServerCommand>,
) -> FrameCallbackMetrics {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CaptureFrameCallbackMetrics(reply))
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report frame callback metrics")
}

pub(in crate::compositor::tests) fn update_interaction_and_report(
    commands: &Sender<ServerCommand>,
    x: f64,
    y: f64,
) -> bool {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::UpdateInteractionResult { x, y, reply })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("server should report interaction update")
}
