use super::*;

impl NativeScanoutBackend {
    pub(crate) fn direct_scanout_pending(&self) -> bool {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.direct_scanout_pending(),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => false,
        }
    }

    pub(crate) fn direct_scanout_surface(&self) -> Option<u32> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.direct_scanout_surface(),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => None,
        }
    }

    pub(crate) fn direct_scanout_info(&self) -> Option<(u64, u32, u32, u64)> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.direct_scanout_info(),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => None,
        }
    }

    pub(crate) fn direct_scanout_presented_info(&self) -> Option<(u32, u32, u64)> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.direct_scanout_presented_info(),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => None,
        }
    }

    pub(crate) fn note_direct_blocker(&mut self, reason: &str) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_direct_blocker(reason);
        }
    }

    pub(crate) fn note_direct_worker_submission(
        &mut self,
        test_only: bool,
        submit_started_at: u64,
        submit_returned_at: u64,
    ) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_direct_worker_submission(test_only, submit_started_at, submit_returned_at);
        }
    }

    pub(crate) fn note_direct_duplicate_feedback(&mut self) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_direct_duplicate_feedback();
        }
    }

    pub(crate) fn direct_scanout_inhibited(&self) -> bool {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.direct_scanout_inhibited(),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => true,
        }
    }

    pub(crate) fn direct_scanout_counters(&self) -> Option<DirectScanoutCounters> {
        match self {
            Self::AtomicEglGbm(scanout) => Some(scanout.direct_scanout_counters()),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => None,
        }
    }

    pub(crate) fn note_direct_composited_render_ahead_suppressed(&mut self) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_composited_render_ahead_suppressed();
        }
    }

    pub(crate) fn direct_pageflip_info(
        &self,
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
    ) -> io::Result<DirectPageflipInfo> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.direct_pageflip_info(transaction_id, token),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => Err(io::Error::other(
                "direct pageflip is unsupported by this backend",
            )),
        }
    }

    pub(crate) fn take_direct_pageflip_surface_damage(
        &mut self,
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
    ) -> io::Result<oblivion_one::compositor::SurfaceDamagePresentation> {
        match self {
            Self::AtomicEglGbm(scanout) => {
                scanout.take_direct_pageflip_surface_damage(transaction_id, token)
            }
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => Err(io::Error::other(
                "direct pageflip is unsupported by this backend",
            )),
        }
    }

    pub(crate) fn note_direct_entry(&mut self) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_direct_entry();
        }
    }

    pub(crate) fn note_direct_replacement(&mut self) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_direct_replacement();
        }
    }

    pub(crate) fn note_direct_rejection(&mut self, test_only: bool, combined_cursor: bool) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_direct_rejection(test_only, combined_cursor);
        }
    }

    pub(crate) fn note_direct_fallback_redraw(&mut self) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_direct_fallback_redraw();
        }
    }

    pub(crate) fn invalidate_presented_damage_history(&mut self) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.invalidate_presented_damage_history();
        }
    }
}
