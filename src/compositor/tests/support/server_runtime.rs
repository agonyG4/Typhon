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
    MinimizeFocused,
    RestoreNextMinimized,
    ToggleMaximizeFocused,
    ToggleFullscreenFocused,
    CaptureRenderGeneration(Sender<u64>),
    CaptureSceneRenderGeneration(Sender<u64>),
    CaptureRenderGenerationCause(Sender<RenderGenerationCause>),
    CaptureRenderableSurfaceCount(Sender<usize>),
    CaptureRenderableSurfaceSnapshot(Sender<Vec<RenderableSurfaceSnapshot>>),
    CaptureClientCursorSnapshot(Sender<Option<ClientCursorSnapshot>>),
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
                    ServerCommand::MinimizeFocused => {
                        server.minimize_focused_window();
                    }
                    ServerCommand::RestoreNextMinimized => {
                        server.restore_next_minimized_window();
                    }
                    ServerCommand::ToggleMaximizeFocused => {
                        server.toggle_maximize_focused_window();
                    }
                    ServerCommand::ToggleFullscreenFocused => {
                        server.toggle_fullscreen_focused_window();
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
                    ServerCommand::CaptureRenderableSurfaceSnapshot(reply) => {
                        let _ = reply.send(
                            server
                                .renderable_surfaces()
                                .iter()
                                .map(|surface| RenderableSurfaceSnapshot {
                                    surface_id: surface.surface_id,
                                    width: surface.width,
                                    height: surface.height,
                                    parent_surface_id: surface.placement.parent_surface_id,
                                    local_x: surface.placement.local_x,
                                    local_y: surface.placement.local_y,
                                    buffer_id: surface.buffer_id().get(),
                                    generation: surface.generation,
                                    resize_preview_active: surface.visual_clip.is_some(),
                                })
                                .collect(),
                        );
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
                    ServerCommand::CaptureXdgRoleSnapshot { surface_id, reply } => {
                        let tracked_surface_id = if server.state.toplevel_surfaces.len() == 1 {
                            *server.state.toplevel_surfaces.keys().next().unwrap()
                        } else if server.state.surface_resources.contains_key(&surface_id) {
                            surface_id
                        } else if server.state.surface_resources.len() == 1 {
                            *server.state.surface_resources.keys().next().unwrap()
                        } else {
                            surface_id
                        };
                        let _ = reply.send(XdgRoleSnapshot {
                            surface_registered: server
                                .state
                                .surface_resources
                                .contains_key(&tracked_surface_id),
                            configured: server
                                .state
                                .configured_xdg_surfaces
                                .contains(&tracked_surface_id),
                            toplevel_count: server.state.toplevel_surfaces.len(),
                            toplevel_registered: server
                                .state
                                .toplevel_surfaces
                                .contains_key(&tracked_surface_id),
                            popup_count: server.state.popup_surfaces.len(),
                            window_geometry_present: server
                                .state
                                .surface_window_geometries
                                .contains_key(&tracked_surface_id),
                            placement: server
                                .state
                                .surface_placements
                                .get(&tracked_surface_id)
                                .copied(),
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
                    ServerCommand::Barrier(reply) => {
                        let _ = reply.send(());
                    }
                    ServerCommand::PrepareFrame => {
                        server.prepare_frame();
                    }
                    ServerCommand::FinishFrame => {
                        server.finish_frame();
                    }
                    ServerCommand::FinishFrameWithPresentation(presentation) => {
                        server.finish_frame_with_presentation(presentation);
                    }
                    ServerCommand::PresentFrame => {
                        server.present_frame();
                    }
                    ServerCommand::Stop => running = false,
                }
            }
            let _ = server.tick();
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
