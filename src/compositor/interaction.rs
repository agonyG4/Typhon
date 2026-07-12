use std::{collections::VecDeque, time::Instant};

use wayland_protocols::xdg::shell::server::xdg_toplevel;
use wayland_server::{WEnum, protocol::wl_surface};

use super::{SurfacePlacement, XdgWindowGeometry, render};

const MIN_WINDOW_WIDTH: u32 = 160;
const MIN_WINDOW_HEIGHT: u32 = 120;
const WINDOW_FRAME_RESIZE_THICKNESS: f64 = render::SERVER_FRAME_BORDER_THICKNESS as f64;
const WINDOW_RESIZE_DRAG_THRESHOLD: i32 = 3;

#[derive(Debug, Clone)]
pub(super) struct PointerTarget {
    pub(super) surface: wl_surface::WlSurface,
    pub(super) surface_x: f64,
    pub(super) surface_y: f64,
}

#[derive(Debug, Clone)]
pub(super) struct PointerPress {
    pub(super) serial: u32,
    pub(super) button: u32,
    pub(super) surface: wl_surface::WlSurface,
    pub(super) root_surface_id: u32,
    pub(super) output_x: f64,
    pub(super) output_y: f64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RootSurfaceHit {
    pub(super) root_surface_id: u32,
    pub(super) local_x: f64,
    pub(super) local_y: f64,
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct WindowFrameHit {
    pub(super) root_surface_id: u32,
    pub(super) kind: WindowInteractionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WindowInteractionKind {
    Move,
    Resize(ResizeEdges),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InteractionCursorShape {
    Move,
    ResizeHorizontal,
    ResizeVertical,
    ResizeDiagonalNwSe,
    ResizeDiagonalNeSw,
}

impl InteractionCursorShape {
    pub(super) const fn for_window_interaction(kind: WindowInteractionKind) -> Self {
        match kind {
            WindowInteractionKind::Move => Self::Move,
            WindowInteractionKind::Resize(edges) => {
                if (edges.top && edges.left) || (edges.bottom && edges.right) {
                    Self::ResizeDiagonalNwSe
                } else if (edges.top && edges.right) || (edges.bottom && edges.left) {
                    Self::ResizeDiagonalNeSw
                } else if edges.left || edges.right {
                    Self::ResizeHorizontal
                } else {
                    Self::ResizeVertical
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct InteractionCursorOverride {
    pub(super) shape: InteractionCursorShape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowInteractionId(u64);

impl WindowInteractionId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WindowInteractionSource {
    NativeBinding,
    XdgToplevelMove,
    XdgToplevelResize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ResizeEdges {
    pub(super) top: bool,
    pub(super) bottom: bool,
    pub(super) left: bool,
    pub(super) right: bool,
}

impl ResizeEdges {
    pub(super) const BOTTOM_RIGHT: Self = Self {
        top: false,
        bottom: true,
        left: false,
        right: true,
    };

    pub(super) const fn new(top: bool, bottom: bool, left: bool, right: bool) -> Self {
        Self {
            top,
            bottom,
            left,
            right,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResizeInteractionId(u64);

impl ResizeInteractionId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct WindowInteraction {
    pub(super) id: WindowInteractionId,
    pub(super) root_surface_id: u32,
    pub(super) kind: WindowInteractionKind,
    pub(super) source: WindowInteractionSource,
    pub(super) trigger_button: Option<u32>,
    pub(super) trigger_serial: Option<u32>,
    pub(super) pointer_motion_surface_id: Option<u32>,
    pub(super) start_pointer_x: f64,
    pub(super) start_pointer_y: f64,
    pub(super) start_placement: SurfacePlacement,
    pub(super) start_width: u32,
    pub(super) start_height: u32,
    pub(super) drag_committed: bool,
    pub(super) resize_interaction_id: Option<ResizeInteractionId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PendingResizeConfigure {
    pub(super) surface_id: u32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) placement: SurfacePlacement,
    pub(super) edges: ResizeEdges,
    pub(super) resizing: bool,
    pub(super) interaction_id: ResizeInteractionId,
}

impl PendingResizeConfigure {
    pub(super) const fn resize_commit(self, serial: u32) -> PendingResizeCommit {
        PendingResizeCommit {
            serial,
            width: self.width,
            height: self.height,
            placement: self.placement,
            edges: self.edges,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PendingResizeCommit {
    pub(super) serial: u32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) placement: SurfacePlacement,
    pub(super) edges: ResizeEdges,
}

impl PendingResizeCommit {
    pub(super) fn placement_for_committed_size(self, width: u32, height: u32) -> SurfacePlacement {
        let mut placement = self.placement;
        if self.edges.left {
            let target_right = placement
                .local_x
                .saturating_add(i32::try_from(self.width).unwrap_or(i32::MAX));
            placement.local_x =
                target_right.saturating_sub(i32::try_from(width).unwrap_or(i32::MAX));
        }
        if self.edges.top {
            let target_bottom = placement
                .local_y
                .saturating_add(i32::try_from(self.height).unwrap_or(i32::MAX));
            placement.local_y =
                target_bottom.saturating_sub(i32::try_from(height).unwrap_or(i32::MAX));
        }
        placement
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ResizeCommitSnapshot {
    pub(super) serial: u32,
    pub(super) sequence: u64,
    pub(super) commit_sequence: u64,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) placement: SurfacePlacement,
    pub(super) edges: ResizeEdges,
    pub(super) resizing: bool,
    pub(super) emitted_at: Instant,
    pub(super) committed_size: Option<(u32, u32)>,
    pub(super) committed_window_geometry: Option<XdgWindowGeometry>,
    pub(super) buffer_id: Option<u64>,
    pub(super) interaction_id: ResizeInteractionId,
}

impl ResizeCommitSnapshot {
    pub(super) const fn resize_commit(self) -> PendingResizeCommit {
        PendingResizeCommit {
            serial: self.serial,
            width: self.width,
            height: self.height,
            placement: self.placement,
            edges: self.edges,
        }
    }

    pub(super) fn placement_for_committed_size(self, width: u32, height: u32) -> SurfacePlacement {
        self.resize_commit()
            .placement_for_committed_size(width, height)
    }

    pub(super) const fn with_committed_size(mut self, width: u32, height: u32) -> Self {
        self.committed_size = Some((width, height));
        self
    }

    pub(super) const fn with_committed_window_geometry(
        mut self,
        window_geometry: XdgWindowGeometry,
    ) -> Self {
        self.committed_window_geometry = Some(window_geometry);
        self
    }

    pub(super) const fn with_buffer_id(mut self, buffer_id: u64) -> Self {
        self.buffer_id = Some(buffer_id);
        self
    }
}

#[derive(Debug, Clone, Copy)]
struct SentResizeConfigure {
    resize: PendingResizeCommit,
    resizing: bool,
    interaction_id: ResizeInteractionId,
    sequence: u64,
    emitted_at: Instant,
    acknowledged: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResizeAckDecision {
    Matched,
    Duplicate,
    Stale,
    #[default]
    Unknown,
}

#[derive(Debug, Default)]
pub(super) struct ResizeConfigureFlow {
    active_interaction: Option<ResizeInteractionId>,
    outstanding: VecDeque<SentResizeConfigure>,
    acked_uncaptured: Option<SentResizeConfigure>,
    captured: VecDeque<ResizeCommitSnapshot>,
    retired_serials: VecDeque<u32>,
    queued_latest: Option<PendingResizeConfigure>,
    final_pending: Option<PendingResizeConfigure>,
}

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ResizeInteractionBeginResult {
    pub(super) obsolete_queued_discarded: bool,
    pub(super) obsolete_final_discarded: bool,
    pub(super) obsolete_in_flight_discarded: usize,
}

impl ResizeConfigureFlow {
    pub(super) fn begin_interaction(
        &mut self,
        interaction_id: ResizeInteractionId,
    ) -> ResizeInteractionBeginResult {
        if self
            .active_interaction
            .is_some_and(|active| active >= interaction_id)
        {
            return ResizeInteractionBeginResult::default();
        }
        self.active_interaction = Some(interaction_id);
        let before_in_flight = self.in_flight_configure_count();
        let mut retained_outstanding = VecDeque::with_capacity(self.outstanding.len());
        while let Some(sent) = self.outstanding.pop_front() {
            if sent.interaction_id >= interaction_id {
                retained_outstanding.push_back(sent);
            } else {
                self.retire_serial(sent.resize.serial);
            }
        }
        self.outstanding = retained_outstanding;
        if self
            .acked_uncaptured
            .is_some_and(|sent| sent.interaction_id < interaction_id)
            && let Some(sent) = self.acked_uncaptured.take()
        {
            self.retire_serial(sent.resize.serial);
        }
        self.captured
            .retain(|snapshot| snapshot.interaction_id >= interaction_id);
        let after_in_flight = self.in_flight_configure_count();
        ResizeInteractionBeginResult {
            obsolete_queued_discarded: self.queued_latest.take().is_some(),
            obsolete_final_discarded: self.final_pending.take().is_some(),
            obsolete_in_flight_discarded: before_in_flight.saturating_sub(after_in_flight),
        }
    }

    pub(super) fn queue(&mut self, desired: PendingResizeConfigure) -> bool {
        self.begin_interaction(desired.interaction_id);
        if self.final_pending == Some(desired)
            || self.queued_latest == Some(desired)
            || self.sent_or_acked_matches(desired)
        {
            return false;
        }
        self.queued_latest = Some(desired);
        true
    }

    pub(super) fn queue_final(&mut self, desired: PendingResizeConfigure) -> bool {
        match self.active_interaction {
            Some(active) if active != desired.interaction_id => return false,
            None if self.outstanding.is_empty() && self.queued_latest.is_none() => {
                self.active_interaction = Some(desired.interaction_id);
            }
            None => return false,
            Some(_) => {}
        }
        if self.final_pending == Some(desired) {
            return false;
        }
        self.queued_latest = None;
        self.final_pending = Some(desired);
        true
    }

    pub(super) fn take_sendable(&mut self) -> Option<PendingResizeConfigure> {
        if self.in_flight_configure_count() > 0 {
            return None;
        }
        self.queued_latest
            .take()
            .or_else(|| self.final_pending.take())
    }

    pub(super) fn mark_sent(
        &mut self,
        desired: PendingResizeConfigure,
        serial: u32,
        sequence: u64,
    ) {
        if self.in_flight_configure_count() > 0 {
            return;
        }
        self.outstanding.push_back(SentResizeConfigure {
            resize: desired.resize_commit(serial),
            resizing: desired.resizing,
            interaction_id: desired.interaction_id,
            sequence,
            emitted_at: Instant::now(),
            acknowledged: false,
        });
    }

    pub(super) fn ack(&mut self, serial: u32) -> ResizeAckDecision {
        if self.retired_serials.contains(&serial) {
            return ResizeAckDecision::Stale;
        }
        if self
            .acked_uncaptured
            .is_some_and(|sent| sent.resize.serial == serial)
        {
            return ResizeAckDecision::Duplicate;
        }
        let Some(index) = self
            .outstanding
            .iter()
            .enumerate()
            .filter(|(_, sent)| sent.resize.serial <= serial)
            .map(|(index, _)| index)
            .next_back()
        else {
            let oldest = self.outstanding.front().map(|sent| sent.resize.serial);
            let newest = self.outstanding.back().map(|sent| sent.resize.serial);
            if newest.is_some_and(|newest| serial > newest) {
                return ResizeAckDecision::Unknown;
            }
            if oldest.is_some_and(|oldest| serial < oldest)
                || self
                    .acked_uncaptured
                    .is_some_and(|sent| serial < sent.resize.serial)
            {
                return ResizeAckDecision::Stale;
            }
            return ResizeAckDecision::Unknown;
        };

        let mut matched = None;
        for _ in 0..=index {
            matched = self.outstanding.pop_front();
        }
        let Some(mut matched) = matched else {
            return ResizeAckDecision::Unknown;
        };
        matched.acknowledged = true;
        self.acked_uncaptured = Some(matched);
        ResizeAckDecision::Matched
    }

    pub(super) fn capture(&mut self, commit_sequence: u64) -> Option<ResizeCommitSnapshot> {
        let sent = self.acked_uncaptured.take()?;
        if !sent.acknowledged {
            return None;
        }
        let snapshot = snapshot_from_sent_resize(sent, commit_sequence);
        self.captured.push_back(snapshot);
        Some(snapshot)
    }

    pub(super) fn complete_applied(&mut self, sequence: u64) -> bool {
        let Some(index) = self
            .captured
            .iter()
            .position(|snapshot| snapshot.sequence == sequence)
        else {
            return false;
        };
        self.captured.remove(index);
        true
    }

    pub(super) fn release_capture(&mut self, commit_sequence: u64) -> bool {
        let Some(index) = self
            .captured
            .iter()
            .position(|snapshot| snapshot.commit_sequence == commit_sequence)
        else {
            return false;
        };
        self.captured.remove(index);
        true
    }

    pub(super) fn in_flight_serial(&self) -> Option<u32> {
        self.outstanding
            .back()
            .map(|sent| sent.resize.serial)
            .or_else(|| {
                self.acked_uncaptured
                    .as_ref()
                    .map(|sent| sent.resize.serial)
            })
    }

    #[cfg(test)]
    pub(super) fn queued_latest(&self) -> Option<PendingResizeConfigure> {
        self.queued_latest
    }

    #[cfg(test)]
    pub(super) fn acked_uncaptured_sequence(&self) -> Option<u64> {
        self.acked_uncaptured.map(|sent| sent.sequence)
    }

    #[cfg(test)]
    pub(super) fn captured_sequences(&self) -> Vec<u64> {
        self.captured
            .iter()
            .map(|snapshot| snapshot.sequence)
            .collect()
    }

    pub(super) fn captured_count(&self) -> usize {
        self.captured.len()
    }

    pub(super) fn in_flight_configure_count(&self) -> usize {
        self.outstanding.len() + usize::from(self.acked_uncaptured.is_some()) + self.captured.len()
    }

    pub(super) fn has_acked_uncaptured(&self) -> bool {
        self.acked_uncaptured.is_some()
    }

    pub(super) fn latest_desired(&self) -> Option<PendingResizeConfigure> {
        self.final_pending.or(self.queued_latest)
    }

    pub(super) fn has_in_flight(&self) -> bool {
        self.in_flight_configure_count() > 0
    }

    pub(super) fn has_sendable(&self) -> bool {
        self.in_flight_configure_count() == 0
            && (self.final_pending.is_some() || self.queued_latest.is_some())
    }

    pub(super) fn in_flight_sequence(&self) -> Option<u64> {
        self.outstanding
            .back()
            .map(|sent| sent.sequence)
            .or_else(|| self.acked_uncaptured.as_ref().map(|sent| sent.sequence))
    }

    pub(super) fn retained_configure_count(&self) -> usize {
        self.retained_in_flight_configure_count()
            + usize::from(self.queued_latest.is_some())
            + usize::from(self.final_pending.is_some())
    }

    pub(super) fn is_empty(&self) -> bool {
        self.retained_configure_count() == 0
    }

    fn retained_in_flight_configure_count(&self) -> usize {
        self.in_flight_configure_count()
    }

    fn retire_serial(&mut self, serial: u32) {
        const MAX_RETIRED_SERIALS: usize = 32;
        self.retired_serials.push_back(serial);
        while self.retired_serials.len() > MAX_RETIRED_SERIALS {
            self.retired_serials.pop_front();
        }
    }

    fn sent_or_acked_matches(&self, desired: PendingResizeConfigure) -> bool {
        let matches = |sent: &SentResizeConfigure| {
            sent.resize == desired.resize_commit(sent.resize.serial)
                && sent.resizing == desired.resizing
                && sent.interaction_id == desired.interaction_id
        };
        let captured_matches = |snapshot: &ResizeCommitSnapshot| {
            snapshot.resize_commit() == desired.resize_commit(snapshot.serial)
                && snapshot.resizing == desired.resizing
                && snapshot.interaction_id == desired.interaction_id
        };
        self.outstanding.iter().any(matches)
            || self.acked_uncaptured.as_ref().is_some_and(matches)
            || self.captured.iter().any(captured_matches)
    }
}

fn snapshot_from_sent_resize(
    sent: SentResizeConfigure,
    commit_sequence: u64,
) -> ResizeCommitSnapshot {
    ResizeCommitSnapshot {
        serial: sent.resize.serial,
        sequence: sent.sequence,
        commit_sequence,
        width: sent.resize.width,
        height: sent.resize.height,
        placement: sent.resize.placement,
        edges: sent.resize.edges,
        resizing: sent.resizing,
        emitted_at: sent.emitted_at,
        committed_size: None,
        committed_window_geometry: None,
        buffer_id: None,
        interaction_id: sent.interaction_id,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ResizeGeometry {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: u32,
    pub(super) height: u32,
}

pub(super) fn interactive_resize_geometry(
    interaction: WindowInteraction,
    edges: ResizeEdges,
    dx: i32,
    dy: i32,
) -> ResizeGeometry {
    let start_width = i32::try_from(interaction.start_width).unwrap_or(i32::MAX);
    let start_height = i32::try_from(interaction.start_height).unwrap_or(i32::MAX);
    let min_width = i32::try_from(MIN_WINDOW_WIDTH).unwrap_or(i32::MAX);
    let min_height = i32::try_from(MIN_WINDOW_HEIGHT).unwrap_or(i32::MAX);
    let mut x = interaction.start_placement.local_x;
    let mut y = interaction.start_placement.local_y;
    let mut width = start_width;
    let mut height = start_height;

    if edges.left {
        width = start_width.saturating_sub(dx).max(min_width);
        x = interaction
            .start_placement
            .local_x
            .saturating_add(start_width.saturating_sub(width));
    } else if edges.right {
        width = start_width.saturating_add(dx).max(min_width);
    }

    if edges.top {
        height = start_height.saturating_sub(dy).max(min_height);
        y = interaction
            .start_placement
            .local_y
            .saturating_add(start_height.saturating_sub(height));
    } else if edges.bottom {
        height = start_height.saturating_add(dy).max(min_height);
    }

    ResizeGeometry {
        x,
        y,
        width: width.try_into().unwrap_or(MIN_WINDOW_WIDTH),
        height: height.try_into().unwrap_or(MIN_WINDOW_HEIGHT),
    }
}

pub(super) fn resize_drag_threshold_reached(edges: ResizeEdges, dx: i32, dy: i32) -> bool {
    let horizontal = edges.left || edges.right;
    let vertical = edges.top || edges.bottom;
    (horizontal && dx.abs() >= WINDOW_RESIZE_DRAG_THRESHOLD)
        || (vertical && dy.abs() >= WINDOW_RESIZE_DRAG_THRESHOLD)
}

pub(super) fn resize_edges_from_xdg(edges: WEnum<xdg_toplevel::ResizeEdge>) -> Option<ResizeEdges> {
    match edges {
        WEnum::Value(xdg_toplevel::ResizeEdge::Top) => {
            Some(ResizeEdges::new(true, false, false, false))
        }
        WEnum::Value(xdg_toplevel::ResizeEdge::Bottom) => {
            Some(ResizeEdges::new(false, true, false, false))
        }
        WEnum::Value(xdg_toplevel::ResizeEdge::Left) => {
            Some(ResizeEdges::new(false, false, true, false))
        }
        WEnum::Value(xdg_toplevel::ResizeEdge::TopLeft) => {
            Some(ResizeEdges::new(true, false, true, false))
        }
        WEnum::Value(xdg_toplevel::ResizeEdge::BottomLeft) => {
            Some(ResizeEdges::new(false, true, true, false))
        }
        WEnum::Value(xdg_toplevel::ResizeEdge::Right) => {
            Some(ResizeEdges::new(false, false, false, true))
        }
        WEnum::Value(xdg_toplevel::ResizeEdge::TopRight) => {
            Some(ResizeEdges::new(true, false, false, true))
        }
        WEnum::Value(xdg_toplevel::ResizeEdge::BottomRight) => Some(ResizeEdges::BOTTOM_RIGHT),
        WEnum::Value(xdg_toplevel::ResizeEdge::None) | WEnum::Unknown(_) => None,
        _ => None,
    }
}

pub(super) fn resize_edges_for_window_point(
    local_x: f64,
    local_y: f64,
    width: u32,
    height: u32,
) -> ResizeEdges {
    let left = local_x < f64::from(width) / 2.0;
    let top = local_y < f64::from(height) / 2.0;
    ResizeEdges::new(top, !top, left, !left)
}

pub(super) fn window_frame_action_for_local_point(
    local_x: f64,
    local_y: f64,
    width: u32,
    height: u32,
) -> Option<WindowInteractionKind> {
    let width = f64::from(width);
    let height = f64::from(height);
    let resize_top = -WINDOW_FRAME_RESIZE_THICKNESS;
    let resize_left = -WINDOW_FRAME_RESIZE_THICKNESS;
    let resize_right = width + WINDOW_FRAME_RESIZE_THICKNESS;
    let resize_bottom = height + WINDOW_FRAME_RESIZE_THICKNESS;

    if local_x < resize_left
        || local_x >= resize_right
        || local_y < resize_top
        || local_y >= resize_bottom
    {
        return None;
    }

    let inside_content = local_x >= 0.0 && local_x < width && local_y >= 0.0 && local_y < height;
    if inside_content {
        return None;
    }

    let within_vertical_frame = local_y >= resize_top && local_y < resize_bottom;
    let within_horizontal_frame = local_x >= resize_left && local_x < resize_right;
    let near_left = local_x >= resize_left && local_x < 0.0 && within_vertical_frame;
    let near_right = local_x >= width && local_x < resize_right && within_vertical_frame;
    let near_top = local_y >= resize_top && local_y < 0.0 && within_horizontal_frame;
    let near_bottom = local_y >= height && local_y < resize_bottom && within_horizontal_frame;

    if near_left || near_right || near_top || near_bottom {
        return Some(WindowInteractionKind::Resize(ResizeEdges::new(
            near_top,
            near_bottom,
            near_left,
            near_right,
        )));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resize_interaction(edges: ResizeEdges) -> WindowInteraction {
        WindowInteraction {
            id: WindowInteractionId::new(1),
            root_surface_id: 1,
            kind: WindowInteractionKind::Resize(edges),
            source: WindowInteractionSource::NativeBinding,
            trigger_button: Some(0x111),
            trigger_serial: None,
            pointer_motion_surface_id: None,
            start_pointer_x: 0.0,
            start_pointer_y: 0.0,
            start_placement: SurfacePlacement::root_at(72, 72),
            start_width: 300,
            start_height: 200,
            drag_committed: true,
            resize_interaction_id: Some(ResizeInteractionId::new(1)),
        }
    }

    #[test]
    fn bottom_right_negative_delta_shrinks_without_growing_first() {
        let edges = ResizeEdges::BOTTOM_RIGHT;

        let geometry = interactive_resize_geometry(resize_interaction(edges), edges, -1, -1);

        assert_eq!(geometry.width, 299);
        assert_eq!(geometry.height, 199);
        assert_eq!(geometry.x, 72);
        assert_eq!(geometry.y, 72);
    }

    #[test]
    fn alt_resize_edges_follow_nearest_window_corner() {
        assert_eq!(
            resize_edges_for_window_point(24.0, 24.0, 300, 200),
            ResizeEdges::new(true, false, true, false)
        );
        assert_eq!(
            resize_edges_for_window_point(276.0, 24.0, 300, 200),
            ResizeEdges::new(true, false, false, true)
        );
        assert_eq!(
            resize_edges_for_window_point(24.0, 176.0, 300, 200),
            ResizeEdges::new(false, true, true, false)
        );
        assert_eq!(
            resize_edges_for_window_point(276.0, 176.0, 300, 200),
            ResizeEdges::BOTTOM_RIGHT
        );
    }

    fn desired_resize(width: u32, resizing: bool) -> PendingResizeConfigure {
        PendingResizeConfigure {
            surface_id: 8,
            width,
            height: 700,
            placement: SurfacePlacement::root_at(10, 20),
            edges: ResizeEdges::BOTTOM_RIGHT,
            resizing,
            interaction_id: ResizeInteractionId::new(1),
        }
    }

    fn desired_resize_for(
        interaction_id: ResizeInteractionId,
        width: u32,
        resizing: bool,
    ) -> PendingResizeConfigure {
        PendingResizeConfigure {
            interaction_id,
            ..desired_resize(width, resizing)
        }
    }

    #[test]
    fn resize_flow_coalesces_one_thousand_updates_behind_one_in_flight_configure() {
        let mut flow = ResizeConfigureFlow::default();
        flow.queue(desired_resize(1000, true));
        let first = flow.take_sendable().expect("first configure");
        flow.mark_sent(first, 308, 1);

        for width in 1001..=2000 {
            flow.queue(desired_resize(width, true));
        }

        assert_eq!(flow.in_flight_serial(), Some(308));
        assert_eq!(flow.queued_latest().map(|resize| resize.width), Some(2000));
        assert_eq!(flow.retained_configure_count(), 2);
    }

    #[test]
    fn duplicate_geometry_does_not_create_an_extra_queued_configure() {
        let mut flow = ResizeConfigureFlow::default();
        let desired = desired_resize(1000, true);
        flow.queue(desired);
        let first = flow.take_sendable().expect("first configure");
        flow.mark_sent(first, 308, 1);

        assert!(!flow.queue(desired));
        assert_eq!(flow.retained_configure_count(), 1);
    }

    #[test]
    fn resize_flows_for_multiple_surfaces_progress_independently() {
        let mut first = ResizeConfigureFlow::default();
        let mut second = ResizeConfigureFlow::default();
        let first_desired = desired_resize(1000, true);
        let second_desired = PendingResizeConfigure {
            surface_id: 9,
            ..desired_resize(1200, true)
        };
        first.mark_sent(first_desired, 308, 1);
        second.mark_sent(second_desired, 409, 2);

        assert_eq!(first.ack(308), ResizeAckDecision::Matched);
        assert_eq!(first.capture(50).map(|snapshot| snapshot.serial), Some(308));
        assert_eq!(second.in_flight_serial(), Some(409));
        assert_eq!(second.ack(308), ResizeAckDecision::Stale);
        assert_eq!(second.ack(409), ResizeAckDecision::Matched);
    }

    #[test]
    fn resize_flow_sends_only_latest_geometry_after_captured_commit_applies() {
        let mut flow = ResizeConfigureFlow::default();
        flow.queue(desired_resize(1000, true));
        let first = flow.take_sendable().expect("first configure");
        flow.mark_sent(first, 308, 1);
        flow.queue(desired_resize(1100, true));
        flow.queue(desired_resize(1200, true));
        assert_eq!(flow.ack(308), ResizeAckDecision::Matched);
        let snapshot = flow.capture(77).expect("ACKed resize snapshot");

        assert_eq!(snapshot.serial, 308);
        assert!(flow.complete_applied(snapshot.sequence));
        assert_eq!(flow.take_sendable().map(|resize| resize.width), Some(1200));
    }

    #[test]
    fn resize_flow_does_not_send_newest_configure_before_prior_commit() {
        let mut flow = ResizeConfigureFlow::default();
        flow.queue(desired_resize(1000, true));
        let first = flow.take_sendable().expect("first configure");
        flow.mark_sent(first, 308, 1);
        flow.queue(desired_resize(1200, true));

        assert!(flow.take_sendable().is_none());
        assert_eq!(flow.queued_latest().map(|resize| resize.width), Some(1200));
        assert_eq!(flow.in_flight_configure_count(), 1);
    }

    #[test]
    fn resize_flow_keeps_at_most_one_sent_configure_in_flight() {
        let mut flow = ResizeConfigureFlow::default();
        flow.mark_sent(desired_resize(1000, true), 308, 1);
        flow.mark_sent(desired_resize(1100, true), 309, 2);
        flow.mark_sent(desired_resize(1200, true), 310, 3);

        assert_eq!(flow.in_flight_configure_count(), 1);
        assert_eq!(flow.in_flight_serial(), Some(308));
        assert_eq!(flow.ack(310), ResizeAckDecision::Matched);
        let snapshot = flow.capture(77).expect("only sent configure is ACKed");
        assert_eq!(snapshot.serial, 308);
    }

    #[test]
    fn resize_flow_sends_newest_target_after_prior_commit_applies() {
        let mut flow = ResizeConfigureFlow::default();
        flow.mark_sent(desired_resize(1000, true), 308, 1);
        flow.queue(desired_resize(1100, true));
        flow.queue(desired_resize(1200, true));
        assert_eq!(flow.ack(308), ResizeAckDecision::Matched);
        let snapshot = flow.capture(77).expect("ACKed configure");

        assert!(flow.take_sendable().is_none());
        assert!(flow.complete_applied(snapshot.sequence));
        let next = flow.take_sendable().expect("latest queued target");
        assert_eq!(next.width, 1200);
    }

    #[test]
    fn resize_flow_retains_latest_target_while_single_configure_is_in_flight() {
        let mut flow = ResizeConfigureFlow::default();
        flow.mark_sent(desired_resize(1000, true), 300, 1);
        flow.queue(desired_resize(2000, true));

        assert!(flow.take_sendable().is_none());
        assert_eq!(flow.queued_latest().map(|resize| resize.width), Some(2000));

        assert_eq!(flow.ack(300), ResizeAckDecision::Matched);
        let snapshot = flow.capture(77).expect("ACKed configure");
        assert!(flow.complete_applied(snapshot.sequence));
        let next = flow.take_sendable().expect("latest retained target");
        assert_eq!(next.width, 2000);
    }

    #[test]
    fn resize_flow_duplicate_or_future_ack_does_not_replace_uncaptured_ack() {
        let mut flow = ResizeConfigureFlow::default();
        flow.mark_sent(desired_resize(1000, true), 308, 1);

        assert_eq!(flow.ack(308), ResizeAckDecision::Matched);
        assert_eq!(flow.ack(308), ResizeAckDecision::Duplicate);
        assert_eq!(flow.ack(310), ResizeAckDecision::Unknown);
        let snapshot = flow.capture(77).expect("original ACK survives");

        assert_eq!(snapshot.serial, 308);
        assert_eq!(snapshot.width, 1000);
    }

    #[test]
    fn task_05_7_new_ack_survives_while_older_capture_is_pending() {
        let mut flow = ResizeConfigureFlow::default();
        flow.mark_sent(desired_resize(1000, true), 308, 1);
        assert_eq!(flow.ack(308), ResizeAckDecision::Matched);
        let capture_a = flow.capture(90).expect("capture A");

        flow.queue(desired_resize(1100, true));
        flow.queue(desired_resize(1200, true));
        assert!(flow.take_sendable().is_none());

        assert_eq!(flow.captured_sequences(), vec![capture_a.sequence]);
        assert_eq!(flow.acked_uncaptured_sequence(), None);
        assert!(flow.complete_applied(capture_a.sequence));
        let configure_c = flow.take_sendable().expect("latest configure");
        assert_eq!(configure_c.width, 1200);
        flow.mark_sent(configure_c, 310, 3);
        assert_eq!(flow.ack(310), ResizeAckDecision::Matched);
        let capture_c = flow.capture(91).expect("capture C");
        assert_eq!(capture_c.serial, 310);
        assert_eq!(capture_c.sequence, 3);
        assert_eq!(flow.captured_sequences(), vec![3]);
        assert_eq!(flow.acked_uncaptured_sequence(), None);
        assert_eq!(flow.retained_configure_count(), 1);
    }

    #[test]
    fn task_05_7_captures_complete_by_exact_sequence() {
        let mut flow = ResizeConfigureFlow::default();
        flow.mark_sent(desired_resize(1000, true), 308, 1);
        assert_eq!(flow.ack(308), ResizeAckDecision::Matched);
        let capture_a = flow.capture(90).expect("capture A");

        flow.queue(desired_resize(1200, true));
        assert!(flow.take_sendable().is_none());
        assert!(flow.complete_applied(capture_a.sequence));
        flow.mark_sent(desired_resize(1200, true), 310, 3);
        assert_eq!(flow.ack(310), ResizeAckDecision::Matched);
        let capture_c = flow.capture(91).expect("capture C");

        assert!(flow.complete_applied(capture_c.sequence));
        assert!(flow.captured_sequences().is_empty());
        assert!(!flow.complete_applied(capture_c.sequence));
        assert!(!flow.complete_applied(capture_a.sequence));
    }

    #[test]
    fn resize_flow_keeps_final_target_sendable_while_prior_configure_is_outstanding() {
        let mut flow = ResizeConfigureFlow::default();
        flow.queue(desired_resize(1000, true));
        let first = flow.take_sendable().expect("first configure");
        flow.mark_sent(first, 308, 1);
        flow.queue(desired_resize(1400, true));
        flow.queue_final(desired_resize(1500, false));

        assert!(flow.take_sendable().is_none());
        assert_eq!(flow.ack(308), ResizeAckDecision::Matched);
        let snapshot = flow.capture(78).expect("ACKed resize snapshot");
        assert!(flow.complete_applied(snapshot.sequence));
        let final_resize = flow.take_sendable().expect("final configure");
        assert_eq!(final_resize.width, 1500);
        assert!(!final_resize.resizing);
        flow.mark_sent(final_resize, 309, 2);
        assert_eq!(flow.in_flight_serial(), Some(309));
    }

    #[test]
    fn delayed_explicit_sync_snapshot_survives_while_newer_geometry_is_sent() {
        let mut flow = ResizeConfigureFlow::default();
        flow.queue(desired_resize(1000, true));
        let first = flow.take_sendable().expect("configure A");
        flow.mark_sent(first, 308, 1);
        assert_eq!(flow.ack(308), ResizeAckDecision::Matched);
        let transaction_a = flow.capture(90).expect("transaction A");
        flow.queue(desired_resize(1400, true));

        assert!(flow.take_sendable().is_none());
        assert!(flow.complete_applied(transaction_a.sequence));
        let second = flow.take_sendable().expect("configure B");
        assert_eq!(second.width, 1400);
        flow.mark_sent(second, 309, 2);
        assert_eq!(flow.in_flight_serial(), Some(309));
        assert_eq!(flow.in_flight_serial(), Some(309));
    }

    #[test]
    fn superseded_fence_releases_capture_without_applying_stale_transaction() {
        let mut flow = ResizeConfigureFlow::default();
        flow.queue(desired_resize(1000, true));
        let first = flow.take_sendable().expect("configure A");
        flow.mark_sent(first, 308, 1);
        assert_eq!(flow.ack(308), ResizeAckDecision::Matched);
        let stale = flow.capture(90).expect("stale transaction");

        assert!(flow.release_capture(stale.commit_sequence));
        assert!(!flow.complete_applied(stale.sequence));
        assert!(flow.is_empty());
    }

    #[test]
    fn surface_eight_serial_308_survives_three_hundred_updates_and_final_resize() {
        let mut flow = ResizeConfigureFlow::default();
        flow.queue(desired_resize(1000, true));
        let first = flow.take_sendable().expect("serial 308 configure");
        flow.mark_sent(first, 308, 1);
        for width in 1001..=1300 {
            flow.queue(desired_resize(width, true));
        }
        flow.queue_final(desired_resize(1300, false));

        assert_eq!(flow.in_flight_serial(), Some(308));
        assert_eq!(flow.retained_configure_count(), 2);
        assert_eq!(flow.ack(308), ResizeAckDecision::Matched);
        let first_commit = flow.capture(100).expect("commit serial 308");
        assert!(flow.complete_applied(first_commit.sequence));

        let final_resize = flow.take_sendable().expect("final resize");
        assert_eq!(final_resize.width, 1300);
        assert!(!final_resize.resizing);
        flow.mark_sent(final_resize, 309, 2);
        assert_eq!(flow.ack(309), ResizeAckDecision::Matched);
        let final_commit = flow.capture(101).expect("final commit");
        assert!(flow.complete_applied(final_commit.sequence));
        assert!(flow.is_empty());
    }

    #[test]
    fn old_final_does_not_outrank_new_active_resize_after_rapid_regrab() {
        let mut flow = ResizeConfigureFlow::default();
        let first = ResizeInteractionId::new(1);
        let second = ResizeInteractionId::new(2);
        flow.queue(desired_resize_for(first, 1000, true));
        let configure_a = flow.take_sendable().expect("configure A");
        flow.mark_sent(configure_a, 308, 1);
        flow.queue_final(desired_resize_for(first, 1100, false));

        flow.begin_interaction(second);
        flow.queue(desired_resize_for(second, 1400, true));
        assert_eq!(flow.ack(308), ResizeAckDecision::Stale);
        assert!(flow.capture(200).is_none());

        let next = flow.take_sendable().expect("B configure");
        assert_eq!(next.interaction_id, second);
        assert!(next.resizing);
        assert_eq!(next.width, 1400);
        assert_eq!(flow.retained_configure_count(), 0);
    }

    #[test]
    fn new_final_survives_when_second_resize_starts_and_ends_behind_old_in_flight() {
        let mut flow = ResizeConfigureFlow::default();
        let first = ResizeInteractionId::new(1);
        let second = ResizeInteractionId::new(2);
        flow.queue(desired_resize_for(first, 1000, true));
        let configure_a = flow.take_sendable().expect("configure A");
        flow.mark_sent(configure_a, 308, 1);
        flow.queue_final(desired_resize_for(first, 1100, false));

        flow.begin_interaction(second);
        flow.queue(desired_resize_for(second, 1400, true));
        flow.queue_final(desired_resize_for(second, 1450, false));
        assert_eq!(flow.ack(308), ResizeAckDecision::Stale);
        assert!(flow.capture(201).is_none());

        let next = flow.take_sendable().expect("B final");
        assert_eq!(next.interaction_id, second);
        assert!(!next.resizing);
        assert_eq!(next.width, 1450);
    }

    #[test]
    fn old_outstanding_configure_does_not_block_new_interaction() {
        let mut flow = ResizeConfigureFlow::default();
        let first = ResizeInteractionId::new(1);
        let second = ResizeInteractionId::new(2);
        flow.queue(desired_resize_for(first, 1000, true));
        let configure_a = flow.take_sendable().expect("configure A");
        flow.mark_sent(configure_a, 308, 1);

        flow.begin_interaction(second);
        flow.queue(desired_resize_for(second, 1400, true));

        let next = flow.take_sendable().expect("B configure");
        assert_eq!(next.interaction_id, second);
        assert_eq!(next.width, 1400);
    }

    #[test]
    fn old_acked_uncaptured_configure_does_not_block_new_interaction() {
        let mut flow = ResizeConfigureFlow::default();
        let first = ResizeInteractionId::new(1);
        let second = ResizeInteractionId::new(2);
        flow.queue(desired_resize_for(first, 1000, true));
        let configure_a = flow.take_sendable().expect("configure A");
        flow.mark_sent(configure_a, 308, 1);
        assert_eq!(flow.ack(308), ResizeAckDecision::Matched);

        flow.begin_interaction(second);
        flow.queue(desired_resize_for(second, 1400, true));

        let next = flow.take_sendable().expect("B configure");
        assert_eq!(next.interaction_id, second);
        assert_eq!(next.width, 1400);
    }

    #[test]
    fn late_ack_for_superseded_interaction_is_stale_and_nonblocking() {
        let mut flow = ResizeConfigureFlow::default();
        let first = ResizeInteractionId::new(1);
        let second = ResizeInteractionId::new(2);
        flow.queue(desired_resize_for(first, 1000, true));
        let configure_a = flow.take_sendable().expect("configure A");
        flow.mark_sent(configure_a, 308, 1);
        flow.begin_interaction(second);
        flow.queue(desired_resize_for(second, 1400, true));

        assert_eq!(flow.ack(308), ResizeAckDecision::Stale);
        assert_eq!(
            flow.take_sendable()
                .map(|configure| configure.interaction_id),
            Some(second)
        );
    }

    #[test]
    fn late_commit_for_superseded_interaction_does_not_replace_new_preview() {
        let mut flow = ResizeConfigureFlow::default();
        let first = ResizeInteractionId::new(1);
        let second = ResizeInteractionId::new(2);
        flow.queue(desired_resize_for(first, 1000, true));
        let configure_a = flow.take_sendable().expect("configure A");
        flow.mark_sent(configure_a, 308, 1);
        assert_eq!(flow.ack(308), ResizeAckDecision::Matched);

        flow.begin_interaction(second);
        flow.queue(desired_resize_for(second, 1400, true));

        assert!(flow.capture(200).is_none());
        assert_eq!(
            flow.take_sendable()
                .map(|configure| configure.interaction_id),
            Some(second)
        );
    }
}
