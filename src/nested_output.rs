use std::{
    error::Error,
    io,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use crate::nested_renderer::{
    NestedOutputRenderer, NestedSceneDrawRequest, OutputRendererPreference,
};
use oblivion_one::{
    compositor::{
        DesktopSceneRenderer, DesktopVisualState, OutputPosition, OwnCompositorServer,
        PointerConstraintBackendId, PointerConstraintBackendRequest, PointerMotionSample,
        RelativePointerMotion, ShellOverlayRenderer, ShellOverlayState, ShellTopbarModel,
        SpotlightModel, dock_item_at,
    },
    spawn_compositor_app,
};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalPosition, LogicalSize},
    event::{DeviceEvent, ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, KeyCode, PhysicalKey},
    monitor::MonitorHandle,
    window::{CursorGrabMode, Window, WindowAttributes, WindowId},
};

type OutputResult<T> = Result<T, Box<dyn Error>>;

const NESTED_OUTPUT_HOST_CURSOR: bool = true;
const IDLE_WAKEUP_INTERVAL: Duration = Duration::from_millis(80);
const INPUT_RESPONSE_WAKEUP_INTERVAL: Duration = Duration::from_millis(1);
const INPUT_RESPONSE_FAST_PATH_DURATION: Duration = Duration::from_millis(48);
const REDRAW_REQUEST_RETRY_INTERVAL: Duration = Duration::from_millis(48);
const WAYLAND_SCROLL_LINE_DISTANCE: f64 = 15.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NestedOutputConfig {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
}

impl Default for NestedOutputConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 800,
            refresh_hz: 60,
        }
    }
}

pub fn run(
    server: OwnCompositorServer,
    renderer_preference: OutputRendererPreference,
    config: NestedOutputConfig,
    app_command: Vec<String>,
) -> OutputResult<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = NestedOutputApp::new(server, renderer_preference, config, app_command);
    event_loop.run_app(&mut app)?;
    app.shutdown_output();
    if let Some(error) = app.take_fatal_error() {
        return Err(io::Error::other(error).into());
    }
    println!("{}", app.shutdown_reason().log_message());
    Ok(())
}

