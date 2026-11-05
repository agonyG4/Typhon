use std::{
    io,
    ops::Range,
    os::unix::fs::FileExt,
    sync::atomic::{AtomicBool, Ordering},
};

use wayland_protocols_wlr::screencopy::v1::server::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};
use wayland_server::{
    WEnum,
    protocol::{wl_buffer, wl_output, wl_shm},
};

use super::dmabuf::DmabufBufferData;
use super::output::OutputSize;
use super::presentation::{PresentationClock, PresentationTimestamp};
use super::shm::ShmBufferData;
use super::*;
use crate::render_backend::buffer::{BufferIdentity, DmabufBufferHandle, DrmFormat};

const XRGB8888_BYTES_PER_PIXEL: u32 = 4;
const FRAME_DAMAGE_SINCE: u32 = 2;
const FRAME_BUFFER_DONE_SINCE: u32 = 3;
const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258;

/// Physical-pixel rectangle inside the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ScreencopyRegion {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScreencopySource {
    output_id: OutputId,
    overlay_cursor: bool,
    output_size: OutputSize,
    region: Option<ScreencopyRegion>,
}

impl ScreencopySource {
    fn buffer_size(self) -> (u32, u32) {
        self.region.map_or(
            (self.output_size.width, self.output_size.height),
            |region| (region.width, region.height),
        )
    }
}

/// One capture stream of a client: the same output and region.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ScreencopyStreamKey {
    client_id: ClientId,
    output_id: OutputId,
    region: Option<ScreencopyRegion>,
}

/// Scene state the last copy taken for a stream was made from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ScreencopyBaseline {
    scene_generation: u64,
    pointer: (f64, f64),
}

#[derive(Debug)]
pub struct ScreencopyFrameData {
    /// `None` when the capture request was rejected; the frame then only reports `failed`.
    source: Option<ScreencopySource>,
    copy_requested: AtomicBool,
}

/// Destination buffer of a screencopy request.
#[derive(Debug)]
enum ScreencopyTarget {
    /// Filled on the CPU from a rendered RGBA capture.
    Shm(ShmBufferData),
    /// Filled on the GPU by blitting the composited display frame.
    Dmabuf {
        handle: DmabufBufferHandle,
        identity: BufferIdentity,
    },
}

#[derive(Debug)]
pub struct ScreencopyPending {
    frame: zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
    client_id: ClientId,
    target: ScreencopyTarget,
    source: ScreencopySource,
    with_damage: bool,
    /// What this stream last received; `None` before its first copy.
    baseline: Option<ScreencopyBaseline>,
}

impl ScreencopyPending {
    pub fn output_id(&self) -> OutputId {
        self.source.output_id
    }

    pub fn overlay_cursor(&self) -> bool {
        self.source.overlay_cursor
    }

    /// `copy_with_damage` waits for damage since the stream's last copy,
    /// not since this request: a client that re-requests a few ms after
    /// `ready` (xdg-desktop-portal-hyprland waits at least 6 ms) would
    /// otherwise skip whatever was committed in between and lose a refresh
    /// per frame. Hyprland and wlroots likewise copy on the next frame.
    pub(crate) fn is_ready(&self, scene_generation: u64, pointer: (f64, f64)) -> bool {
        copy_is_ready(
            self.with_damage,
            self.source.overlay_cursor,
            self.baseline,
            ScreencopyBaseline {
                scene_generation,
                pointer,
            },
        )
    }

    fn stream_key(&self) -> ScreencopyStreamKey {
        ScreencopyStreamKey {
            client_id: self.client_id.clone(),
            output_id: self.source.output_id,
            region: self.source.region,
        }
    }

    fn is_gpu_copy(&self) -> bool {
        matches!(self.target, ScreencopyTarget::Dmabuf { .. })
    }

    /// The client DMA-BUF of a GPU copy request.
    pub fn dmabuf_target(&self) -> Option<(&DmabufBufferHandle, &BufferIdentity)> {
        match &self.target {
            ScreencopyTarget::Dmabuf { handle, identity } => Some((handle, identity)),
            ScreencopyTarget::Shm(_) => None,
        }
    }

