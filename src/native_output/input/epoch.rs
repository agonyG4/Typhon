/// Native input epochs are the unit at which pointer-constraint semantics are
/// intended to remain stable.  The transition gate is deliberately kept
/// separate from compositor protocol state.
///
/// `active_id` remains set across bounded drain continuations.  Consequently,
/// a queued backend transition cannot reinterpret a later chunk of the same
/// materialized native-input backlog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeInputConstraintSettlementPoint {
    BeforeInputEpoch,
    AfterInputEpoch(Option<u64>),
}

#[derive(Debug, Default)]
pub(crate) struct NativeInputEpoch {
    next_id: u64,
    active_id: Option<u64>,
    backlog_pending: bool,
    deferred_wayland_progression: bool,
}

impl NativeInputEpoch {
    pub(crate) fn begin(&mut self, continuation: bool) -> u64 {
        if continuation {
            self.active_id
                .expect("input backlog continuation without an active epoch")
        } else {
            self.next_id = self.next_id.saturating_add(1).max(1);
            self.active_id = Some(self.next_id);
            self.next_id
        }
    }

    pub(crate) fn finish(&mut self, budget_exhausted: bool) {
        self.backlog_pending = budget_exhausted;
        if !budget_exhausted {
            self.active_id = None;
        }
    }

    pub(crate) const fn active_id(&self) -> Option<u64> {
        self.active_id
    }

    pub(crate) const fn backlog_pending(&self) -> bool {
        self.backlog_pending
    }

    pub(crate) const fn constraint_settlement_allowed(&self) -> bool {
        self.active_id.is_none()
    }

    pub(crate) fn request_deferred_wayland_progression(&mut self) {
        self.deferred_wayland_progression = true;
    }

    pub(crate) fn take_deferred_wayland_progression(&mut self) -> bool {
        std::mem::take(&mut self.deferred_wayland_progression)
    }
}

#[cfg(test)]
mod tests {
    use super::NativeInputEpoch;

    #[test]
    fn deferred_wayland_progression_survives_a_budget_continuation() {
        let mut epoch = NativeInputEpoch::default();
        epoch.begin(false);
        epoch.request_deferred_wayland_progression();
        epoch.finish(true);

        assert!(epoch.backlog_pending());
        assert_eq!(epoch.begin(true), 1);
        epoch.finish(false);

        assert!(epoch.take_deferred_wayland_progression());
        assert!(!epoch.take_deferred_wayland_progression());
    }
}
