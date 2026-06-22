use std::{
    io,
    sync::{Arc, Mutex},
};

use wayland_protocols::xdg::shell::server::{xdg_popup, xdg_surface, xdg_toplevel};
use wayland_server::{
    Resource,
    protocol::{wl_buffer, wl_callback, wl_surface},
};

use crate::render_backend::buffer::{
    BufferId, BufferSize, CommittedSurfaceBuffer, DmabufBufferHandle,
};

use super::{
    RenderableSurface, RenderableSurfaceDamage, SurfaceDamageRect, SurfacePlacement,
    dmabuf::DmabufBufferData,
    explicit_sync::{ExplicitSyncPoint, SyncobjSurfaceState},
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

#[derive(Debug, Default)]
pub(super) struct SurfaceData {
    surface_id: u32,
    pending_buffer: Mutex<Option<PendingSurfaceAttachment>>,
    pending_offset: Mutex<Option<(i32, i32)>>,
    pending_damage: Mutex<Vec<SurfaceDamageRect>>,
    frame_callbacks: Mutex<Vec<wl_callback::WlCallback>>,
    explicit_sync: Mutex<Option<Arc<SyncobjSurfaceState>>>,
    viewport: Mutex<SurfaceViewportState>,
    buffer_scale: Mutex<SurfaceBufferScaleState>,
    input_region: Mutex<SurfaceInputRegionState>,
}

#[derive(Debug)]
pub(super) struct PendingSurfaceDamage {
    pub(super) damage: RenderableSurfaceDamage,
    pub(super) explicit: bool,
}

impl PendingSurfaceDamage {
    pub(super) fn explicit(self) -> Option<RenderableSurfaceDamage> {
        self.explicit.then_some(self.damage)
    }
}

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
                        resize_serial: None,
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
                            resize_serial: None,
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

    pub(super) fn push_damage(&self, x: i32, y: i32, width: i32, height: i32) {
        let Some(rect) = SurfaceDamageRect::from_wayland_rect(x, y, width, height) else {
            return;
        };
        if let Ok(mut damage) = self.pending_damage.lock() {
            damage.push(rect);
        }
    }

    pub(super) fn take_damage(&self) -> PendingSurfaceDamage {
        let rects: Vec<SurfaceDamageRect> = self
            .pending_damage
            .lock()
            .map(|mut damage| damage.drain(..).collect())
            .unwrap_or_default();
        let explicit = !rects.is_empty();
        PendingSurfaceDamage {
            damage: RenderableSurfaceDamage::from_rects(rects),
            explicit,
        }
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

    pub(super) fn commit_pending_viewport(&self) -> Option<BufferSize> {
        self.viewport
            .lock()
            .map(|mut viewport| {
                if let Some(destination) = viewport.pending_destination.take() {
                    viewport.destination = destination;
                }
                viewport.destination
            })
            .unwrap_or_default()
    }

    pub(super) fn set_pending_buffer_scale(&self, scale: u32) {
        if let Ok(mut buffer_scale) = self.buffer_scale.lock() {
            buffer_scale.pending = Some(scale.max(1));
        }
    }

    pub(super) fn commit_pending_buffer_scale(&self) -> u32 {
        self.buffer_scale
            .lock()
            .map(|mut buffer_scale| {
                if let Some(scale) = buffer_scale.pending.take() {
                    buffer_scale.committed = scale.max(1);
                }
                buffer_scale.committed
            })
            .unwrap_or(1)
    }

    pub(super) fn set_pending_input_region(&self, region: SurfaceInputRegion) {
        if let Ok(mut state) = self.input_region.lock() {
            state.pending = Some(region);
        }
    }

    pub(super) fn commit_pending_input_region(&self) -> bool {
        let Ok(mut state) = self.input_region.lock() else {
            return false;
        };
        let Some(pending) = state.pending.take() else {
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
    pub(super) resize_serial: Option<u32>,
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
            resize_preview: None,
            generation,
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
