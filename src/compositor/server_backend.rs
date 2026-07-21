use super::*;
use crate::xwayland::xwm::{
    RESIZE_SYNC_TIMEOUT_NS, X11ConfigureRequest, X11Geometry, X11MoveResizeDirection as Direction,
    X11MoveResizeRequest, XwmCommand,
};

impl OwnCompositorServer {
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
}
