use std::{
    fmt, io,
    os::fd::{AsFd, BorrowedFd},
    sync::Arc,
};

use wayland_protocols::ext::data_control::v1::server::ext_data_control_manager_v1;
use wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_v1;
use wayland_protocols::wp::linux_drm_syncobj::v1::server::wp_linux_drm_syncobj_manager_v1;
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
use wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_shell_v1;
use wayland_server::{
    Display, DisplayHandle, ListeningSocket,
    protocol::{
        wl_compositor, wl_data_device_manager, wl_output, wl_seat, wl_shm, wl_subcompositor,
    },
};

use crate::astrea_shortcuts::server::astrea_shortcuts_manager_v1;
use crate::render_backend::egl_gles::EglGlesDmabufFeedback;
use crate::syncobj::DrmSyncobjDevice;
use crate::wayland_drm::server::wl_drm;

use super::{
    AcquireCommitId, AcquireWatchChange, ClientCursorRenderState, CompositorState,
    ExplicitSyncPoint, FramePresentation, InputProtocolCapabilities, OutputRect, PresentationClock,
    RenderGenerationCause, RenderableSurface, RendererProtocolCapabilities, ResizeFlowMetrics,
    SelectionProtocolCapabilities, SubsurfaceTransactionMetrics, color,
    input::{PointerConstraintBackendId, PointerConstraintBackendRequest, PointerMotionSample},
};

#[derive(Debug)]
pub struct OwnCompositorServer {
    pub(super) display: Display<CompositorState>,
    pub(super) socket: ListeningSocket,
    pub(super) socket_name: String,
    pub(super) state: CompositorState,
    gpu_buffer_protocols_enabled: bool,
}

impl Drop for OwnCompositorServer {
    fn drop(&mut self) {
        self.state.release_cached_resources_for_shutdown();
    }
}

impl OwnCompositorServer {
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
        let syncobj_device = DrmSyncobjDevice::open_available();
        register_minimum_globals(
            &display.handle(),
            syncobj_device.is_some(),
            gpu_buffers_enabled,
            input_capabilities,
            selection_capabilities,
            renderer_capabilities,
        );
        let socket = ListeningSocket::bind(&socket_name)
            .map_err(|error| CompositorError::Bind(error.to_string()))?;

