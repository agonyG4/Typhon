use super::*;
use crate::xwayland::trace::{self, TraceCategory, TraceFields};
use crate::xwayland::xwm::XwmCommand;

impl OwnCompositorServer {
    pub(super) fn apply_xwayland_window_ready(
        &mut self,
        snapshot: crate::xwayland::xwm::X11WindowSnapshot,
    ) -> Vec<XwmCommand> {
        let surface_id = snapshot.surface_id;
        let handle = snapshot.handle;
        let snapshot_for_trace = snapshot.clone();
        let focus_before = self.focused_x11_window_xid();
        match self.state.insert_x11_window(snapshot) {
            Ok(window_id) => {
                let published = self
                    .state
                    .adopt_current_xwayland_surface_content(surface_id);
                let wants_initial_focus = self.state.x11_window_wants_initial_focus(window_id);
                let focus_outcome = if wants_initial_focus {
                    self.state
                        .focus_desktop_window(window_id, WindowFocusReason::ShellActivation)
                } else {
                    WindowFocusOutcome::Unavailable
                };
                self.state.refresh_pointer_focus_at_last_position();
                let focus_after = self.focused_x11_window_xid();
                trace::emit("focus_decision", || {
                    TraceFields::new()
                        .field("source", "compositor")
                        .field("xid", handle.xid())
                        .field("surface_id", surface_id)
                        .field("focus_decision", "initial_focus")
                        .field("focus_requested", wants_initial_focus)
                        .field("focus_result", format!("{focus_outcome:?}"))
                        .optional("focus_before", focus_before)
                        .optional("focus_after", focus_after)
                        .field(
                            "window_types",
                            format!("{:?}", snapshot_for_trace.window_types),
                        )
                        .field(
                            "override_redirect_stored",
                            snapshot_for_trace.override_redirect,
                        )
                        .optional(
                            "transient_for",
                            snapshot_for_trace.transient_for.map(|parent| parent.xid()),
                        )
                });
                eprintln!(
                    "oblivion-one compositor: event=xwayland_window_admitted surface_id={surface_id} retained_buffer={published} published={published} focus_outcome={focus_outcome:?}"
                );
                let mut commands = Vec::with_capacity(3);
                if let Some(management) = self
                    .state
                    .window(window_id)
                    .and_then(|window| window.management)
                {
                    match management.location() {
                        crate::wm::WorkspaceLocation::Regular(workspace) => {
                            commands.push(XwmCommand::SetWorkspace {
                                window: handle,
                                workspace: workspace.to_ewmh(),
                            });
                        }
                        crate::wm::WorkspaceLocation::Special(_) => {
                            commands.push(XwmCommand::ClearWorkspace { window: handle });
                        }
                    }
                }
                if self
                    .state
                    .window(window_id)
                    .and_then(|window| window.x11_placement_policy)
                    == Some(
                        crate::compositor::desktop_window::X11PlacementPolicy::CompositorManaged,
                    )
                    && let Some(geometry) = self.state.x11_authoritative_geometry(handle)
                {
                    commands.push(XwmCommand::ConfigureFrame {
                        window: handle,
                        geometry,
                        frame_extents: self.state.x11_decoration_frame_extents(handle),
                    });
                }
                if !self.state.defer_client_list_sync() {
                    commands.push(self.sync_xwayland_client_lists());
                }
                commands
            }
            Err(error) => {
                let override_redirect = snapshot_for_trace.override_redirect;
                trace::emit_category(
                    TraceCategory::Lifecycle,
                    "xwayland_window_admission_failed",
                    || {
                        TraceFields::new()
                            .field("source", "compositor")
                            .field("xid", handle.xid())
                            .field("generation", handle.generation().get())
                            .field("surface_id", surface_id)
                            .field("override_redirect", override_redirect)
                            .field("terminal_reason", "admission_rejected")
                    },
                );
                eprintln!(
                    "oblivion-one compositor: event=xwayland_window_admission_failed surface_id={surface_id} error={error:?}"
                );
                Vec::new()
            }
        }
    }

    pub(super) fn apply_xwayland_window_teardown(
        &mut self,
        handle: X11WindowHandle,
        destroyed: bool,
    ) -> Vec<XwmCommand> {
        let focus_before = self.focused_x11_window_xid();
        if self.remove_x11_desktop_window(handle) {
            self.state.refresh_pointer_focus_at_last_position();
            trace::emit_category(
                TraceCategory::Lifecycle,
                if destroyed {
                    "window_destroyed"
                } else {
                    "window_withdrawn"
                },
                || {
                    TraceFields::new()
                        .field("source", "compositor")
                        .field("xid", handle.xid())
                        .optional("focus_before", focus_before)
                        .optional("focus_after", self.focused_x11_window_xid())
                        .field(
                            "teardown_reason",
                            if destroyed {
                                "x11_destroy"
                            } else {
                                "x11_unmap"
                            },
                        )
                        .field("destruction_outcome", "first_effective_destruction")
                },
            );
            if self.state.defer_client_list_sync() {
                Vec::new()
            } else {
                vec![self.sync_xwayland_client_lists()]
            }
        } else {
            trace::emit_category(TraceCategory::Lifecycle, "window_cleanup_redundant", || {
                TraceFields::new()
                    .field("source", "compositor")
                    .field("xid", handle.xid())
                    .field("teardown_reason", "redundant_idempotent_cleanup")
                    .field("destruction_outcome", "redundant_noop_cleanup")
            });
            Vec::new()
        }
    }
}
