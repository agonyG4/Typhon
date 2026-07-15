use std::{
    io,
    sync::{Arc, Mutex},
};

use wayland_protocols::xdg::shell::server::{xdg_popup, xdg_surface, xdg_toplevel};
use wayland_server::{
    Resource,
    backend::ClientId,
    protocol::{wl_buffer, wl_callback, wl_output, wl_surface},
};

use crate::compositor::{DragSessionPhase, XdgAssociationReservation};
use crate::render_backend::buffer::{
    BufferId, BufferSize, CommittedSurfaceBuffer, DmabufBufferHandle,
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CoreComplianceMetrics {
    pub protocol_errors_total: u64,
    pub supported_request_unhandled_total: u64,
    pub client_state_leaks_detected: u64,
    pub xdg_same_role_reassociations_total: u64,
    pub xdg_cross_role_reassociation_rejections: u64,
    pub xdg_role_destroyed_pending_commits_retired: u64,
    pub xdg_role_destroyed_pending_trees_retired: u64,
    pub xdg_role_destroyed_acquire_watches_cancelled: u64,
    pub xdg_reassociation_blocked_stale_unpublished_work: u64,
    pub surface_enter_events: u64,
    pub surface_leave_events: u64,
    pub preferred_scale_events: u64,
    pub preferred_transform_events: u64,
    pub dnd_sessions_started: u64,
    pub dnd_sessions_finished: u64,
    pub dnd_sessions_cancelled: u64,
    pub dnd_duplicate_terminal_attempts: u64,
    pub dnd_orphaned_resources_detected: u64,
    pub dnd_source_cancelled_events: u64,
    pub dnd_source_finished_events: u64,
    pub dnd_offer_action_events: u64,
    pub dnd_source_action_events: u64,
    pub(in crate::compositor) dnd_last_terminal_phase: Option<DragSessionPhase>,
    pub pointer_axis_frames: u64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum UnhandledRequestClass {
    FutureVersionOrGeneratedNonExhaustive,
    SupportedButUnhandled,
}

impl CoreComplianceMetrics {
    pub(in crate::compositor) fn note_protocol_error(&mut self) {
        self.protocol_errors_total = self.protocol_errors_total.saturating_add(1);
    }

    pub(in crate::compositor) fn note_unhandled_request(
        &mut self,
        interface: &str,
        version: u32,
        class: UnhandledRequestClass,
    ) {
        match class {
            UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive => {}
            UnhandledRequestClass::SupportedButUnhandled => {
                eprintln!(
                    "oblivion-one compliance: supported request unhandled interface={interface} version={version}"
                );
                self.note_unhandled_supported_request();
            }
        }
    }

    pub(in crate::compositor) fn note_unhandled_supported_request(&mut self) {
        self.supported_request_unhandled_total =
            self.supported_request_unhandled_total.saturating_add(1);
    }

    pub(in crate::compositor) fn note_xdg_same_role_reassociation(&mut self) {
        self.xdg_same_role_reassociations_total =
            self.xdg_same_role_reassociations_total.saturating_add(1);
    }

    pub(in crate::compositor) fn note_xdg_cross_role_reassociation_rejection(&mut self) {
        self.xdg_cross_role_reassociation_rejections = self
            .xdg_cross_role_reassociation_rejections
            .saturating_add(1);
    }

    pub(in crate::compositor) fn note_xdg_role_destroyed_pending_commits_retired(
        &mut self,
        count: usize,
    ) {
        self.xdg_role_destroyed_pending_commits_retired = self
            .xdg_role_destroyed_pending_commits_retired
            .saturating_add(count as u64);
    }

    pub(in crate::compositor) fn note_xdg_role_destroyed_pending_trees_retired(
        &mut self,
        count: usize,
    ) {
        self.xdg_role_destroyed_pending_trees_retired = self
            .xdg_role_destroyed_pending_trees_retired
            .saturating_add(count as u64);
    }

    pub(in crate::compositor) fn note_xdg_role_destroyed_acquire_watches_cancelled(
        &mut self,
        count: usize,
    ) {
        self.xdg_role_destroyed_acquire_watches_cancelled = self
            .xdg_role_destroyed_acquire_watches_cancelled
            .saturating_add(count as u64);
    }

    pub(in crate::compositor) fn note_xdg_reassociation_blocked_stale_unpublished_work(&mut self) {
        self.xdg_reassociation_blocked_stale_unpublished_work = self
            .xdg_reassociation_blocked_stale_unpublished_work
            .saturating_add(1);
    }

    pub(in crate::compositor) fn note_dnd_duplicate_terminal_attempt(&mut self) {
        self.dnd_duplicate_terminal_attempts =
            self.dnd_duplicate_terminal_attempts.saturating_add(1);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportSourceRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ViewportSourceRect {
    pub(in crate::compositor) fn new(x: f64, y: f64, width: f64, height: f64) -> Option<Self> {
        (x >= 0.0 && y >= 0.0 && width > 0.0 && height > 0.0).then_some(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub(in crate::compositor) fn logical_size(self) -> Option<BufferSize> {
        let width = self.width.ceil();
        let height = self.height.ceil();
        if !(width.is_finite() && height.is_finite()) || width <= 0.0 || height <= 0.0 {
            return None;
        }
        BufferSize::new(width as u32, height as u32)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(in crate::compositor) struct SurfaceViewportCommit {
    pub(in crate::compositor) source: Option<ViewportSourceRect>,
    pub(in crate::compositor) destination: Option<BufferSize>,
}

use super::{
    RenderableSurface, RenderableSurfaceDamage, SurfaceCommitSequence, SurfaceDamageRect,
    SurfacePlacement,
    dmabuf::DmabufBufferData,
    explicit_sync::{ExplicitSyncPoint, SyncobjSurfaceState},
    interaction::ResizeCommitSnapshot,
    popup::XdgPositionerState,
    same_buffer_resource,
    shm::{ShmBufferData, invalid_buffer_for_cpu_read, invalid_shm_buffer},
    window_state::WindowState,
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct ToplevelSizeConstraints {
    pub(super) min_width: Option<u32>,
    pub(super) min_height: Option<u32>,
    pub(super) max_width: Option<u32>,
    pub(super) max_height: Option<u32>,
}

#[derive(Debug, Clone)]
pub(super) struct ToplevelSurface {
    pub(super) app_id: Option<String>,
    pub(super) title: Option<String>,
    pub(super) parent_surface_id: Option<u32>,
    pub(super) xdg_surface: xdg_surface::XdgSurface,
    pub(super) toplevel: xdg_toplevel::XdgToplevel,
    pub(super) window: WindowState,
    pub(super) constraints: ToplevelSizeConstraints,
    pub(super) pending_constraints: Option<ToplevelSizeConstraints>,
    pub(super) wm_capabilities_sent: bool,
}

#[derive(Debug, Clone)]
pub(super) struct PopupSurface {
    pub(super) parent_surface_id: Option<u32>,
    pub(super) xdg_surface: xdg_surface::XdgSurface,
    pub(super) popup: xdg_popup::XdgPopup,
    pub(super) positioner: XdgPositionerState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PopupOwner {
    Toplevel(u32),
    LayerSurface(u32),
    Popup(u32),
}

impl PopupOwner {
    pub(super) const fn surface_id(self) -> u32 {
        match self {
            Self::Toplevel(surface_id)
            | Self::LayerSurface(surface_id)
            | Self::Popup(surface_id) => surface_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PopupLifecycle {
    Alive,
    Inert,
    Destroyed,
}

#[derive(Debug, Clone)]
pub(super) struct PopupNode {
    pub(super) owner_root_id: u32,
    pub(super) parent: PopupOwner,
    pub(super) children: Vec<u32>,
    pub(super) lifecycle: PopupLifecycle,
    pub(super) mapped: bool,
    pub(super) configured: bool,
    pub(super) popup_done_sent: bool,
    pub(super) grab_generation: Option<u64>,
}

#[derive(Debug, Clone)]
pub(super) struct PopupGrab {
    pub(super) owner_client: ClientId,
    pub(super) owner_root_id: u32,
    pub(super) tree_root_popup_id: u32,
    pub(super) focused_popup_id: u32,
    pub(super) serial: u32,
    pub(super) generation: u64,
}

#[derive(Debug, Clone)]
pub(super) struct PendingActivationToken {
    pub(super) client_id: ClientId,
    pub(super) serial: Option<u32>,
    pub(super) surface_id: Option<u32>,
    pub(super) app_id: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct ActivationTokenState {
    pub(super) client_id: ClientId,
    pub(super) serial: Option<u32>,
    pub(super) surface_id: Option<u32>,
    pub(super) app_id: Option<String>,
    pub(super) generation: u64,
    pub(super) used: bool,
}

#[derive(Debug, Clone)]
pub(super) struct LayerSurfaceData {
    pub(super) surface: wl_surface::WlSurface,
}

#[derive(Debug, Default)]
pub(super) struct SurfaceData {
    surface_id: u32,
    pending_buffer: Mutex<Option<PendingSurfaceAttachment>>,
    pending_offset: Mutex<Option<(i32, i32)>>,
    pending_surface_damage: Mutex<Vec<PendingSurfaceDamageRect>>,
    pending_buffer_damage: Mutex<Vec<PendingBufferDamageRect>>,
    frame_callbacks: Mutex<Vec<wl_callback::WlCallback>>,
    explicit_sync: Mutex<Option<Arc<SyncobjSurfaceState>>>,
    viewport: Mutex<SurfaceViewportState>,
    buffer_scale: Mutex<SurfaceBufferScaleState>,
    buffer_transform: Mutex<SurfaceBufferTransformState>,
    input_region: Mutex<SurfaceInputRegionState>,
    opaque_region: Mutex<SurfaceInputRegionState>,
}

#[derive(Debug)]
pub(super) struct PendingSurfaceDamage {
    pub(super) damage: RenderableSurfaceDamage,
}

impl PendingSurfaceDamage {
    pub(super) fn explicit(self) -> Option<RenderableSurfaceDamage> {
        (!self.damage.is_empty()).then_some(self.damage)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingDamageRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl PendingDamageRect {
    const fn new(x: i32, y: i32, width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        Some(Self {
            x,
            y,
            width,
            height,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingSurfaceDamageRect(PendingDamageRect);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingBufferDamageRect(PendingDamageRect);

impl SurfaceData {
    pub(super) fn new(surface_id: u32) -> Self {
        Self {
            surface_id,
            ..Self::default()
        }
    }

    pub(super) const fn surface_id(&self) -> u32 {
        self.surface_id
    }

    pub(super) fn set_pending(&self, buffer: Option<wl_buffer::WlBuffer>, x: i32, y: i32) {
        let pending = match buffer {
            Some(resource) => {
                if let Some(data) = resource.data::<ShmBufferData>().cloned() {
                    Some(PendingSurfaceAttachment::Buffer(PendingSurfaceBuffer {
                        resource,
                        data: PendingBufferData::Shm(data),
                        x,
                        y,
                        explicit_release: None,
                        surface_size: None,
                        viewport_source: None,
                        viewport_destination: None,
                        buffer_scale: 1,
                        commit_sequence: SurfaceCommitSequence::initial(),
                        resize_commit: None,
                        resize_capture_finalized: false,
                        buffer_transform: wl_output::Transform::Normal,
                    }))
                } else {
                    resource.data::<DmabufBufferData>().cloned().map(|data| {
                        PendingSurfaceAttachment::Buffer(PendingSurfaceBuffer {
                            resource,
                            data: PendingBufferData::Dmabuf(data),
                            x,
                            y,
                            explicit_release: None,
                            surface_size: None,
                            viewport_source: None,
                            viewport_destination: None,
                            buffer_scale: 1,
                            commit_sequence: SurfaceCommitSequence::initial(),
                            resize_commit: None,
                            resize_capture_finalized: false,
                            buffer_transform: wl_output::Transform::Normal,
                        })
                    })
                }
            }
            None => Some(PendingSurfaceAttachment::RemoveContent),
        };

        if let Ok(mut guard) = self.pending_buffer.lock() {
            *guard = pending;
        }
    }

    pub(super) fn has_pending_buffer(&self) -> bool {
        self.pending_buffer
            .lock()
            .ok()
            .is_some_and(|pending| pending.is_some())
    }

    pub(super) fn take_pending(&self) -> Option<PendingSurfaceAttachment> {
        self.pending_buffer.lock().ok()?.take()
    }

    pub(super) fn set_pending_offset(&self, x: i32, y: i32) {
        if let Ok(mut guard) = self.pending_offset.lock() {
            *guard = Some((x, y));
        }
    }

    pub(super) fn take_pending_offset(&self) -> Option<(i32, i32)> {
        self.pending_offset.lock().ok()?.take()
    }

    pub(super) fn push_surface_damage(&self, x: i32, y: i32, width: i32, height: i32) {
        let Some(rect) = PendingDamageRect::new(x, y, width, height) else {
            return;
        };
        if let Ok(mut damage) = self.pending_surface_damage.lock() {
            damage.push(PendingSurfaceDamageRect(rect));
        }
    }

    pub(super) fn push_buffer_damage(&self, x: i32, y: i32, width: i32, height: i32) {
        let Some(rect) = PendingDamageRect::new(x, y, width, height) else {
            return;
        };
        if let Ok(mut damage) = self.pending_buffer_damage.lock() {
            damage.push(PendingBufferDamageRect(rect));
        }
    }

    pub(super) fn take_damage(
        &self,
        buffer_size: Option<BufferSize>,
        buffer_scale: u32,
        viewport: SurfaceViewportCommit,
    ) -> PendingSurfaceDamage {
        let surface_rects = self
            .pending_surface_damage
            .lock()
            .map(|mut damage| damage.drain(..).collect())
            .unwrap_or_else(|_| Vec::new());
        let buffer_rects = self
            .pending_buffer_damage
            .lock()
            .map(|mut damage| damage.drain(..).collect())
            .unwrap_or_else(|_| Vec::new());
        let damage = convert_pending_damage(
            surface_rects,
            buffer_rects,
            buffer_size,
            buffer_scale,
            viewport,
        );
        PendingSurfaceDamage { damage }
    }

    pub(super) fn push_frame_callback(&self, callback: wl_callback::WlCallback) {
        if let Ok(mut callbacks) = self.frame_callbacks.lock() {
            callbacks.push(callback);
        }
    }

    pub(super) fn take_frame_callbacks(&self) -> Vec<wl_callback::WlCallback> {
        self.frame_callbacks
            .lock()
            .map(|mut callbacks| callbacks.drain(..).collect())
            .unwrap_or_default()
    }

    pub(super) fn attach_explicit_sync(&self, state: Arc<SyncobjSurfaceState>) -> bool {
        let Ok(mut explicit_sync) = self.explicit_sync.lock() else {
            return false;
        };
        if explicit_sync
            .as_ref()
            .is_some_and(|existing| existing.resource_is_alive())
        {
            return false;
        }
        *explicit_sync = Some(state);
        true
    }

    pub(super) fn explicit_sync(&self) -> Option<Arc<SyncobjSurfaceState>> {
        self.explicit_sync
            .lock()
            .ok()
            .and_then(|state| state.as_ref().cloned())
            .filter(|state| state.resource_is_alive())
    }

    pub(super) fn set_pending_viewport_destination(&self, destination: Option<BufferSize>) {
        if let Ok(mut viewport) = self.viewport.lock() {
            viewport.pending_destination = Some(destination);
        }
    }

    pub(super) fn set_pending_viewport_source(&self, source: Option<ViewportSourceRect>) {
        if let Ok(mut viewport) = self.viewport.lock() {
            viewport.pending_source = Some(source);
        }
    }

    pub(super) fn take_pending_viewport(&self) -> PendingViewportChange {
        self.viewport
            .lock()
            .map(|mut viewport| PendingViewportChange {
                source: viewport.pending_source.take(),
                destination: viewport.pending_destination.take(),
            })
            .unwrap_or_default()
    }

    pub(super) fn apply_viewport_change(
        &self,
        change: PendingViewportChange,
    ) -> SurfaceViewportCommit {
        self.viewport
            .lock()
            .map(|mut viewport| {
                if let Some(source) = change.source {
                    viewport.source = source;
                }
                if let Some(destination) = change.destination {
                    viewport.destination = destination;
                }
                SurfaceViewportCommit {
                    source: viewport.source,
                    destination: viewport.destination,
                }
            })
            .unwrap_or_default()
    }

    pub(super) fn viewport_for_change(
        &self,
        change: PendingViewportChange,
    ) -> SurfaceViewportCommit {
        self.viewport
            .lock()
            .map(|viewport| SurfaceViewportCommit {
                source: change.source.unwrap_or(viewport.source),
                destination: change.destination.unwrap_or(viewport.destination),
            })
            .unwrap_or_default()
    }

    pub(super) fn set_pending_buffer_scale(&self, scale: u32) {
        if let Ok(mut buffer_scale) = self.buffer_scale.lock() {
            buffer_scale.pending = Some(scale.max(1));
        }
    }

    pub(super) fn set_pending_buffer_transform(&self, transform: wl_output::Transform) {
        if let Ok(mut state) = self.buffer_transform.lock() {
            state.pending = Some(transform);
        }
    }

    pub(super) fn take_pending_buffer_transform(&self) -> Option<wl_output::Transform> {
        self.buffer_transform
            .lock()
            .ok()
            .and_then(|mut state| state.pending.take())
    }

    pub(super) fn buffer_transform_for_change(
        &self,
        transform: Option<wl_output::Transform>,
    ) -> wl_output::Transform {
        transform.unwrap_or_else(|| {
            self.buffer_transform
                .lock()
                .map(|state| state.committed)
                .unwrap_or(wl_output::Transform::Normal)
        })
    }

    pub(super) fn apply_buffer_transform_change(
        &self,
        transform: Option<wl_output::Transform>,
    ) -> wl_output::Transform {
        self.buffer_transform
            .lock()
            .map(|mut state| {
                if let Some(transform) = transform {
                    state.committed = transform;
                }
                state.committed
            })
            .unwrap_or(wl_output::Transform::Normal)
    }

    pub(super) fn take_pending_buffer_scale(&self) -> Option<u32> {
        self.buffer_scale
            .lock()
            .ok()
            .and_then(|mut buffer_scale| buffer_scale.pending.take())
    }

    pub(super) fn apply_buffer_scale_change(&self, scale: Option<u32>) -> u32 {
        self.buffer_scale
            .lock()
            .map(|mut buffer_scale| {
                if let Some(scale) = scale {
                    buffer_scale.committed = scale.max(1);
                }
                buffer_scale.committed
            })
            .unwrap_or(1)
    }

    pub(super) fn buffer_scale_for_change(&self, scale: Option<u32>) -> u32 {
        scale.unwrap_or_else(|| {
            self.buffer_scale
                .lock()
                .map(|buffer_scale| buffer_scale.committed)
                .unwrap_or(1)
        })
    }

    pub(super) fn set_pending_input_region(&self, region: SurfaceInputRegion) {
        if let Ok(mut state) = self.input_region.lock() {
            state.pending = Some(region);
        }
    }

    pub(super) fn set_pending_opaque_region(&self, region: SurfaceInputRegion) {
        if let Ok(mut state) = self.opaque_region.lock() {
            state.pending = Some(region);
        }
    }

    pub(super) fn take_pending_opaque_region(&self) -> Option<SurfaceInputRegion> {
        self.opaque_region
            .lock()
            .ok()
            .and_then(|mut state| state.pending.take())
    }

    pub(super) fn apply_opaque_region_change(&self, pending: Option<SurfaceInputRegion>) -> bool {
        let Ok(mut state) = self.opaque_region.lock() else {
            return false;
        };
        let Some(pending) = pending else {
            return false;
        };
        let changed = state.committed != pending;
        state.committed = pending;
        changed
    }

    pub(super) fn take_pending_input_region(&self) -> Option<SurfaceInputRegion> {
        self.input_region
            .lock()
            .ok()
            .and_then(|mut state| state.pending.take())
    }

    pub(super) fn apply_input_region_change(&self, pending: Option<SurfaceInputRegion>) -> bool {
        let Ok(mut state) = self.input_region.lock() else {
            return false;
        };
        let Some(pending) = pending else {
            return false;
        };
        let changed = state.committed != pending;
        state.committed = pending;
        changed
    }

    pub(super) fn input_region_contains(
        &self,
        surface_x: f64,
        surface_y: f64,
        surface_width: u32,
        surface_height: u32,
    ) -> bool {
        self.input_region
            .lock()
            .map(|state| {
                state
                    .committed
                    .contains(surface_x, surface_y, surface_width, surface_height)
            })
            .unwrap_or(true)
    }
}

fn transform_swaps_dimensions(transform: wl_output::Transform) -> bool {
    matches!(transform as i32, 1 | 3 | 5 | 7)
}

fn convert_pending_damage(
    surface_rects: Vec<PendingSurfaceDamageRect>,
    buffer_rects: Vec<PendingBufferDamageRect>,
    buffer_size: Option<BufferSize>,
    buffer_scale: u32,
    viewport: SurfaceViewportCommit,
) -> RenderableSurfaceDamage {
    if surface_rects.is_empty() && buffer_rects.is_empty() {
        return RenderableSurfaceDamage::Empty;
    }
    let Some(buffer_size) = buffer_size else {
        return RenderableSurfaceDamage::Full;
    };
    let mut converted = Vec::with_capacity(surface_rects.len() + buffer_rects.len());
    for PendingBufferDamageRect(rect) in buffer_rects {
        let Some(rect) = clip_pending_rect(rect, buffer_size.width, buffer_size.height) else {
            continue;
        };
        converted.push(rect);
    }
    for PendingSurfaceDamageRect(rect) in surface_rects {
        let mapped = match viewport.destination {
            Some(destination) => {
                map_viewport_damage(rect, destination, buffer_size, viewport.source)
            }
            None => map_scaled_surface_damage(rect, buffer_scale.max(1), buffer_size),
        };
        let Some(rect) = mapped else {
            return RenderableSurfaceDamage::Full;
        };
        if let Some(rect) = rect {
            converted.push(rect);
        }
    }
    RenderableSurfaceDamage::from_rects(converted)
        .normalized_for_surface(buffer_size.width, buffer_size.height)
}

fn map_scaled_surface_damage(
    rect: PendingDamageRect,
    scale: u32,
    buffer_size: BufferSize,
) -> Option<Option<SurfaceDamageRect>> {
    let scale = i64::from(scale);
    let left = i64::from(rect.x).checked_mul(scale)?;
    let top = i64::from(rect.y).checked_mul(scale)?;
    let right = i64::from(rect.x)
        .checked_add(i64::from(rect.width))?
        .checked_mul(scale)?;
    let bottom = i64::from(rect.y)
        .checked_add(i64::from(rect.height))?
        .checked_mul(scale)?;
    Some(clip_i64_rect(
        left,
        top,
        right,
        bottom,
        buffer_size.width,
        buffer_size.height,
    ))
}

fn map_viewport_damage(
    rect: PendingDamageRect,
    destination: BufferSize,
    buffer_size: BufferSize,
    source: Option<ViewportSourceRect>,
) -> Option<Option<SurfaceDamageRect>> {
    if destination.width == 0 || destination.height == 0 {
        return None;
    }
    let left = i64::from(rect.x).clamp(0, i64::from(destination.width));
    let top = i64::from(rect.y).clamp(0, i64::from(destination.height));
    let right = i64::from(rect.x)
        .checked_add(i64::from(rect.width))?
        .clamp(0, i64::from(destination.width));
    let bottom = i64::from(rect.y)
        .checked_add(i64::from(rect.height))?
        .clamp(0, i64::from(destination.height));
    if right <= left || bottom <= top {
        return Some(None);
    }
    let (source_x, source_y, source_width, source_height) = source
        .map(|source| (source.x, source.y, source.width, source.height))
        .unwrap_or((
            0.0,
            0.0,
            f64::from(buffer_size.width),
            f64::from(buffer_size.height),
        ));
    let scale_x = source_width / f64::from(destination.width);
    let scale_y = source_height / f64::from(destination.height);
    let mapped_left = (source_x + left as f64 * scale_x).floor() as i64;
    let mapped_top = (source_y + top as f64 * scale_y).floor() as i64;
    let mapped_right = (source_x + right as f64 * scale_x).ceil() as i64;
    let mapped_bottom = (source_y + bottom as f64 * scale_y).ceil() as i64;
    Some(clip_i64_rect(
        mapped_left,
        mapped_top,
        mapped_right,
        mapped_bottom,
        buffer_size.width,
        buffer_size.height,
    ))
}

fn clip_pending_rect(
    rect: PendingDamageRect,
    width: u32,
    height: u32,
) -> Option<SurfaceDamageRect> {
    let right = i64::from(rect.x).checked_add(i64::from(rect.width))?;
    let bottom = i64::from(rect.y).checked_add(i64::from(rect.height))?;
    clip_i64_rect(
        i64::from(rect.x),
        i64::from(rect.y),
        right,
        bottom,
        width,
        height,
    )
}

fn clip_i64_rect(
    left: i64,
    top: i64,
    right: i64,
    bottom: i64,
    width: u32,
    height: u32,
) -> Option<SurfaceDamageRect> {
    let left = left.clamp(0, i64::from(width));
    let top = top.clamp(0, i64::from(height));
    let right = right.clamp(0, i64::from(width));
    let bottom = bottom.clamp(0, i64::from(height));
    (right > left && bottom > top).then_some(SurfaceDamageRect {
        x: left as u32,
        y: top as u32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    })
}

#[derive(Debug, Default, Clone, Copy)]
struct SurfaceViewportState {
    source: Option<ViewportSourceRect>,
    destination: Option<BufferSize>,
    pending_source: Option<Option<ViewportSourceRect>>,
    pending_destination: Option<Option<BufferSize>>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(super) struct PendingViewportChange {
    pub(super) source: Option<Option<ViewportSourceRect>>,
    pub(super) destination: Option<Option<BufferSize>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SurfaceBufferScaleState {
    committed: u32,
    pending: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SurfaceBufferTransformState {
    committed: wl_output::Transform,
    pending: Option<wl_output::Transform>,
}

impl Default for SurfaceBufferTransformState {
    fn default() -> Self {
        Self {
            committed: wl_output::Transform::Normal,
            pending: None,
        }
    }
}

#[cfg(test)]
mod damage_space_tests {
    use super::*;

    fn size(width: u32, height: u32) -> BufferSize {
        BufferSize::new(width, height).unwrap()
    }

    #[test]
    fn surface_and_buffer_damage_match_at_scale_one() {
        let surface = convert_pending_damage(
            vec![PendingSurfaceDamageRect(
                PendingDamageRect::new(2, 3, 4, 5).unwrap(),
            )],
            Vec::new(),
            Some(size(20, 20)),
            1,
            SurfaceViewportCommit::default(),
        );
        let buffer = convert_pending_damage(
            Vec::new(),
            vec![PendingBufferDamageRect(
                PendingDamageRect::new(2, 3, 4, 5).unwrap(),
            )],
            Some(size(20, 20)),
            1,
            SurfaceViewportCommit::default(),
        );

        assert_eq!(surface, buffer);
    }

    #[test]
    fn surface_damage_uses_integer_buffer_scale() {
        let damage = convert_pending_damage(
            vec![PendingSurfaceDamageRect(
                PendingDamageRect::new(2, 3, 4, 5).unwrap(),
            )],
            Vec::new(),
            Some(size(40, 40)),
            2,
            SurfaceViewportCommit::default(),
        );

        assert_eq!(
            damage,
            RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
                x: 4,
                y: 6,
                width: 8,
                height: 10,
            }])
        );
    }

    #[test]
    fn surface_damage_uses_supported_viewport_destination() {
        let damage = convert_pending_damage(
            vec![PendingSurfaceDamageRect(
                PendingDamageRect::new(5, 5, 10, 10).unwrap(),
            )],
            Vec::new(),
            Some(size(200, 100)),
            1,
            SurfaceViewportCommit {
                source: None,
                destination: Some(size(100, 50)),
            },
        );

        assert_eq!(
            damage,
            RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
                x: 10,
                y: 10,
                width: 20,
                height: 20,
            }])
        );
    }

    #[test]
    fn surface_damage_with_viewport_source_maps_inside_source_region() {
        let damage = convert_pending_damage(
            vec![PendingSurfaceDamageRect(
                PendingDamageRect::new(0, 0, 200, 100).unwrap(),
            )],
            Vec::new(),
            Some(size(200, 100)),
            1,
            SurfaceViewportCommit {
                source: ViewportSourceRect::new(20.0, 10.0, 100.0, 50.0),
                destination: Some(size(400, 200)),
            },
        );

        assert_eq!(
            damage,
            RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
                x: 20,
                y: 10,
                width: 50,
                height: 25,
            }])
        );
    }

    #[test]
    fn combined_damage_clips_every_buffer_edge() {
        let damage = convert_pending_damage(
            Vec::new(),
            vec![
                PendingBufferDamageRect(PendingDamageRect::new(-2, 2, 4, 3).unwrap()),
                PendingBufferDamageRect(PendingDamageRect::new(8, 2, 4, 3).unwrap()),
                PendingBufferDamageRect(PendingDamageRect::new(2, -2, 3, 4).unwrap()),
                PendingBufferDamageRect(PendingDamageRect::new(2, 8, 3, 4).unwrap()),
            ],
            Some(size(10, 10)),
            1,
            SurfaceViewportCommit::default(),
        );

        assert_eq!(damage.clipped_rects(10, 10).len(), 4);
    }

    #[test]
    fn missing_mapping_falls_back_to_full_and_no_requests_stay_empty() {
        assert_eq!(
            convert_pending_damage(
                Vec::new(),
                Vec::new(),
                None,
                1,
                SurfaceViewportCommit::default()
            ),
            RenderableSurfaceDamage::Empty
        );
        assert_eq!(
            convert_pending_damage(
                vec![PendingSurfaceDamageRect(
                    PendingDamageRect::new(0, 0, 1, 1).unwrap(),
                )],
                Vec::new(),
                None,
                1,
                SurfaceViewportCommit::default(),
            ),
            RenderableSurfaceDamage::Full
        );
    }
}

impl Default for SurfaceBufferScaleState {
    fn default() -> Self {
        Self {
            committed: 1,
            pending: None,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct SurfaceInputRegionState {
    committed: SurfaceInputRegion,
    pending: Option<SurfaceInputRegion>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) enum SurfaceInputRegion {
    #[default]
    Default,
    Custom(Vec<InputRegionOp>),
}

impl SurfaceInputRegion {
    pub(super) fn contains(
        &self,
        surface_x: f64,
        surface_y: f64,
        surface_width: u32,
        surface_height: u32,
    ) -> bool {
        match self {
            Self::Default => {
                surface_x >= 0.0
                    && surface_y >= 0.0
                    && surface_x < f64::from(surface_width)
                    && surface_y < f64::from(surface_height)
            }
            Self::Custom(ops) => {
                let mut contains = false;
                for op in ops {
                    match op {
                        InputRegionOp::Add(rect) if rect.contains(surface_x, surface_y) => {
                            contains = true;
                        }
                        InputRegionOp::Subtract(rect) if rect.contains(surface_x, surface_y) => {
                            contains = false;
                        }
                        _ => {}
                    }
                }
                contains
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct RegionData {
    ops: Arc<Mutex<Vec<InputRegionOp>>>,
}

impl RegionData {
    pub(super) fn snapshot(&self) -> SurfaceInputRegion {
        SurfaceInputRegion::Custom(self.ops.lock().map(|ops| ops.clone()).unwrap_or_default())
    }

    pub(super) fn push(&self, op: InputRegionOp) {
        if let Ok(mut ops) = self.ops.lock() {
            ops.push(op);
        }
    }
}

impl Default for RegionData {
    fn default() -> Self {
        Self {
            ops: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InputRegionOp {
    Add(InputRegionRect),
    Subtract(InputRegionRect),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct InputRegionRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl InputRegionRect {
    pub(super) const fn new(x: i32, y: i32, width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        Some(Self {
            x,
            y,
            width,
            height,
        })
    }

    fn contains(self, surface_x: f64, surface_y: f64) -> bool {
        surface_x >= f64::from(self.x)
            && surface_y >= f64::from(self.y)
            && surface_x < f64::from(self.x.saturating_add(self.width))
            && surface_y < f64::from(self.y.saturating_add(self.height))
    }
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub(super) enum PendingSurfaceAttachment {
    Buffer(PendingSurfaceBuffer),
    RemoveContent,
}

#[derive(Debug, Clone)]
pub(super) struct PendingSurfaceBuffer {
    pub(super) resource: wl_buffer::WlBuffer,
    pub(super) data: PendingBufferData,
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) explicit_release: Option<ExplicitSyncPoint>,
    pub(super) surface_size: Option<BufferSize>,
    pub(super) viewport_source: Option<ViewportSourceRect>,
    pub(super) viewport_destination: Option<BufferSize>,
    pub(super) buffer_scale: u32,
    pub(super) commit_sequence: SurfaceCommitSequence,
    pub(super) resize_commit: Option<Box<ResizeCommitSnapshot>>,
    pub(super) resize_capture_finalized: bool,
    pub(super) buffer_transform: wl_output::Transform,
}

impl PendingSurfaceBuffer {
    pub(super) fn apply_committed_surface_state(
        &mut self,
        viewport: SurfaceViewportCommit,
        buffer_scale: u32,
        buffer_transform: wl_output::Transform,
    ) -> io::Result<()> {
        self.viewport_source = viewport.source;
        self.viewport_destination = viewport.destination;
        self.buffer_scale = buffer_scale;
        self.buffer_transform = buffer_transform;
        self.surface_size =
            Some(self.surface_size_for_state(viewport, buffer_scale, buffer_transform)?);
        Ok(())
    }

    pub(super) fn surface_size_for_state(
        &self,
        viewport: SurfaceViewportCommit,
        buffer_scale: u32,
        buffer_transform: wl_output::Transform,
    ) -> io::Result<BufferSize> {
        self.validate_viewport_source(viewport.source)?;
        if let Some(destination) = viewport.destination {
            return Ok(destination);
        }
        if let Some(source) = viewport.source.and_then(ViewportSourceRect::logical_size) {
            return Ok(source);
        }
        self.surface_size_for_buffer_scale(buffer_scale, buffer_transform)
    }

    fn validate_viewport_source(&self, source: Option<ViewportSourceRect>) -> io::Result<()> {
        let Some(source) = source else {
            return Ok(());
        };
        let width = f64::from(self.data.width()?);
        let height = f64::from(self.data.height()?);
        let tolerance = 1.0 / 256.0;
        if source.x + source.width > width + tolerance
            || source.y + source.height > height + tolerance
        {
            return Err(invalid_shm_buffer());
        }
        Ok(())
    }

    pub(super) fn surface_size_for_buffer_scale(
        &self,
        buffer_scale: u32,
        buffer_transform: wl_output::Transform,
    ) -> io::Result<BufferSize> {
        let buffer_scale = buffer_scale.max(1);
        let (buffer_width, buffer_height) = if transform_swaps_dimensions(buffer_transform) {
            (self.data.height()?, self.data.width()?)
        } else {
            (self.data.width()?, self.data.height()?)
        };
        if buffer_width % buffer_scale != 0 || buffer_height % buffer_scale != 0 {
            return Err(invalid_shm_buffer());
        }
        let width = buffer_width / buffer_scale;
        let height = buffer_height / buffer_scale;
        BufferSize::new(width, height).ok_or_else(invalid_shm_buffer)
    }

    pub(super) fn to_renderable_surface(
        &self,
        surface_id: u32,
        placement: SurfacePlacement,
        generation: u64,
        damage: RenderableSurfaceDamage,
    ) -> io::Result<RenderableSurface> {
        let width = self.data.width()?;
        let height = self.data.height()?;
        let size = BufferSize::new(width, height).ok_or_else(invalid_shm_buffer)?;
        let surface_size = self.surface_size.unwrap_or(size);
        Ok(RenderableSurface {
            surface_id,
            x: self.x,
            y: self.y,
            width: surface_size.width,
            height: surface_size.height,
            placement,
            render_placement: None,
            visual_clip: None,
            generation,
            commit_sequence: self.commit_sequence,
            buffer: self.data.to_committed_buffer_for_size(size)?,
            buffer_scale: self.buffer_scale,
            buffer_transform: self.buffer_transform,
            viewport_source: self.viewport_source,
            viewport_destination: self.viewport_destination,
            damage,
        })
    }

    pub(super) fn release_target(&self) -> SurfaceBufferRelease {
        if let Some(point) = self.explicit_release.clone() {
            SurfaceBufferRelease::ExplicitSync(point)
        } else {
            SurfaceBufferRelease::WlBuffer(self.resource.clone())
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum PendingBufferData {
    Shm(ShmBufferData),
    Dmabuf(DmabufBufferData),
}

impl PendingBufferData {
    pub(super) const fn buffer_id(&self) -> BufferId {
        match self {
            Self::Shm(data) => data.identity.id(),
            Self::Dmabuf(data) => data.identity.id(),
        }
    }

    pub(super) const fn is_shm(&self) -> bool {
        matches!(self, Self::Shm(_))
    }

    pub(super) const fn is_dmabuf(&self) -> bool {
        matches!(self, Self::Dmabuf(_))
    }

    pub(super) fn dmabuf_handle(&self) -> Option<&DmabufBufferHandle> {
        match self {
            Self::Dmabuf(data) => Some(&data.handle),
            Self::Shm(_) => None,
        }
    }

    pub(super) fn width(&self) -> io::Result<u32> {
        match self {
            Self::Shm(data) => data.width(),
            Self::Dmabuf(data) => Ok(data.width()),
        }
    }

    pub(super) fn height(&self) -> io::Result<u32> {
        match self {
            Self::Shm(data) => data.height(),
            Self::Dmabuf(data) => Ok(data.height()),
        }
    }

    pub(super) fn read_pixels_into_with_damage(
        &self,
        pixels: &mut Vec<u32>,
        damage: &RenderableSurfaceDamage,
    ) -> io::Result<()> {
        match self {
            Self::Shm(data) => data.read_pixels_into_with_damage(pixels, damage),
            Self::Dmabuf(_) => Err(invalid_buffer_for_cpu_read()),
        }
    }

    pub(super) fn to_committed_buffer_for_size(
        &self,
        size: BufferSize,
    ) -> io::Result<CommittedSurfaceBuffer> {
        match self {
            Self::Shm(data) => Ok(CommittedSurfaceBuffer::shm_snapshot(
                data.identity.clone(),
                size,
                data.read_pixels()?,
            )),
            Self::Dmabuf(data) => Ok(CommittedSurfaceBuffer::dmabuf_handle(
                data.identity.clone(),
                data.handle.clone(),
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum SurfaceBufferRelease {
    WlBuffer(wl_buffer::WlBuffer),
    ExplicitSync(ExplicitSyncPoint),
}

impl SurfaceBufferRelease {
    /// Compares protocol completion identity, not the underlying client buffer allocation.
    /// An explicit-sync reuse of one `wl_buffer` has a new token when its timeline point changes.
    pub(super) fn same_release_token(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::WlBuffer(left), Self::WlBuffer(right)) => same_buffer_resource(left, right),
            (Self::ExplicitSync(left), Self::ExplicitSync(right)) => left == right,
            _ => false,
        }
    }

    pub(super) fn release(self) {
        match self {
            Self::WlBuffer(buffer) => {
                let _ = buffer.send_event(wl_buffer::Event::Release);
            }
            Self::ExplicitSync(point) => {
                point.signal();
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct XdgSurfaceData {
    pub(super) surface: wl_surface::WlSurface,
    pub(super) reservation: XdgAssociationReservation,
}

#[derive(Debug)]
pub(super) struct SubsurfaceData {
    pub(super) surface: wl_surface::WlSurface,
    pub(super) parent: wl_surface::WlSurface,
}

#[derive(Debug)]
pub(super) struct XdgToplevelData {
    pub(super) surface: wl_surface::WlSurface,
}

#[derive(Debug)]
pub(super) struct XdgPopupData {
    pub(super) surface: wl_surface::WlSurface,
}

#[derive(Debug)]
pub(super) struct ViewportData {
    pub(super) surface: wl_surface::WlSurface,
}

impl From<wl_surface::WlSurface> for ViewportData {
    fn from(surface: wl_surface::WlSurface) -> Self {
        Self { surface }
    }
}

#[derive(Debug)]
pub(super) struct FractionalScaleData {
    pub(super) surface: wl_surface::WlSurface,
}

impl FractionalScaleData {
    pub(super) fn new(surface: wl_surface::WlSurface) -> Self {
        Self { surface }
    }

    pub(super) fn surface_id(&self) -> u32 {
        compositor_surface_id(&self.surface)
    }
}

pub(super) fn compositor_surface_id(surface: &wl_surface::WlSurface) -> u32 {
    surface
        .data::<SurfaceData>()
        .map(SurfaceData::surface_id)
        .filter(|surface_id| *surface_id != 0)
        .unwrap_or_else(|| surface.id().protocol_id())
}

impl From<wl_surface::WlSurface> for FractionalScaleData {
    fn from(surface: wl_surface::WlSurface) -> Self {
        Self::new(surface)
    }
}

#[cfg(test)]
mod surface_region_tests {
    use super::*;

    #[test]
    fn opaque_region_is_copied_and_double_buffered() {
        let surface = SurfaceData::new(1);
        let region = SurfaceInputRegion::Custom(vec![InputRegionOp::Add(
            InputRegionRect::new(2, 3, 10, 11).unwrap(),
        )]);
        surface.set_pending_opaque_region(region.clone());
        assert_eq!(surface.take_pending_opaque_region(), Some(region));
    }

    #[test]
    fn null_input_region_resets_to_infinite_default() {
        let surface = SurfaceData::new(1);
        let region = SurfaceInputRegion::Custom(vec![InputRegionOp::Add(
            InputRegionRect::new(0, 0, 4, 4).unwrap(),
        )]);
        surface.set_pending_input_region(region);
        let pending = surface.take_pending_input_region();
        assert!(surface.apply_input_region_change(pending));
        assert!(surface.input_region_contains(1.0, 1.0, 10, 10));

        surface.set_pending_input_region(SurfaceInputRegion::Default);
        let pending = surface.take_pending_input_region();
        assert!(surface.apply_input_region_change(pending));
        assert!(surface.input_region_contains(9.0, 9.0, 10, 10));
    }

    #[test]
    fn region_destroy_after_set_does_not_change_pending_copy() {
        let region = RegionData::default();
        region.push(InputRegionOp::Add(
            InputRegionRect::new(0, 0, 4, 4).unwrap(),
        ));
        let snapshot = region.snapshot();
        region.push(InputRegionOp::Add(
            InputRegionRect::new(8, 8, 4, 4).unwrap(),
        ));
        assert_eq!(
            snapshot,
            SurfaceInputRegion::Custom(vec![InputRegionOp::Add(
                InputRegionRect::new(0, 0, 4, 4).unwrap(),
            )])
        );
    }
}