struct NestedOutputApp {
    server: OwnCompositorServer,
    window: Option<Arc<Window>>,
    output_renderer: Option<NestedOutputRenderer>,
    renderer: DesktopSceneRenderer,
    shell_overlay_renderer: ShellOverlayRenderer,
    renderer_preference: OutputRendererPreference,
    config: NestedOutputConfig,
    active_wakeup_interval: Duration,
    app_command: Vec<String>,
    app_launched: bool,
    spotlight: SpotlightModel,
    shell_generation: u64,
    last_render_generation: u64,
    last_toplevel_count: usize,
    last_popup_count: usize,
    last_surface_count: usize,
    cursor_x: i32,
    cursor_y: i32,
    cursor_physical_x: i32,
    cursor_physical_y: i32,
    alt_pressed: bool,
    ctrl_pressed: bool,
    super_pressed: bool,
    forwarded_control_keys: Vec<KeyCode>,
    suppressed_window_shortcut_keys: Vec<KeyCode>,
    window_interaction_active: bool,
    redraw_pending: bool,
    redraw_requested_at: Option<Instant>,
    input_response_until: Option<Instant>,
    perf: NestedPerfCounters,
    host_monitor_refresh_millihz: Option<u32>,
    active_pointer_constraint: Option<NestedPointerConstraint>,
    pending_host_cursor_warp: Option<NestedPendingCursorWarp>,
    host_cursor_client_visible: bool,
    host_window_focused: bool,
    input_clock_start: Instant,
    fatal_error: Option<String>,
    shutdown_reason: ShutdownReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NestedPointerConstraint {
    id: PointerConstraintBackendId,
    mode: NestedPointerConstraintMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NestedPendingCursorWarp {
    physical_x: i32,
    physical_y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NestedPointerConstraintMode {
    Locked,
    Confined,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct NestedPerfCounters {
    redraw_requests: u64,
    presented_frames: u64,
    redraw_coalesced: u64,
    idle_wakeups: u64,
    active_wakeups: u64,
    last_log_at: Option<Instant>,
}

impl NestedOutputApp {
    fn new(
        server: OwnCompositorServer,
        renderer_preference: OutputRendererPreference,
        config: NestedOutputConfig,
        app_command: Vec<String>,
    ) -> Self {
        let (cursor_x, cursor_y) = nested_output_initial_cursor(config);
        Self {
            server,
            window: None,
            output_renderer: None,
            renderer: DesktopSceneRenderer::default(),
            shell_overlay_renderer: ShellOverlayRenderer::default(),
            renderer_preference,
            config,
            active_wakeup_interval: refresh_interval(config.refresh_hz),
            app_command,
            app_launched: false,
            spotlight: SpotlightModel::default(),
            shell_generation: 0,
            last_render_generation: 0,
            last_toplevel_count: 0,
            last_popup_count: 0,
            last_surface_count: 0,
            cursor_x,
            cursor_y,
            cursor_physical_x: cursor_x,
            cursor_physical_y: cursor_y,
            alt_pressed: false,
            ctrl_pressed: false,
            super_pressed: false,
            forwarded_control_keys: Vec::new(),
            suppressed_window_shortcut_keys: Vec::new(),
            window_interaction_active: false,
            redraw_pending: false,
            redraw_requested_at: None,
            input_response_until: None,
            perf: NestedPerfCounters::default(),
            host_monitor_refresh_millihz: None,
            active_pointer_constraint: None,
            pending_host_cursor_warp: None,
            host_cursor_client_visible: true,
            host_window_focused: true,
            input_clock_start: Instant::now(),
            fatal_error: None,
            shutdown_reason: ShutdownReason::EventLoopExited,
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, message: String) {
        self.fatal_error = Some(message);
        event_loop.exit();
    }

    fn take_fatal_error(&mut self) -> Option<String> {
        self.fatal_error.take()
    }

    const fn shutdown_reason(&self) -> ShutdownReason {
        self.shutdown_reason
    }

    fn shutdown_output(&mut self) {
        if let Some(constraint) = self.active_pointer_constraint {
            self.release_host_pointer_constraint(None);
            self.server
                .pointer_constraint_backend_deactivated(constraint.id);
        }
        // EGL's wl_egl_window must be destroyed before winit drops the underlying wl_surface.
        drop_renderer_before_window(&mut self.output_renderer, &mut self.window);
    }

    fn request_redraw(&mut self) {
        let now = Instant::now();
        if !should_issue_redraw_request(self.redraw_pending, self.redraw_requested_at, now) {
            self.perf.redraw_coalesced = self.perf.redraw_coalesced.saturating_add(1);
            return;
        }

        if let Some(window) = &self.window {
            if debug_frame_logging_enabled() {
                let retry = self.redraw_pending;
                eprintln!(
                    "oblivion-one compositor: request_redraw render_generation={} shell_generation={} retry={retry}",
                    self.server.render_generation(),
                    self.shell_generation
                );
            }
            window.request_redraw();
            self.perf.redraw_requests = self.perf.redraw_requests.saturating_add(1);
            self.redraw_pending = true;
            self.redraw_requested_at = Some(now);
        }
    }

    fn bump_shell_generation(&mut self) {
        self.shell_generation = self.shell_generation.wrapping_add(1);
    }

    fn tick_server(&mut self, event_loop: &ActiveEventLoop) {
        match self.server.tick() {
            Ok(accepted) => {
                if accepted > 0 {
                    println!(
                        "accepted {accepted} client(s); total {}",
                        self.server.accepted_clients()
                    );
                }
            }
            Err(error) => {
                self.fail(
                    event_loop,
                    format!("oblivion-one compositor: Wayland tick failed: {error}"),
                );
            }
        }
        self.process_pointer_constraint_backend_requests();

        let toplevel_count = self.server.xdg_toplevels();
        if toplevel_count != self.last_toplevel_count {
            self.last_toplevel_count = toplevel_count;
            println!("xdg toplevels: {toplevel_count}");
        }

        let popup_count = self.server.xdg_popups();
        if popup_count != self.last_popup_count {
            self.last_popup_count = popup_count;
            println!("xdg popups: {popup_count}");
        }

        let surface_count = self.server.renderable_surfaces().len();
        if debug_surface_logging_enabled() && surface_count != self.last_surface_count {
            self.last_surface_count = surface_count;
            println!("renderable surfaces: {surface_count}");
        }
    }

    fn process_pointer_constraint_backend_requests(&mut self) {
        for request in self.server.take_pointer_constraint_backend_requests() {
            match request {
                PointerConstraintBackendRequest::ActivateLocked { id, .. } => {
                    if self
                        .server
                        .pointer_constraint_backend_activation_current(id)
                    {
                        self.activate_host_pointer_constraint(
                            id,
                            NestedPointerConstraintMode::Locked,
                        );
                    } else {
                        pointer_debug_log(format!(
                            "backend activation dropped stale id={:?} rollback=not_needed",
                            id
                        ));
                    }
                }
                PointerConstraintBackendRequest::ActivateConfined { id, .. } => {
                    if self
                        .server
                        .pointer_constraint_backend_activation_current(id)
                    {
                        self.activate_host_pointer_constraint(
                            id,
                            NestedPointerConstraintMode::Confined,
                        );
                    } else {
                        pointer_debug_log(format!(
                            "backend activation dropped stale id={:?} rollback=not_needed",
                            id
                        ));
                    }
                }
                PointerConstraintBackendRequest::UpdateConfinedRegion { .. } => {}
                PointerConstraintBackendRequest::Deactivate {
                    id,
                    restore_position,
                } => {
                    if self
                        .active_pointer_constraint
                        .is_some_and(|constraint| constraint.id == id)
                    {
                        self.release_host_pointer_constraint(restore_position);
                        self.server.pointer_constraint_backend_deactivated(id);
                    }
                }
                PointerConstraintBackendRequest::ApplyCursorVisibility { visible } => {
                    self.host_cursor_client_visible = visible;
                    self.apply_host_cursor_visibility();
                }
                PointerConstraintBackendRequest::WarpPointer { position } => {
                    self.warp_host_pointer(position);
                }
            }
        }
    }

    fn activate_host_pointer_constraint(
        &mut self,
        id: PointerConstraintBackendId,
        mode: NestedPointerConstraintMode,
    ) {
        if let Some(active) = self.active_pointer_constraint {
            if active.id == id {
                pointer_debug_log(format!(
                    "backend activate requested id={:?} already_active=true",
                    id
                ));
                self.server.pointer_constraint_backend_activated(id);
                return;
            }
            pointer_debug_log(format!(
                "backend activation rejected/replaced old={:?} new={:?}",
                active.id, id
            ));
            self.server
                .pointer_constraint_backend_failed(id, "nested pointer constraint already active");
            return;
        }
        let Some(window) = self.window.as_deref() else {
            self.server
                .pointer_constraint_backend_failed(id, "nested window is not available");
            return;
        };
        if !self.host_window_focused {
            self.server
                .pointer_constraint_backend_failed(id, "nested window is not focused");
            return;
        }
        let grab_mode = match mode {
            NestedPointerConstraintMode::Locked => CursorGrabMode::Locked,
            NestedPointerConstraintMode::Confined => CursorGrabMode::Confined,
        };
        if let Err(error) = window.set_cursor_grab(grab_mode) {
            eprintln!(
                "oblivion-one compositor: pointer constraint {:?} grab failed for {:?}: {error}",
                id, mode
            );
            self.server
                .pointer_constraint_backend_failed(id, error.to_string());
            return;
        }
        self.active_pointer_constraint = Some(NestedPointerConstraint { id, mode });
        self.apply_host_cursor_visibility();
        pointer_debug_log(format!("backend activate requested id={:?}", id));
        self.server.pointer_constraint_backend_activated(id);
    }

    fn release_host_pointer_constraint(&mut self, restore_position: Option<OutputPosition>) {
        let Some(constraint) = self.active_pointer_constraint.take() else {
            return;
        };
        if let Some(window) = self.window.as_deref() {
            if let Some(position) = restore_position {
                match window.set_cursor_position(LogicalPosition::new(position.x, position.y)) {
                    Ok(()) => {
                        let scale_factor = output_scale_for_window(window);
                        self.cursor_x = position.x.round() as i32;
                        self.cursor_y = position.y.round() as i32;
                        self.cursor_physical_x =
                            logical_coordinate_to_physical(self.cursor_x, scale_factor);
                        self.cursor_physical_y =
                            logical_coordinate_to_physical(self.cursor_y, scale_factor);
                        self.pending_host_cursor_warp = Some(NestedPendingCursorWarp {
                            physical_x: self.cursor_physical_x,
                            physical_y: self.cursor_physical_y,
                        });
                        pointer_debug_log(format!(
                            "host cursor positioned while locked x={} y={}",
                            position.x, position.y
                        ));
                    }
                    Err(error) => {
                        pointer_debug_log(format!(
                            "host cursor position while locked failed x={} y={} error={error}",
                            position.x, position.y
                        ));
                    }
                }
            }
            if let Err(error) = window.set_cursor_grab(CursorGrabMode::None) {
                eprintln!("oblivion-one compositor: failed to release pointer grab: {error}");
            }
        }
        pointer_debug_log(format!("host grab released id={:?}", constraint.id));
        self.apply_host_cursor_visibility();
    }

    fn warp_host_pointer(&mut self, position: OutputPosition) {
        let Some(window) = self.window.as_deref() else {
            pointer_debug_log(format!(
                "backend warp skipped reason=no_window position=({},{})",
                position.x, position.y
            ));
            return;
        };
        match window.set_cursor_position(LogicalPosition::new(position.x, position.y)) {
            Ok(()) => {
                let scale_factor = output_scale_for_window(window);
                self.cursor_x = position.x.round() as i32;
                self.cursor_y = position.y.round() as i32;
                self.cursor_physical_x =
                    logical_coordinate_to_physical(self.cursor_x, scale_factor);
                self.cursor_physical_y =
                    logical_coordinate_to_physical(self.cursor_y, scale_factor);
                self.pending_host_cursor_warp = Some(NestedPendingCursorWarp {
                    physical_x: self.cursor_physical_x,
                    physical_y: self.cursor_physical_y,
                });
                pointer_debug_log(format!(
                    "backend warp applied position=({},{}) physical=({},{})",
                    position.x, position.y, self.cursor_physical_x, self.cursor_physical_y
                ));
            }
            Err(error) => {
                pointer_debug_log(format!(
                    "backend warp failed position=({},{}) error={error}",
                    position.x, position.y
                ));
            }
        }
    }

    fn apply_host_cursor_visibility(&self) {
        let Some(window) = self.window.as_deref() else {
            return;
        };
        let lock_hides_cursor = self
            .active_pointer_constraint
            .is_some_and(|constraint| constraint.mode == NestedPointerConstraintMode::Locked);
        window.set_cursor_visible(
            nested_output_uses_host_cursor()
                && self.host_cursor_client_visible
                && !lock_hides_cursor,
        );
    }

    fn locked_pointer_constraint_active(&self) -> bool {
        self.active_pointer_constraint
            .is_some_and(|constraint| constraint.mode == NestedPointerConstraintMode::Locked)
    }

    fn consume_pending_host_cursor_warp(&mut self, physical_x: i32, physical_y: i32) -> bool {
        let Some(pending) = self.pending_host_cursor_warp else {
            return false;
        };
        if host_cursor_warp_matches(pending, physical_x, physical_y) {
            self.pending_host_cursor_warp = None;
            true
        } else {
            false
        }
    }

    fn forward_host_relative_mouse_delta(&mut self, dx: f64, dy: f64) -> bool {
        if !self.host_window_focused {
            return false;
        }
        let Some(sample) = host_relative_delta_sample(
            self.locked_pointer_constraint_active(),
            self.monotonic_input_timestamp_usec(),
            dx,
            dy,
        ) else {
            return false;
        };
        self.server.send_pointer_motion_sample(sample);
        true
    }

    fn monotonic_input_timestamp_usec(&self) -> u64 {
        self.input_clock_start.elapsed().as_micros() as u64
    }

    fn draw(&mut self) -> OutputResult<()> {
        let Some(window) = &self.window else {
            return Ok(());
        };

        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);
        let output_scale = output_scale_for_window(window);
        self.server.prepare_frame();
        let shell_state = ShellOverlayState {
            topbar: ShellTopbarModel::visible("Oblivion One").with_trailing_text("Super+Space"),
            dock_items: self.server.shell_dock_items(),
            spotlight: self.spotlight.clone(),
            generation: self.shell_generation,
        };
        let shell_overlay = self
            .shell_overlay_renderer
            .render(width, height, &shell_state);
        let content_generation = nested_scene_content_generation(
            self.server.render_generation(),
            shell_overlay.generation,
        );
        let Some(output_renderer) = &mut self.output_renderer else {
            return Ok(());
        };
        output_renderer.draw_desktop_scene(NestedSceneDrawRequest {
            width,
            height,
            output_scale,
            surfaces: self.server.renderable_surfaces(),
            content_generation,
            visual_state: nested_visual_state(self.cursor_x, self.cursor_y),
            shell_overlay: Some(shell_overlay),
            cpu_scene_renderer: &mut self.renderer,
        })?;
        if debug_frame_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: draw presented render_generation={} surfaces={}",
                self.server.render_generation(),
                self.server.renderable_surfaces().len()
            );
        }
        self.perf.presented_frames = self.perf.presented_frames.saturating_add(1);
        self.server.finish_frame();
        Ok(())
    }

    fn next_wakeup_interval(&self) -> Duration {
        nested_output_wakeup_interval(
            self.active_wakeup_interval,
            !self.server.renderable_surfaces().is_empty(),
            self.window_interaction_active || self.server.window_interaction_active(),
            self.redraw_pending,
            self.server.has_pending_frame_prepare_work() || self.server.has_pending_frame_work(),
            self.input_response_fast_path_active(Instant::now()),
        )
    }

    fn mark_client_input_forwarded(&mut self) {
        self.input_response_until = Some(Instant::now() + INPUT_RESPONSE_FAST_PATH_DURATION);
    }

    fn input_response_fast_path_active(&self, now: Instant) -> bool {
        self.input_response_until.is_some_and(|until| now <= until)
    }

    fn record_wakeup_interval(&mut self, interval: Duration) {
        if interval == IDLE_WAKEUP_INTERVAL {
            self.perf.idle_wakeups = self.perf.idle_wakeups.saturating_add(1);
        } else {
            self.perf.active_wakeups = self.perf.active_wakeups.saturating_add(1);
        }
    }

    fn maybe_log_perf(&mut self, now: Instant) {
        if !nested_perf_logging_enabled() {
            return;
        }
        let should_log = self.perf.last_log_at.is_none_or(|last_log_at| {
            now.saturating_duration_since(last_log_at) >= Duration::from_secs(2)
        });
        if !should_log {
            return;
        }
        self.perf.last_log_at = Some(now);
        let host_refresh = self
            .host_monitor_refresh_millihz
            .map(|refresh| refresh.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        eprintln!(
            "perf nested.timing nested_target_refresh_hz={} nested_target_interval_us={} nested_redraw_requests={} nested_presented_frames={} nested_redraw_coalesced={} nested_idle_wakeups={} nested_active_wakeups={} host_monitor_refresh_millihz={}",
            self.config.refresh_hz,
            self.active_wakeup_interval.as_micros(),
            self.perf.redraw_requests,
            self.perf.presented_frames,
            self.perf.redraw_coalesced,
            self.perf.idle_wakeups,
            self.perf.active_wakeups,
            host_refresh
        );
    }

    fn send_client_keyboard_key(&mut self, key: u32, pressed: bool) {
        if !self.host_window_focused {
            return;
        }
        self.server.send_keyboard_key(key, pressed);
        self.mark_client_input_forwarded();
    }

    fn send_client_pointer_motion(&mut self, x: f64, y: f64) {
        if !self.host_window_focused {
            return;
        }
        self.server.send_pointer_motion(x, y);
        self.mark_client_input_forwarded();
    }

    fn send_client_pointer_button(&mut self, button: u32, pressed: bool) {
        if !self.host_window_focused {
            return;
        }
        self.server.send_pointer_button(button, pressed);
        self.mark_client_input_forwarded();
    }

    fn send_client_pointer_axis(&mut self, horizontal: f64, vertical: f64) {
        if !self.host_window_focused {
            return;
        }
        self.server.send_pointer_axis(horizontal, vertical);
        self.mark_client_input_forwarded();
    }

    fn launch_app_if_needed(&mut self, event_loop: &ActiveEventLoop) {
        if self.app_launched || self.app_command.is_empty() {
            return;
        }

        let Some(program) = self.app_command.first().cloned() else {
            return;
        };
        let socket_name = self.server.socket_name().to_string();
        match spawn_compositor_app(&socket_name, &self.app_command) {
            Ok(Some(pid)) => {
                println!(
                    "spawned `{program}` on Oblivion Wayland socket `{socket_name}` as pid {pid}"
                );
                self.app_launched = true;
            }
            Ok(None) => {
                self.app_launched = true;
            }
            Err(error) => {
                self.fail(
                    event_loop,
                    format!("oblivion-one compositor: failed to spawn `{program}`: {error}"),
                );
            }
        }
    }

    fn sync_output_geometry_for_window(&mut self, window: &Window) {
        let scale_factor = output_scale_for_window(window);
        let size = window.inner_size();
        let (width, height) =
            logical_output_size_from_physical(size.width, size.height, scale_factor);
        self.server.set_output_scale_factor(scale_factor);
        self.server.set_output_size(width, height);
    }

    fn launch_shell_command(&mut self, event_loop: &ActiveEventLoop, command: Vec<String>) {
        let Some(program) = command.first().cloned() else {
            return;
        };
        let socket_name = self.server.socket_name().to_string();
        match spawn_compositor_app(&socket_name, &command) {
            Ok(Some(pid)) => {
                println!(
                    "spawned `{program}` from Spotlight on Oblivion Wayland socket `{socket_name}` as pid {pid}"
                );
            }
            Ok(None) => {}
            Err(error) => {
                self.fail(
                    event_loop,
                    format!("oblivion-one compositor: failed to spawn `{program}`: {error}"),
                );
            }
        }
    }

    fn suppress_window_shortcut_key(&mut self, code: KeyCode) -> bool {
        if self.suppressed_window_shortcut_keys.contains(&code) {
            return false;
        }

        self.suppressed_window_shortcut_keys.push(code);
        true
    }

    fn release_suppressed_window_shortcut_key(&mut self, code: KeyCode) -> bool {
        let Some(index) = self
            .suppressed_window_shortcut_keys
            .iter()
            .position(|suppressed| *suppressed == code)
        else {
            return false;
        };

        self.suppressed_window_shortcut_keys.swap_remove(index);
        true
    }

    fn track_forwarded_control_key(&mut self, code: KeyCode) {
        if !self.forwarded_control_keys.contains(&code) {
            self.forwarded_control_keys.push(code);
        }
    }

    fn release_forwarded_control_key(&mut self, code: KeyCode) -> bool {
        let Some(index) = self
            .forwarded_control_keys
            .iter()
            .position(|forwarded| *forwarded == code)
        else {
            return false;
        };

        self.forwarded_control_keys.swap_remove(index);
        true
    }

    fn release_forwarded_control_modifiers(&mut self) {
        let forwarded = std::mem::take(&mut self.forwarded_control_keys);
        for code in forwarded {
            if let Some(key) = keycode_to_evdev_key(code) {
                self.send_client_keyboard_key(key, false);
            }
        }
    }

    fn apply_window_management_shortcut(&mut self, shortcut: WindowManagementShortcut) -> bool {
        match shortcut {
            WindowManagementShortcut::Minimize => self.server.minimize_focused_window(),
            WindowManagementShortcut::RestoreMinimized => {
                self.server.restore_next_minimized_window()
            }
            WindowManagementShortcut::ToggleMaximize => {
                self.server.toggle_maximize_focused_window()
            }
            WindowManagementShortcut::ToggleFullscreen => {
                self.server.toggle_fullscreen_focused_window()
            }
        }
    }

    fn toggle_spotlight(&mut self) {
        self.spotlight.toggle();
        self.bump_shell_generation();
        self.request_redraw();
    }

    fn handle_spotlight_key(
        &mut self,
        code: KeyCode,
        pressed: bool,
        text: Option<&str>,
    ) -> Option<Vec<String>> {
        if !self.spotlight.is_visible() {
            return None;
        }
        if !pressed {
            return None;
        }

        match code {
            KeyCode::Escape => {
                self.spotlight.hide();
                self.bump_shell_generation();
                self.request_redraw();
                None
            }
            KeyCode::Backspace => {
                if self.spotlight.backspace() {
                    self.bump_shell_generation();
                    self.request_redraw();
                }
                None
            }
            KeyCode::ArrowDown => {
                if self.spotlight.select_next() {
                    self.bump_shell_generation();
                    self.request_redraw();
                }
                None
            }
            KeyCode::ArrowUp => {
                if self.spotlight.select_previous() {
                    self.bump_shell_generation();
                    self.request_redraw();
                }
                None
            }
            KeyCode::Enter => {
                let command = self.spotlight.selected_launch_command();
                self.spotlight.hide();
                self.bump_shell_generation();
                self.request_redraw();
                command
            }
            _ => {
                if let Some(text) = text {
                    self.spotlight.push_text(text);
                    self.bump_shell_generation();
                    self.request_redraw();
                }
                None
            }
        }
    }
}

impl ApplicationHandler for NestedOutputApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        println!(
            "nested output requested: {}x{} logical @ {} Hz",
            self.config.width, self.config.height, self.config.refresh_hz
        );
        self.server.set_output_refresh_hz(self.config.refresh_hz);
        let attributes = WindowAttributes::default()
            .with_title("Oblivion One")
            .with_inner_size(LogicalSize::new(self.config.width, self.config.height))
            .with_min_inner_size({
                let (width, height) = nested_output_minimum_size(self.config);
                LogicalSize::new(width, height)
            })
            .with_transparent(false)
            .with_resizable(true);

        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                self.fail(
                    event_loop,
                    format!("oblivion-one compositor: failed to create output window: {error}"),
                );
                return;
            }
        };
        window.set_cursor_visible(nested_output_uses_host_cursor());
        self.host_monitor_refresh_millihz =
            report_host_monitor_refresh(window.current_monitor().as_ref(), self.config.refresh_hz);
        self.sync_output_geometry_for_window(window.as_ref());
        let scale_factor = output_scale_for_window(window.as_ref());
        let size = window.inner_size();
        let (cursor_physical_x, cursor_physical_y) = nested_output_initial_cursor_physical(
            self.config,
            scale_factor,
            size.width,
            size.height,
        );
        self.cursor_physical_x = cursor_physical_x;
        self.cursor_physical_y = cursor_physical_y;

