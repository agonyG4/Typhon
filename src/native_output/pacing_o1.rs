use super::NativeFramePacing;
use oblivion_one::native::buffering::O1AdmissionObservation;

impl NativeFramePacing {
    pub(crate) fn note_o1_credit2_outcome(
        &mut self,
        admission: Option<O1AdmissionObservation>,
        target_hit: bool,
    ) {
        let Some(admission) = admission else {
            return;
        };
        if !admission.used_extra_credit {
            return;
        }
        match (target_hit, admission.overlap_required_ns > 0) {
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

    pub(crate) fn note_o1_credit2_extra_credit_consumed(&mut self) {
        self.o1_credit2_pending_grant = false;
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

#[cfg(test)]
mod tests {
    use super::super::NativeFramePacing;
    use oblivion_one::native::buffering::{O1AdmissionObservation, PresentationOpportunityId};

    #[test]
    fn outcomes_use_the_exact_admission_observation() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        let frame_a = O1AdmissionObservation {
            opportunity: PresentationOpportunityId::new(1, 10),
            desired_credit: 2,
            owned_future_depth_before: 1,
            overlap_required_ns: 800_000,
            used_extra_credit: true,
        };
        let frame_b = O1AdmissionObservation {
            opportunity: PresentationOpportunityId::new(1, 11),
            overlap_required_ns: 0,
            ..frame_a
        };

        pacing.note_o1_credit2_outcome(Some(frame_b), true);
        pacing.note_o1_credit2_outcome(Some(frame_a), true);

        assert_eq!(pacing.o1_credit2_useful_hits, 1);
        assert_eq!(pacing.o1_credit2_unnecessary_hits, 1);

        pacing.note_o1_credit2_outcome(
            Some(O1AdmissionObservation {
                used_extra_credit: false,
                ..frame_a
            }),
            true,
        );
        pacing.note_o1_credit2_outcome(Some(frame_a), false);
        assert_eq!(pacing.o1_credit2_ineffective_misses, 1);
    }

    #[test]
    fn grant_not_consumed_is_a_bounded_transition_metric() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.note_o1_credit2_grant();
        pacing.note_o1_credit2_granted_not_consumed();
        assert_eq!(pacing.o1_credit2_granted_not_consumed, 1);

        pacing.note_o1_credit2_grant();
        pacing.note_o1_credit2_extra_credit_consumed();
        pacing.note_o1_credit2_granted_not_consumed();
        assert_eq!(pacing.o1_credit2_granted_not_consumed, 1);
    }
}
