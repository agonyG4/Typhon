use super::*;

use crate::wm::LayoutMembership;
use crate::wm::layout::{
    LayoutRect, RatioOverride, ResizeEdges as LayoutResizeEdges, SplitRatio, TiledLayoutSolution,
    TiledResizeHandle,
};

#[derive(Debug, Clone, Copy)]
pub(in crate::compositor) struct TiledResizeSession {
    pub(in crate::compositor) interaction_id: WindowInteractionId,
    pub(in crate::compositor) resize_interaction_id: ResizeInteractionId,
    pub(in crate::compositor) window_id: WindowId,
    pub(in crate::compositor) location: WorkspaceLocation,
    pub(in crate::compositor) edges: ResizeEdges,
    pub(in crate::compositor) handle: TiledResizeHandle,
    pub(in crate::compositor) topology_generation: u64,
    pub(in crate::compositor) last_horizontal: Option<SplitRatio>,
    pub(in crate::compositor) last_vertical: Option<SplitRatio>,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::compositor) struct PendingTiledResize {
    pub(in crate::compositor) interaction_id: WindowInteractionId,
    pub(in crate::compositor) window_id: WindowId,
    pub(in crate::compositor) location: WorkspaceLocation,
    pub(in crate::compositor) horizontal_requested_ratio: Option<f64>,
    pub(in crate::compositor) vertical_requested_ratio: Option<f64>,
}

#[derive(Debug)]
pub(in crate::compositor) struct TiledResizePreparation {
    pub(in crate::compositor) location: WorkspaceLocation,
    pub(in crate::compositor) edges: ResizeEdges,
    pub(in crate::compositor) handle: TiledResizeHandle,
    pub(in crate::compositor) solution: TiledLayoutSolution,
}

impl CompositorState {
    pub(in crate::compositor) fn prepare_tiled_resize(
        &self,
        window_id: WindowId,
        edges: ResizeEdges,
    ) -> Option<TiledResizePreparation> {
        let location = self
            .window(window_id)
            .and_then(|window| window.management)
            .filter(|management| management.layout() == LayoutMembership::Tiled)
            .map(|management| management.location())?;
        if !self.location_is_visible_for_layout(location) {
            return None;
        }
        let tree = self.tiled_layout.tree(location)?;
        let solution = self
            .tiled_layout
            .calculate(
                location,
                self.layout_root_rect(),
                &self.layout_snapshots(location),
            )
            .ok()?;
        let layout_edges = LayoutResizeEdges::new(edges.top, edges.bottom, edges.left, edges.right);
        let handle = TiledResizeHandle::from_solution(tree, &solution, window_id, layout_edges)?;
        Some(TiledResizePreparation {
            location,
            edges,
            handle,
            solution,
        })
    }

    pub(in crate::compositor) fn install_tiled_resize_session(
        &mut self,
        interaction_id: WindowInteractionId,
        resize_interaction_id: ResizeInteractionId,
        window_id: WindowId,
        preparation: &TiledResizePreparation,
    ) {
        let last_horizontal = preparation
            .handle
            .horizontal()
            .and_then(|axis| {
                preparation
                    .solution
                    .splits()
                    .iter()
                    .find(|split| split.node() == axis.split())
            })
            .map(|split| split.effective_ratio());
        let last_vertical = preparation
            .handle
            .vertical()
            .and_then(|axis| {
                preparation
                    .solution
                    .splits()
                    .iter()
                    .find(|split| split.node() == axis.split())
            })
            .map(|split| split.effective_ratio());
        let topology_generation = self
            .tiled_layout
            .tree(preparation.location)
            .map(|tree| tree.topology_generation())
            .unwrap_or_default();
        self.tiled_resize_session = Some(TiledResizeSession {
            interaction_id,
            resize_interaction_id,
            window_id,
            location: preparation.location,
            edges: preparation.edges,
            handle: preparation.handle,
            topology_generation,
            last_horizontal,
            last_vertical,
        });
        self.resize_flow_metrics.tiled_resize_interactions_started = self
            .resize_flow_metrics
            .tiled_resize_interactions_started
            .saturating_add(1);
    }

