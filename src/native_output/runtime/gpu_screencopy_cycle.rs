//! Reactor side of zero-copy DMA-BUF screencopy.

use super::*;
use crate::native_output::gpu_screencopy::ScreencopyBlitRegion;

impl NativeRuntime {
    /// Move ready DMA-BUF screencopy requests onto the next composited frame
    /// and keep a redraw requested until a frame carries them.
    pub(super) fn stage_gpu_screencopy_requests(
        &mut self,
        cycle: &mut NativeCycleState,
    ) -> NativeResult<()> {
        if self.server.has_pending_gpu_screencopy() {
            let (width, height) = self.head.scanout.dimensions();
            for pending in self.server.take_ready_gpu_screencopies() {
                let output_id = pending.output_id();
                if output_id != self.head.output_id
                    || !self.server.screen_capture_output_is_current(output_id)
                {
                    pending.fail();
                    continue;
                }
                let Some((buffer, identity)) = pending
                    .dmabuf_target()
                    .map(|(buffer, identity)| (buffer.clone(), identity.downgrade()))
                else {
                    pending.fail();
                    continue;
                };
                let (x, y, region_width, region_height) = pending.physical_region();
                let region = ScreencopyBlitRegion {
                    x,
                    y,
                    width: region_width,
                    height: region_height,
                };
                if !region.fits_within(width, height) {
                    pending.fail();
                    continue;
                }
                let overlay_cursor = pending.overlay_cursor();
                let target =
                    self.gpu_screencopy
                        .admit(pending, buffer, identity, region, overlay_cursor);
                // Non-Atomic backends have no composited slot to copy from.
                if let Err(target) = self.head.scanout.stage_gpu_screencopy(target) {
                    drop(target);
                }
            }
        }
        // Blitted copies are delivered once their render fence signals; the
        // reactor wakes on that fence, so settle every cycle while any wait.
        if self.gpu_screencopy.has_outstanding() {
            self.settle_gpu_screencopy()?;
        }
        if self.head.scanout.has_staged_gpu_screencopy() {
            cycle.redraw_requested = true;
            self.queued_redraw_requested = true;
        }
        Ok(())
    }

    /// Fail unblitted requests, run the fence fallback for copies whose frame
    /// was never presented, and flush whatever was sent, including `ready`
    /// events from pageflip handling.
    pub(super) fn settle_gpu_screencopy(&mut self) -> NativeResult<()> {
        if self.gpu_screencopy.settle(monotonic_now_ns()?) > 0 {
            self.server.flush_wayland_clients()?;
        }
        Ok(())
    }

    /// Settle everything not yet delivered: staged targets, in-flight requests
    /// and ready requests still queued in the compositor. Blitted copies whose
    /// fence already signaled are still delivered.
    pub(super) fn fail_gpu_screencopy(&mut self) {
        self.head.scanout.clear_staged_gpu_screencopy();
        self.gpu_screencopy.fail_all();
        for pending in self.server.take_ready_gpu_screencopies() {
            pending.fail();
        }
    }
}
