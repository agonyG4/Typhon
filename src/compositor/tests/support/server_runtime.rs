#![allow(unused_imports)]
use super::super::*;
use super::{
    client_setup::*, clipboard_dmabuf::*, frame_buffer_client::*, input_client::*,
    locked_relative::*, output_bindings::*, registry_state::*, subsurface_client::*, window_ops::*,
};
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
    KeyboardKey {
        key: u32,
        pressed: bool,
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
    MinimizeFocused,
    RestoreNextMinimized,
    FocusRootWindow(u32),
    RaiseRootWindow(u32),
    ToggleMaximizeFocused,
    ToggleFullscreenFocused,
    SetFocusedRootVisualGeometry {
        placement: SurfacePlacement,
        width: u32,
        height: u32,
    },
    CaptureRenderGeneration(Sender<u64>),
    CaptureSceneRenderGeneration(Sender<u64>),
    CaptureRenderGenerationCause(Sender<RenderGenerationCause>),
    CaptureRenderableSurfaceCount(Sender<usize>),
    CaptureSurfaceResourceCount(Sender<usize>),
    CaptureRenderableSurfaceSnapshot(Sender<Vec<RenderableSurfaceSnapshot>>),
    CaptureCommittedWindowGeometry(Sender<Option<XdgWindowGeometry>>),
    CaptureToplevelVisualGeometry(Sender<Option<ToplevelVisualGeometrySnapshot>>),
    CaptureFullscreenPresentationEligibility(Sender<FullscreenPresentationEligibility>),
    CaptureClientCursorSnapshot(Sender<Option<ClientCursorSnapshot>>),
    CaptureInteractionCursorState(Sender<InteractionCursorStateSnapshot>),
    CaptureClipboardState(Sender<ClipboardStateSnapshot>),
    CaptureXdgRoleSnapshot {
        surface_id: u32,
        reply: Sender<XdgRoleSnapshot>,
    },
    CapturePendingFrameCallbacks(Sender<bool>),
    CaptureOnlyPendingSurfaceFrameCallbacks(Sender<bool>),
    CapturePendingFrameWork(Sender<bool>),
    CaptureIdleInhibited(Sender<bool>),
    CapturePointerConstraintBackendRequests(Sender<Vec<PointerConstraintBackendRequest>>),
    CapturePointerConstraintIds(Sender<Vec<u64>>),
    CaptureLastPointerPosition(Sender<(f64, f64)>),
    CapturePointerFocusSurfaceId(Sender<Option<u32>>),
    CaptureFocusedSurfaceId(Sender<Option<u32>>),
    CaptureUsableOutputGeometry(Sender<OutputRect>),
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
    PointerConstraintBackendActivated(PointerConstraintBackendId),
    PointerConstraintBackendFailed(PointerConstraintBackendId),
    #[allow(dead_code)]
    PointerConstraintBackendDeactivated(PointerConstraintBackendId),
    ClearPointerEnterTracking,
    Barrier(Sender<()>),
    PrepareFrame,
    FinishFrame,
    FinishFrameWithPresentation(FramePresentation),
    CaptureFrameBatch {
        frame_id: u64,
        reply: Sender<CompositorFrameBatchId>,
    },
    CaptureFrameBatchSurfaceIds {
        batch_id: CompositorFrameBatchId,
        reply: Sender<Vec<u32>>,
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
    CompleteDirectFrameBatch {
        frame_id: u64,
        batch_id: CompositorFrameBatchId,
        direct_surface_id: u32,
        presentation: FramePresentation,
    },
    DiscardFrameBatch {
        batch_id: CompositorFrameBatchId,
        reason: FrameBatchDiscardReason,
    },
    PresentFrame,
    Stop,
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
                    ServerCommand::KeyboardKey { key, pressed } => {
                        server.send_keyboard_key(key, pressed);
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
                    ServerCommand::MinimizeFocused => {
                        server.minimize_focused_window();
                    }
                    ServerCommand::RestoreNextMinimized => {
                        server.restore_next_minimized_window();
                    }
                    ServerCommand::FocusRootWindow(surface_id) => {
                        if let Some(surface) = server.state.surface_resource_by_id(surface_id) {
                            server.state.focus_surface(surface);
                        }
                    }
                    ServerCommand::RaiseRootWindow(surface_id) => {
                        server.state.raise_root_window(surface_id);
                    }
                    ServerCommand::ToggleMaximizeFocused => {
                        server.toggle_maximize_focused_window();
                    }
                    ServerCommand::ToggleFullscreenFocused => {
                        server.toggle_fullscreen_focused_window();
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
                    ServerCommand::CaptureRenderGeneration(reply) => {
                        let _ = reply.send(server.render_generation());
                    }
                    ServerCommand::CaptureSceneRenderGeneration(reply) => {
                        let _ = reply.send(server.scene_render_generation());
                    }
                    ServerCommand::CaptureRenderGenerationCause(reply) => {
                        let _ = reply.send(server.render_generation_cause());
                    }
                    ServerCommand::CaptureRenderableSurfaceCount(reply) => {
                        let _ = reply.send(server.renderable_surfaces().len());
                    }
                    ServerCommand::CaptureSurfaceResourceCount(reply) => {
                        let _ = reply.send(server.state.surface_resources.len());
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
                                        origin_x,
                                        origin_y,
                                        buffer_id: surface.buffer_id().get(),
                                        generation: surface.generation,
                                        resize_preview_active: surface.visual_clip.is_some(),
                                    },
                                )
                                .collect(),
                        );
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
                    ServerCommand::CaptureFullscreenPresentationEligibility(reply) => {
                        let _ = reply.send(server.state.fullscreen_presentation_eligibility());
                    }
                    ServerCommand::CaptureClientCursorSnapshot(reply) => {
                        let snapshot = server.client_cursor_render_state().map(|cursor| {
                            ClientCursorSnapshot {
                                surface_id: cursor.surface.surface_id,
                                logical_x: cursor.logical_x,
                                logical_y: cursor.logical_y,
                                width: cursor.surface.width,
                                height: cursor.surface.height,
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
                    ServerCommand::CaptureClipboardState(reply) => {
                        let _ = reply.send(ClipboardStateSnapshot {
                            active_source: server.state.active_clipboard.is_some(),
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
                            toplevel_has_app_id: toplevel
                                .is_some_and(|toplevel| toplevel.app_id.is_some()),
                            toplevel_has_title: toplevel
                                .is_some_and(|toplevel| toplevel.title.is_some()),
                            toplevel_has_non_default_constraints: toplevel.is_some_and(
                                |toplevel| {
                                    toplevel.constraints != ToplevelSizeConstraints::default()
                                        || toplevel.pending_constraints.is_some()
                                },
                            ),
                            toplevel_mode: toplevel.map(|toplevel| toplevel.window.mode()),
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
                        });
                    }
                    ServerCommand::CapturePendingFrameCallbacks(reply) => {
                        let _ = reply.send(server.has_pending_frame_callbacks());
                    }
                    ServerCommand::CaptureOnlyPendingSurfaceFrameCallbacks(reply) => {
                        let _ = reply.send(server.has_only_pending_surface_frame_callbacks());
                    }
                    ServerCommand::CapturePendingFrameWork(reply) => {
                        let _ = reply.send(server.has_pending_frame_work());
                    }
                    ServerCommand::CaptureIdleInhibited(reply) => {
                        let _ = reply.send(server.state.idle_inhibited());
                    }
                    ServerCommand::CapturePointerConstraintBackendRequests(reply) => {
                        let _ = reply.send(server.take_pointer_constraint_backend_requests());
                    }
                    ServerCommand::CapturePointerConstraintIds(reply) => {
                        let ids = server.state.pointer_constraints.keys().copied().collect();
                        let _ = reply.send(ids);
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
                    ServerCommand::CaptureFocusedSurfaceId(reply) => {
                        let _ = reply.send(
                            server
                                .state
                                .focused_surface
                                .as_ref()
                                .map(compositor_surface_id),
                        );
                    }
                    ServerCommand::CaptureUsableOutputGeometry(reply) => {
                        let _ = reply.send(server.state.usable_output_geometry());
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
                    ServerCommand::FinishFrame => {
                        server.finish_frame();
                    }
                    ServerCommand::FinishFrameWithPresentation(presentation) => {
                        server.finish_frame_with_presentation(presentation);
                    }
                    ServerCommand::CaptureFrameBatch { frame_id, reply } => {
                        let _ = reply.send(server.take_frame_batch_for_render(frame_id));
                    }
                    ServerCommand::CaptureFrameBatchSurfaceIds { batch_id, reply } => {
                        let _ =
                            reply.send(server.test_frame_batch_presentation_surface_ids(batch_id));
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
                    ServerCommand::DiscardFrameBatch { batch_id, reason } => {
                        server.discard_frame_batch(batch_id, reason);
                    }
                    ServerCommand::PresentFrame => {
                        server.present_frame();
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