    pub(in crate::compositor) fn rebase_tiled_resize_preparation(
        &self,
        root_surface_id: u32,
        window_id: WindowId,
        preparation: &mut TiledResizePreparation,
    ) {
        let Some(presented_rect) = self
            .presented_root_geometry(root_surface_id)
            .map(PresentedRootGeometry::presented_rect)
        else {
            return;
        };
        let Some(canonical_client) = preparation
            .solution
            .target_for_window(window_id)
            .map(|target| target.client())
        else {
            return;
        };
        let (horizontal_delta, vertical_delta) =
            interaction_boundary_deltas(canonical_client, presented_rect, preparation.edges);
        preparation.handle = preparation
            .handle
            .rebase_boundaries(horizontal_delta, vertical_delta);
    }

    pub(in crate::compositor) fn update_pending_tiled_resize(
        &mut self,
        interaction: WindowInteraction,
        x: f64,
        y: f64,
    ) -> bool {
        let Some(session) = self.tiled_resize_session else {
            return false;
        };
        if session.interaction_id != interaction.id {
            return false;
        }
        let dx = (x - interaction.start_pointer_x).round() as i32;
        let dy = (y - interaction.start_pointer_y).round() as i32;
        let pending = PendingTiledResize {
            interaction_id: interaction.id,
            window_id: session.window_id,
            location: session.location,
            horizontal_requested_ratio: session
                .handle
                .horizontal()
                .map(|axis| axis.requested_ratio(dx)),
            vertical_requested_ratio: session
                .handle
                .vertical()
                .map(|axis| axis.requested_ratio(dy)),
        };
        if self.pending_tiled_resize.is_some() {
            self.resize_flow_metrics.tiled_resize_pending_replaced = self
                .resize_flow_metrics
                .tiled_resize_pending_replaced
                .saturating_add(1);
        }
        self.pending_tiled_resize = Some(pending);
        self.resize_flow_metrics.tiled_resize_raw_updates = self
            .resize_flow_metrics
            .tiled_resize_raw_updates
            .saturating_add(1);
        true
    }