        let output_renderer =
            match NestedOutputRenderer::new(Arc::clone(&window), self.renderer_preference) {
                Ok(renderer) => renderer,
                Err(error) => {
                    self.fail(
                        event_loop,
                        format!(
                            "oblivion-one compositor: failed to create output renderer: {error}"
                        ),
                    );
                    return;
                }
            };
        self.server.set_dmabuf_feedback(
            output_renderer.dmabuf_feedback(),
            output_renderer.dmabuf_main_device(),
            output_renderer.dmabuf_main_device_path(),
        );
        println!("output renderer: {}", output_renderer.backend().as_str());
        println!("Spotlight: focus the nested output and press Super+Space (Ctrl+Space fallback)");
        self.output_renderer = Some(output_renderer);
        self.window = Some(window);
        self.launch_app_if_needed(event_loop);
        self.request_redraw();
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.tick_server(event_loop);
        let render_generation = self.server.render_generation();
        if render_generation != self.last_render_generation {
            if debug_frame_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: render_generation advanced {} -> {}",
                    self.last_render_generation, render_generation
                );
            }
            self.last_render_generation = render_generation;
            self.request_redraw();
        }
        if self.server.has_pending_frame_prepare_work() || self.server.has_pending_frame_work() {
            self.request_redraw();
        }
        let now = Instant::now();
        let interval = self.next_wakeup_interval();
        self.record_wakeup_interval(interval);
        self.maybe_log_perf(now);
        event_loop.set_control_flow(ControlFlow::WaitUntil(now + interval));
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                self.shutdown_reason = ShutdownReason::HostWindowClosed;
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.draw() {
                    self.fail(
                        event_loop,
                        format!("oblivion-one compositor: draw failed: {error}"),
                    );
                }
                self.redraw_pending = false;
                self.redraw_requested_at = None;
            }
            WindowEvent::CursorMoved { position, .. } => {
                let physical_x = position.x.round() as i32;
                let physical_y = position.y.round() as i32;
                if self.consume_pending_host_cursor_warp(physical_x, physical_y) {
                    self.cursor_physical_x = physical_x;
                    self.cursor_physical_y = physical_y;
                    let output_scale = self
                        .window
                        .as_deref()
                        .map(output_scale_for_window)
                        .unwrap_or(1.0);
                    let x = logical_coordinate_from_physical(position.x, output_scale).max(0.0);
                    let y = logical_coordinate_from_physical(position.y, output_scale).max(0.0);
                    self.cursor_x = x.round() as i32;
                    self.cursor_y = y.round() as i32;
                    return;
                }
                if self.locked_pointer_constraint_active() {
                    return;
                }
                self.cursor_physical_x = physical_x;
                self.cursor_physical_y = physical_y;
                let output_scale = self
                    .window
                    .as_deref()
                    .map(output_scale_for_window)
                    .unwrap_or(1.0);
                let x = logical_coordinate_from_physical(position.x, output_scale).max(0.0);
                let y = logical_coordinate_from_physical(position.y, output_scale).max(0.0);
                self.cursor_x = x.round() as i32;
                self.cursor_y = y.round() as i32;
                let interaction_active =
                    self.window_interaction_active || self.server.window_interaction_active();
                if interaction_active {
                    if self.server.update_window_interaction(x, y) {
                        self.window_interaction_active = true;
                        self.request_redraw();
                    }
                    return;
                }

                if shell_overlay_captures_client_input(self.spotlight.is_visible()) {
                    if !nested_output_uses_host_cursor() {
                        self.request_redraw();
                    }
                    return;
                }

                self.send_client_pointer_motion(x, y);
                if !nested_output_uses_host_cursor() {
                    self.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if shell_overlay_captures_client_input(self.spotlight.is_visible()) {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        self.request_redraw();
                    }
                    return;
                }

                if state == ElementState::Pressed
                    && button == MouseButton::Left
                    && !self.spotlight.is_visible()
                    && let Some(window) = &self.window
                {
                    let size = window.inner_size();
                    let dock_items = self.server.shell_dock_items();
                    if let Some(surface_id) = dock_item_at(
                        size.width,
                        size.height,
                        &dock_items,
                        self.cursor_physical_x.max(0),
                        self.cursor_physical_y.max(0),
                    ) {
                        if self.server.activate_window(surface_id) {
                            self.bump_shell_generation();
                            self.request_redraw();
                        }
                        return;
                    }
                }

                if state == ElementState::Pressed
                    && let Some(intent) = window_drag_intent(self.alt_pressed, button, state)
                {
                    let x = f64::from(self.cursor_x.max(0));
                    let y = f64::from(self.cursor_y.max(0));
                    let started = match intent {
                        WindowDragIntent::Move => self.server.begin_window_move_at(x, y),
                        WindowDragIntent::Resize => self.server.begin_window_resize_at(x, y),
                    };
                    if started {
                        self.window_interaction_active = true;
                        self.request_redraw();
                        return;
                    }
                }

                if state == ElementState::Pressed && button == MouseButton::Left {
                    let x = f64::from(self.cursor_x.max(0));
                    let y = f64::from(self.cursor_y.max(0));
                    let started = self.server.begin_window_frame_action_at(x, y);
                    if started {
                        self.window_interaction_active = true;
                        self.request_redraw();
                        return;
                    }
                }

                if state == ElementState::Released
                    && (self.window_interaction_active || self.server.window_interaction_active())
                {
                    self.server.end_window_interaction();
                    self.window_interaction_active = false;
                    self.request_redraw();
                    return;
                }

                if let Some(button) = mouse_button_to_wayland(button) {
                    self.send_client_pointer_button(button, state == ElementState::Pressed);
                    if forwarded_client_input_requests_redraw(true) {
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if shell_overlay_captures_client_input(self.spotlight.is_visible()) {
                    return;
                }

                let output_scale = self
                    .window
                    .as_deref()
                    .map(output_scale_for_window)
                    .unwrap_or(1.0);
                let (horizontal, vertical) =
                    mouse_scroll_delta_to_wayland_axes(delta, output_scale);
                self.send_client_pointer_axis(horizontal, vertical);
                if forwarded_client_input_requests_redraw(true) {
                    self.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key
                    && let Some(key) = keycode_to_evdev_key(code)
                {
                    if is_alt_key(code) {
                        self.alt_pressed = event.state == ElementState::Pressed;
                        if !self.alt_pressed && self.window_interaction_active {
                            self.server.end_window_interaction();
                            self.window_interaction_active = false;
                            self.request_redraw();
                        }
                        return;
                    }
                    if is_super_key(code) {
                        self.super_pressed = event.state == ElementState::Pressed;
                        return;
                    }
                    if is_control_key(code) {
                        self.ctrl_pressed = event.state == ElementState::Pressed;
                        if event.state == ElementState::Pressed {
                            if !self.spotlight.is_visible() {
                                self.track_forwarded_control_key(code);
                                self.send_client_keyboard_key(key, true);
                                if forwarded_client_input_requests_redraw(true) {
                                    self.request_redraw();
                                }
                            }
                            return;
                        }

                        if self.release_forwarded_control_key(code) {
                            self.send_client_keyboard_key(key, false);
                            if forwarded_client_input_requests_redraw(true) {
                                self.request_redraw();
                            }
                        }
                        return;
                    }
                    if event.state == ElementState::Released
                        && self.release_suppressed_window_shortcut_key(code)
                    {
                        return;
                    }
                    if is_spotlight_toggle_shortcut(self.shortcut_modifiers(), code) {
                        if event.state == ElementState::Pressed
                            && self.suppress_window_shortcut_key(code)
                        {
                            self.release_forwarded_control_modifiers();
                            self.toggle_spotlight();
                        }
                        return;
                    }
                    if self.spotlight.is_visible() {
                        let text =
                            spotlight_text_for_key(event.text.as_deref(), &event.logical_key);
                        if let Some(command) = self.handle_spotlight_key(
                            code,
                            event.state == ElementState::Pressed,
                            text.as_deref(),
                        ) {
                            self.launch_shell_command(event_loop, command);
                        }
                        return;
                    }
                    if let Some(shortcut) = window_management_shortcut(self.alt_pressed, code) {
                        if event.state == ElementState::Pressed
                            && self.suppress_window_shortcut_key(code)
                            && self.apply_window_management_shortcut(shortcut)
                        {
                            self.request_redraw();
                        }
                        return;
                    }
                    if self.alt_pressed {
                        return;
                    }
                    self.send_client_keyboard_key(key, event.state == ElementState::Pressed);
                    if forwarded_client_input_requests_redraw(true) {
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::Resized(size) => {
                let scale_factor = self
                    .window
                    .as_deref()
                    .map(output_scale_for_window)
                    .unwrap_or(1.0);
                let (width, height) =
                    logical_output_size_from_physical(size.width, size.height, scale_factor);
                self.server.set_output_scale_factor(scale_factor);
                self.server.set_output_size(width, height);
                self.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let size = self
                    .window
                    .as_deref()
                    .map(Window::inner_size)
                    .unwrap_or_default();
                let scale_factor = sanitize_output_scale(scale_factor);
                let (width, height) =
                    logical_output_size_from_physical(size.width, size.height, scale_factor);
                self.server.set_output_scale_factor(scale_factor);
                self.server.set_output_size(width, height);
                self.request_redraw();
            }
            WindowEvent::Focused(focused) => {
                self.host_window_focused = focused;
                if std::env::var_os("TYPHON_POINTER_DEBUG").is_some() {
                    eprintln!("typhon pointer: host_window_focused={focused}");
                }
                if focused {
                    self.apply_host_cursor_visibility();
                    self.request_redraw();
                    return;
                }
                if let Some(constraint) = self.active_pointer_constraint {
                    self.release_host_pointer_constraint(None);
                    self.server
                        .pointer_constraint_backend_deactivated(constraint.id);
                }
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        let DeviceEvent::MouseMotion { delta: (dx, dy) } = event else {
            return;
        };
        if self.forward_host_relative_mouse_delta(dx, dy)
            && forwarded_client_input_requests_redraw(true)
        {
            self.request_redraw();
        }
    }
}

fn host_relative_delta_sample(
    locked: bool,
    timestamp_usec: u64,
    dx: f64,
    dy: f64,
) -> Option<PointerMotionSample> {
    let relative = RelativePointerMotion {
        dx,
        dy,
        dx_unaccelerated: dx,
        dy_unaccelerated: dy,
    };
    if relative.is_zero() {
        return None;
    }
    if std::env::var_os("TYPHON_POINTER_DEBUG").is_some() {
        eprintln!("typhon pointer: host relative delta locked={locked} dx={dx} dy={dy}");
    }
    Some(PointerMotionSample {
        timestamp_usec,
        absolute: None,
        relative: Some(relative),
    })
}

impl Drop for NestedOutputApp {
    fn drop(&mut self) {
        self.shutdown_output();
    }
}

impl NestedOutputApp {
    const fn shortcut_modifiers(&self) -> ShortcutModifiers {
        ShortcutModifiers {
            super_pressed: self.super_pressed,
            ctrl_pressed: self.ctrl_pressed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownReason {
    HostWindowClosed,
    EventLoopExited,
}

impl ShutdownReason {
    const fn log_message(self) -> &'static str {
        match self {
            Self::HostWindowClosed => {
                "oblivion-one compositor: nested output closed by host window"
            }
            Self::EventLoopExited => "oblivion-one compositor: nested output event loop exited",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowDragIntent {
    Move,
    Resize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowManagementShortcut {
    Minimize,
    RestoreMinimized,
    ToggleMaximize,
    ToggleFullscreen,
}

fn drop_renderer_before_window<Renderer, Window>(
    output_renderer: &mut Option<Renderer>,
    window: &mut Option<Window>,
) {
    drop(output_renderer.take());
    drop(window.take());
}

const fn nested_output_uses_host_cursor() -> bool {
    NESTED_OUTPUT_HOST_CURSOR
}

fn output_scale_for_window(window: &Window) -> f64 {
    sanitize_output_scale(window.scale_factor())
}

fn sanitize_output_scale(scale_factor: f64) -> f64 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

fn logical_output_size_from_physical(
    physical_width: u32,
    physical_height: u32,
    scale_factor: f64,
) -> (u32, u32) {
    let scale_factor = sanitize_output_scale(scale_factor);
    (
        logical_extent_from_physical(physical_width, scale_factor),
        logical_extent_from_physical(physical_height, scale_factor),
    )
}

fn logical_extent_from_physical(physical: u32, scale_factor: f64) -> u32 {
    (f64::from(physical.max(1)) / sanitize_output_scale(scale_factor))
        .round()
        .max(1.0) as u32
}

fn logical_coordinate_from_physical(physical: f64, scale_factor: f64) -> f64 {
    physical / sanitize_output_scale(scale_factor)
}

const fn refresh_interval(refresh_hz: u32) -> Duration {
    Duration::from_nanos(1_000_000_000_u64 / refresh_hz as u64)
}

const fn nested_output_minimum_size(config: NestedOutputConfig) -> (u32, u32) {
    (min_u32(config.width, 640), min_u32(config.height, 360))
}

const fn min_u32(left: u32, right: u32) -> u32 {
    if left < right { left } else { right }
}

const fn nested_output_initial_cursor(config: NestedOutputConfig) -> (i32, i32) {
    ((config.width / 2) as i32, (config.height / 2) as i32)
}

fn nested_output_initial_cursor_physical(
    config: NestedOutputConfig,
    scale_factor: f64,
    physical_width: u32,
    physical_height: u32,
) -> (i32, i32) {
    let (cursor_x, cursor_y) = nested_output_initial_cursor(config);
    (
        logical_coordinate_to_physical(cursor_x, scale_factor)
            .clamp(0, physical_width.saturating_sub(1) as i32),
        logical_coordinate_to_physical(cursor_y, scale_factor)
            .clamp(0, physical_height.saturating_sub(1) as i32),
    )
}

fn logical_coordinate_to_physical(logical: i32, scale_factor: f64) -> i32 {
    (f64::from(logical.max(0)) * sanitize_output_scale(scale_factor)).round() as i32
}

fn host_cursor_warp_matches(
    pending: NestedPendingCursorWarp,
    physical_x: i32,
    physical_y: i32,
) -> bool {
    const TOLERANCE: i32 = 2;
    (pending.physical_x - physical_x).abs() <= TOLERANCE
        && (pending.physical_y - physical_y).abs() <= TOLERANCE
}

fn report_host_monitor_refresh(
    monitor: Option<&MonitorHandle>,
    requested_refresh_hz: u32,
) -> Option<u32> {
    let Some(refresh_millihertz) = monitor.and_then(MonitorHandle::refresh_rate_millihertz) else {
        println!("host monitor refresh: unknown");
        return None;
    };

    println!(
        "host monitor refresh: {}.{:03} Hz",
        refresh_millihertz / 1_000,
        refresh_millihertz % 1_000
    );
    if refresh_millihertz < requested_refresh_hz.saturating_mul(1_000) {
        println!(
            "requested nested refresh is {requested_refresh_hz} Hz, but the current host monitor reports {} Hz; actual presentation will be host-limited",
            refresh_millihertz / 1_000
        );
    }
    Some(refresh_millihertz)
}

fn debug_surface_logging_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("OBLIVION_ONE_DEBUG_SURFACES").is_some())
}

fn pointer_debug_logging_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("TYPHON_POINTER_DEBUG").is_some())
}

fn pointer_debug_log(message: impl AsRef<str>) {
    if pointer_debug_logging_enabled() {
        eprintln!("typhon pointer: {}", message.as_ref());
    }
}

fn debug_frame_logging_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("OBLIVION_ONE_DEBUG_FRAMES").is_some())
}

fn nested_perf_logging_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("OBLIVION_ONE_PERF_LOG")
            .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "debug" | "DEBUG"))
    })
}

