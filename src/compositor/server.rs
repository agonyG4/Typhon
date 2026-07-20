use std::{
    borrow::Cow,
    collections::HashMap,
    io,
    os::fd::{AsFd, BorrowedFd},
    sync::{Arc, Mutex},
};

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

use super::gpu_protocol_capabilities::GpuProtocolCapabilities;
use super::protocols::versions;
use crate::astrea_shell_control::server::astrea_shell_control_manager_v1;
use crate::astrea_shortcuts::server::astrea_shortcuts_manager_v1;
#[cfg(test)]
use crate::render_backend::buffer::BufferId;
use crate::render_backend::egl_gles::EglGlesDmabufFeedback;
use crate::syncobj::DrmSyncobjDevice;
use crate::xwayland::xwm::{RESIZE_SYNC_TIMEOUT_NS, XwmCommand, XwmEvent};
use crate::xwayland::{X11WindowHandle, XwaylandAssociationEvent, XwaylandGeneration};
#[path = "server_gpu_globals.rs"]
mod server_gpu_globals;
use super::{
    AcquireCommitId, AcquireWatchChange, AstreaShortcutPhase, BufferReleaseMetrics,
    ClientCursorRenderState, CompositorError, CompositorFrameBatchId, CompositorState,
    CoreComplianceMetrics, DirectScanoutSceneCandidate, DirectScanoutSceneRejection,
    ExplicitSyncPoint, FrameBatchDiscardReason, FramePresentation, FullscreenRenderPlanMetrics,
    InputProtocolCapabilities, OutputRect, PendingProcessLaunch, PointerAxisFrame,
    PresentationClock, RenderGenerationCause, RenderableSurface, RendererProtocolCapabilities,
    ResizeFlowMetrics, SelectionProtocolCapabilities, SubsurfaceTransactionMetrics,
    SurfaceDamagePresentation, ToplevelMode, WindowInteractionDebugSnapshot,
    WindowInteractionEndReason, color,
    input::{PointerConstraintBackendId, PointerConstraintBackendRequest, PointerMotionSample},
};
#[derive(Debug)]
pub struct OwnCompositorServer {
    pub(super) display: Display<CompositorState>,
    pub(super) socket: ListeningSocket,
    pub(super) socket_name: String,
    pub(super) state: CompositorState,
    disconnected_clients: Arc<Mutex<Vec<DisconnectedClient>>>,
    client_pids: Arc<Mutex<HashMap<ClientId, i32>>>,
    xwayland_global_data: XwaylandShellGlobalData,
    xwayland_disconnects: Vec<XwaylandClientIdentity>,
    gpu_buffer_protocols_enabled: bool,
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
        self.finish_commit_debug_for_shutdown();
    }
}

impl OwnCompositorServer {
    pub fn core_compliance_metrics(&self) -> CoreComplianceMetrics {
        self.state.compliance_metrics
    }