    pub(in crate::compositor) fn flush_pending_tiled_resize(&mut self) -> bool {
        let Some(pending) = self.pending_tiled_resize.take() else {
            return false;
        };
        let Some(session) = self.tiled_resize_session else {
            return false;
        };
        if session.interaction_id != pending.interaction_id
            || session.window_id != pending.window_id
            || session.location != pending.location
        {
            return false;
        }
        let Some(tree) = self.tiled_layout.tree(session.location) else {
            self.cancel_tiled_resize_after_invalid_pending(session.interaction_id);
            return false;
        };
        if tree.topology_generation() != session.topology_generation {
            self.cancel_tiled_resize_after_invalid_pending(session.interaction_id);
            return false;
        }
        let mut overrides = Vec::with_capacity(2);
        if let Some(ratio) = pending.horizontal_requested_ratio {
            let Some(axis) = session.handle.horizontal() else {
                return false;
            };
            let Some(ratio) = SplitRatio::new(ratio.clamp(SplitRatio::MIN, SplitRatio::MAX)) else {
                self.cancel_tiled_resize_after_invalid_pending(session.interaction_id);
                return false;
            };
            overrides.push(RatioOverride::new(axis.split(), ratio));
        }
        if let Some(ratio) = pending.vertical_requested_ratio {
            let Some(axis) = session.handle.vertical() else {
                return false;
            };
            let Some(ratio) = SplitRatio::new(ratio.clamp(SplitRatio::MIN, SplitRatio::MAX)) else {
                self.cancel_tiled_resize_after_invalid_pending(session.interaction_id);
                return false;
            };
            overrides.push(RatioOverride::new(axis.split(), ratio));
        }
        let snapshots = self.layout_snapshots(session.location);
        self.resize_flow_metrics.tiled_resize_frame_snapshot_windows = snapshots.len() as u64;
        let Ok(solution) =
            crate::wm::layout::TiledLayoutManager::calculate_tree_with_ratio_overrides(
                tree,
                session.location,
                self.layout_root_rect(),
                &snapshots,
                &overrides,
            )
        else {
            self.cancel_tiled_resize_after_invalid_pending(session.interaction_id);
            return false;
        };
        let horizontal = session
            .handle
            .horizontal()
            .and_then(|axis| {
                solution
                    .splits()
                    .iter()
                    .find(|split| split.node() == axis.split())
            })
            .map(|split| split.effective_ratio());
        self.resize_flow_metrics.tiled_resize_frame_node_visits = solution.node_visits() as u64;
        let vertical = session
            .handle
            .vertical()
            .and_then(|axis| {
                solution
                    .splits()
                    .iter()
                    .find(|split| split.node() == axis.split())
            })
            .map(|split| split.effective_ratio());
        let unchanged = horizontal == session.last_horizontal && vertical == session.last_vertical;
        let effective_changed =
            horizontal != session.last_horizontal || vertical != session.last_vertical;
        if unchanged || !effective_changed {
            self.resize_flow_metrics.tiled_resize_unchanged_flushes = self
                .resize_flow_metrics
                .tiled_resize_unchanged_flushes
                .saturating_add(1);
            return false;
        }
        if let Some(axis) = session.handle.horizontal()
            && let Some(ratio) = horizontal
            && self
                .tiled_layout
                .tree_mut(session.location)
                .set_split_ratio(axis.split(), ratio.value())
                .is_err()
        {
            self.cancel_tiled_resize_after_invalid_pending(session.interaction_id);
            return false;
        }
        if let Some(axis) = session.handle.vertical()
            && let Some(ratio) = vertical
            && self
                .tiled_layout
                .tree_mut(session.location)
                .set_split_ratio(axis.split(), ratio.value())
                .is_err()
        {
            self.cancel_tiled_resize_after_invalid_pending(session.interaction_id);
            return false;
        }
        if horizontal != pending.horizontal_requested_ratio.and_then(SplitRatio::new) {
            self.resize_flow_metrics.tiled_resize_ratio_clamps = self
                .resize_flow_metrics
                .tiled_resize_ratio_clamps
                .saturating_add(1);
        }
        if vertical != pending.vertical_requested_ratio.and_then(SplitRatio::new) {
            self.resize_flow_metrics.tiled_resize_ratio_clamps = self
                .resize_flow_metrics
                .tiled_resize_ratio_clamps
                .saturating_add(1);
        }
        if let Some(session) = self.tiled_resize_session.as_mut() {
            session.last_horizontal = horizontal;
            session.last_vertical = vertical;
        }
        self.resize_flow_metrics.tiled_resize_frame_flushes = self
            .resize_flow_metrics
            .tiled_resize_frame_flushes
            .saturating_add(1);
        self.apply_tiled_layout_plan(solution, None)
    }

    fn cancel_tiled_resize_after_invalid_pending(&mut self, interaction_id: WindowInteractionId) {
        self.pending_tiled_resize = None;
        self.tiled_resize_session = None;
        if self
            .window_interaction
            .is_some_and(|interaction| interaction.id == interaction_id)
        {
            let _ = self.end_window_interaction_by_id_with_reason(
                interaction_id,
                WindowInteractionEndReason::ExplicitCancel,
            );
        }
    }

    pub(in crate::compositor) fn clear_tiled_resize_state(&mut self) {
        self.pending_tiled_resize = None;
        self.tiled_resize_session = None;
    }

    pub(in crate::compositor) fn cancel_tiled_resize_for_location(
        &mut self,
        location: WorkspaceLocation,
        reason: WindowInteractionEndReason,
    ) {
        if self
            .tiled_resize_session
            .is_none_or(|session| session.location != location)
        {
            return;
        }
        if let Some(interaction) = self.window_interaction {
            let _ = self.end_window_interaction_by_id_with_reason(interaction.id, reason);
        } else {
            self.clear_tiled_resize_state();
        }
    }