        Ok(Self {
            display,
            socket,
            socket_name,
            state: CompositorState::new(syncobj_device),
            gpu_buffer_protocols_enabled: gpu_buffers_enabled,
        })
    }

    pub fn enable_gpu_buffer_protocols(&mut self) {
        if self.gpu_buffer_protocols_enabled {
            return;
        }
        register_gpu_buffer_globals(&self.display.handle(), self.state.syncobj_device.is_some());
        self.gpu_buffer_protocols_enabled = true;
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

    pub fn external_overlay_surface_ids(&self) -> Vec<u32> {
        self.state.external_overlay_surface_ids()
    }

    pub fn mark_render_damage_presented(&mut self) {
        self.state.mark_render_damage_presented();
    }

    pub fn client_cursor_render_state(&self) -> Option<ClientCursorRenderState<'_>> {
        self.state.client_cursor_render_state()
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

    pub fn send_pointer_button(&mut self, button: u32, pressed: bool) {
        self.state.send_pointer_button(button, pressed);
        let _ = self.display.flush_clients();
    }

    pub fn send_pointer_axis(&mut self, horizontal: f64, vertical: f64) {
        self.state.send_pointer_axis(horizontal, vertical);
        let _ = self.display.flush_clients();
    }

    pub fn take_pointer_constraint_backend_requests(
        &mut self,
    ) -> Vec<PointerConstraintBackendRequest> {
        self.state.take_pointer_constraint_backend_requests()
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

    pub fn begin_window_resize_at(&mut self, x: f64, y: f64) -> bool {
        let started = self.state.begin_window_resize_at(x, y);
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

    pub fn window_interaction_active(&self) -> bool {
        self.state.window_interaction_active()
    }

    pub fn emit_astrea_shortcut_pressed(
        &mut self,
        namespace: &str,
        name: &str,
        timestamp: u32,
    ) -> usize {
        let dispatched = self
            .state
            .emit_astrea_shortcut_pressed(namespace, name, timestamp);
        let _ = self.display.flush_clients();
        dispatched
    }

    pub fn authorize_astrea_shell_pid(&mut self, pid: u32) {
        self.state.authorize_astrea_shell_pid(pid);
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

    pub fn finish_frame(&mut self) {
        let Ok(presentation) = FramePresentation::software_now(self.state.presentation_clock)
        else {
            self.state.discard_all_pending_presentation_feedbacks();
            self.state.release_pending_buffers();
            self.state.complete_pending_frame_callbacks();
            let _ = self.display.flush_clients();
            return;
        };
        self.finish_frame_with_presentation(presentation);
    }

    pub fn finish_frame_with_presentation(&mut self, presentation: FramePresentation) {
        self.state.release_pending_buffers();
        self.state.complete_pending_frame_callbacks();
        self.state
            .complete_pending_presentation_feedbacks(presentation);
        let _ = self.display.flush_clients();
    }

    pub fn present_frame(&mut self) {
        self.prepare_frame();
        self.finish_frame();
    }

    pub fn tick(&mut self) -> Result<usize, CompositorError> {
        let mut accepted = 0;
        while let Some(stream) = self.socket.accept()? {
            let mut handle = self.display.handle();
            handle.insert_client(stream, Arc::new(()))?;
            accepted += 1;
        }

        self.state.accepted_clients += accepted;
        self.state.poll_clipboard_bridge();
        self.state.begin_client_dispatch_cycle();
        self.display.dispatch_clients(&mut self.state)?;
        self.state.finish_client_dispatch_cycle();
        self.state.clear_dead_active_clipboard_source();
        self.state.poll_clipboard_bridge();
        self.display.flush_clients()?;
        Ok(accepted)
    }
}

fn register_minimum_globals(
    display: &DisplayHandle,
    syncobj_available: bool,
    gpu_buffers_enabled: bool,
    input_capabilities: InputProtocolCapabilities,
    selection_capabilities: SelectionProtocolCapabilities,
    renderer_capabilities: RendererProtocolCapabilities,
) {
    display.create_global::<CompositorState, wl_compositor::WlCompositor, _>(6, ());
    display.create_global::<CompositorState, wl_subcompositor::WlSubcompositor, _>(1, ());
    if selection_capabilities.clipboard {
        display.create_global::<CompositorState, wl_data_device_manager::WlDataDeviceManager, _>(
            3,
            (),
        );
    }
    display.create_global::<CompositorState, wl_shm::WlShm, _>(2, ());
    display.create_global::<CompositorState, wp_viewporter::WpViewporter, _>(1, ());
    display.create_global::<
        CompositorState,
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        _,
    >(1, ());
    display.create_global::<CompositorState, wp_presentation::WpPresentation, _>(2, ());
    display.create_global::<CompositorState, zwlr_layer_shell_v1::ZwlrLayerShellV1, _>(4, ());
    if renderer_capabilities.color_management {
        color::register_color_management_global(display);
    }
    if input_capabilities.relative_pointer {
        display.create_global::<
            CompositorState,
            zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
            _,
        >(1, ());
    }
    if input_capabilities.pointer_constraints {
        display.create_global::<
            CompositorState,
            zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
            _,
        >(1, ());
    }
    if input_capabilities.pointer_warp {
        display.create_global::<CompositorState, wp_pointer_warp_v1::WpPointerWarpV1, _>(1, ());
    }
    if input_capabilities.idle_inhibit {
        display.create_global::<
            CompositorState,
            zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1,
            _,
        >(1, ());
    }
    if selection_capabilities.primary_selection {
        display.create_global::<
            CompositorState,
            zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1,
            _,
        >(1, ());
    }
    if selection_capabilities.data_control {
        display.create_global::<
            CompositorState,
            ext_data_control_manager_v1::ExtDataControlManagerV1,
            _,
        >(1, ());
    }
    display
        .create_global::<CompositorState, zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, _>(
            1,
            (),
        );
    if gpu_buffers_enabled {
        register_gpu_buffer_globals(display, syncobj_available);
    }
    display.create_global::<CompositorState, xdg_activation_v1::XdgActivationV1, _>(1, ());
    display
        .create_global::<CompositorState, astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1, _>(
            1,
            (),
        );
    display.create_global::<CompositorState, xdg_wm_base::XdgWmBase, _>(6, ());
    display.create_global::<CompositorState, wl_output::WlOutput, _>(4, ());
    display.create_global::<CompositorState, wl_seat::WlSeat, _>(7, ());
}

fn register_gpu_buffer_globals(display: &DisplayHandle, syncobj_available: bool) {
    display.create_global::<CompositorState, zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, _>(4, ());
    if syncobj_available {
        display.create_global::<
            CompositorState,
            wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1,
            _,
        >(1, ());
    }
    display.create_global::<CompositorState, wl_drm::WlDrm, _>(2, ());
}

#[derive(Debug)]
pub enum CompositorError {
    DisplayInit(String),
    Bind(String),
    Io(io::Error),
}

impl fmt::Display for CompositorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DisplayInit(error) => {
                write!(formatter, "failed to initialize Wayland display: {error}")
            }
            Self::Bind(error) => write!(formatter, "failed to bind Wayland socket: {error}"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for CompositorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::DisplayInit(_) | Self::Bind(_) => None,
        }
    }
}

impl From<io::Error> for CompositorError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