const fn nested_visual_state(cursor_x: i32, cursor_y: i32) -> DesktopVisualState {
    if nested_output_uses_host_cursor() {
        DesktopVisualState::wallpaper_only()
    } else {
        DesktopVisualState::with_cursor(cursor_x, cursor_y)
    }
}

const fn nested_output_wakeup_interval(
    active_interval: Duration,
    has_renderable_surfaces: bool,
    interaction_active: bool,
    redraw_pending: bool,
    pending_frame_work: bool,
    input_response_fast_path_active: bool,
) -> Duration {
    if input_response_fast_path_active {
        INPUT_RESPONSE_WAKEUP_INTERVAL
    } else if has_renderable_surfaces || interaction_active || redraw_pending || pending_frame_work
    {
        active_interval
    } else {
        IDLE_WAKEUP_INTERVAL
    }
}

fn should_issue_redraw_request(
    redraw_pending: bool,
    redraw_requested_at: Option<Instant>,
    now: Instant,
) -> bool {
    !redraw_pending
        || redraw_requested_at.is_none_or(|requested_at| {
            now.saturating_duration_since(requested_at) >= REDRAW_REQUEST_RETRY_INTERVAL
        })
}

const fn nested_scene_content_generation(
    render_generation: u64,
    shell_overlay_generation: u64,
) -> u64 {
    render_generation
        .wrapping_mul(1_000_003)
        .wrapping_add(shell_overlay_generation)
}