    pub(in crate::compositor) fn tiled_resize_cursor_kind(
        &self,
        handle: TiledResizeHandle,
        original: WindowInteractionKind,
    ) -> WindowInteractionKind {
        let horizontal = handle.horizontal().is_some();
        let vertical = handle.vertical().is_some();
        let edges = ResizeEdges::new(
            vertical && matches!(original, WindowInteractionKind::Resize(edges) if edges.top),
            vertical && matches!(original, WindowInteractionKind::Resize(edges) if edges.bottom),
            horizontal && matches!(original, WindowInteractionKind::Resize(edges) if edges.left),
            horizontal && matches!(original, WindowInteractionKind::Resize(edges) if edges.right),
        );
        WindowInteractionKind::Resize(edges)
    }

    pub(in crate::compositor) fn tiled_resize_owner(
        &self,
        window_id: WindowId,
    ) -> Option<(ResizeEdges, ResizeInteractionId)> {
        self.tiled_resize_session
            .filter(|session| session.window_id == window_id)
            .map(|session| (session.edges, session.resize_interaction_id))
    }
}

fn saturating_i32_from_f64(value: f64) -> i32 {
    if !value.is_finite() {
        return if value.is_sign_negative() {
            i32::MIN
        } else {
            i32::MAX
        };
    }
    value
        .round()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

fn interaction_boundary_deltas(
    canonical_client: LayoutRect,
    presented_rect: PresentationRect,
    edges: ResizeEdges,
) -> (i32, i32) {
    let horizontal_delta = if edges.left || edges.right {
        let canonical_edge = if edges.right {
            canonical_client.right()
        } else {
            canonical_client.x()
        };
        let presented_edge = if edges.right {
            presented_rect.x() + presented_rect.width()
        } else {
            presented_rect.x()
        };
        saturating_i32_from_f64(presented_edge - f64::from(canonical_edge))
    } else {
        0
    };
    let vertical_delta = if edges.top || edges.bottom {
        let canonical_edge = if edges.bottom {
            canonical_client.bottom()
        } else {
            canonical_client.y()
        };
        let presented_edge = if edges.bottom {
            presented_rect.y() + presented_rect.height()
        } else {
            presented_rect.y()
        };
        saturating_i32_from_f64(presented_edge - f64::from(canonical_edge))
    } else {
        0
    };
    (horizontal_delta, vertical_delta)
}

#[cfg(test)]
mod tests {
    use super::{ResizeEdges, interaction_boundary_deltas};
    use crate::compositor::PresentationRect;
    use crate::wm::layout::LayoutRect;

    #[test]
    fn constrained_client_offset_is_preserved_when_rebasing_right_edge() {
        let canonical_client = LayoutRect::new(25, 10, 400, 300).expect("client rect");
        let presented = PresentationRect::new(40.0, 20.0, 400.0, 300.0).expect("presented rect");
        let (horizontal, vertical) = interaction_boundary_deltas(
            canonical_client,
            presented,
            edges(false, false, false, true),
        );

        assert_eq!((horizontal, vertical), (15, 0));
        assert_eq!(500_i32.saturating_add(horizontal) - 440, 75);
        assert_ne!(500_i32.saturating_add(horizontal), 440);
    }

    #[test]
    fn all_selected_edges_use_the_matching_presented_client_edge() {
        let canonical_client = LayoutRect::new(25, 10, 400, 300).expect("client rect");
        let presented = PresentationRect::new(40.0, 20.0, 420.0, 320.0).expect("presented rect");

        assert_eq!(
            interaction_boundary_deltas(
                canonical_client,
                presented,
                edges(false, false, true, false)
            ),
            (15, 0)
        );
        assert_eq!(
            interaction_boundary_deltas(
                canonical_client,
                presented,
                edges(false, true, false, false)
            ),
            (0, 30)
        );
        assert_eq!(
            interaction_boundary_deltas(
                canonical_client,
                presented,
                edges(true, false, false, false)
            ),
            (0, 10)
        );
    }

    const fn edges(top: bool, bottom: bool, left: bool, right: bool) -> ResizeEdges {
        ResizeEdges {
            top,
            bottom,
            left,
            right,
        }
    }
}
