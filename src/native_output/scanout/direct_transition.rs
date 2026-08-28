use super::*;
use crate::native_output::runtime::DirectCallbackLeakMetrics;

impl NativeScanoutBackend {
    pub(crate) fn direct_scanout_pending(&self) -> bool {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.direct_scanout_pending(),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => false,
        }
    }

    pub(crate) fn direct_scanout_info(&self) -> Option<(u64, u32, u32, u64)> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.direct_scanout_info(),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => None,
        }
    }

    pub(crate) fn direct_scanout_submitted_info(&self) -> Option<(u32, u64, u32, u64)> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.direct_scanout_submitted_info(),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => None,
        }
    }

    pub(crate) fn direct_scanout_presented_info(&self) -> Option<(u32, u64, u32, u64)> {
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

    pub(crate) fn note_direct_test_only(&mut self, duration_ns: u64, rejected: bool) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_direct_test_only(duration_ns, rejected);
        }
    }

    pub(crate) fn note_direct_worker_admission_rejected(&mut self, queue_overflow: bool) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_direct_worker_admission_rejected(queue_overflow);
        }
    }

    pub(crate) fn note_direct_real_submit_attempt(&mut self, rejected: bool) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_direct_real_submit_attempt(rejected);
        }
    }

    pub(crate) fn note_direct_callback_owner_leaks(&mut self, leaks: DirectCallbackLeakMetrics) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_direct_callback_owner_leaks(leaks);
        }
    }

    pub(crate) fn note_direct_duplicate_feedback(&mut self) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_direct_duplicate_feedback();
        }
    }

    pub(crate) fn note_dmabuf_feedback_unchanged_rebuild(&mut self) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_dmabuf_feedback_unchanged_rebuild();
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

    pub(crate) fn note_direct_entry(&mut self) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_direct_entry();
        }
    }

    pub(crate) fn note_direct_presentation(&mut self) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_direct_presentation();
        }
    }

    pub(crate) fn note_direct_fallback_cycles(&mut self, cycles: u64) {
        if let Self::AtomicEglGbm(scanout) = self {
            scanout.note_direct_fallback_cycles(cycles);
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

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn try_direct_scanout(
        &mut self,
        kms: &KmsBackendSelection,
        server: &mut OwnCompositorServer,
        output_transactions: &mut OutputTransactionLedger,
        target: oblivion_one::native::presentation_deadline::PresentationTarget,
        cursor: Option<&AtomicCursorVisualState>,
        cursor_source_key: Option<crate::native_output::output::NativeCursorImageKey>,
        cursor_revision: Option<crate::native_output::presentation::plane::CursorRevision>,
        cursor_epoch: u64,
        pacing_mode: NativeOutputPacingMode,
        confirmed_content_type: oblivion_one::compositor::DrmContentType,
        worker: Option<&crate::native_output::kms_worker::KmsCommitWorkerHandle>,
    ) -> io::Result<DirectScanoutAttempt> {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.try_direct_scanout(
                kms,
                server,
                output_transactions,
                target,
                cursor,
                cursor_source_key,
                cursor_revision,
                cursor_epoch,
                pacing_mode,
                confirmed_content_type,
                worker,
            ),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => Err(io::Error::other(
                "direct scanout is unsupported by this backend",
            )),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn complete_direct_pageflip(
        &mut self,
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
        presented_at: MonotonicTimestampNs,
    ) -> io::Result<DirectPageflipCompletion> {
        match self {
            Self::AtomicEglGbm(scanout) => {
                scanout.complete_direct_pageflip(transaction_id, token, presented_at)
            }
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => Err(io::Error::other(
                "direct pageflip is unsupported by this backend",
            )),
        }
    }

    pub(crate) fn prepare_direct_pageflip(
        &self,
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
        presented_at: MonotonicTimestampNs,
    ) -> io::Result<PreparedDirectPageflip> {
        match self {
            Self::AtomicEglGbm(scanout) => {
                scanout.prepare_direct_pageflip(transaction_id, token, presented_at)
            }
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => Err(io::Error::other(
                "direct pageflip is unsupported by this backend",
            )),
        }
    }

    pub(crate) fn commit_prepared_direct_pageflip(
        &mut self,
        prepared: PreparedDirectPageflip,
    ) -> DirectPageflipCompletion {
        match self {
            Self::AtomicEglGbm(scanout) => scanout.commit_prepared_direct_pageflip(prepared),
            Self::NativeEglGbm(_) | Self::Gbm(_) | Self::Dumb(_) => {
                panic!("direct pageflip is unsupported by this backend")
            }
        }
    }
}
