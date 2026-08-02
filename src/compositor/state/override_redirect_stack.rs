use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverrideRedirectStackSnapshotResult {
    Rejected,
    Applied { logical_stack_changed: bool },
}

impl CompositorState {
    pub(in crate::compositor) fn apply_override_redirect_stack_snapshot(
        &mut self,
        generation: XwaylandGeneration,
        epoch: u64,
        bottom_to_top: &[X11WindowHandle],
    ) -> OverrideRedirectStackSnapshotResult {
        if !self.validate_override_redirect_stack_snapshot_header(generation, epoch, bottom_to_top)
        {
            return OverrideRedirectStackSnapshotResult::Rejected;
        }

        let override_redirect_ids = self
            .window_stacking
            .iter()
            .copied()
            .filter(|window_id| {
                self.window(*window_id)
                    .is_some_and(|window| window.x11_role == Some(X11DesktopRole::OverrideRedirect))
            })
            .collect::<Vec<_>>();
        if override_redirect_ids.iter().any(|window_id| {
            self.window(*window_id).is_some_and(|window| {
                matches!(
                    window.backend,
                    WindowBackend::X11(handle) if handle.generation() != generation
                )
            })
        }) {
            self.note_override_redirect_snapshot_rejected_generation();
            return OverrideRedirectStackSnapshotResult::Rejected;
        }
        if override_redirect_ids.is_empty() {
            self.applied_override_redirect_stack = Some((generation, epoch));
            self.xwayland_scene_batch
                .metrics
                .override_redirect_stack_snapshots_applied = self
                .xwayland_scene_batch
                .metrics
                .override_redirect_stack_snapshots_applied
                .saturating_add(1);
            return OverrideRedirectStackSnapshotResult::Applied {
                logical_stack_changed: false,
            };
        }

        let override_redirect_set = override_redirect_ids
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        let mut ordered = bottom_to_top
            .iter()
            .filter_map(|handle| self.window_id_for_x11_handle(*handle))
            .filter(|window_id| override_redirect_set.contains(window_id))
            .collect::<Vec<_>>();
        for window_id in override_redirect_ids.iter().copied() {
            if !ordered.contains(&window_id) {
                ordered.push(window_id);
            }
        }

        let logical_stack_changed = override_redirect_ids
            .iter()
            .zip(ordered.iter())
            .any(|(before, after)| before != after);
        let mut next = ordered.into_iter();
        for window_id in &mut self.window_stacking {
            if override_redirect_set.contains(window_id)
                && let Some(replacement) = next.next()
            {
                *window_id = replacement;
            }
        }
        self.applied_override_redirect_stack = Some((generation, epoch));
        self.xwayland_scene_batch
            .metrics
            .override_redirect_stack_snapshots_applied = self
            .xwayland_scene_batch
            .metrics
            .override_redirect_stack_snapshots_applied
            .saturating_add(1);
        if logical_stack_changed {
            self.reorder_renderable_surfaces_by_window_stack();
        }
        OverrideRedirectStackSnapshotResult::Applied {
            logical_stack_changed,
        }
    }

    pub(in crate::compositor) fn validate_override_redirect_stack_snapshot_header(
        &mut self,
        generation: XwaylandGeneration,
        epoch: u64,
        bottom_to_top: &[X11WindowHandle],
    ) -> bool {
        let Some(identity) = self.xwayland.client_identity.as_ref() else {
            self.note_override_redirect_snapshot_rejected_generation();
            return false;
        };
        if identity.generation != generation
            || bottom_to_top
                .iter()
                .any(|handle| handle.generation() != generation)
        {
            self.note_override_redirect_snapshot_rejected_generation();
            return false;
        }
        if self.applied_override_redirect_stack.is_some_and(
            |(applied_generation, applied_epoch)| {
                applied_generation == generation && applied_epoch >= epoch
            },
        ) {
            self.note_override_redirect_snapshot_rejected_stale();
            return false;
        }
        true
    }
}
