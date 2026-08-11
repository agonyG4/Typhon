use super::state::{CapturedSurfacePacing, CommitTimingConstraint};
use super::state_data::SurfaceData;

impl SurfaceData {
    pub(super) fn set_pending_fifo_barrier(&self) {
        if let Ok(mut pacing) = self.pending_pacing.lock() {
            pacing.fifo_set_barrier = true;
        }
    }

    pub(super) fn set_pending_fifo_wait(&self) {
        if let Ok(mut pacing) = self.pending_pacing.lock() {
            pacing.fifo_wait_barrier = true;
        }
    }

    pub(super) fn set_pending_commit_timing(&self, timing: CommitTimingConstraint) -> bool {
        let Ok(mut pacing) = self.pending_pacing.lock() else {
            return false;
        };
        if pacing.commit_timing.is_some() {
            return false;
        }
        pacing.commit_timing = Some(timing);
        true
    }

    pub(super) fn take_pending_surface_pacing(&self) -> CapturedSurfacePacing {
        self.pending_pacing
            .lock()
            .map(|mut pacing| CapturedSurfacePacing::from_pending(std::mem::take(&mut *pacing)))
            .unwrap_or_default()
    }
}