    pub fn finish_commit_debug_for_shutdown(&mut self) {
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
        )
    }

    fn bind_with_gpu_buffers_and_capabilities(
        socket_name: impl Into<String>,
        gpu_buffers_enabled: bool,
        input_capabilities: InputProtocolCapabilities,
        selection_capabilities: SelectionProtocolCapabilities,
        renderer_capabilities: RendererProtocolCapabilities,
    ) -> Result<Self, CompositorError> {
        let socket_name = socket_name.into();
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
        register_minimum_globals(
            &display.handle(),
            &gpu_capabilities,
            gpu_buffers_enabled,
            input_capabilities,
            selection_capabilities,
            renderer_capabilities,
            xwayland_global_data.clone(),
        );
        let socket = ListeningSocket::bind(&socket_name)
            .map_err(|error| CompositorError::Bind(error.to_string()))?;

        let mut state = CompositorState::new(syncobj_device);
        state.set_gpu_protocol_capabilities(gpu_capabilities.clone());
        state.set_typhon_socket_name(socket_name.clone());
        let disconnected_clients = Arc::new(Mutex::new(Vec::new()));
        let client_pids = Arc::new(Mutex::new(HashMap::new()));
        Ok(Self {
            display,
            socket,
            socket_name,
            state,
            disconnected_clients,
            client_pids,
            xwayland_global_data,
            xwayland_disconnects: Vec::new(),
            gpu_buffer_protocols_enabled: gpu_buffers_enabled
                && gpu_capabilities.any_global_enabled(),
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

    pub fn insert_xwayland_client(
        &mut self,
        stream: std::os::unix::net::UnixStream,
        generation: XwaylandGeneration,
    ) -> io::Result<XwaylandClientIdentity> {
        let mut handle = self.display.handle();
        let client = handle.insert_client(
            stream,
            Arc::new(TyphonClientData {
                disconnected_clients: self.disconnected_clients.clone(),
                client_pids: self.client_pids.clone(),
            }),
        )?;
        let identity = XwaylandClientIdentity {
            client_id: client.id(),
            generation,
        };
        if let Ok(mut active) = self.xwayland_global_data.active.lock() {
            *active = Some(identity.clone());
        }
        self.state.xwayland.client_identity = Some(identity.clone());
        Ok(identity)
    }

    pub fn revoke_xwayland_generation(&mut self, generation: XwaylandGeneration) {
        let revoke = self
            .xwayland_global_data
            .active
            .lock()
            .ok()
            .is_some_and(|active| {
                active
                    .as_ref()
                    .is_some_and(|identity| identity.generation == generation)
            });
        if revoke {
            if let Ok(mut active) = self.xwayland_global_data.active.lock() {
                *active = None;
            }
            if self
                .state
                .xwayland
                .client_identity
                .as_ref()
                .is_some_and(|identity| identity.generation == generation)
            {
                self.state.xwayland.client_identity = None;
            }
            self.state.clear_xwayland_generation(generation);
        }
    }

    pub fn take_xwayland_shell_bind_events(&mut self) -> Vec<XwaylandClientIdentity> {
        self.xwayland_global_data
            .bind_events
            .lock()
            .map(|mut events| std::mem::take(&mut *events))
            .unwrap_or_default()
    }

    pub fn take_xwayland_client_disconnect_events(&mut self) -> Vec<XwaylandClientIdentity> {
        std::mem::take(&mut self.xwayland_disconnects)
    }

    pub fn take_xwayland_association_events(&mut self) -> Vec<XwaylandAssociationEvent> {
        self.state.take_xwayland_association_events()
    }

    pub fn take_xwayland_buffer_ready_events(&mut self) -> Vec<(XwaylandGeneration, u32)> {
        self.state.take_xwayland_buffer_ready_events()
    }

    #[cfg(test)]
    pub(crate) fn current_surface_buffer_id(&self, surface_id: u32) -> Option<BufferId> {
        self.state
            .current_surface_buffers
            .get(&surface_id)
            .map(|pending| pending.data.buffer_id())
    }

    pub fn take_xwayland_backend_commands(&mut self, now_ns: u64) -> Vec<XwmCommand> {
        self.state
            .take_backend_commands()
            .into_iter()
            .filter_map(|command| match command {
                crate::compositor::window_backend::WindowBackendCommand::Configure {
                    window,
                    geometry,
                    mode: _,
                    resizing,
                } => {
                    let handle = match self.state.window(window)?.backend {
                        super::WindowBackend::X11(handle) => handle,
                        super::WindowBackend::Xdg(_) => return None,
                    };
                    let x11_geometry = crate::xwayland::xwm::X11Geometry {
                        x: geometry.placement.local_x,
                        y: geometry.placement.local_y,
                        width: geometry.width,
                        height: geometry.height,
                    };
                    if resizing {
                        Some(XwmCommand::BeginResizeSync {
                            window: handle,
                            geometry: x11_geometry,
                            counter_value: 0,
                            deadline_ns: now_ns.saturating_add(RESIZE_SYNC_TIMEOUT_NS),
                            final_pending: false,
                        })
                    } else {
                        Some(XwmCommand::Configure {
                            window: handle,
                            geometry: x11_geometry,
                            fields: crate::xwayland::xwm::X11ConfigureFlags::all(),
                            border_width: 0,
                        })
                    }
                }
                crate::compositor::window_backend::WindowBackendCommand::Close { window } => {
                    let handle = match self.state.window(window)?.backend {
                        super::WindowBackend::X11(handle) => handle,
                        super::WindowBackend::Xdg(_) => return None,
                    };
                    Some(XwmCommand::Close(handle))
                }
                crate::compositor::window_backend::WindowBackendCommand::SetActivated {
                    window,
                    activated,
                } => {
                    let handle = match self.state.window(window)?.backend {
                        super::WindowBackend::X11(handle) => handle,
                        super::WindowBackend::Xdg(_) => return None,
                    };
                    Some(XwmCommand::Focus {
                        window: activated.then_some(handle),
                        timestamp: 0,
                    })
                }
                crate::compositor::window_backend::WindowBackendCommand::PublishState {
                    window,
                    mode,
                    minimized,
                    activated,
                } => {
                    let handle = match self.state.window(window)?.backend {
                        super::WindowBackend::X11(handle) => handle,
                        super::WindowBackend::Xdg(_) => return None,
                    };
                    Some(XwmCommand::SetState {
                        window: handle,
                        state: crate::xwayland::xwm::X11PublishedState {
                            fullscreen: mode == ToplevelMode::Fullscreen,
                            maximized: mode == ToplevelMode::Maximized,
                            hidden: minimized,
                            activated,
                        },
                    })
                }
            })
            .collect()
    }

    pub fn apply_xwayland_window_event(&mut self, event: XwmEvent) -> Vec<XwmCommand> {
        match event {
            XwmEvent::WindowMapRequested(handle) => vec![XwmCommand::Map(handle)],
            XwmEvent::WindowReady(snapshot) => {
                let surface_id = snapshot.surface_id;
                match self.state.insert_x11_window(snapshot) {
                    Ok(_) => {
                        let published = self
                            .state
                            .adopt_current_xwayland_surface_content(surface_id);
                        eprintln!(
                            "oblivion-one compositor: event=xwayland_window_admitted surface_id={surface_id} retained_buffer={published} published={published}"
                        );
                        vec![self.sync_xwayland_client_lists()]
                    }
                    Err(error) => {
                        eprintln!(
                            "oblivion-one compositor: event=xwayland_window_admission_failed surface_id={surface_id} error={error:?}"
                        );
                        Vec::new()
                    }
                }
            }
            XwmEvent::WindowDestroyed(handle) => {
                if self.remove_x11_desktop_window(handle) {
                    vec![self.sync_xwayland_client_lists()]
                } else {
                    Vec::new()
                }
            }
            XwmEvent::WindowWithdrawn(handle) => {
                if self.remove_x11_desktop_window(handle) {
                    vec![self.sync_xwayland_client_lists()]
                } else {
                    Vec::new()
                }
            }
            XwmEvent::MetadataChanged { window, delta } => {
                let prior_id = self.state.window_id_for_x11_handle(window);
                let prior_focused =
                    prior_id.is_some_and(|id| self.state.focused_window_id == Some(id));
                let publish_lists = matches!(
                    &delta,
                    crate::xwayland::xwm::X11MetadataDelta::TransientFor(_)
                        | crate::xwayland::xwm::X11MetadataDelta::WindowType(_)
                );
                self.state.apply_x11_metadata_delta(window, delta);
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
                publish_lists
                    .then(|| self.sync_xwayland_client_lists())
                    .into_iter()
                    .collect()
            }
            XwmEvent::ConfigureRequested { window, request } => {
                if self.state.x11_resize_active(window) {
                    let mut commands = Vec::with_capacity(2);
                    if let Some(mode) = request.stack_mode {
                        commands.push(XwmCommand::Stack {
                            window,
                            sibling: request.sibling,
                            mode,
                        });
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
                    return commands;
                }
                let constraints = self
                    .state
                    .window_id_for_x11_handle(window)
                    .and_then(|id| self.state.window(id))
                    .map(|window| window.constraints)
                    .unwrap_or_default();
                let geometry = crate::xwayland::xwm::icccm::apply_configure_request(
                    request.requested,
                    request.requested,
                    request.fields,
                    constraints,
                );
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
                    border_width: request.border_width,
                }];
                if let Some(mode) = request.stack_mode {
                    commands.push(XwmCommand::Stack {
                        window,
                        sibling: request.sibling,
                        mode,
                    });
                }
                commands
            }
            XwmEvent::ConfigureNotify { window, geometry } => {
                let _ = self.state.reconcile_x11_configure_notify(window, geometry);
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
                let mut commands = Vec::with_capacity(2);
                if state.hidden != was_hidden {
                    commands.push(if state.hidden {
                        XwmCommand::Unmap(window)
                    } else {
                        XwmCommand::Map(window)
                    });
                }
                commands.push(XwmCommand::SetState { window, state });
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
                    .is_some_and(|window_id| self.state.focus_desktop_window(window_id))
                {
                    vec![
                        XwmCommand::Focus {
                            window: Some(window),
                            timestamp,
                        },
                        XwmCommand::Raise(window),
                        self.sync_xwayland_client_lists(),
                    ]
                } else {
                    Vec::new()
                }
            }
            XwmEvent::ResizeSyncAcked { window, .. } => {
                vec![XwmCommand::SetAllowCommits {
                    window,
                    allowed: true,
                }]
            }
            XwmEvent::ResizeSyncPresented(window) => {
                let _ = self.state.finalize_x11_resize(window);
                vec![XwmCommand::CompleteResizeSync(window)]
            }
            XwmEvent::ResizeSyncPresentedIntermediate(window) => {
                vec![XwmCommand::CompleteResizeSync(window)]
            }
            XwmEvent::ResizeSyncImmediate(window) => {
                let _ = self.state.finalize_x11_resize(window);
                Vec::new()
            }
            XwmEvent::ResizeSyncTimedOut(window) => {
                let _ = self.state.finalize_x11_resize_if_interaction_ended(window);
                Vec::new()
            }
            XwmEvent::ResizeSyncTimedOutWithFollowup(_) => Vec::new(),
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
    fn remove_x11_desktop_window(&mut self, handle: X11WindowHandle) -> bool {
        let Some(window_id) = self.state.window_id_for_x11_handle(handle) else {
            return false;
        };
        let was_focused = self.state.focused_window_id == Some(window_id);
        let parent_id = self
            .state
            .window(window_id)
            .and_then(|window| window.relationships.transient_for);
        let removed = self.state.remove_desktop_window(window_id).is_some();
        if removed && was_focused {
            self.state.focused_window_id = None;
            self.state.focused_surface = None;
            self.state.clear_keyboard_focus();
            if let Some(parent_id) = parent_id {
                if !self.state.focus_desktop_window(parent_id) {
                    let _ = self.state.focus_topmost_renderable_toplevel();
                }
            } else {
                let _ = self.state.focus_topmost_renderable_toplevel();
            }
        }
        removed
    }
    fn sync_xwayland_client_lists(&self) -> XwmCommand {
        let (client_list, stacking) = self.state.x11_client_lists();
        XwmCommand::SyncClientLists {
            client_list,
            stacking,
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

    pub fn external_overlay_surface_ids(&self) -> Vec<u32> {
        self.state.external_overlay_surface_ids()
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

    pub fn client_cursor_request_active(&self) -> bool {
        self.state.active_client_cursor.is_some()
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

    pub fn fullscreen_render_plan_metrics(&self) -> FullscreenRenderPlanMetrics {
        self.state.fullscreen_render_plan_metrics()
    }

    pub fn direct_scanout_scene_candidate(
        &self,
    ) -> Result<DirectScanoutSceneCandidate, DirectScanoutSceneRejection> {
        self.state.direct_scanout_scene_candidate()
    }

    pub fn has_pending_frame_callbacks(&self) -> bool {
        self.state.has_pending_frame_callbacks()
    }

    pub fn has_only_pending_surface_frame_callbacks(&self) -> bool {
        self.state.has_only_pending_surface_frame_callbacks()
    }

    pub fn has_pending_frame_prepare_work(&self) -> bool {
        self.state.has_pending_frame_prepare_work()
    }

    pub fn has_pending_explicit_sync_work(&self) -> bool {
        self.state.has_pending_explicit_sync_work()
    }

    pub fn has_pending_frame_work(&self) -> bool {
        self.state.has_pending_frame_work()
    }

    pub fn set_dmabuf_feedback(
        &mut self,
        feedback: EglGlesDmabufFeedback,
        main_device: Option<u64>,
        main_device_path: Option<String>,
    ) {
        self.state
            .set_dmabuf_feedback(feedback, main_device, main_device_path);
        let _ = self.display.flush_clients();
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
        self.state.presentation_clock = clock;
    }

    pub fn presentation_clock(&self) -> PresentationClock {
        self.state.presentation_clock
    }

    pub fn send_keyboard_key(&mut self, key: u32, pressed: bool) {
        self.state.send_keyboard_key(key, pressed);
        let _ = self.display.flush_clients();
    }

    pub fn send_pointer_motion(&mut self, x: f64, y: f64) {
        self.state.send_pointer_motion(x, y);
        let _ = self.display.flush_clients();
    }

    pub fn update_pointer_position_without_client_dispatch(&mut self, x: f64, y: f64) -> bool {
        self.state
            .update_pointer_position_without_client_dispatch(x, y)
    }

    pub fn send_pointer_motion_sample(&mut self, sample: PointerMotionSample) {
        self.state.send_pointer_motion_sample(sample);
        let _ = self.display.flush_clients();
    }

    pub fn send_window_interaction_pointer_motion(
        &mut self,
        timestamp_usec: u64,
        x: f64,
        y: f64,
    ) -> usize {
        let dispatched = self
            .state
            .send_window_interaction_pointer_motion(timestamp_usec, x, y);
        let _ = self.display.flush_clients();
        dispatched
    }

    pub fn send_pointer_button(&mut self, button: u32, pressed: bool) {
        self.state.send_pointer_button(button, pressed);
        let _ = self.display.flush_clients();
    }

    pub fn send_pointer_axis(&mut self, horizontal: f64, vertical: f64) {
        self.state.send_pointer_axis(horizontal, vertical);
        let _ = self.display.flush_clients();
    }

    pub fn send_pointer_axis_frame(&mut self, frame: PointerAxisFrame) {
        self.state.send_pointer_axis_frame(frame);
        let _ = self.display.flush_clients();
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
        let _ = self.display.flush_clients();
    }

    pub fn pointer_constraint_backend_activation_current(
        &self,
        id: PointerConstraintBackendId,
    ) -> bool {
        self.state.pointer_constraint_backend_activation_current(id)
    }

    pub fn pointer_constraint_backend_deactivated(&mut self, id: PointerConstraintBackendId) {
        self.state.pointer_constraint_backend_deactivated(id);
        let _ = self.display.flush_clients();
    }

    pub fn pointer_constraint_backend_failed(
        &mut self,
        id: PointerConstraintBackendId,
        reason: impl AsRef<str>,
    ) {
        self.state
            .pointer_constraint_backend_failed(id, reason.as_ref());
        let _ = self.display.flush_clients();
    }

    pub fn begin_window_move_at(&mut self, x: f64, y: f64) -> bool {
        let started = self.state.begin_window_move_at(x, y);
        let _ = self.display.flush_clients();
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
        let _ = self.display.flush_clients();
        started
    }

    pub fn begin_window_resize_at(&mut self, x: f64, y: f64) -> bool {
        let started = self.state.begin_window_resize_at(x, y);
        let _ = self.display.flush_clients();
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
        let _ = self.display.flush_clients();
        started
    }

    pub fn begin_window_frame_action_at(&mut self, x: f64, y: f64) -> bool {
        let started = self.state.begin_window_frame_action_at(x, y);
        let _ = self.display.flush_clients();
        started
    }

    pub fn update_window_interaction(&mut self, x: f64, y: f64) -> bool {
        let updated = self.state.update_window_interaction(x, y);
        let _ = self.display.flush_clients();
        updated
    }

    pub fn end_window_interaction(&mut self) {
        self.state.end_window_interaction();
        let _ = self.display.flush_clients();
    }

    pub fn cancel_window_interaction_for_session_suspend(&mut self) -> bool {
        let cancelled = self
            .state
            .cancel_window_interaction(WindowInteractionEndReason::SessionSuspended);
        let _ = self.display.flush_clients();
        cancelled
    }

    pub fn end_window_interaction_for_button(&mut self, button: u32) -> bool {
        let ended = self.state.end_window_interaction_for_button(button);
        let _ = self.display.flush_clients();
        ended
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
        let _ = self.display.flush_clients();
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
        let _ = self.display.flush_clients();
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

    pub fn minimize_focused_window(&mut self) -> bool {
        let minimized = self.state.minimize_focused_window();
        let _ = self.display.flush_clients();
        minimized
    }

    pub fn close_focused_window(&mut self) -> bool {
        let closed = self.state.close_focused_window();
        let _ = self.display.flush_clients();
        closed
    }

    pub fn restore_next_minimized_window(&mut self) -> bool {
        let restored = self.state.restore_next_minimized_window();
        let _ = self.display.flush_clients();
        restored
    }

    pub fn activate_window(&mut self, surface_id: u32) -> bool {
        let activated = self.state.activate_root_window(surface_id);
        let _ = self.display.flush_clients();
        activated
    }

    pub fn toggle_maximize_focused_window(&mut self) -> bool {
        let changed = self.state.toggle_maximize_focused_window();
        let _ = self.display.flush_clients();
        changed
    }

    pub fn toggle_fullscreen_focused_window(&mut self) -> bool {
        let changed = self.state.toggle_fullscreen_focused_window();
        let _ = self.display.flush_clients();
        changed
    }

    pub fn prepare_frame(&mut self) {
        self.state.commit_ready_explicit_sync_buffers();
        color::flush_pending_color_info(&mut self.state);
        self.state.apply_pending_interactive_resize_update();
        self.state.flush_pending_resize_configure();
        let _ = self.display.flush_clients();
    }

    pub fn capture_frame_callbacks_for_render(&mut self) {
        self.state.capture_frame_callbacks_for_render();
    }

    #[doc(hidden)]
    pub fn take_frame_batch_for_render(&mut self, frame_id: u64) -> CompositorFrameBatchId {
        self.state.take_frame_batch_for_render(frame_id)
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

    pub fn verbose_trace_dropped_entries(&self) -> u64 {
        super::pacing::client_pacing_trace_dropped_entries()
            .saturating_add(super::pacing::commit_debug_trace_dropped_entries())
    }

    pub fn present_frame(&mut self) {
        self.prepare_frame();
        self.finish_frame();
    }

    pub fn tick(&mut self) -> Result<usize, CompositorError> {
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
        self.teardown_disconnected_clients();
        self.state.clear_dead_active_clipboard_source();
        self.state.poll_clipboard_bridge();
        self.display.flush_clients()?;
        dispatch_result?;
        Ok(accepted)
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

fn register_minimum_globals(
    display: &DisplayHandle,
    gpu_capabilities: &GpuProtocolCapabilities,
    gpu_buffers_enabled: bool,
    input_capabilities: InputProtocolCapabilities,
    selection_capabilities: SelectionProtocolCapabilities,
    renderer_capabilities: RendererProtocolCapabilities,
    xwayland_global_data: XwaylandShellGlobalData,
) {
    debug_assert!(
        versions::all_globals()
            .iter()
            .all(|global| global.version > 0 && !global.interface.is_empty())
    );
    display.create_global::<CompositorState, wl_compositor::WlCompositor, _>(
        versions::WL_COMPOSITOR,
        (),
    );
    display.create_global::<CompositorState, wl_subcompositor::WlSubcompositor, _>(
        versions::WL_SUBCOMPOSITOR,
        (),
    );
    if selection_capabilities.clipboard {
        display.create_global::<CompositorState, wl_data_device_manager::WlDataDeviceManager, _>(
            versions::WL_DATA_DEVICE_MANAGER,
            (),
        );
    }
    display.create_global::<CompositorState, wl_shm::WlShm, _>(versions::WL_SHM, ());
    display.create_global::<CompositorState, wp_viewporter::WpViewporter, _>(
        versions::WP_VIEWPORTER,
        (),
    );
    display.create_global::<
        CompositorState,
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        _,
    >(versions::WP_FRACTIONAL_SCALE_MANAGER_V1, ());
    display.create_global::<CompositorState, wp_presentation::WpPresentation, _>(
        versions::WP_PRESENTATION,
        (),
    );
    display.create_global::<CompositorState, zwlr_layer_shell_v1::ZwlrLayerShellV1, _>(
        versions::ZWLR_LAYER_SHELL_V1,
        (),
    );
    if renderer_capabilities.color_management {
        color::register_color_management_global(display);
    }
    if input_capabilities.relative_pointer {
        display.create_global::<
            CompositorState,
            zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
            _,
        >(versions::ZWP_RELATIVE_POINTER_MANAGER_V1, ());
    }
    if input_capabilities.pointer_constraints {
        display.create_global::<
            CompositorState,
            zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
            _,
        >(versions::ZWP_POINTER_CONSTRAINTS_V1, ());
    }
    if input_capabilities.pointer_warp {
        display.create_global::<CompositorState, wp_pointer_warp_v1::WpPointerWarpV1, _>(
            versions::WP_POINTER_WARP_V1,
            (),
        );
    }
    if input_capabilities.idle_inhibit {
        display.create_global::<
            CompositorState,
            zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1,
            _,
        >(versions::ZWP_IDLE_INHIBIT_MANAGER_V1, ());
    }
    if selection_capabilities.primary_selection {
        display.create_global::<
            CompositorState,
            zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1,
            _,
        >(versions::ZWP_PRIMARY_SELECTION_DEVICE_MANAGER_V1, ());
    }
    if selection_capabilities.data_control {
        display.create_global::<
            CompositorState,
            ext_data_control_manager_v1::ExtDataControlManagerV1,
            _,
        >(versions::EXT_DATA_CONTROL_MANAGER_V1, ());
    }
    display
        .create_global::<CompositorState, zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, _>(
            versions::ZXDG_DECORATION_MANAGER_V1,
            (),
        );
    if gpu_buffers_enabled {
        server_gpu_globals::register_gpu_buffer_globals(display, gpu_capabilities);
    }
    display.create_global::<CompositorState, xdg_activation_v1::XdgActivationV1, _>(
        versions::XDG_ACTIVATION_V1,
        (),
    );
    display
        .create_global::<CompositorState, astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1, _>(
            versions::ASTREA_SHORTCUTS_MANAGER_V1,
            (),
        );
    display.create_global::<
        CompositorState,
        astrea_shell_control_manager_v1::AstreaShellControlManagerV1,
        _,
    >(versions::ASTREA_SHELL_CONTROL_MANAGER_V1, ());
    display.create_global::<CompositorState, xdg_wm_base::XdgWmBase, _>(versions::XDG_WM_BASE, ());
    display.create_global::<CompositorState, wl_output::WlOutput, _>(versions::WL_OUTPUT, ());
    display.create_global::<CompositorState, wl_seat::WlSeat, _>(versions::WL_SEAT, ());
    display.create_global::<CompositorState, xwayland_shell_v1::XwaylandShellV1, _>(
        versions::XWAYLAND_SHELL_V1,
        xwayland_global_data,
    );
}