fn window_drag_intent(
    alt_pressed: bool,
    button: MouseButton,
    state: ElementState,
) -> Option<WindowDragIntent> {
    if !alt_pressed || state != ElementState::Pressed {
        return None;
    }

    match button {
        MouseButton::Left => Some(WindowDragIntent::Move),
        MouseButton::Right => Some(WindowDragIntent::Resize),
        _ => None,
    }
}

fn is_alt_key(code: KeyCode) -> bool {
    matches!(code, KeyCode::AltLeft | KeyCode::AltRight)
}

fn is_control_key(code: KeyCode) -> bool {
    matches!(code, KeyCode::ControlLeft | KeyCode::ControlRight)
}

fn is_super_key(code: KeyCode) -> bool {
    matches!(code, KeyCode::SuperLeft | KeyCode::SuperRight)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ShortcutModifiers {
    super_pressed: bool,
    ctrl_pressed: bool,
}

fn is_spotlight_toggle_shortcut(modifiers: ShortcutModifiers, code: KeyCode) -> bool {
    code == KeyCode::Space && (modifiers.super_pressed || modifiers.ctrl_pressed)
}

fn window_management_shortcut(
    alt_pressed: bool,
    code: KeyCode,
) -> Option<WindowManagementShortcut> {
    if !alt_pressed {
        return None;
    }

    match code {
        KeyCode::KeyM => Some(WindowManagementShortcut::Minimize),
        KeyCode::KeyR => Some(WindowManagementShortcut::RestoreMinimized),
        KeyCode::KeyF => Some(WindowManagementShortcut::ToggleMaximize),
        KeyCode::Enter | KeyCode::F11 => Some(WindowManagementShortcut::ToggleFullscreen),
        _ => None,
    }
}

fn mouse_button_to_wayland(button: MouseButton) -> Option<u32> {
    match button {
        MouseButton::Left => Some(0x110),
        MouseButton::Right => Some(0x111),
        MouseButton::Middle => Some(0x112),
        MouseButton::Back => Some(0x113),
        MouseButton::Forward => Some(0x114),
        MouseButton::Other(button) => Some(u32::from(button)),
    }
}

fn mouse_scroll_delta_to_wayland_axes(delta: MouseScrollDelta, output_scale: f64) -> (f64, f64) {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => (
            f64::from(x) * WAYLAND_SCROLL_LINE_DISTANCE,
            -f64::from(y) * WAYLAND_SCROLL_LINE_DISTANCE,
        ),
        MouseScrollDelta::PixelDelta(position) => (
            logical_coordinate_from_physical(position.x, output_scale),
            -logical_coordinate_from_physical(position.y, output_scale),
        ),
    }
}

