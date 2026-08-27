use std::{
    borrow::Cow,
    collections::HashMap,
    io,
    os::fd::{AsFd, BorrowedFd},
    sync::{Arc, Mutex},
};

#[cfg(test)]
use super::astrea_shell_capability::test_capability_path;
use super::astrea_shell_capability::{AstreaShellCapability, AstreaShellCapabilityVerifier};
use super::gpu_protocol_capabilities::GpuProtocolCapabilities;
use super::protocols::versions;
use crate::astrea_shell_control::server::astrea_shell_control_manager_v1;
use crate::astrea_shortcuts::server::astrea_shortcuts_manager_v1;
use crate::astrea_toplevel_management::server::astrea_toplevel_manager_v1;
#[cfg(test)]
use crate::render_backend::buffer::BufferId;
use crate::render_backend::egl_gles::EglGlesDmabufFeedback;
use crate::syncobj::DrmSyncobjDevice;
use crate::xwayland::trace::{self, TraceFields};
use wayland_protocols::ext::data_control::v1::server::ext_data_control_manager_v1;
use wayland_protocols::wp::{
    fractional_scale::v1::server::wp_fractional_scale_manager_v1,
    idle_inhibit::zv1::server::zwp_idle_inhibit_manager_v1,
    pointer_constraints::zv1::server::zwp_pointer_constraints_v1,
    pointer_warp::v1::server::wp_pointer_warp_v1, presentation_time::server::wp_presentation,
    primary_selection::zv1::server::zwp_primary_selection_device_manager_v1,
    relative_pointer::zv1::server::zwp_relative_pointer_manager_v1,
    viewporter::server::wp_viewporter,
};
use wayland_protocols::xdg::{
    activation::v1::server::xdg_activation_v1, decoration::zv1::server::zxdg_decoration_manager_v1,
    shell::server::xdg_wm_base,
};
use wayland_protocols::xwayland::shell::v1::server::xwayland_shell_v1;
use wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_shell_v1;
use wayland_server::{
    Display, DisplayHandle, ListeningSocket,
    backend::{ClientData, ClientId, DisconnectReason},
    protocol::{
        wl_compositor, wl_data_device_manager, wl_output, wl_seat, wl_shm, wl_subcompositor,
    },
};
#[path = "server_control.rs"]
mod control_api;
#[path = "server_xwayland.rs"]
mod xwayland_api;
use crate::wm::{WorkspaceId, WorkspaceSwitchOutcome};
use crate::xwayland::xwm::{ConfigureSource, XwmCommand, XwmEvent};
use crate::xwayland::{X11WindowHandle, XwaylandAssociationEvent, XwaylandGeneration};
#[path = "server_globals.rs"]
mod server_globals;
#[path = "server_gpu_globals.rs"]
mod server_gpu_globals;
#[path = "server_shortcut_inhibition.rs"]
mod server_shortcut_inhibition;
use super::{
    AcquireCommitId, AcquireWatchChange, AstreaShortcutPhase, BufferReleaseMetrics,
    ClientCursorRenderState, CompositorError, CompositorFrameBatchId, CompositorState,
    CoreComplianceMetrics, DecorationRenderInstance, DirectScanoutFeedbackCapabilities,
    DirectScanoutSceneBlockers, DirectScanoutSceneCandidate, DirectScanoutSceneRejection,
    ExplicitSyncPoint, FrameBatchDiscardReason, FrameCallbackMetrics, FrameCallbackTime,
    FramePacingProtocolCapabilities, FramePresentation, FullscreenRenderPlanMetrics,
    InputProtocolCapabilities, InteractionUpdateOutcome, OutputRect, PendingProcessLaunch,
    PointerAxisFrame, PresentationClock, PresentationProtocolCapabilities, ProtocolOnlyCompletion,
    RenderGenerationCause, RenderableSurface, RendererProtocolCapabilities, ResizeFlowMetrics,
    SelectionProtocolCapabilities, SubsurfaceTransactionMetrics, SurfaceDamagePresentation,
    SurfacePacingMetrics, SurfacePresentationMetadata, WindowActivationOutcome, WindowFocusOutcome,
    WindowFocusReason, WindowInteractionDebugSnapshot, WindowInteractionEndReason,
    XwaylandSceneBatchError, XwaylandSceneBatchToken, XwaylandSceneMetricsSnapshot, color,
    input::{PointerConstraintBackendId, PointerConstraintBackendRequest},
};
#[derive(Debug)]
pub struct OwnCompositorServer {
    pub(super) display: Display<CompositorState>,
    pub(super) socket: ListeningSocket,
    pub(super) socket_name: String,
    pub(super) state: CompositorState,
    #[allow(dead_code)]
    astrea_shell_capability: Option<AstreaShellCapability>,
    disconnected_clients: Arc<Mutex<Vec<DisconnectedClient>>>,
    client_pids: Arc<Mutex<HashMap<ClientId, i32>>>,
    xwayland_global_data: XwaylandShellGlobalData,
    xwayland_disconnects: Vec<XwaylandClientIdentity>,
    gpu_buffer_protocols_enabled: bool,
    shutdown_releases_armed: bool,
    pub(super) native_input_batch_active: bool,
    pub(super) native_input_batch_flush_pending: bool,
    #[cfg(test)]
    pub(super) wayland_flush_count: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XwaylandClientIdentity {
    pub client_id: ClientId,
    pub generation: XwaylandGeneration,
}
#[derive(Debug, Clone)]
pub(in crate::compositor) struct XwaylandShellGlobalData {
    pub(in crate::compositor) active: Arc<Mutex<Option<XwaylandClientIdentity>>>,
    pub(in crate::compositor) bind_events: Arc<Mutex<Vec<XwaylandClientIdentity>>>,
}
#[derive(Debug)]
struct TyphonClientData {
    disconnected_clients: Arc<Mutex<Vec<DisconnectedClient>>>,
    client_pids: Arc<Mutex<HashMap<ClientId, i32>>>,
}
#[derive(Debug, Clone)]
struct DisconnectedClient {
    client_id: ClientId,
    pid: Option<i32>,
}
impl ClientData for TyphonClientData {
    fn disconnected(&self, client_id: ClientId, _reason: DisconnectReason) {
        let pid = self
            .client_pids
            .lock()
            .ok()
            .and_then(|mut pids| pids.remove(&client_id));
        if let Ok(mut clients) = self.disconnected_clients.lock() {
            clients.push(DisconnectedClient { client_id, pid });
        }
    }
}
impl Drop for OwnCompositorServer {
    fn drop(&mut self) {
        if self.shutdown_releases_armed {
            self.finish_commit_debug_for_shutdown();
        }
    }
}
impl OwnCompositorServer {
    pub(crate) fn focused_x11_window_xid(&self) -> Option<u32> {
        let window_id = self.state.focused_window_id?;
        match self.state.window(window_id)?.backend {
            super::WindowBackend::X11(handle) => Some(handle.xid()),
            _ => None,
        }
    }

    pub fn core_compliance_metrics(&self) -> CoreComplianceMetrics {
        self.state.compliance_metrics
    }

