use super::NativeFramePacing;
use oblivion_one::native::presentation_deadline::PresentationTargetReason;

impl NativeFramePacing {
    pub(crate) fn note_o1_credit2_outcome(
        &mut self,
        reason: PresentationTargetReason,
        admission_overlap_required_ns: u64,
        target_hit: bool,
    ) {
        if !matches!(
            reason,
            PresentationTargetReason::PredictedPressure
                | PresentationTargetReason::ProvenReadinessMiss
                | PresentationTargetReason::ForcedValidation
        ) {
            return;
        }
        match (target_hit, admission_overlap_required_ns > 0) {
            (true, true) => {
                self.o1_credit2_useful_hits = self.o1_credit2_useful_hits.saturating_add(1)
            }
            (true, false) => {
                self.o1_credit2_unnecessary_hits =
                    self.o1_credit2_unnecessary_hits.saturating_add(1)
            }
            (false, _) => {
                self.o1_credit2_ineffective_misses =
                    self.o1_credit2_ineffective_misses.saturating_add(1)
            }
        }
    }

    pub(crate) fn note_o1_credit2_grant(&mut self) {
        self.o1_credit2_pending_grant = true;
    }

    pub(crate) fn note_o1_credit2_granted_not_consumed(&mut self) {
        if self.o1_credit2_pending_grant {
            self.o1_credit2_granted_not_consumed =
                self.o1_credit2_granted_not_consumed.saturating_add(1);
            self.o1_credit2_pending_grant = false;
        }
    }

    pub(crate) fn note_o1_credit2_drain(&mut self) {
        self.o1_credit2_drain_events = self.o1_credit2_drain_events.saturating_add(1);
    }

    pub(crate) fn note_o1_credit2_refill_suppressed_while_draining(&mut self) {
        self.o1_credit2_refill_suppressed_while_draining = self
            .o1_credit2_refill_suppressed_while_draining
            .saturating_add(1);
    }
}