const fn forwarded_client_input_requests_redraw(forwarded: bool) -> bool {
    forwarded
}

const fn shell_overlay_captures_client_input(spotlight_visible: bool) -> bool {
    spotlight_visible
}

fn spotlight_text_for_key(event_text: Option<&str>, logical_key: &Key) -> Option<String> {
    event_text
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .or_else(|| match logical_key {
            Key::Character(text) if !text.is_empty() => Some(text.to_string()),
            _ => None,
        })
}

fn keycode_to_evdev_key(code: KeyCode) -> Option<u32> {
    match code {
        KeyCode::Escape => Some(1),
        KeyCode::Digit1 => Some(2),
        KeyCode::Digit2 => Some(3),
        KeyCode::Digit3 => Some(4),
        KeyCode::Digit4 => Some(5),
        KeyCode::Digit5 => Some(6),
        KeyCode::Digit6 => Some(7),
        KeyCode::Digit7 => Some(8),
        KeyCode::Digit8 => Some(9),
        KeyCode::Digit9 => Some(10),
        KeyCode::Digit0 => Some(11),
        KeyCode::Minus => Some(12),
        KeyCode::Equal => Some(13),
        KeyCode::Backspace => Some(14),
        KeyCode::Tab => Some(15),
        KeyCode::KeyQ => Some(16),
        KeyCode::KeyW => Some(17),
        KeyCode::KeyE => Some(18),
        KeyCode::KeyR => Some(19),
        KeyCode::KeyT => Some(20),
        KeyCode::KeyY => Some(21),
        KeyCode::KeyU => Some(22),
        KeyCode::KeyI => Some(23),
        KeyCode::KeyO => Some(24),
        KeyCode::KeyP => Some(25),
        KeyCode::BracketLeft => Some(26),
        KeyCode::BracketRight => Some(27),
        KeyCode::Enter => Some(28),
        KeyCode::ControlLeft => Some(29),
        KeyCode::KeyA => Some(30),
        KeyCode::KeyS => Some(31),
        KeyCode::KeyD => Some(32),
        KeyCode::KeyF => Some(33),
        KeyCode::KeyG => Some(34),
        KeyCode::KeyH => Some(35),
        KeyCode::KeyJ => Some(36),
        KeyCode::KeyK => Some(37),
        KeyCode::KeyL => Some(38),
        KeyCode::Semicolon => Some(39),
        KeyCode::Quote => Some(40),
        KeyCode::Backquote => Some(41),
        KeyCode::ShiftLeft => Some(42),
        KeyCode::Backslash => Some(43),
        KeyCode::KeyZ => Some(44),
        KeyCode::KeyX => Some(45),
        KeyCode::KeyC => Some(46),
        KeyCode::KeyV => Some(47),
        KeyCode::KeyB => Some(48),
        KeyCode::KeyN => Some(49),
        KeyCode::KeyM => Some(50),
        KeyCode::Comma => Some(51),
        KeyCode::Period => Some(52),
        KeyCode::Slash => Some(53),
        KeyCode::ShiftRight => Some(54),
        KeyCode::AltLeft => Some(56),
        KeyCode::Space => Some(57),
        KeyCode::CapsLock => Some(58),
        KeyCode::F1 => Some(59),
        KeyCode::F2 => Some(60),
        KeyCode::F3 => Some(61),
        KeyCode::F4 => Some(62),
        KeyCode::F5 => Some(63),
        KeyCode::F6 => Some(64),
        KeyCode::F7 => Some(65),
        KeyCode::F8 => Some(66),
        KeyCode::F9 => Some(67),
        KeyCode::F10 => Some(68),
        KeyCode::F11 => Some(87),
        KeyCode::F12 => Some(88),
        KeyCode::AltRight => Some(100),
        KeyCode::ControlRight => Some(97),
        KeyCode::SuperLeft => Some(125),
        KeyCode::SuperRight => Some(126),
        KeyCode::ArrowUp => Some(103),
        KeyCode::ArrowLeft => Some(105),
        KeyCode::ArrowRight => Some(106),
        KeyCode::ArrowDown => Some(108),
        KeyCode::Insert => Some(110),
        KeyCode::Delete => Some(111),
        KeyCode::Home => Some(102),
        KeyCode::End => Some(107),
        KeyCode::PageUp => Some(104),
        KeyCode::PageDown => Some(109),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::PhysicalPosition;
    use winit::keyboard::NamedKey;

    #[test]
    fn keycode_mapping_uses_linux_evdev_codes_for_typing_keys() {
        assert_eq!(keycode_to_evdev_key(KeyCode::KeyA), Some(30));
        assert_eq!(keycode_to_evdev_key(KeyCode::Enter), Some(28));
        assert_eq!(keycode_to_evdev_key(KeyCode::Space), Some(57));
    }

    #[test]
    fn mouse_button_mapping_uses_linux_button_codes() {
        assert_eq!(mouse_button_to_wayland(MouseButton::Left), Some(0x110));
        assert_eq!(mouse_button_to_wayland(MouseButton::Right), Some(0x111));
    }

    #[test]
    fn mouse_wheel_delta_maps_to_wayland_axis_direction() {
        assert_eq!(
            mouse_scroll_delta_to_wayland_axes(MouseScrollDelta::LineDelta(0.0, 1.0), 1.0),
            (0.0, -WAYLAND_SCROLL_LINE_DISTANCE)
        );
        assert_eq!(
            mouse_scroll_delta_to_wayland_axes(MouseScrollDelta::LineDelta(0.0, -1.0), 1.0),
            (0.0, WAYLAND_SCROLL_LINE_DISTANCE)
        );
        assert_eq!(
            mouse_scroll_delta_to_wayland_axes(
                MouseScrollDelta::PixelDelta(PhysicalPosition::new(12.0, -24.0)),
                2.0
            ),
            (6.0, 12.0)
        );
    }

    #[test]
    fn forwarded_client_input_requests_redraw_to_flush_frame_callbacks() {
        assert!(forwarded_client_input_requests_redraw(true));
        assert!(!forwarded_client_input_requests_redraw(false));
    }

    #[test]
    fn nested_output_uses_active_wakeup_while_frame_work_is_pending() {
        let active_interval = refresh_interval(165);
        assert_eq!(
            nested_output_wakeup_interval(active_interval, false, false, false, true, false),
            active_interval
        );
    }

    #[test]
    fn refresh_interval_uses_exact_nanosecond_division() {
        assert_eq!(refresh_interval(60), Duration::from_nanos(16_666_666));
        assert_eq!(refresh_interval(120), Duration::from_nanos(8_333_333));
        assert_eq!(refresh_interval(144), Duration::from_nanos(6_944_444));
        assert_eq!(refresh_interval(165), Duration::from_nanos(6_060_606));
        assert_eq!(refresh_interval(240), Duration::from_nanos(4_166_666));
    }

    #[test]
    fn nested_output_config_uses_validated_defaults() {
        assert_eq!(
            NestedOutputConfig::default(),
            NestedOutputConfig {
                width: 1280,
                height: 800,
                refresh_hz: 60,
            }
        );
    }

    #[test]
    fn nested_output_minimum_size_never_exceeds_requested_size() {
        assert_eq!(
            nested_output_minimum_size(NestedOutputConfig {
                width: 320,
                height: 240,
                refresh_hz: 60,
            }),
            (320, 240)
        );
        assert_eq!(
            nested_output_minimum_size(NestedOutputConfig {
                width: 1600,
                height: 900,
                refresh_hz: 165,
            }),
            (640, 360)
        );
    }

    #[test]
    fn nested_output_cursor_starts_at_configured_center() {
        assert_eq!(
            nested_output_initial_cursor(NestedOutputConfig {
                width: 1600,
                height: 900,
                refresh_hz: 165,
            }),
            (800, 450)
        );
    }

    #[test]
    fn nested_output_initial_cursor_physical_uses_host_scale() {
        assert_eq!(
            nested_output_initial_cursor_physical(
                NestedOutputConfig {
                    width: 1600,
                    height: 900,
                    refresh_hz: 165,
                },
                1.5,
                2400,
                1350
            ),
            (1200, 675)
        );
    }

    #[test]
    fn nested_output_wakeup_uses_configured_active_interval() {
        let active_interval = refresh_interval(165);
        assert_eq!(
            nested_output_wakeup_interval(active_interval, false, false, false, false, false),
            IDLE_WAKEUP_INTERVAL
        );
        assert_eq!(
            nested_output_wakeup_interval(active_interval, true, false, false, false, false),
            active_interval
        );
        assert_eq!(
            nested_output_wakeup_interval(active_interval, false, true, false, false, false),
            active_interval
        );
        assert_eq!(
            nested_output_wakeup_interval(active_interval, false, false, true, false, false),
            active_interval
        );
    }

    #[test]
    fn nested_output_uses_fast_wakeup_while_waiting_for_input_response() {
        assert_eq!(
            nested_output_wakeup_interval(refresh_interval(165), false, false, false, false, true),
            Duration::from_millis(1)
        );
    }

    #[test]
    fn stale_pending_redraw_request_can_be_retried() {
        let requested_at = Instant::now();

        assert!(should_issue_redraw_request(false, None, requested_at));
        assert!(!should_issue_redraw_request(
            true,
            Some(requested_at),
            requested_at + REDRAW_REQUEST_RETRY_INTERVAL - Duration::from_millis(1)
        ));
        assert!(should_issue_redraw_request(
            true,
            Some(requested_at),
            requested_at + REDRAW_REQUEST_RETRY_INTERVAL
        ));
    }

    #[test]
    fn physical_host_size_is_advertised_as_logical_output_size() {
        assert_eq!(
            logical_output_size_from_physical(1920, 1080, 1.5),
            (1280, 720)
        );
        assert_eq!(
            logical_output_size_from_physical(1280, 800, 0.0),
            (1280, 800)
        );
    }

    #[test]
    fn physical_pointer_coordinates_are_forwarded_as_logical_coordinates() {
        assert_eq!(logical_coordinate_from_physical(300.0, 1.5), 200.0);
        assert_eq!(logical_coordinate_from_physical(300.0, f64::NAN), 300.0);
    }

    #[test]
    fn alt_mouse_buttons_map_to_window_interactions() {
        assert_eq!(
            window_drag_intent(true, MouseButton::Left, ElementState::Pressed),
            Some(WindowDragIntent::Move)
        );
        assert_eq!(
            window_drag_intent(true, MouseButton::Right, ElementState::Pressed),
            Some(WindowDragIntent::Resize)
        );
        assert_eq!(
            window_drag_intent(false, MouseButton::Left, ElementState::Pressed),
            None
        );
    }

    #[test]
    fn alt_keyboard_shortcuts_map_to_window_management_actions() {
        assert_eq!(
            window_management_shortcut(true, KeyCode::KeyM),
            Some(WindowManagementShortcut::Minimize)
        );
        assert_eq!(
            window_management_shortcut(true, KeyCode::KeyR),
            Some(WindowManagementShortcut::RestoreMinimized)
        );
        assert_eq!(
            window_management_shortcut(true, KeyCode::KeyF),
            Some(WindowManagementShortcut::ToggleMaximize)
        );
        assert_eq!(
            window_management_shortcut(true, KeyCode::Enter),
            Some(WindowManagementShortcut::ToggleFullscreen)
        );
        assert_eq!(window_management_shortcut(false, KeyCode::KeyM), None);
    }

    #[test]
    fn system_space_maps_to_spotlight_toggle() {
        assert!(is_spotlight_toggle_shortcut(
            ShortcutModifiers {
                super_pressed: true,
                ctrl_pressed: false,
            },
            KeyCode::Space
        ));
        assert!(is_spotlight_toggle_shortcut(
            ShortcutModifiers {
                super_pressed: false,
                ctrl_pressed: true,
            },
            KeyCode::Space
        ));
        assert!(!is_spotlight_toggle_shortcut(
            ShortcutModifiers {
                super_pressed: false,
                ctrl_pressed: false,
            },
            KeyCode::Space
        ));
        assert!(!is_spotlight_toggle_shortcut(
            ShortcutModifiers {
                super_pressed: false,
                ctrl_pressed: true,
            },
            KeyCode::KeyF
        ));
    }

    #[test]
    fn spotlight_text_falls_back_to_logical_character_when_event_text_is_missing() {
        assert_eq!(
            spotlight_text_for_key(None, &Key::Character("fire".into())).as_deref(),
            Some("fire")
        );
        assert_eq!(
            spotlight_text_for_key(Some("br"), &Key::Character("ignored".into())).as_deref(),
            Some("br")
        );
        assert_eq!(
            spotlight_text_for_key(None, &Key::Named(NamedKey::Enter)),
            None
        );
    }

    #[test]
    fn visible_spotlight_captures_client_pointer_input() {
        assert!(shell_overlay_captures_client_input(true));
        assert!(!shell_overlay_captures_client_input(false));
    }

    #[test]
    fn nested_output_drops_renderer_before_window_surface() {
        use std::{cell::RefCell, rc::Rc};

        #[derive(Debug)]
        struct DropProbe {
            name: &'static str,
            log: Rc<RefCell<Vec<&'static str>>>,
        }

        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.log.borrow_mut().push(self.name);
            }
        }

        let log = Rc::new(RefCell::new(Vec::new()));
        let mut renderer = Some(DropProbe {
            name: "renderer",
            log: Rc::clone(&log),
        });
        let mut window = Some(DropProbe {
            name: "window",
            log: Rc::clone(&log),
        });

        drop_renderer_before_window(&mut renderer, &mut window);

        assert_eq!(log.borrow().as_slice(), ["renderer", "window"]);
    }

    #[test]
    fn nested_output_uses_host_cursor_instead_of_recompositing_cursor() {
        assert!(nested_output_uses_host_cursor());
        assert_eq!(
            nested_visual_state(120, 80),
            DesktopVisualState::wallpaper_only()
        );
    }

    #[test]
    fn pending_host_cursor_warp_matches_with_rounding_tolerance() {
        let pending = NestedPendingCursorWarp {
            physical_x: 120,
            physical_y: 80,
        };

        assert!(host_cursor_warp_matches(pending, 122, 78));
        assert!(!host_cursor_warp_matches(pending, 123, 80));
        assert!(!host_cursor_warp_matches(pending, 120, 77));
    }

    #[test]
    fn nested_scene_generation_tracks_shell_overlay_generation() {
        assert_ne!(
            nested_scene_content_generation(42, 1),
            nested_scene_content_generation(42, 99)
        );
        assert_ne!(
            nested_scene_content_generation(41, 99),
            nested_scene_content_generation(42, 99)
        );
    }

    #[test]
    fn nested_output_uses_active_wakeup_only_when_work_is_pending() {
        let active_interval = refresh_interval(120);
        assert_eq!(
            nested_output_wakeup_interval(active_interval, false, false, false, false, false),
            IDLE_WAKEUP_INTERVAL
        );
        assert_eq!(
            nested_output_wakeup_interval(active_interval, true, false, false, false, false),
            active_interval
        );
        assert_eq!(
            nested_output_wakeup_interval(active_interval, false, true, false, false, false),
            active_interval
        );
        assert_eq!(
            nested_output_wakeup_interval(active_interval, false, false, true, false, false),
            active_interval
        );
    }

    #[test]
    fn locked_host_delta_bridge_forwards_relative_sample() {
        let sample = host_relative_delta_sample(true, 55, 3.5, -2.0).unwrap();
        let relative = sample.relative.unwrap();

        assert_eq!(sample.timestamp_usec, 55);
        assert_eq!(sample.absolute, None);
        assert_eq!(relative.dx, 3.5);
        assert_eq!(relative.dy, -2.0);
        assert_eq!(relative.dx_unaccelerated, 3.5);
        assert_eq!(relative.dy_unaccelerated, -2.0);
    }

    #[test]
    fn host_delta_bridge_forwards_relative_sample_before_lock() {
        let sample = host_relative_delta_sample(false, 55, 3.5, -2.0)
            .expect("raw host deltas should produce relative samples before lock");
        let relative = sample.relative.unwrap();

        assert_eq!(sample.timestamp_usec, 55);
        assert_eq!(sample.absolute, None);
        assert_eq!(relative.dx, 3.5);
        assert_eq!(relative.dy, -2.0);
    }

    #[test]
    fn zero_locked_host_delta_is_ignored() {
        assert!(host_relative_delta_sample(true, 55, 0.0, 0.0).is_none());
    }

    #[test]
    fn nested_output_shutdown_reasons_are_explicit_in_logs() {
        assert_eq!(
            ShutdownReason::HostWindowClosed.log_message(),
            "oblivion-one compositor: nested output closed by host window"
        );
        assert_eq!(
            ShutdownReason::EventLoopExited.log_message(),
            "oblivion-one compositor: nested output event loop exited"
        );
    }
}
