use std::{
    io,
    sync::{Arc, Mutex},
};

use wayland_protocols::xdg::shell::server::{xdg_popup, xdg_surface, xdg_toplevel};
use wayland_server::{
    Resource,
    backend::ClientId,
    protocol::{wl_buffer, wl_callback, wl_surface},
};

use crate::render_backend::buffer::{
    BufferId, BufferSize, CommittedSurfaceBuffer, DmabufBufferHandle,
};

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
    pub(super) xdg_surface: xdg_surface::XdgSurface,
    pub(super) toplevel: xdg_toplevel::XdgToplevel,
    pub(super) window: WindowState,
    pub(super) constraints: ToplevelSizeConstraints,
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
    input_region: Mutex<SurfaceInputRegionState>,
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
                        commit_sequence: SurfaceCommitSequence::initial(),
                        resize_commit: None,
                        resize_capture_finalized: false,
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
                            commit_sequence: SurfaceCommitSequence::initial(),
                            resize_commit: None,
                            resize_capture_finalized: false,
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
        viewport_destination: Option<BufferSize>,
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
            viewport_destination,
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

    pub(super) fn has_frame_callbacks(&self) -> bool {
        self.frame_callbacks
            .lock()
            .is_ok_and(|callbacks| !callbacks.is_empty())
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

    pub(super) fn take_pending_viewport(&self) -> Option<Option<BufferSize>> {
        self.viewport
            .lock()
            .ok()
            .and_then(|mut viewport| viewport.pending_destination.take())
    }

    pub(super) fn apply_viewport_change(
        &self,
        destination: Option<Option<BufferSize>>,
    ) -> Option<BufferSize> {
        self.viewport
            .lock()
            .map(|mut viewport| {
                if let Some(destination) = destination {
                    viewport.destination = destination;
                }
                viewport.destination
            })
            .unwrap_or_default()
    }

    pub(super) fn viewport_destination_for_change(
        &self,
        destination: Option<Option<BufferSize>>,
    ) -> Option<BufferSize> {
        destination.unwrap_or_else(|| {
            self.viewport
                .lock()
                .map(|viewport| viewport.destination)
                .unwrap_or_default()
        })
    }

    pub(super) fn set_pending_buffer_scale(&self, scale: u32) {
        if let Ok(mut buffer_scale) = self.buffer_scale.lock() {
            buffer_scale.pending = Some(scale.max(1));
        }
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

fn convert_pending_damage(
    surface_rects: Vec<PendingSurfaceDamageRect>,
    buffer_rects: Vec<PendingBufferDamageRect>,
    buffer_size: Option<BufferSize>,
    buffer_scale: u32,
    viewport_destination: Option<BufferSize>,
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
        let mapped = match viewport_destination {
            Some(destination) => map_viewport_damage(rect, destination, buffer_size),
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
    let mapped_left =
        left.checked_mul(i64::from(buffer_size.width))? / i64::from(destination.width);
    let mapped_top =
        top.checked_mul(i64::from(buffer_size.height))? / i64::from(destination.height);
    let mapped_right = div_ceil_i64(
        right.checked_mul(i64::from(buffer_size.width))?,
        i64::from(destination.width),
    )?;
    let mapped_bottom = div_ceil_i64(
        bottom.checked_mul(i64::from(buffer_size.height))?,
        i64::from(destination.height),
    )?;
    Some(clip_i64_rect(
        mapped_left,
        mapped_top,
        mapped_right,
        mapped_bottom,
        buffer_size.width,
        buffer_size.height,
    ))
}

fn div_ceil_i64(value: i64, divisor: i64) -> Option<i64> {
    (divisor > 0).then_some(value.checked_add(divisor - 1)? / divisor)
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
    destination: Option<BufferSize>,
    pending_destination: Option<Option<BufferSize>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SurfaceBufferScaleState {
    committed: u32,
    pending: Option<u32>,
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
            None,
        );
        let buffer = convert_pending_damage(
            Vec::new(),
            vec![PendingBufferDamageRect(
                PendingDamageRect::new(2, 3, 4, 5).unwrap(),
            )],
            Some(size(20, 20)),
            1,
            None,
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
            None,
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
            Some(size(100, 50)),
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
            None,
        );

        assert_eq!(damage.clipped_rects(10, 10).len(), 4);
    }

    #[test]
    fn missing_mapping_falls_back_to_full_and_no_requests_stay_empty() {
        assert_eq!(
            convert_pending_damage(Vec::new(), Vec::new(), None, 1, None),
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
                None,
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
    pub(super) commit_sequence: SurfaceCommitSequence,
    pub(super) resize_commit: Option<Box<ResizeCommitSnapshot>>,
    pub(super) resize_capture_finalized: bool,
}

impl PendingSurfaceBuffer {
    pub(super) fn apply_committed_surface_state(
        &mut self,
        viewport_destination: Option<BufferSize>,
        buffer_scale: u32,
    ) -> io::Result<()> {
        self.surface_size = Some(self.surface_size_for_state(viewport_destination, buffer_scale)?);
        Ok(())
    }

    pub(super) fn surface_size_for_state(
        &self,
        viewport_destination: Option<BufferSize>,
        buffer_scale: u32,
    ) -> io::Result<BufferSize> {
        if let Some(destination) = viewport_destination {
            return Ok(destination);
        }
        self.surface_size_for_buffer_scale(buffer_scale)
    }

    pub(super) fn surface_size_for_buffer_scale(
        &self,
        buffer_scale: u32,
    ) -> io::Result<BufferSize> {
        let buffer_scale = buffer_scale.max(1);
        let width = self.data.width()?.div_ceil(buffer_scale);
        let height = self.data.height()?.div_ceil(buffer_scale);
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
    pub(super) fn same_buffer_resource(&self, other: &Self) -> bool {
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
