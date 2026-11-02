//! Render-thread cache of client DMA-BUFs imported as screencopy blit targets.

use std::collections::HashMap;

use khronos_egl as egl;
use oblivion_one::render_backend::buffer::WeakBufferIdentity;

use crate::{
    egl_renderer::{
        EglInstance, GlEglImageTargetTexture2DOes,
        screencopy_blit::{DmabufRenderTarget, ScreencopyBlitRegion, blit_framebuffer_region},
    },
    native_output::gpu_screencopy::GpuScreencopyTarget,
};

/// A capture client cycles a small buffer pool; an entry unused for this many
/// blit frames belongs to a stopped stream.
const UNUSED_FRAME_LIMIT: u64 = 240;

struct ScreencopyDestination {
    identity: WeakBufferIdentity,
    target: DmabufRenderTarget,
    last_used_frame: u64,
}

#[derive(Default)]
pub(super) struct ScreencopyDestinationCache {
    entries: HashMap<u64, ScreencopyDestination>,
    frame: u64,
}

impl ScreencopyDestinationCache {
    /// Blit `source` into every target; failures stay unblitted and fail.
    /// `draw_cursor` adds the cursor to each `overlay_cursor` copy after its
    /// blit; it draws nothing when the source already composited the cursor.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn blit_targets(
        &mut self,
        egl: &EglInstance,
        display: egl::Display,
        gl: &glow::Context,
        image_target: GlEglImageTargetTexture2DOes,
        source: glow::Framebuffer,
        source_size: (u32, u32),
        targets: &mut [GpuScreencopyTarget],
        mut draw_cursor: impl FnMut(glow::Framebuffer, ScreencopyBlitRegion) -> std::io::Result<()>,
    ) {
        self.frame = self.frame.wrapping_add(1);
        self.evict(gl, egl, display);
        for target in targets {
            let region = target.region();
            let buffer_size = target.buffer().size();
            if !region.fits_within(source_size.0, source_size.1)
                || (buffer_size.width, buffer_size.height) != (region.width, region.height)
            {
                continue;
            }
            let buffer_id = target.buffer_identity().id().get();
            if !self.entries.contains_key(&buffer_id) {
                match DmabufRenderTarget::import(egl, display, gl, image_target, target.buffer()) {
                    Ok(imported) => {
                        self.entries.insert(
                            buffer_id,
                            ScreencopyDestination {
                                identity: target.buffer_identity().clone(),
                                target: imported,
                                last_used_frame: self.frame,
                            },
                        );
                    }
                    Err(error) => {
                        eprintln!("screencopy DMA-BUF import failed: {error}");
                        continue;
                    }
                }
            }
            let Some(destination) = self.entries.get_mut(&buffer_id) else {
                continue;
            };
            destination.last_used_frame = self.frame;
            let framebuffer = destination.target.framebuffer();
            match blit_framebuffer_region(gl, source, framebuffer, region) {
                Ok(()) => {
                    // A copy without its cursor still beats a failed request.
                    if target.overlay_cursor()
                        && let Err(error) = draw_cursor(framebuffer, region)
                    {
                        eprintln!("{error}");
                    }
                    target.note_blitted();
                }
                Err(error) => eprintln!("{error}"),
            }
        }
    }

    fn evict(&mut self, gl: &glow::Context, egl: &EglInstance, display: egl::Display) {
        let frame = self.frame;
        let stale = self
            .entries
            .iter()
            .filter(|(_, entry)| {
                !entry.identity.is_alive()
                    || frame.wrapping_sub(entry.last_used_frame) > UNUSED_FRAME_LIMIT
            })
            .map(|(buffer_id, _)| *buffer_id)
            .collect::<Vec<_>>();
        for buffer_id in stale {
            if let Some(entry) = self.entries.remove(&buffer_id) {
                entry.target.destroy(gl, egl, display);
            }
        }
    }

    pub(super) fn teardown(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        display: egl::Display,
    ) {
        for (_, entry) in self.entries.drain() {
            entry.target.destroy(gl, egl, display);
        }
    }
}