    pub fn finish_commit_debug_for_shutdown(&mut self) {
        if !self.shutdown_releases_armed {
            return;
        }
        self.state.release_cached_resources_for_shutdown();
        self.state.discard_all_pending_presentation_feedbacks();
        self.state.release_client_buffers_for_shutdown();
        println!(
            "oblivion-one compliance: {:?}",
            self.state.compliance_metrics
        );
        if let Some(summary) = self.state.take_commit_debug_summary_line() {
            println!("{summary}");
        }
    }

    pub fn disarm_shutdown_releases(&mut self) {
        self.shutdown_releases_armed = false;
    }

    #[cfg(test)]
    pub(super) const fn shutdown_releases_armed_for_test(&self) -> bool {
        self.shutdown_releases_armed
    }

    pub fn bind(socket_name: impl Into<String>) -> Result<Self, CompositorError> {
        Self::bind_with_gpu_buffers(socket_name, true)
    }

    pub fn bind_cpu_composition(socket_name: impl Into<String>) -> Result<Self, CompositorError> {
        Self::bind_with_gpu_buffers(socket_name, false)
    }

    pub fn bind_native_base(socket_name: impl Into<String>) -> Result<Self, CompositorError> {
        Self::bind_with_gpu_buffers_and_capabilities(
            socket_name,
            false,
            InputProtocolCapabilities::native_libinput(),
            SelectionProtocolCapabilities::core_clipboard(),
            RendererProtocolCapabilities::unsupported(),
            FramePacingProtocolCapabilities::qualified_native(),
            PresentationProtocolCapabilities::qualified_native(),
        )
    }

    pub fn bind_with_capabilities(
        socket_name: impl Into<String>,
        gpu_buffers_enabled: bool,
        input_capabilities: InputProtocolCapabilities,
        selection_capabilities: SelectionProtocolCapabilities,
        renderer_capabilities: RendererProtocolCapabilities,
    ) -> Result<Self, CompositorError> {
        Self::bind_with_gpu_buffers_and_capabilities(
            socket_name,
            gpu_buffers_enabled,
            input_capabilities,
            selection_capabilities,
            renderer_capabilities,
            FramePacingProtocolCapabilities::safe_baseline(),
            PresentationProtocolCapabilities::safe_baseline(),
        )
    }

    pub fn bind_with_capabilities_and_frame_pacing(
        socket_name: impl Into<String>,
        gpu_buffers_enabled: bool,
        input_capabilities: InputProtocolCapabilities,
        selection_capabilities: SelectionProtocolCapabilities,
        renderer_capabilities: RendererProtocolCapabilities,
        frame_pacing_capabilities: FramePacingProtocolCapabilities,
    ) -> Result<Self, CompositorError> {
        Self::bind_with_gpu_buffers_and_capabilities(
            socket_name,
            gpu_buffers_enabled,
            input_capabilities,
            selection_capabilities,
            renderer_capabilities,
            frame_pacing_capabilities,
            PresentationProtocolCapabilities::safe_baseline(),
        )
    }

    #[cfg(test)]
    pub(super) fn bind_with_input_capabilities(
        socket_name: impl Into<String>,
        input_capabilities: InputProtocolCapabilities,
    ) -> Result<Self, CompositorError> {
        Self::bind_with_gpu_buffers_and_capabilities(
            socket_name,
            false,
            input_capabilities,
            SelectionProtocolCapabilities::core_clipboard(),
            RendererProtocolCapabilities::unsupported(),
            FramePacingProtocolCapabilities::safe_baseline(),
            PresentationProtocolCapabilities::safe_baseline(),
        )
    }

    #[cfg(test)]
    pub(super) fn bind_with_selection_capabilities(
        socket_name: impl Into<String>,
        selection_capabilities: SelectionProtocolCapabilities,
    ) -> Result<Self, CompositorError> {
        Self::bind_with_gpu_buffers_and_capabilities(
            socket_name,
            false,
            InputProtocolCapabilities::desktop_baseline(),
            selection_capabilities,
            RendererProtocolCapabilities::unsupported(),
            FramePacingProtocolCapabilities::safe_baseline(),
            PresentationProtocolCapabilities::safe_baseline(),
        )
    }

    #[cfg(test)]
    pub(super) fn bind_with_clipboard_bridge(
        socket_name: impl Into<String>,
        clipboard_bridge: Box<dyn super::ClipboardBridge>,
    ) -> Result<Self, CompositorError> {
        let mut server = Self::bind_with_gpu_buffers_and_capabilities(
            socket_name,
            false,
            InputProtocolCapabilities::desktop_baseline(),
            SelectionProtocolCapabilities::core_clipboard(),
            RendererProtocolCapabilities::unsupported(),
            FramePacingProtocolCapabilities::safe_baseline(),
            PresentationProtocolCapabilities::safe_baseline(),
        )?;
        server.state.clipboard_bridge = Some(clipboard_bridge);
        Ok(server)
    }

    fn bind_with_gpu_buffers(
        socket_name: impl Into<String>,
        gpu_buffers_enabled: bool,
    ) -> Result<Self, CompositorError> {
        Self::bind_with_gpu_buffers_and_capabilities(
            socket_name,
            gpu_buffers_enabled,
            InputProtocolCapabilities::desktop_baseline(),
            SelectionProtocolCapabilities::core_clipboard(),
            RendererProtocolCapabilities::unsupported(),
            FramePacingProtocolCapabilities::safe_baseline(),
            PresentationProtocolCapabilities::safe_baseline(),
        )
    }

