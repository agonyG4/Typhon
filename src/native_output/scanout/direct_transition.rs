use super::*;

impl NativeScanoutBackend {
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
