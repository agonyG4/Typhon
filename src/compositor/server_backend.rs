use super::*;
use crate::xwayland::trace::{self, TraceFields};
use crate::xwayland::xwm::{
    ConfigureSource, RESIZE_SYNC_TIMEOUT_NS, X11ConfigureRequest, X11Geometry,
    X11MoveResizeDirection as Direction, X11MoveResizeRequest, XwmCommand,
};

pub(super) struct X11ConfigureRequestGeometry {
    pub icccm_geometry: X11Geometry,
    pub accepted_geometry: X11Geometry,
    pub constraint_reason: String,
    pub usable_area: crate::compositor::OutputRect,
    pub frame_extents: [u32; 4],
}

fn trace_x11_resize_backend_command(
    window: X11WindowHandle,
    command_kind: &'static str,
    decision: &'static str,
    resize_epoch: Option<u64>,
    current_resize_epoch: Option<u64>,
    geometry: X11Geometry,
    reason: &'static str,
    translation: Option<&'static str>,
) {
    trace::emit("xwayland_resize_backend_command", || {
        TraceFields::new()
            .field("source", "compositor")
            .field("xid", window.xid())
            .field("command", command_kind)
            .field("decision", decision)
            .optional("resize_epoch", resize_epoch)
            .optional("current_resize_epoch", current_resize_epoch)
            .field("geometry", format!("{geometry:?}"))
            .field("reason", reason)
            .optional("translation", translation)
    });
}

