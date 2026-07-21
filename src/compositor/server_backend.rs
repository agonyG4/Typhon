use super::*;
use crate::xwayland::xwm::{RESIZE_SYNC_TIMEOUT_NS, XwmCommand};

impl OwnCompositorServer {
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