    fn bind_with_gpu_buffers_and_capabilities(
        socket_name: impl Into<String>,
        gpu_buffers_enabled: bool,
        input_capabilities: InputProtocolCapabilities,
        selection_capabilities: SelectionProtocolCapabilities,
        renderer_capabilities: RendererProtocolCapabilities,
        frame_pacing_capabilities: FramePacingProtocolCapabilities,
        presentation_capabilities: PresentationProtocolCapabilities,
    ) -> Result<Self, CompositorError> {
        let socket_name = socket_name.into();
        let astrea_shell_capability = {
            #[cfg(test)]
            {
                AstreaShellCapability::create_for_path(test_capability_path(&socket_name))?
            }
            #[cfg(not(test))]
            {
                AstreaShellCapability::create_from_environment()?
            }
        };
        let display =
            Display::new().map_err(|error| CompositorError::DisplayInit(error.to_string()))?;
        #[cfg(test)]
        let syncobj_device = if gpu_buffers_enabled {
            DrmSyncobjDevice::open_available()
        } else {
            None
        };
        #[cfg(not(test))]
        let syncobj_device = None;
        let gpu_capabilities = {
            #[cfg(test)]
            {
                if gpu_buffers_enabled {
                    GpuProtocolCapabilities::test_contract(syncobj_device.is_some())
                } else {
                    GpuProtocolCapabilities::default()
                }
            }
            #[cfg(not(test))]
            {
                GpuProtocolCapabilities::default()
            }
        };
        let xwayland_global_data = XwaylandShellGlobalData {
            active: Arc::new(Mutex::new(None)),
            bind_events: Arc::new(Mutex::new(Vec::new())),
        };
        server_globals::register_minimum_globals(
            &display.handle(),
            &gpu_capabilities,
            gpu_buffers_enabled,
            input_capabilities,
            selection_capabilities,
            renderer_capabilities,
            frame_pacing_capabilities,
            presentation_capabilities,
            xwayland_global_data.clone(),
        );
        let socket = ListeningSocket::bind(&socket_name)
            .map_err(|error| CompositorError::Bind(error.to_string()))?;

        let mut state = CompositorState::new(syncobj_device);
        state.load_persisted_decoration_theme();
        state.set_gpu_protocol_capabilities(gpu_capabilities.clone());
        let verifier: AstreaShellCapabilityVerifier = astrea_shell_capability.verifier();
        state.set_astrea_shell_capability_verifier(verifier);
        state.set_typhon_socket_name(socket_name.clone());
        let disconnected_clients = Arc::new(Mutex::new(Vec::new()));
        let client_pids = Arc::new(Mutex::new(HashMap::new()));
        Ok(Self {
            display,
            socket,
            socket_name,
            state,
            astrea_shell_capability: Some(astrea_shell_capability),
            disconnected_clients,
            client_pids,
            xwayland_global_data,
            xwayland_disconnects: Vec::new(),
            gpu_buffer_protocols_enabled: gpu_buffers_enabled
                && gpu_capabilities.any_global_enabled(),
            shutdown_releases_armed: true,
            native_input_batch_active: false,
            native_input_batch_flush_pending: false,
            #[cfg(test)]
            wayland_flush_count: 0,
        })
    }

    pub fn enable_gpu_buffer_protocols_with_capabilities(
        &mut self,
        capabilities: GpuProtocolCapabilities,
    ) {
        if self.gpu_buffer_protocols_enabled {
            return;
        }
        self.state
            .set_gpu_protocol_capabilities(capabilities.clone());
        server_gpu_globals::register_gpu_buffer_globals(&self.display.handle(), &capabilities);
        self.gpu_buffer_protocols_enabled = capabilities.any_global_enabled();
    }

    #[cfg(test)]
    pub fn enable_gpu_buffer_protocols(&mut self) {
        let capabilities =
            GpuProtocolCapabilities::test_contract(self.state.syncobj_device.is_some());
        self.enable_gpu_buffer_protocols_with_capabilities(capabilities);
    }

    #[doc(hidden)]
    pub fn set_native_syncobj_device(&mut self, device: Option<DrmSyncobjDevice>) {
        assert!(
            !self.gpu_buffer_protocols_enabled,
            "native syncobj device must be selected before GPU globals are enabled"
        );
        self.state.syncobj_device = device;
    }

    #[doc(hidden)]
    pub fn enable_external_acquire_readiness(&mut self) {
        self.state.enable_external_acquire_readiness();
    }

    #[doc(hidden)]
    pub fn take_acquire_watch_changes(&mut self) -> Vec<AcquireWatchChange> {
        self.state.take_acquire_watch_changes()
    }

    #[doc(hidden)]
    pub fn mark_acquire_commit_eventfd_backed(&mut self, commit_id: AcquireCommitId) -> bool {
        self.state.mark_acquire_commit_eventfd_backed(commit_id)
    }

    #[doc(hidden)]
    pub fn mark_acquire_commit_fallback_backed(&mut self, commit_id: AcquireCommitId) -> bool {
        self.state.mark_acquire_commit_fallback_backed(commit_id)
    }

    #[doc(hidden)]
    pub fn mark_acquire_commit_ready(
        &mut self,
        commit_id: AcquireCommitId,
        surface_id: u32,
        acquire: &ExplicitSyncPoint,
    ) -> bool {
        self.state
            .mark_acquire_commit_ready(commit_id, surface_id, acquire)
    }

    #[doc(hidden)]
    pub fn set_commit_debug_pageflip_pending(&mut self, pending: bool) {
        self.state.set_commit_debug_pageflip_pending(pending);
    }

    pub const fn gpu_buffer_protocols_enabled(&self) -> bool {
        self.gpu_buffer_protocols_enabled
    }

    pub fn socket_name(&self) -> &str {
        &self.socket_name
    }

    pub fn listener_fd(&self) -> BorrowedFd<'_> {
        self.socket.as_fd()
    }