impl OwnCompositorServer {
    pub(super) fn trace_x11_configure_request_normalized(
        &self,
        window: X11WindowHandle,
        request: X11ConfigureRequest,
        current_authoritative: Option<X11Geometry>,
        normalization: &X11ConfigureRequestGeometry,
    ) {
        let window_id = self.state.window_id_for_x11_handle(window);
        let managed = window_id.and_then(|id| self.state.window(id));
        let pending_restore =
            window_id.is_some_and(|id| self.state.pending_xwayland_restores.contains_key(&id));
        let root_surface_id = managed.map(|window| window.root_surface_id);
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
        let usable = normalization.usable_area;
        let [frame_left, frame_right, frame_top, frame_bottom] = normalization.frame_extents;
        trace::emit("x11_configure_request_normalized", || {
            TraceFields::new()
                .field("source", "compositor")
                .optional("window_id", window_id.map(|id| id.get()))
                .field("xid", window.xid())
                .field("requested", format!("{:?}", request.requested))
                .field("fields", format!("{:?}", request.fields))
                .optional("client_event_sequence", request.client_event_sequence)
                .optional(
                    "current_authoritative_geometry",
                    current_authoritative.map(|value| format!("{value:?}")),
                )
                .field(
                    "icccm_geometry",
                    format!("{:?}", normalization.icccm_geometry),
                )
                .field(
                    "wm_accepted_geometry",
                    format!("{:?}", normalization.accepted_geometry),
                )
                .field(
                    "constraint_reason",
                    normalization.constraint_reason.as_str(),
                )
                .field(
                    "constrained",
                    normalization.icccm_geometry != normalization.accepted_geometry,
                )
                .field("usable_x", usable.x)
                .field("usable_y", usable.y)
                .field("usable_width", usable.width)
                .field("usable_height", usable.height)
                .field("frame_extents_left", frame_left)
                .field("frame_extents_right", frame_right)
                .field("frame_extents_top", frame_top)
                .field("frame_extents_bottom", frame_bottom)
                .optional(
                    "window_minimized",
                    managed.map(|window| window.state.is_minimized()),
                )
                .optional(
                    "pending_restore",
                    window_id.map(|id| self.state.pending_xwayland_restores.contains_key(&id)),
                )
                .optional("active_resize", Some(self.state.x11_resize_active(window)))
                .optional(
                    "layout_membership",
                    managed
                        .and_then(|window| window.management)
                        .map(|value| format!("{:?}", value.layout())),
                )
                .optional(
                    "mode",
                    managed.map(|window| format!("{:?}", window.state.mode())),
                )
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
        if pending_restore
            && matches!(
                std::env::var("ASTREA_LIFECYCLE_DIAGNOSTICS").as_deref(),
                Ok("1" | "true" | "on")
            )
        {
            let [left, right, top, bottom] = normalization.frame_extents;
            let mapping_lifecycle = managed.map_or("unknown", |window| {
                if window.state.is_minimized() {
                    "iconic_restore_pending"
                } else {
                    "normal_restore_pending"
                }
            });
            let accepted = normalization.accepted_geometry;
            let icccm = normalization.icccm_geometry;
            eprintln!(
                "typhon.x11_geometry event=configure_request_during_restore window_id={} xid={} generation={} surface_id={} pending_restore=true logical_minimized={} mapping_lifecycle={} frame_extents_left={} frame_extents_right={} frame_extents_top={} frame_extents_bottom={} raw_x={} raw_y={} raw_width={} raw_height={} icccm_x={} icccm_y={} icccm_width={} icccm_height={} wm_accepted_x={} wm_accepted_y={} wm_accepted_width={} wm_accepted_height={} constraint_reason={}",
                window_id.map_or_else(|| "none".to_owned(), |id| id.get().to_string()),
                window.xid(),
                window.generation().get(),
                managed
                    .and_then(|managed| managed.x11_surface_id)
                    .map_or_else(|| "none".to_owned(), |surface_id| surface_id.to_string()),
                managed.is_some_and(|window| window.state.is_minimized()),
                mapping_lifecycle,
                left,
                right,
                top,
                bottom,
                request.requested.x,
                request.requested.y,
                request.requested.width,
                request.requested.height,
                icccm.x,
                icccm.y,
                icccm.width,
                icccm.height,
                accepted.x,
                accepted.y,
                accepted.width,
                accepted.height,
                normalization.constraint_reason,
            );
        }
    }

    pub(super) fn x11_configure_request_geometry(
        &self,
        window: X11WindowHandle,
        request: X11ConfigureRequest,
        constraints: WindowConstraints,
    ) -> X11ConfigureRequestGeometry {
        let current = self
            .state
            .x11_authoritative_geometry(window)
            .unwrap_or(request.requested);
        let icccm_geometry = crate::xwayland::xwm::icccm::apply_configure_request(
            current,
            request.requested,
            request.fields,
            constraints,
        );
        let usable_area = self.state.usable_output_geometry();
        let frame_extents = self.state.x11_decoration_frame_extents(window);
        let managed = self
            .state
            .window_id_for_x11_handle(window)
            .and_then(|window_id| self.state.window(window_id))
            .filter(|managed| managed.kind == crate::compositor::DesktopWindowKind::Managed);
        let policy = crate::compositor::x11_geometry_policy::ManagedX11GeometryPolicy {
            role: managed.and_then(|window| window.x11_role),
            placement: managed.and_then(|window| window.x11_placement_policy),
            mode: managed.map_or(crate::compositor::ToplevelMode::Normal, |window| {
                window.state.mode()
            }),
            tiled: managed.is_some_and(|window| {
                window.management.is_some_and(|management| {
                    management.layout() == crate::wm::LayoutMembership::Tiled
                })
            }),
            usable_area,
            frame_extents,
        };

        let mut reasons = Vec::new();
        if (request.fields.x && icccm_geometry.x != request.requested.x)
            || (request.fields.y && icccm_geometry.y != request.requested.y)
            || (request.fields.width && icccm_geometry.width != request.requested.width)
            || (request.fields.height && icccm_geometry.height != request.requested.height)
        {
            reasons.push("icccm");
        }

        let policy_result =
            crate::compositor::x11_geometry_policy::apply_managed_x11_geometry_policy(
                icccm_geometry,
                current,
                request.fields,
                &policy,
            );
        reasons.extend(policy_result.reasons);

        X11ConfigureRequestGeometry {
            icccm_geometry,
            accepted_geometry: policy_result.accepted,
            constraint_reason: if reasons.is_empty() {
                "none".to_string()
            } else {
                reasons.join("+")
            },
            usable_area,
            frame_extents,
        }
    }

    pub(super) fn trace_x11_client_configure_authority_change(
        &self,
        handle: X11WindowHandle,
        request: X11ConfigureRequest,
        normalization: &X11ConfigureRequestGeometry,
        old_client: Option<X11Geometry>,
        old_frame: Option<crate::compositor::WindowGeometry>,
        old_materialized: Option<crate::compositor::PresentationRect>,
    ) {
        if !matches!(
            std::env::var("ASTREA_LIFECYCLE_DIAGNOSTICS").as_deref(),
            Ok("1" | "true" | "on")
        ) {
            return;
        }
        let Some(window_id) = self.state.window_id_for_x11_handle(handle) else {
            return;
        };
        let Some(window) = self.state.window(window_id) else {
            return;
        };
        let new_client = window.x11_geometry.map(|geometry| geometry.client);
        let new_materialized = window.x11_materialized_client_rect;
        let client_fields = |prefix: &str, value: Option<X11Geometry>| match value {
            Some(value) => format!(
                "{prefix}_x={} {prefix}_y={} {prefix}_width={} {prefix}_height={}",
                value.x, value.y, value.width, value.height,
            ),
            None => {
                format!("{prefix}_x=none {prefix}_y=none {prefix}_width=none {prefix}_height=none")
            }
        };
        let rect_fields =
            |prefix: &str, rect: Option<crate::compositor::PresentationRect>| match rect {
                Some(rect) => format!(
                    "{prefix}_x={:.1} {prefix}_y={:.1} {prefix}_width={:.1} {prefix}_height={:.1}",
                    rect.x(),
                    rect.y(),
                    rect.width(),
                    rect.height(),
                ),
                None => format!(
                    "{prefix}_x=none {prefix}_y=none {prefix}_width=none {prefix}_height=none"
                ),
            };
        let usable = normalization.usable_area;
        let [left, right, top, bottom] = normalization.frame_extents;
        let layout = window
            .management
            .map(|management| format!("{:?}", management.layout()))
            .unwrap_or_else(|| "none".to_string());
        eprintln!(
            "typhon.x11_geometry event=authority_change window_id={} xid={} source=client_configure_request window_minimized={} pending_restore={} active_resize={} layout_membership={} mode={:?} {} {} old_frame={:?} new_frame={:?} {} {} requested_x={} requested_y={} requested_width={} requested_height={} request_fields={:?} icccm_x={} icccm_y={} icccm_width={} icccm_height={} wm_accepted_x={} wm_accepted_y={} wm_accepted_width={} wm_accepted_height={} constraint_reason={} usable_x={:.1} usable_y={:.1} usable_width={:.1} usable_height={:.1} frame_extents_left={} frame_extents_right={} frame_extents_top={} frame_extents_bottom={}",
            window_id.get(),
            handle.xid(),
            window.state.is_minimized(),
            self.state
                .pending_xwayland_restores
                .contains_key(&window_id),
            self.state.x11_resize_active(handle),
            layout,
            window.state.mode(),
            client_fields("old_client", old_client),
            client_fields("new_client", new_client),
            old_frame,
            window.x11_geometry.map(|geometry| geometry.frame),
            rect_fields("old_materialized", old_materialized),
            rect_fields("new_materialized", new_materialized),
            request.requested.x,
            request.requested.y,
            request.requested.width,
            request.requested.height,
            request.fields,
            normalization.icccm_geometry.x,
            normalization.icccm_geometry.y,
            normalization.icccm_geometry.width,
            normalization.icccm_geometry.height,
            normalization.accepted_geometry.x,
            normalization.accepted_geometry.y,
            normalization.accepted_geometry.width,
            normalization.accepted_geometry.height,
            normalization.constraint_reason,
            usable.x,
            usable.y,
            usable.width,
            usable.height,
            left,
            right,
            top,
            bottom,
        );
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
                    resize_epoch,
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
                    let current_resize_epoch = self.state.x11_resize_interaction_epoch(handle);
                    if resizing {
                        if resize_epoch.is_none() || current_resize_epoch != resize_epoch {
                            trace_x11_resize_backend_command(
                                handle,
                                "intermediate_configure",
                                "discarded",
                                resize_epoch,
                                current_resize_epoch,
                                x11_geometry,
                                if resize_epoch.is_none() {
                                    "missing_enqueue_epoch"
                                } else {
                                    "resize_epoch_superseded"
                                },
                                None,
                            );
                            return None;
                        }
                        trace_x11_resize_backend_command(
                            handle,
                            "intermediate_configure",
                            "translated",
                            resize_epoch,
                            current_resize_epoch,
                            x11_geometry,
                            "resize_epoch_matches",
                            Some("begin_resize_sync"),
                        );
                        return Some(XwmCommand::BeginResizeSync {
                            window: handle,
                            geometry: x11_geometry,
                            counter_value: 0,
                            deadline_ns: now_ns.saturating_add(RESIZE_SYNC_TIMEOUT_NS),
                            final_pending: false,
                            resize_epoch,
                        });
                    }
                    match (resize_epoch, current_resize_epoch) {
                        (None, _) => Some(XwmCommand::ConfigureFrame {
                            window: handle,
                            geometry: x11_geometry,
                            frame_extents: self.state.x11_decoration_frame_extents(handle),
                        }),
                        (Some(stored_epoch), Some(current_epoch))
                            if stored_epoch == current_epoch =>
                        {
                            trace_x11_resize_backend_command(
                                handle,
                                "position_only_configure",
                                "translated",
                                Some(stored_epoch),
                                Some(current_epoch),
                                x11_geometry,
                                "resize_epoch_matches",
                                Some("configure"),
                            );
                            Some(XwmCommand::Configure {
                                window: handle,
                                geometry: x11_geometry,
                                fields: crate::xwayland::xwm::X11ConfigureFlags {
                                    x: true,
                                    y: true,
                                    ..Default::default()
                                },
                                source: ConfigureSource::Compositor,
                                border_width: 0,
                                resize_epoch: Some(stored_epoch),
                            })
                        }
                        (Some(stored_epoch), None) => {
                            trace_x11_resize_backend_command(
                                handle,
                                "position_only_configure",
                                "translated",
                                Some(stored_epoch),
                                None,
                                x11_geometry,
                                "resize_context_retired",
                                Some("configure_frame"),
                            );
                            Some(XwmCommand::ConfigureFrame {
                                window: handle,
                                geometry: x11_geometry,
                                frame_extents: self.state.x11_decoration_frame_extents(handle),
                            })
                        }
                        (Some(stored_epoch), Some(current_epoch)) => {
                            trace_x11_resize_backend_command(
                                handle,
                                "position_only_configure",
                                "discarded",
                                Some(stored_epoch),
                                Some(current_epoch),
                                x11_geometry,
                                "newer_resize_epoch_superseded",
                                None,
                            );
                            None
                        }
                    }
                }
                crate::compositor::window_backend::WindowBackendCommand::FinalizeResize {
                    window,
                    geometry,
                    mode: _,
                    resize_epoch,
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
                    let current_resize_epoch = self.state.x11_resize_interaction_epoch(handle);
                    if resize_epoch.is_none()
                        || current_resize_epoch != resize_epoch
                    {
                        trace_x11_resize_backend_command(
                            handle,
                            "finalize_resize",
                            "discarded",
                            resize_epoch,
                            current_resize_epoch,
                            x11_geometry,
                            if resize_epoch.is_none() {
                                "missing_enqueue_epoch"
                            } else {
                                "resize_epoch_superseded"
                            },
                            None,
                        );
                        return None;
                    }
                    trace_x11_resize_backend_command(
                        handle,
                        "finalize_resize",
                        "translated",
                        resize_epoch,
                        current_resize_epoch,
                        x11_geometry,
                        "resize_epoch_matches",
                        Some("begin_resize_sync"),
                    );
                    Some(XwmCommand::BeginResizeSync {
                        window: handle,
                        geometry: x11_geometry,
                        counter_value: 0,
                        deadline_ns: now_ns.saturating_add(RESIZE_SYNC_TIMEOUT_NS),
                        final_pending: true,
                        resize_epoch,
                    })
                }
                crate::compositor::window_backend::WindowBackendCommand::Close { window } => {
                    let handle = match self.state.window(window)?.backend {
                        super::WindowBackend::X11(handle) => handle,
                        super::WindowBackend::Xdg(_) => return None,
                    };
                    Some(XwmCommand::Close(handle))
                }
                crate::compositor::window_backend::WindowBackendCommand::SetMapped {
                    window,
                    mapped,
                } => {
                    let handle = match self.state.window(window)?.backend {
                        super::WindowBackend::X11(handle) => handle,
                        super::WindowBackend::Xdg(_) => return None,
                    };
                    Some(if mapped {
                        XwmCommand::Map(handle)
                    } else {
                        XwmCommand::Unmap(handle)
                    })
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
                crate::compositor::window_backend::WindowBackendCommand::ClearWorkspace { window } => {
                    let handle = match self.state.window(window)?.backend {
                        super::WindowBackend::X11(handle) => handle,
                        super::WindowBackend::Xdg(_) => return None,
                    };
                    Some(XwmCommand::ClearWorkspace { window: handle })
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