    /// Captured rectangle in output physical pixels, top-left origin:
    /// `(x, y, width, height)`.
    pub fn physical_region(&self) -> (u32, u32, u32, u32) {
        let (width, height) = self.source.buffer_size();
        self.source.region.map_or((0, 0, width, height), |region| {
            (region.x, region.y, region.width, region.height)
        })
    }

    pub fn fail(&self) {
        self.frame.failed();
    }

    /// Report a frame whose pixels the GPU has already written into the
    /// client's DMA-BUF, stamped with the CLOCK_MONOTONIC time the copied
    /// frame was presented, or the current time when it never was.
    pub fn deliver_gpu_copy(self, presented_at_ns: Option<u64>) {
        debug_assert!(self.is_gpu_copy());
        if let Err(error) = self.send_ready_at(presented_at_ns) {
            eprintln!("screencopy GPU copy timestamp failed: {error}");
            self.frame.failed();
        }
    }

    fn send_ready(&self) -> io::Result<()> {
        self.send_ready_at(None)
    }

    fn send_ready_at(&self, presented_at_ns: Option<u64>) -> io::Result<()> {
        let (seconds, nanoseconds) = match presented_at_ns {
            Some(ns) => (ns / 1_000_000_000, (ns % 1_000_000_000) as u32),
            None => {
                let timestamp = PresentationTimestamp::from_clock(PresentationClock::Monotonic)?;
                let (hi, lo) = timestamp.protocol_seconds();
                (
                    (u64::from(hi) << 32) | u64::from(lo),
                    timestamp.nanoseconds(),
                )
            }
        };
        let (buffer_width, buffer_height) = self.source.buffer_size();
        self.frame.flags(zwlr_screencopy_frame_v1::Flags::empty());
        if self.with_damage && self.frame.version() >= FRAME_DAMAGE_SINCE {
            self.frame.damage(0, 0, buffer_width, buffer_height);
        }
        self.frame
            .ready((seconds >> 32) as u32, seconds as u32, nanoseconds);
        Ok(())
    }

    /// Write a captured top-left RGBA frame into the client's wl_shm buffer.
    pub fn deliver_rgba(self, rgba: &[u8], width: u32, height: u32) -> io::Result<()> {
        match self
            .write_rgba(rgba, width, height)
            .and_then(|()| self.send_ready())
        {
            Ok(()) => Ok(()),
            Err(error) => {
                self.frame.failed();
                Err(error)
            }
        }
    }

    fn write_rgba(&self, rgba: &[u8], width: u32, height: u32) -> io::Result<()> {
        let ScreencopyTarget::Shm(shm) = &self.target else {
            return Err(invalid_capture(
                "target: DMA-BUF requests are copied on the GPU",
            ));
        };
        let source = self.source.output_size;
        if (width, height) != (source.width, source.height) {
            return Err(invalid_capture("screencopy source size changed"));
        }
        if u64::try_from(rgba.len()).ok() != Some(u64::from(width) * u64::from(height) * 4) {
            return Err(invalid_capture("payload length"));
        }
        let origin = self
            .source
            .region
            .map_or((0, 0), |region| (region.x, region.y));
        let (buffer_width, buffer_height) = self.source.buffer_size();
        let stride = u64::try_from(shm.stride).map_err(|_| invalid_capture("stride"))?;
        let mut row = vec![0u8; (buffer_width * XRGB8888_BYTES_PER_PIXEL) as usize];
        for row_index in 0..buffer_height {
            let span = rgba_row_span(width, origin, row_index, buffer_width);
            convert_rgba_row_to_xrgb8888(&rgba[span], &mut row);
            let offset = u64::try_from(shm.offset).map_err(|_| invalid_capture("offset"))?
                + u64::from(row_index) * stride;
            shm.pool.file.write_all_at(&row, offset)?;
        }
        Ok(())
    }
}

fn copy_is_ready(
    with_damage: bool,
    overlay_cursor: bool,
    baseline: Option<ScreencopyBaseline>,
    current: ScreencopyBaseline,
) -> bool {
    let Some(baseline) = baseline.filter(|_| with_damage) else {
        return true;
    };
    current.scene_generation != baseline.scene_generation
        || (overlay_cursor && current.pointer != baseline.pointer)
}