    pub fn client_dispatch_fd(&self) -> BorrowedFd<'_> {
        self.display.as_fd()
    }

    pub fn begin_xwayland_scene_batch(
        &mut self,
    ) -> Result<XwaylandSceneBatchToken, XwaylandSceneBatchError> {
        self.state.begin_xwayland_scene_batch()
    }

    pub fn commit_xwayland_scene_batch(
        &mut self,
        token: XwaylandSceneBatchToken,
    ) -> Result<Vec<XwmCommand>, XwaylandSceneBatchError> {
        let dirty = self.state.commit_xwayland_scene_batch(token)?;
        if dirty.render_stack_dirty && self.state.normalize_window_stacking() {
            self.state.mark_xwayland_scene_repaint();
        }
        let mut commands = Vec::new();
        if dirty.client_lists_dirty {
            commands.push(self.sync_xwayland_client_lists());
        }
        if dirty.pointer_focus_dirty {
            self.state.commit_pointer_crossing_at_last_position();
            self.state.note_committed_pointer_refresh();
        }
        self.publish_astrea_toplevel_updates();
        Ok(commands)
    }

    pub fn abort_xwayland_scene_batch(
        &mut self,
        token: XwaylandSceneBatchToken,
    ) -> Result<(), XwaylandSceneBatchError> {
        self.state.abort_xwayland_scene_batch(token)
    }

    pub fn take_xwayland_scene_repaint_request(&mut self) -> bool {
        self.state.take_xwayland_scene_repaint_request()
    }

    pub fn xwayland_scene_metrics(&self) -> XwaylandSceneMetricsSnapshot {
        self.state.xwayland_scene_metrics()
    }

    #[cfg(test)]
    pub(crate) fn xwayland_scene_batch_dirty_for_test(&self) -> bool {
        self.state.xwayland_scene_batch_dirty_for_test()
    }

    pub fn apply_xwayland_window_event(&mut self, event: XwmEvent) -> Vec<XwmCommand> {
        let scene_generation_before = self.state.scene_render_generation;
        let commands = self.apply_xwayland_window_event_inner(event);
        if self.state.xwayland_scene_batch_active()
            && self.state.scene_render_generation != scene_generation_before
        {
            self.state.mark_xwayland_scene_repaint();
        }
        if !self.state.xwayland_scene_batch_active() {
            self.publish_astrea_toplevel_updates();
        }
        commands
    }

    fn apply_xwayland_window_event_inner(&mut self, event: XwmEvent) -> Vec<XwmCommand> {
        self.state.note_xwayland_scene_mutation();
        match event {
            XwmEvent::WindowMapRequested(handle) => vec![XwmCommand::Map(handle)],
            XwmEvent::WindowReady(snapshot) => self.apply_xwayland_window_ready(snapshot),
            XwmEvent::WindowAdmissionCancelled { window, reason } => {
                self.state.note_pre_admission_popup_cancellation();
                trace::emit("xwayland_popup_admission_cancelled", || {
                    TraceFields::new()
                        .field("source", "compositor")
                        .field("xid", window.xid())
                        .field("generation", window.generation().get())
                        .field("reason", format!("{reason:?}"))
                });
                Vec::new()
            }
            XwmEvent::WindowDestroyed(handle) => self.apply_xwayland_window_teardown(handle, true),
            XwmEvent::WindowWithdrawn(handle) => self.apply_xwayland_window_teardown(handle, false),
            XwmEvent::MetadataChanged { window, delta } => {
                let prior_id = self.state.window_id_for_x11_handle(window);
                let delta_debug = format!("{delta:?}");
                let focus_before = self.focused_x11_window_xid();
                let prior_focused =
                    prior_id.is_some_and(|id| self.state.focused_window_id == Some(id));
                let publish_lists = matches!(
                    &delta,
                    crate::xwayland::xwm::X11MetadataDelta::TransientFor(_)
                        | crate::xwayland::xwm::X11MetadataDelta::WindowTypes(_)
                        | crate::xwayland::xwm::X11MetadataDelta::Kind(_)
                );
                let old_policy = self.state.x11_placement_policy(window);
                self.state.apply_x11_metadata_delta(window, delta);
                let new_policy = self.state.x11_placement_policy(window);
                let mut commands = Vec::new();
                if old_policy != new_policy
                    && let Some(geometry) = self.state.x11_authoritative_geometry(window)
                {
                    commands.push(XwmCommand::ConfigureFrame {
                        window,
                        geometry,
                        frame_extents: self.state.x11_decoration_frame_extents(window),
                    });
                }
                if prior_focused
                    && prior_id.is_some_and(|id| {
                        self.state
                            .window(id)
                            .is_some_and(|window| !window.is_normal_x11_role())
                    })
                {
                    self.state.focused_window_id = None;
                    self.state.focused_surface = None;
                    self.state.clear_keyboard_focus();
                    let _ = self.state.focus_topmost_renderable_toplevel();
                }
                self.state.refresh_pointer_focus_at_last_position();
                trace::emit("metadata_changed", || {
                    TraceFields::new()
                        .field("source", "compositor")
                        .field("xid", window.xid())
                        .field("metadata_delta", delta_debug)
                        .optional("focus_before", focus_before)
                        .optional("focus_after", self.focused_x11_window_xid())
                        .field("focus_repaired", prior_focused)
                });
                if !publish_lists {
                    return commands;
                }
                if !self.state.defer_client_list_sync() {
                    commands.push(self.sync_xwayland_client_lists());
                }
                commands
            }
            XwmEvent::ConfigureRequested { window, request } => {
                if self.state.x11_resize_active(window) {
                    let mut commands = Vec::with_capacity(2);
                    if let Some(mode) = request.stack_mode
                        && self
                            .state
                            .apply_x11_stack_request(window, request.sibling, mode)
                    {
                        commands.push(self.restack_xwayland_windows());
                    }
                    if (request.fields.x
                        || request.fields.y
                        || request.fields.width
                        || request.fields.height
                        || request.fields.border_width)
                        && let Some(geometry) = self.state.x11_authoritative_geometry(window)
                    {
                        commands.push(XwmCommand::ConfigureNotify { window, geometry });
                    }
                    self.state.refresh_pointer_focus_at_last_position();
                    return commands;
                }
                let constraints = self
                    .state
                    .window_id_for_x11_handle(window)
                    .and_then(|id| self.state.window(id))
                    .map(|window| window.constraints)
                    .unwrap_or_default();
                let current_authoritative = self.state.x11_authoritative_geometry(window);
                let geometry = self.x11_configure_request_geometry(window, request, constraints);
                self.trace_x11_configure_request_normalized(
                    window,
                    request,
                    current_authoritative,
                    geometry,
                );
                let border_width = self
                    .state
                    .effective_managed_x11_border_width(window, request.border_width);
                if request.fields.x
                    || request.fields.y
                    || request.fields.width
                    || request.fields.height
                {
                    let _ = self.state.set_x11_geometry(window, geometry);
                }
                let mut commands = vec![XwmCommand::Configure {
                    window,
                    geometry,
                    fields: request.fields,
                    source: ConfigureSource::ClientRequest,
                    border_width,
                }];
                if let Some(mode) = request.stack_mode
                    && self
                        .state
                        .apply_x11_stack_request(window, request.sibling, mode)
                {
                    commands.push(self.restack_xwayland_windows());
                }
                self.state.refresh_pointer_focus_at_last_position();
                commands
            }
            XwmEvent::MoveResizeRequested { window, request } => {
                self.handle_x11_move_resize_request(window, request);
                Vec::new()
            }
            XwmEvent::ConfigureNotify {
                window,
                geometry,
                above_sibling,
            } => {
                let placement_policy = self.state.x11_placement_policy(window);
                if placement_policy
                    != Some(
                        crate::compositor::desktop_window::X11PlacementPolicy::CompositorManaged,
                    )
                {
                    let _ = self.state.reconcile_x11_configure_notify(window, geometry);
                } else {
                    trace::emit("x11_managed_configure_notify_preserved", || {
                        TraceFields::new()
                            .field("source", "compositor")
                            .field("xid", window.xid())
                            .field("geometry", format!("{geometry:?}"))
                            .field("reason", "managed_geometry_authority")
                    });
                }
                let is_override_redirect = self
                    .state
                    .window_id_for_x11_handle(window)
                    .and_then(|id| self.state.window(id))
                    .is_some_and(|window| {
                        window.x11_role
                            == Some(
                                crate::compositor::desktop_window::X11DesktopRole::OverrideRedirect,
                            )
                    });
                if is_override_redirect && above_sibling.is_some() {
                    self.state
                        .note_override_redirect_restack_writeback_prevented();
                }
                self.state.refresh_pointer_focus_at_last_position();
                Vec::new()
            }
            XwmEvent::OverrideRedirectStackSnapshot {
                generation,
                epoch,
                bottom_to_top,
            } => {
                let outcome = if self.state.xwayland_scene_batch_active() {
                    self.state.stage_override_redirect_stack_snapshot(
                        generation,
                        epoch,
                        bottom_to_top,
                    );
                    crate::compositor::OverrideRedirectStackSnapshotResult::Rejected
                } else {
                    self.state.apply_override_redirect_stack_snapshot(
                        generation,
                        epoch,
                        &bottom_to_top,
                    )
                };
                if matches!(
                    outcome,
                    crate::compositor::OverrideRedirectStackSnapshotResult::Applied {
                        logical_stack_changed: true,
                    }
                ) {
                    self.state.refresh_pointer_focus_at_last_position();
                }
                Vec::new()
            }
            XwmEvent::StateRequested { window, request } => {
                let was_hidden = self
                    .state
                    .window_id_for_x11_handle(window)
                    .and_then(|id| self.state.window(id))
                    .is_some_and(|window| window.state.is_minimized());
                let Some(state) = self.state.apply_x11_state_request(window, request) else {
                    return Vec::new();
                };
                let mut commands = Vec::with_capacity(1);
                if state.hidden != was_hidden {
                    commands.push(if state.hidden {
                        XwmCommand::Unmap(window)
                    } else {
                        XwmCommand::Map(window)
                    });
                }
                commands
            }
            XwmEvent::FocusRequested {
                window,
                source,
                timestamp,
                current_time,
                user_time,
            } => {
                let relationship_allowed = self.state.x11_focus_request_allowed(window);
                if crate::xwayland::xwm::focus::activation_allowed(
                    source == 2,
                    timestamp,
                    current_time,
                    user_time,
                    relationship_allowed,
                    relationship_allowed,
                    false,
                ) && self
                    .state
                    .window_id_for_x11_handle(window)
                    .is_some_and(|window_id| {
                        !matches!(
                            self.state.activate_desktop_window(
                                window_id,
                                WindowFocusReason::ShellActivation
                            ),
                            WindowActivationOutcome::Unavailable
                        )
                    })
                {
                    if let Some(window_id) = self.state.window_id_for_x11_handle(window) {
                        let _ = self.state.raise_window_id(window_id);
                    }
                    vec![XwmCommand::Focus {
                        window: Some(window),
                        timestamp,
                    }]
                } else {
                    Vec::new()
                }
            }
            XwmEvent::CurrentDesktopRequested(workspace) => {
                let _ = self.state.activate_workspace(workspace);
                Vec::new()
            }
            XwmEvent::WindowWorkspaceRequested { window, workspace } => {
                let Some(window_id) = self.state.window_id_for_x11_handle(window) else {
                    return Vec::new();
                };
                self.state
                    .move_window_family_to_workspace(window_id, workspace);
                Vec::new()
            }
            XwmEvent::ResizeSyncAckObserved {
                window,
                counter_value,
            } => {
                let Some((association_serial, commit_floor)) =
                    self.xwayland_resize_commit_floor(window)
                else {
                    return Vec::new();
                };
                vec![XwmCommand::ReleaseResizeCommits {
                    window,
                    counter_value,
                    association_serial,
                    commit_floor,
                }]
            }
            XwmEvent::ResizeSyncPresented {
                window,
                transaction_id,
                geometry,
            } => {
                let accepted = self
                    .state
                    .finalize_x11_resize_with_geometry(window, Some(geometry));
                trace::emit("xwayland_resize_presentation", || {
                    TraceFields::new()
                        .field("source", "compositor")
                        .field("xid", window.xid())
                        .field("transaction_id", transaction_id)
                        .field("geometry", format!("{geometry:?}"))
                        .field("accepted", accepted)
                        .field(
                            "reason",
                            if accepted {
                                "current_content_epoch"
                            } else {
                                "older_content_epoch"
                            },
                        )
                });
                vec![XwmCommand::CompleteResizeSync(window)]
            }
            XwmEvent::ResizeSyncPresentedIntermediate { window, .. } => {
                vec![XwmCommand::CompleteResizeSync(window)]
            }
            XwmEvent::ResizeSyncImmediate { window, geometry } => {
                let accepted = self
                    .state
                    .finalize_x11_resize_with_geometry(window, Some(geometry));
                trace::emit("xwayland_resize_presentation", || {
                    TraceFields::new()
                        .field("source", "compositor")
                        .field("xid", window.xid())
                        .field("transaction_id", 0)
                        .field("geometry", format!("{geometry:?}"))
                        .field("accepted", accepted)
                        .field(
                            "reason",
                            if accepted {
                                "immediate_content_epoch"
                            } else {
                                "older_content_epoch"
                            },
                        )
                });
                Vec::new()
            }
            XwmEvent::ResizeSyncTimedOut(window)
            | XwmEvent::ResizeSyncTimedOutWithFollowup(window) => {
                let _ = self.state.finalize_x11_resize_if_interaction_ended(window);
                Vec::new()
            }
            XwmEvent::CloseRequestedByClient(window) => {
                if let Some(window_id) = self.state.window_id_for_x11_handle(window) {
                    self.state.backend_commands.push(
                        crate::compositor::window_backend::WindowBackendCommand::Close {
                            window: window_id,
                        },
                    );
                }
                Vec::new()
            }
        }
    }
    pub fn accepted_clients(&self) -> usize {
        self.state.accepted_clients
    }

    pub fn xdg_toplevels(&self) -> usize {
        self.state.xdg_toplevels
    }

    pub fn last_app_id(&self) -> Option<&str> {
        self.state.last_app_id.as_deref()
    }

    pub fn xdg_popups(&self) -> usize {
        self.state.xdg_popups
    }

    pub fn renderable_surfaces(&self) -> &[RenderableSurface] {
        &self.state.renderable_surfaces
    }

    pub fn native_frame_renderable_surfaces(&self) -> Cow<'_, [RenderableSurface]> {
        self.state.native_frame_renderable_surfaces()
    }

    pub fn native_decoration_render_instances(
        &self,
        surfaces: &[RenderableSurface],
    ) -> Vec<DecorationRenderInstance> {
        self.state.native_decoration_render_instances(surfaces)
    }

    pub fn native_decoration_render_instances_for_scale(
        &self,
        surfaces: &[RenderableSurface],
        output_scale: f64,
    ) -> Vec<DecorationRenderInstance> {
        self.state
            .native_decoration_render_instances_for_scale(surfaces, output_scale)
    }

    pub fn external_overlay_surface_ids(&self) -> Vec<u32> {
        self.state.external_overlay_surface_ids()
    }

    pub fn active_workspace(&self) -> WorkspaceId {
        self.state.active_workspace()
    }

    pub fn activate_workspace(&mut self, workspace: WorkspaceId) -> WorkspaceSwitchOutcome {
        self.state.activate_workspace(workspace)
    }

    pub fn move_focused_window_to_workspace(&mut self, workspace: WorkspaceId) -> bool {
        self.state.move_focused_window_to_workspace(workspace)
    }

    pub fn mark_render_damage_presented(&mut self) {
        self.state.mark_render_damage_presented();
    }

    #[doc(hidden)]
    pub fn capture_surface_damage_presentation(&self) -> SurfaceDamagePresentation {
        self.state.capture_surface_damage_presentation()
    }

    #[doc(hidden)]
    pub fn capture_surface_damage_presentation_for_surface(
        &self,
        surface_id: u32,
    ) -> SurfaceDamagePresentation {
        self.state
            .capture_surface_damage_presentation_for_surface(surface_id)
    }

    #[doc(hidden)]
    pub fn commit_surface_damage_presented(&mut self, token: SurfaceDamagePresentation) {
        self.state.commit_surface_damage_presented(token);
    }

    pub fn client_cursor_render_state(&self) -> Option<ClientCursorRenderState<'_>> {
        self.state.client_cursor_render_state()
    }

    pub fn interaction_cursor_override_active(&self) -> bool {
        self.state.interaction_cursor_override_active()
    }

    pub fn compositor_cursor_shape(&self) -> crate::cursor_theme::CompositorCursorShape {
        self.state.compositor_cursor_shape()
    }

    pub fn client_cursor_request_active(&self) -> bool {
        self.state
            .focused_client_cursor
            .as_ref()
            .is_some_and(|choice| !choice.is_hidden())
    }

    pub fn client_cursor_explicitly_hidden(&self) -> bool {
        self.state.client_cursor_explicitly_hidden()
    }

    pub fn client_cursor_shape(&self) -> Option<u32> {
        self.state.client_cursor_shape()
    }

    pub fn cursor_visibility_requested(&self) -> bool {
        self.state.cursor_visibility.visible
    }

    pub fn last_pointer_position(&self) -> (f64, f64) {
        (self.state.last_pointer_x, self.state.last_pointer_y)
    }

    pub fn render_generation(&self) -> u64 {
        self.state.render_generation
    }

    pub fn cursor_generation(&self) -> u64 {
        self.state.cursor_generation
    }

    pub fn scene_render_generation(&self) -> u64 {
        self.state.scene_render_generation
    }

    pub fn render_generation_cause(&self) -> RenderGenerationCause {
        self.state.render_generation_cause()
    }

    pub fn usable_output_geometry(&self) -> OutputRect {
        self.state.usable_output_geometry()
    }

    pub const fn resize_flow_metrics(&self) -> ResizeFlowMetrics {
        self.state.resize_flow_metrics
    }

    pub const fn subsurface_transaction_metrics(&self) -> SubsurfaceTransactionMetrics {
        self.state.subsurface_transaction_metrics
    }

    pub const fn surface_pacing_metrics(&self) -> SurfacePacingMetrics {
        self.state.surface_pacing_metrics
    }

    pub fn fullscreen_render_plan_metrics(&self) -> FullscreenRenderPlanMetrics {
        self.state.fullscreen_render_plan_metrics()
    }

    pub fn direct_scanout_scene_candidate(
        &self,
    ) -> Result<DirectScanoutSceneCandidate, DirectScanoutSceneRejection> {
        self.state.direct_scanout_scene_candidate()
    }

    pub fn direct_scanout_scene_blockers(&self) -> DirectScanoutSceneBlockers {
        self.state.direct_scanout_scene_blockers()
    }

    pub fn fullscreen_tree_presentation_metadata(&self) -> Option<SurfacePresentationMetadata> {
        self.state.fullscreen_tree_presentation_metadata()
    }

    /// Returns the immutable publication epoch for the currently published
    /// buffer. Bufferless and metadata-only commits do not advance it.
    pub fn surface_content_epoch(&self, surface_id: u32) -> Option<u64> {
        self.state
            .surface_content_epoch(surface_id)
            .map(|sequence| sequence.get())
    }

    pub fn has_pending_frame_callbacks(&self) -> bool {
        self.state.has_pending_frame_callbacks()
    }

    pub fn frame_callback_time_for_output(&mut self) -> FrameCallbackTime {
        FrameCallbackTime::new(self.state.frame_callback_time_ms())
    }

    pub fn complete_protocol_only_frame_tick(
        &mut self,
        output_time: FrameCallbackTime,
    ) -> ProtocolOnlyCompletion {
        let completion = self.state.complete_protocol_only_frame_tick(output_time);
        let _ = self.display.flush_clients();
        completion
    }

    pub fn has_only_pending_surface_frame_callbacks(&self) -> bool {
        self.state.has_only_pending_surface_frame_callbacks()
    }

    pub fn has_pending_frame_prepare_work(&self) -> bool {
        self.state.has_pending_frame_prepare_work()
    }

    pub fn has_pending_interactive_visual_work(&self) -> bool {
        self.state.has_pending_interactive_visual_work()
    }

    pub fn record_interactive_scheduler_decision(&mut self) {
        self.state.record_interactive_scheduler_decision();
    }

    pub fn has_pending_acquire_watch_changes(&self) -> bool {
        self.state.has_pending_acquire_watch_changes()
    }

    pub fn has_unowned_frame_work(&self) -> bool {
        self.state.has_unowned_frame_work()
    }

    pub fn set_dmabuf_feedback(
        &mut self,
        feedback: EglGlesDmabufFeedback,
        main_device: Option<u64>,
        main_device_path: Option<String>,
    ) -> bool {
        let changed = self
            .state
            .set_dmabuf_feedback(feedback, main_device, main_device_path);
        let _ = self.display.flush_clients();
        changed
    }

    pub fn set_dmabuf_feedback_with_scanout_capabilities(
        &mut self,
        feedback: EglGlesDmabufFeedback,
        main_device: Option<u64>,
        main_device_path: Option<String>,
        scanout_capabilities: Option<DirectScanoutFeedbackCapabilities>,
    ) -> bool {
        let changed = self.state.set_dmabuf_feedback_with_scanout_capabilities(
            feedback,
            main_device,
            main_device_path,
            scanout_capabilities,
        );
        let _ = self.display.flush_clients();
        changed
    }

    pub fn set_dmabuf_feedback_with_scanout_capabilities_and_target(
        &mut self,
        feedback: EglGlesDmabufFeedback,
        main_device: Option<u64>,
        main_device_path: Option<String>,
        scanout_capabilities: Option<DirectScanoutFeedbackCapabilities>,
        scanout_target_device_override: Option<u64>,
    ) -> bool {
        let changed = self
            .state
            .set_dmabuf_feedback_with_scanout_capabilities_and_target(
                feedback,
                main_device,
                main_device_path,
                scanout_capabilities,
                scanout_target_device_override,
            );
        let _ = self.display.flush_clients();
        changed
    }

    pub fn set_output_size(&mut self, width: u32, height: u32) -> bool {
        let changed = self.state.set_output_size(width, height);
        let _ = self.display.flush_clients();
        changed
    }

    pub fn set_output_scale_factor(&mut self, scale_factor: f64) -> bool {
        let changed = self.state.set_output_scale_factor(scale_factor);
        let _ = self.display.flush_clients();
        changed
    }

    pub fn set_output_preferred_transform(&mut self, transform: wl_output::Transform) -> bool {
        let changed = self.state.set_output_preferred_transform(transform);
        let _ = self.display.flush_clients();
        changed
    }

    pub fn set_output_refresh_hz(&mut self, refresh_hz: u32) -> bool {
        let changed = self.state.set_output_refresh_hz(refresh_hz);
        let _ = self.display.flush_clients();
        changed
    }

    pub fn set_presentation_clock(&mut self, clock: PresentationClock) {
        if self.state.presentation_clock != clock {
            self.state.presentation_clock = clock;
            self.state.invalidate_pending_commit_timing_targets();
        }
    }

    pub fn presentation_clock(&self) -> PresentationClock {
        self.state.presentation_clock
    }

    pub fn send_pointer_axis(&mut self, horizontal: f64, vertical: f64) {
        self.state.send_pointer_axis(horizontal, vertical);
        let _ = self.flush_wayland_clients();
    }

    pub fn send_pointer_axis_frame(&mut self, frame: PointerAxisFrame) {
        self.state.send_pointer_axis_frame(frame);
        let _ = self.flush_wayland_clients();
    }

    pub fn take_pointer_constraint_backend_requests(
        &mut self,
    ) -> Vec<PointerConstraintBackendRequest> {
        self.state.take_pointer_constraint_backend_requests()
    }

    #[doc(hidden)]
    pub fn take_pending_process_launches(&mut self) -> Vec<PendingProcessLaunch> {
        self.state.take_pending_process_launches()
    }

    pub fn pointer_constraint_backend_activated(&mut self, id: PointerConstraintBackendId) {
        self.state.pointer_constraint_backend_activated(id);
        let _ = self.flush_wayland_clients();
    }

    pub fn pointer_constraint_backend_activation_current(
        &self,
        id: PointerConstraintBackendId,
    ) -> bool {
        self.state.pointer_constraint_backend_activation_current(id)
    }

    pub fn pointer_constraint_backend_deactivated(&mut self, id: PointerConstraintBackendId) {
        self.state.pointer_constraint_backend_deactivated(id);
        let _ = self.flush_wayland_clients();
    }

    pub fn pointer_constraint_backend_failed(
        &mut self,
        id: PointerConstraintBackendId,
        reason: impl AsRef<str>,
    ) {
        self.state
            .pointer_constraint_backend_failed(id, reason.as_ref());
        let _ = self.flush_wayland_clients();
    }

    pub fn begin_window_move_at(&mut self, x: f64, y: f64) -> bool {
        let started = self.state.begin_window_move_at(x, y);
        let _ = self.flush_wayland_clients();
        started
    }

    pub fn begin_window_move_at_with_trigger(
        &mut self,
        x: f64,
        y: f64,
        trigger_button: u32,
    ) -> bool {
        let started = self
            .state
            .begin_window_move_at_with_trigger(x, y, trigger_button);
        let _ = self.flush_wayland_clients();
        started
    }

    pub fn begin_window_resize_at(&mut self, x: f64, y: f64) -> bool {
        let started = self.state.begin_window_resize_at(x, y);
        let _ = self.flush_wayland_clients();
        started
    }

    pub fn begin_window_resize_at_with_trigger(
        &mut self,
        x: f64,
        y: f64,
        trigger_button: u32,
    ) -> bool {
        let started = self
            .state
            .begin_window_resize_at_with_trigger(x, y, trigger_button);
        let _ = self.flush_wayland_clients();
        started
    }

    #[doc(hidden)]
    pub fn x11_resize_active(&self, handle: X11WindowHandle) -> bool {
        self.state.x11_resize_active(handle)
    }

    #[doc(hidden)]
    pub fn begin_x11_resize_for_test(
        &mut self,
        handle: X11WindowHandle,
        geometry: crate::xwayland::xwm::X11Geometry,
    ) -> bool {
        self.state.begin_x11_resize_for_test(handle, geometry)
    }

    #[doc(hidden)]
    pub fn finalize_x11_resize_for_test(
        &mut self,
        handle: X11WindowHandle,
        geometry: crate::xwayland::xwm::X11Geometry,
    ) -> bool {
        self.state.finalize_x11_resize_for_test(handle, geometry)
    }

    pub fn begin_window_frame_action_at(&mut self, x: f64, y: f64) -> bool {
        let started = self.state.begin_window_frame_action_at(x, y);
        let _ = self.flush_wayland_clients();
        started
    }

    pub fn update_window_interaction(&mut self, x: f64, y: f64) -> bool {
        let updated = self.state.update_window_interaction(x, y);
        let _ = self.flush_wayland_clients();
        updated
    }

    pub fn update_window_interaction_for_input(
        &mut self,
        x: f64,
        y: f64,
    ) -> InteractionUpdateOutcome {
        let outcome = self.state.update_window_interaction_for_input(x, y);
        let _ = self.flush_wayland_clients();
        outcome
    }

    pub fn update_window_interaction_for_input_without_flush(
        &mut self,
        x: f64,
        y: f64,
    ) -> InteractionUpdateOutcome {
        self.state.update_window_interaction_for_input(x, y)
    }

    pub fn end_window_interaction(&mut self) {
        self.state.end_window_interaction();
        let _ = self.flush_wayland_clients();
    }

    pub fn cancel_window_interaction_for_session_suspend(&mut self) -> bool {
        let cancelled = self
            .state
            .cancel_window_interaction(WindowInteractionEndReason::SessionSuspended);
        let _ = self.flush_wayland_clients();
        cancelled
    }

    pub fn window_interaction_active(&self) -> bool {
        self.state.window_interaction_active()
    }
    pub fn active_window_interaction_trigger_button(&self) -> Option<u32> {
        self.state.active_window_interaction_trigger_button()
    }
    pub fn reconcile_window_interaction_trigger(&mut self, trigger_pressed: bool) -> bool {
        let reconciled = self
            .state
            .reconcile_window_interaction_trigger(trigger_pressed);
        let _ = self.flush_wayland_clients();
        reconciled
    }

    pub fn window_interaction_debug_snapshot(&self) -> Option<WindowInteractionDebugSnapshot> {
        self.state.window_interaction_debug_snapshot()
    }

    pub fn emit_astrea_shortcut(
        &mut self,
        namespace: &str,
        name: &str,
        phase: AstreaShortcutPhase,
        timestamp: u32,
    ) -> usize {
        let dispatched = self
            .state
            .emit_astrea_shortcut(namespace, name, phase, timestamp);
        let _ = self.flush_wayland_clients();
        dispatched
    }

    pub fn authorize_astrea_shell_pid(&mut self, pid: u32) {
        self.state.authorize_astrea_shell_pid(pid);
    }
    #[cfg(test)]
    pub(crate) fn clear_astrea_shell_authorization(&mut self) {
        self.state.clear_astrea_shell_authorization();
    }

    pub fn resize_focused_window_to(&mut self, width: u32, height: u32) -> bool {
        let resized = self.state.resize_focused_window_to(width, height);
        let _ = self.display.flush_clients();
        resized
    }

    pub fn prepare_frame(&mut self) {
        self.state.commit_ready_explicit_sync_buffers();
        color::flush_pending_color_info(&mut self.state);
        self.state.flush_pending_resize_configure();
        let _ = self.display.flush_clients();
    }

    pub fn flush_pending_interactive_visual_state_for_render_admission(
        &mut self,
        render_ahead: bool,
    ) -> bool {
        let pending = self.state.has_pending_interactive_visual_work();
        let tiled = self.state.flush_pending_tiled_resize();
        let floating = self.state.flush_pending_floating_interaction_geometry();
        let applied = tiled || floating;
        if pending && applied {
            self.state.record_interactive_render_admission(render_ahead);
        }
        self.state.flush_pending_resize_configure();
        let _ = self.display.flush_clients();
        applied
    }

    #[doc(hidden)]
    pub fn restore_frame_batch_after_render_failure(&mut self, batch_id: CompositorFrameBatchId) {
        self.state
            .restore_frame_batch_after_render_failure(batch_id);
    }

    #[doc(hidden)]
    pub fn discard_frame_batch(
        &mut self,
        batch_id: CompositorFrameBatchId,
        reason: FrameBatchDiscardReason,
    ) {
        self.state.discard_frame_batch(batch_id, reason);
    }

    #[doc(hidden)]
    pub fn complete_frame_batch_after_safe_abandonment(
        &mut self,
        batch_id: CompositorFrameBatchId,
        reason: FrameBatchDiscardReason,
    ) {
        self.state
            .complete_frame_batch_after_safe_abandonment(batch_id, reason);
        let _ = self.display.flush_clients();
    }

    #[doc(hidden)]
    pub fn complete_presented_frame_batch(
        &mut self,
        frame_id: u64,
        batch_id: CompositorFrameBatchId,
        presentation: FramePresentation,
    ) {
        self.state
            .complete_presented_frame_batch(frame_id, batch_id, presentation);
        let _ = self.display.flush_clients();
    }

    #[doc(hidden)]
    pub fn complete_direct_presented_frame_batch(
        &mut self,
        frame_id: u64,
        batch_id: CompositorFrameBatchId,
        direct_surface_id: u32,
        presentation: FramePresentation,
    ) {
        self.state.complete_direct_presented_frame_batch(
            frame_id,
            batch_id,
            direct_surface_id,
            presentation,
        );
        let _ = self.display.flush_clients();
    }

    #[cfg(test)]
    pub(super) fn test_frame_batch_presentation_surface_ids(
        &self,
        batch_id: CompositorFrameBatchId,
    ) -> Vec<u32> {
        self.state
            .test_frame_batch_presentation_surface_ids(batch_id)
    }

    pub fn mark_prepared_frame_submitted(&mut self) {
        self.state.mark_prepared_frame_submitted();
    }

    pub fn finish_frame(&mut self) {
        self.state.capture_frame_callbacks_for_render();
        let Ok(presentation) = FramePresentation::software_now(self.state.presentation_clock)
        else {
            self.state.discard_all_pending_presentation_feedbacks();
            let batch_id = self
                .state
                .legacy_prepared_frame_batch
                .expect("software frame capture did not create a frame batch");
            self.state.complete_frame_batch_after_safe_abandonment(
                batch_id,
                FrameBatchDiscardReason::OutputDestroyed,
            );
            let _ = self.display.flush_clients();
            return;
        };
        if let Some(batch_id) = self.state.legacy_prepared_frame_batch {
            self.state.complete_rendered_frame_callbacks(batch_id);
        }
        self.finish_frame_with_presentation(presentation);
    }

    pub fn finish_frame_with_presentation(&mut self, presentation: FramePresentation) {
        if !self.state.has_submitted_frame_batch() {
            self.state.capture_frame_callbacks_for_render();
        }
        self.state
            .complete_pending_presentation_feedbacks(presentation);
        let _ = self.display.flush_clients();
    }

    #[doc(hidden)]
    pub fn buffer_release_metrics(&self) -> BufferReleaseMetrics {
        self.state.buffer_release_metrics()
    }

    #[doc(hidden)]
    pub fn frame_callback_metrics(&self) -> FrameCallbackMetrics {
        self.state.frame_callback_metrics()
    }

    pub fn verbose_trace_dropped_entries(&self) -> u64 {
        super::pacing::client_pacing_trace_dropped_entries()
            .saturating_add(super::pacing::commit_debug_trace_dropped_entries())
    }

    pub fn present_frame(&mut self) {
        self.prepare_frame();
        let _ = self.flush_pending_interactive_visual_state_for_render_admission(false);
        self.finish_frame();
    }

    pub fn tick(&mut self) -> Result<usize, CompositorError> {
        self.tick_with_outcome().map(|(accepted, _)| accepted)
    }

    /// Dispatch readable Wayland work and flush the resulting protocol output.
    ///
    /// This deliberately excludes surface-pacing progression. Native input
    /// and Wayland-readiness service this boundary independently; the boolean
    /// reports a pacing-readiness generation transition created while
    /// dispatching readable clients so the native runtime can service it
    /// explicitly in its pacing domain.
    #[doc(hidden)]
    pub fn dispatch_wayland_with_outcome(&mut self) -> Result<(usize, bool), CompositorError> {
        let pacing_generation_before = self.state.surface_pacing_readiness_generation();
        let mut accepted = 0;
        while let Some(stream) = self.socket.accept()? {
            let mut handle = self.display.handle();
            let client = handle.insert_client(
                stream,
                Arc::new(TyphonClientData {
                    disconnected_clients: self.disconnected_clients.clone(),
                    client_pids: self.client_pids.clone(),
                }),
            )?;
            if let Ok(credentials) = client.get_credentials(&handle)
                && let Ok(mut client_pids) = self.client_pids.lock()
            {
                client_pids.insert(client.id(), credentials.pid);
            }
            accepted += 1;
        }

        self.state.accepted_clients += accepted;
        self.state.poll_clipboard_bridge();
        self.state.begin_client_dispatch_cycle();
        let dispatch_result = self.display.dispatch_clients(&mut self.state);
        self.state.finish_client_dispatch_cycle();
        self.kill_pending_resource_exhaustion_clients();
        self.teardown_disconnected_clients();
        self.publish_astrea_toplevel_updates();
        self.state.clear_dead_active_clipboard_source();
        self.state.poll_clipboard_bridge();
        let pacing_readiness_changed =
            pacing_generation_before != self.state.surface_pacing_readiness_generation();
        self.flush_wayland_clients()?;
        dispatch_result?;
        Ok((accepted, pacing_readiness_changed))
    }

    pub fn tick_with_outcome(&mut self) -> Result<(usize, bool), CompositorError> {
        let (accepted, _) = self.dispatch_wayland_with_outcome()?;
        let pacing_visual_work =
            self.progress_surface_pacing(super::pacing::client_pacing_now_ns())?;
        Ok((accepted, pacing_visual_work))
    }

    fn teardown_disconnected_clients(&mut self) {
        let disconnected = self
            .disconnected_clients
            .lock()
            .map(|mut clients| std::mem::take(&mut *clients))
            .unwrap_or_default();
        for disconnected in disconnected {
            let summary = self
                .state
                .teardown_client_resources(&disconnected.client_id);
            let xwayland_identity =
                self.xwayland_global_data
                    .active
                    .lock()
                    .ok()
                    .and_then(|active| {
                        active
                            .as_ref()
                            .filter(|identity| identity.client_id == disconnected.client_id)
                            .cloned()
                    });
            if let Some(identity) = xwayland_identity {
                self.revoke_xwayland_generation(identity.generation);
                self.xwayland_disconnects.push(identity);
            }
            eprintln!(
                "oblivion-one compositor: client_disconnect client={:?} pid={} surfaces_removed={} visible_removed={} repaint_scheduled={}",
                disconnected.client_id,
                disconnected
                    .pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                summary.surfaces_removed,
                summary.renderables_removed,
                summary.repaint_scheduled
            );
        }
    }
}
