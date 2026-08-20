use super::*;
use crate::xwayland::trace::{self, TraceFields};
use crate::xwayland::xwm::{
    ConfigureSource, RESIZE_SYNC_TIMEOUT_NS, X11ConfigureRequest, X11Geometry,
    X11MoveResizeDirection as Direction, X11MoveResizeRequest, XwmCommand,
};

impl OwnCompositorServer {
    pub(super) fn trace_x11_configure_request_normalized(
        &self,
        window: X11WindowHandle,
        request: X11ConfigureRequest,
        current_authoritative: Option<X11Geometry>,
        desired: X11Geometry,
    ) {
        let root_surface_id = self
            .state
            .window_id_for_x11_handle(window)
            .and_then(|id| self.state.window(id))
            .map(|window| window.root_surface_id);
        let visual_geometry = root_surface_id
            .and_then(|surface_id| self.state.current_visual_root_window_geometry(surface_id))
            .map(|value| format!("{value:?}"));
        let committed_content_extent = root_surface_id.and_then(|surface_id| {
            self.renderable_surfaces()
                .iter()
                .find(|surface| surface.surface_id == surface_id)
                .map(|surface| format!("{}x{}", surface.width, surface.height))
        });
        let interaction = self.state.window_interaction_debug_snapshot();
        trace::emit("x11_configure_request_normalized", || {
            TraceFields::new()
                .field("source", "compositor")
                .field("xid", window.xid())
                .field("requested", format!("{:?}", request.requested))
                .field("fields", format!("{:?}", request.fields))
                .optional("client_event_sequence", request.client_event_sequence)
                .optional(
                    "current_authoritative_geometry",
                    current_authoritative.map(|value| format!("{value:?}")),
                )
                .field("desired_geometry", format!("{desired:?}"))
                .field("active_resize", self.state.x11_resize_active(window))
                .optional(
                    "active_interaction_id",
                    interaction.map(|value| value.interaction_id),
                )
                .optional(
                    "active_resize_interaction_id",
                    interaction.and_then(|value| value.resize_interaction_id),
                )
                .optional("visual_geometry", visual_geometry)
                .optional("committed_content_extent", committed_content_extent)
        });
    }

    pub(super) fn x11_configure_request_geometry(
        &self,
        window: X11WindowHandle,
        request: X11ConfigureRequest,
        constraints: WindowConstraints,
    ) -> X11Geometry {
        let current = self
            .state
            .x11_authoritative_geometry(window)
            .unwrap_or(request.requested);
        crate::xwayland::xwm::icccm::apply_configure_request(
            current,
            request.requested,
            request.fields,
            constraints,
        )
    }

    pub(super) fn handle_x11_move_resize_request(
        &mut self,
        window: X11WindowHandle,
        request: X11MoveResizeRequest,
    ) {
        let kind = match request.direction {
            Direction::TopLeft => Some(WindowInteractionKind::Resize(ResizeEdges::new(
                true, false, true, false,
            ))),
            Direction::Top => Some(WindowInteractionKind::Resize(ResizeEdges::new(
                true, false, false, false,
            ))),
            Direction::TopRight => Some(WindowInteractionKind::Resize(ResizeEdges::new(
                true, false, false, true,
            ))),
            Direction::Right => Some(WindowInteractionKind::Resize(ResizeEdges::new(
                false, false, false, true,
            ))),
            Direction::BottomRight => {
                Some(WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT))
            }
            Direction::Bottom => Some(WindowInteractionKind::Resize(ResizeEdges::new(
                false, true, false, false,
            ))),
            Direction::BottomLeft => Some(WindowInteractionKind::Resize(ResizeEdges::new(
                false, true, true, false,
            ))),
            Direction::Left => Some(WindowInteractionKind::Resize(ResizeEdges::new(
                false, false, true, false,
            ))),
            Direction::Move => Some(WindowInteractionKind::Move),
            Direction::Cancel => {
                let _ = self.state.cancel_x11_client_window_interaction(window);
                None
            }
            Direction::KeyboardSize | Direction::KeyboardMove => None,
        };
        if let Some(kind) = kind {
            let _ = self.state.begin_x11_client_window_interaction(
                window,
                f64::from(request.root_x),
                f64::from(request.root_y),
                kind,
                request.button,
            );
        }
    }

    pub fn take_xwayland_backend_commands(&mut self, now_ns: u64) -> Vec<XwmCommand> {
        self.state
            .take_backend_commands()
            .into_iter()
            .filter_map(|command| {
                match command {
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
                    let position_only = !resizing && self.state.x11_resize_active(handle);
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
                            fields: if position_only {
                                crate::xwayland::xwm::X11ConfigureFlags {
                                    x: true,
                                    y: true,
                                    ..Default::default()
                                }
                            } else {
                                crate::xwayland::xwm::X11ConfigureFlags::all()
                            },
                            source: ConfigureSource::Compositor,
                            border_width: 0,
                        })
                    }
                }
                crate::compositor::window_backend::WindowBackendCommand::FinalizeResize {
                    window,
                    geometry,
                    mode: _,
                } => {
                    let handle = match self.state.window(window)?.backend {
                        super::WindowBackend::X11(handle) => handle,
                        super::WindowBackend::Xdg(_) => return None,
                    };
                    Some(XwmCommand::BeginResizeSync {
                        window: handle,
                        geometry: crate::xwayland::xwm::X11Geometry {
                            x: geometry.placement.local_x,
                            y: geometry.placement.local_y,
                            width: geometry.width,
                            height: geometry.height,
                        },
                        counter_value: 0,
                        deadline_ns: now_ns.saturating_add(RESIZE_SYNC_TIMEOUT_NS),
                        final_pending: true,
                    })
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
                crate::compositor::window_backend::WindowBackendCommand::Restack { window } => {
                    let handle = match self.state.window(window)?.backend {
                        super::WindowBackend::X11(handle) => handle,
                        super::WindowBackend::Xdg(_) => return None,
                    };
                    let (client_list, stacking) = self.state.x11_client_lists();
                    Some(XwmCommand::RaiseAndSync {
                        window: handle,
                        client_list,
                        stacking,
                    })
                }
                crate::compositor::window_backend::WindowBackendCommand::RestackExact {
                    windows,
                } => {
                    let order = windows
                        .into_iter()
                        .filter_map(|window| match self.state.window(window)?.backend {
                            super::WindowBackend::X11(handle) => Some(handle),
                            super::WindowBackend::Xdg(_) => None,
                        })
                        .collect();
                    let (client_list, stacking) = self.state.x11_client_lists();
                    Some(XwmCommand::RestackExact {
                        order,
                        client_list,
                        stacking,
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
                        frame_extents: self.state.x11_decoration_frame_extents(handle),
                    })
                }
                crate::compositor::window_backend::WindowBackendCommand::SetWorkspace {
                    window,
                    workspace,
                } => {
                    let handle = match self.state.window(window)?.backend {
                        super::WindowBackend::X11(handle) => handle,
                        super::WindowBackend::Xdg(_) => return None,
                    };
                    Some(XwmCommand::SetWorkspace {
                        window: handle,
                        workspace,
                    })
                }
                crate::compositor::window_backend::WindowBackendCommand::PublishWorkspaceState {
                    workspace_count,
                    current_workspace,
                    output_width,
                    output_height,
                } => Some(XwmCommand::PublishDesktopState {
                    workspace_count,
                    current_workspace,
                    output_width,
                    output_height,
                }),
            }
            })
            .collect()
    }
}