/// Byte range of one cropped row inside a full-width top-left RGBA frame.
fn rgba_row_span(
    source_width: u32,
    origin: (u32, u32),
    row_index: u32,
    row_width: u32,
) -> Range<usize> {
    let source_row = origin.1 as usize + row_index as usize;
    let start = (source_row * source_width as usize + origin.0 as usize) * 4;
    start..start + row_width as usize * 4
}

/// Convert RGBA bytes to little-endian XRGB8888 bytes (B, G, R, 0xff).
fn convert_rgba_row_to_xrgb8888(source: &[u8], target: &mut [u8]) {
    debug_assert_eq!(source.len(), target.len());
    for (pixel, out) in source.chunks_exact(4).zip(target.chunks_exact_mut(4)) {
        out[0] = pixel[2];
        out[1] = pixel[1];
        out[2] = pixel[0];
        out[3] = 0xff;
    }
}

fn invalid_capture(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid screencopy {what}"),
    )
}

fn physical_region(
    scale: OutputScale,
    output_size: OutputSize,
    logical: (i32, i32, i32, i32),
) -> Option<ScreencopyRegion> {
    let (x, y, width, height) = logical;
    let x = u32::try_from(x).ok()?;
    let y = u32::try_from(y).ok()?;
    let width = u32::try_from(width).ok().filter(|width| *width > 0)?;
    let height = u32::try_from(height).ok().filter(|height| *height > 0)?;
    let (left, right) = scale.logical_to_physical_bounds(x, width);
    let (top, bottom) = scale.logical_to_physical_bounds(y, height);
    let right = right.min(output_size.width);
    let bottom = bottom.min(output_size.height);
    (left < right && top < bottom).then(|| ScreencopyRegion {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

impl GlobalDispatch<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, ()> for CompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_screencopy_manager_v1::Request::Destroy => {}
            zwlr_screencopy_manager_v1::Request::CaptureOutput {
                frame,
                overlay_cursor,
                output,
            } => {
                state.create_screencopy_frame(frame, overlay_cursor, &output, None, data_init);
            }
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion {
                frame,
                overlay_cursor,
                output,
                x,
                y,
                width,
                height,
            } => {
                state.create_screencopy_frame(
                    frame,
                    overlay_cursor,
                    &output,
                    Some((x, y, width, height)),
                    data_init,
                );
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, ScreencopyFrameData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &ScreencopyFrameData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => {
                state.request_screencopy_copy(client, resource, &buffer, data, false);
            }
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => {
                state.request_screencopy_copy(client, resource, &buffer, data, true);
            }
            zwlr_screencopy_frame_v1::Request::Destroy => {
                state.remove_screencopy_frame(resource);
            }
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _client_id: ClientId,
        resource: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        _data: &ScreencopyFrameData,
    ) {
        state.remove_screencopy_frame(resource);
    }
}

impl CompositorState {
    fn create_screencopy_frame(
        &mut self,
        frame: New<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1>,
        overlay_cursor: i32,
        output: &wl_output::WlOutput,
        region: Option<(i32, i32, i32, i32)>,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let source = self.output_id_for_binding(output).and_then(|output_id| {
            let output_size = self.output_size;
            let region = match region {
                Some(logical) => Some(physical_region(self.output_scale, output_size, logical)?),
                None => None,
            };
            Some(ScreencopySource {
                output_id,
                overlay_cursor: overlay_cursor != 0,
                output_size,
                region,
            })
        });
        let frame = data_init.init(
            frame,
            ScreencopyFrameData {
                source,
                copy_requested: AtomicBool::new(false),
            },
        );
        let Some(source) = source else {
            frame.failed();
            return;
        };
        let (width, height) = source.buffer_size();
        frame.buffer(
            wl_shm::Format::Xrgb8888,
            width,
            height,
            width * XRGB8888_BYTES_PER_PIXEL,
        );
        if frame.version() >= FRAME_BUFFER_DONE_SINCE {
            // DMA-BUF copies are blitted from the composited display frame.
            frame.linux_dmabuf(DRM_FORMAT_XRGB8888, width, height);
            frame.buffer_done();
        }
    }

    fn request_screencopy_copy(
        &mut self,
        client: &Client,
        resource: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        buffer: &wl_buffer::WlBuffer,
        data: &ScreencopyFrameData,
        with_damage: bool,
    ) {
        if data.copy_requested.swap(true, Ordering::Relaxed) {
            self.post_protocol_error(
                client,
                resource,
                zwlr_screencopy_frame_v1::Error::AlreadyUsed,
                "screencopy frame already has a copy request",
            );
            return;
        }
        let Some(source) = data.source else {
            resource.failed();
            return;
        };
        if with_damage && resource.version() < FRAME_DAMAGE_SINCE {
            self.post_protocol_error(
                client,
                resource,
                zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                "copy_with_damage requires frame version 2",
            );
            return;
        }
        let (width, height) = source.buffer_size();
        let target = if let Some(dmabuf) = buffer.data::<DmabufBufferData>() {
            if !screencopy_dmabuf_matches(&dmabuf.handle, width, height) {
                self.post_protocol_error(
                    client,
                    resource,
                    zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                    "screencopy DMA-BUF does not match the frame",
                );
                return;
            }
            ScreencopyTarget::Dmabuf {
                handle: dmabuf.handle.clone(),
                identity: dmabuf.identity.clone(),
            }
        } else if let Some(shm) = buffer.data::<ShmBufferData>().cloned() {
            let format_ok = matches!(
                shm.format,
                WEnum::Value(wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888)
            );
            let size_ok = u32::try_from(shm.width).ok() == Some(width)
                && u32::try_from(shm.height).ok() == Some(height);
            let stride_ok = i64::from(shm.stride) >= i64::from(width) * 4;
            if !format_ok || !size_ok || !stride_ok || !shm.fits_in_pool() {
                self.post_protocol_error(
                    client,
                    resource,
                    zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                    "screencopy buffer does not match the frame",
                );
                return;
            }
            ScreencopyTarget::Shm(shm)
        } else {
            resource.failed();
            return;
        };
        let mut pending = ScreencopyPending {
            frame: resource.clone(),
            client_id: client.id(),
            target,
            source,
            with_damage,
            baseline: None,
        };
        pending.baseline = self
            .screencopy_baselines
            .get(&pending.stream_key())
            .copied();
        self.screencopy_pending.push_back(pending);
    }

    /// Ready wl_shm requests, served by the private re-render capture path.
    pub(in crate::compositor) fn has_ready_screencopy(&self) -> bool {
        let pointer = (self.last_pointer_x, self.last_pointer_y);
        self.screencopy_pending.iter().any(|pending| {
            !pending.is_gpu_copy() && pending.is_ready(self.scene_render_generation, pointer)
        })
    }

    pub(in crate::compositor) fn take_ready_screencopy(&mut self) -> Option<ScreencopyPending> {
        let generation = self.scene_render_generation;
        let pointer = (self.last_pointer_x, self.last_pointer_y);
        let index = self
            .screencopy_pending
            .iter()
            .position(|pending| !pending.is_gpu_copy() && pending.is_ready(generation, pointer))?;
        let pending = self.screencopy_pending.remove(index)?;
        self.note_screencopy_taken(&pending);
        Some(pending)
    }

    /// The copy of `pending` shows the current scene; later requests of its
    /// stream wait for damage past this point.
    fn note_screencopy_taken(&mut self, pending: &ScreencopyPending) {
        self.screencopy_baselines.insert(
            pending.stream_key(),
            ScreencopyBaseline {
                scene_generation: self.scene_render_generation,
                pointer: (self.last_pointer_x, self.last_pointer_y),
            },
        );
    }

    /// Ready DMA-BUF requests, served from the next composited display frame.
    pub(in crate::compositor) fn has_pending_gpu_screencopy(&self) -> bool {
        let pointer = (self.last_pointer_x, self.last_pointer_y);
        self.screencopy_pending.iter().any(|pending| {
            pending.is_gpu_copy() && pending.is_ready(self.scene_render_generation, pointer)
        })
    }

    pub(in crate::compositor) fn take_ready_gpu_screencopies(&mut self) -> Vec<ScreencopyPending> {
        let generation = self.scene_render_generation;
        let pointer = (self.last_pointer_x, self.last_pointer_y);
        let (ready, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.screencopy_pending)
            .into_iter()
            .partition(|pending| pending.is_gpu_copy() && pending.is_ready(generation, pointer));
        self.screencopy_pending = kept.into();
        for pending in &ready {
            self.note_screencopy_taken(pending);
        }
        ready
    }

    pub(in crate::compositor) fn remove_screencopy_frame(
        &mut self,
        resource: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
    ) {
        self.screencopy_pending
            .retain(|pending| pending.frame.id() != resource.id());
    }

    pub(in crate::compositor) fn clear_screencopy_client(&mut self, client_id: &ClientId) {
        self.screencopy_pending
            .retain(|pending| pending.client_id != *client_id);
        self.screencopy_baselines
            .retain(|key, _| key.client_id != *client_id);
    }

    pub(in crate::compositor) fn fail_screencopy_for_output(&mut self, output_id: OutputId) {
        let (failed, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.screencopy_pending)
            .into_iter()
            .partition(|pending| pending.output_id() == output_id);
        self.screencopy_pending = kept.into();
        self.screencopy_baselines
            .retain(|key, _| key.output_id != output_id);
        for pending in failed {
            pending.fail();
        }
    }
}

impl OwnCompositorServer {
    pub fn has_pending_gpu_screencopy(&self) -> bool {
        self.state.has_pending_gpu_screencopy()
    }

    pub fn take_ready_gpu_screencopies(&mut self) -> Vec<ScreencopyPending> {
        self.state.take_ready_gpu_screencopies()
    }
}

fn screencopy_dmabuf_matches(handle: &DmabufBufferHandle, width: u32, height: u32) -> bool {
    let size = handle.size();
    (size.width, size.height) == (width, height)
        && matches!(handle.format(), DrmFormat::Xrgb8888 | DrmFormat::Argb8888)
        && !handle.planes().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_with_damage_waits_only_for_damage_since_the_last_copy() {
        let at = |scene_generation, pointer| ScreencopyBaseline {
            scene_generation,
            pointer,
        };
        let last = Some(at(5, (1.0, 1.0)));
        // First copy of a stream: nothing to compare against.
        assert!(copy_is_ready(true, false, None, at(5, (1.0, 1.0))));
        // Committed after the last copy, before this request: ready.
        assert!(copy_is_ready(true, false, last, at(6, (1.0, 1.0))));
        assert!(!copy_is_ready(true, false, last, at(5, (1.0, 1.0))));
        // Pointer motion only counts when the cursor is in the copy.
        assert!(!copy_is_ready(true, false, last, at(5, (2.0, 1.0))));
        assert!(copy_is_ready(true, true, last, at(5, (2.0, 1.0))));
        // Plain copy never waits.
        assert!(copy_is_ready(false, false, last, at(5, (1.0, 1.0))));
    }

    #[test]
    fn rgba_row_converts_to_little_endian_xrgb() {
        let source = [1, 2, 3, 9, 40, 50, 60, 70];
        let mut target = [0; 8];
        convert_rgba_row_to_xrgb8888(&source, &mut target);
        assert_eq!(target, [3, 2, 1, 0xff, 60, 50, 40, 0xff]);
    }

    #[test]
    fn rgba_row_span_crops_region_origin() {
        // 4x3 frame, region starting at (1, 1), two pixels wide.
        assert_eq!(rgba_row_span(4, (1, 1), 0, 2), 20..28);
        assert_eq!(rgba_row_span(4, (1, 1), 1, 2), 36..44);
        assert_eq!(rgba_row_span(4, (0, 0), 2, 4), 32..48);
    }

    #[test]
    fn physical_region_scales_and_clamps_to_output() {
        let output = OutputSize::new(1920, 1080);
        let scale = OutputScale::from_factor(2.0);
        let region = physical_region(scale, output, (10, 20, 100, 50));
        assert_eq!(
            region,
            Some(ScreencopyRegion {
                x: 20,
                y: 40,
                width: 200,
                height: 100,
            })
        );
        let clipped = physical_region(scale, output, (900, 500, 100, 100));
        assert_eq!(
            clipped,
            Some(ScreencopyRegion {
                x: 1800,
                y: 1000,
                width: 120,
                height: 80,
            })
        );
        assert_eq!(physical_region(scale, output, (2000, 0, 10, 10)), None);
        assert_eq!(physical_region(scale, output, (0, 0, 0, 10)), None);
    }
}
