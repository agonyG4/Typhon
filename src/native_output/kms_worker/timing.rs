//! Binary-runtime KMS worker integration for dispatch timing.

use crate::native_output::presentation::kms_timing::KmsModeTimingKey;
use crate::native_output::runtime::AtomicCommitKind;
pub(crate) use oblivion_one::native::kms_worker_dispatch_timing::{
    DISPATCH_GUARD_NS, KmsWorkerDispatchBudget, KmsWorkerDispatchModel,
    KmsWorkerDispatchTailObservation, dispatch_fairness,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KmsServiceClass {
    Primary,
    CursorPlaneDelta,
    DirectScanout,
}

impl KmsServiceClass {
    pub(crate) const fn from_kind(kind: AtomicCommitKind) -> Self {
        match kind {
            AtomicCommitKind::CompositedPrimary { .. } => Self::Primary,
            AtomicCommitKind::PlaneDelta { .. } => Self::CursorPlaneDelta,
            AtomicCommitKind::DirectPrimary { .. } => Self::DirectScanout,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KmsDispatchModels {
    primary: KmsWorkerDispatchModel,
    cursor: KmsWorkerDispatchModel,
    direct: KmsWorkerDispatchModel,
    cursor_generation: Option<u64>,
    cursor_mode_key: Option<KmsModeTimingKey>,
}

impl Default for KmsDispatchModels {
    fn default() -> Self {
        Self {
            primary: KmsWorkerDispatchModel::default(),
            cursor: KmsWorkerDispatchModel::cursor(),
            direct: KmsWorkerDispatchModel::default(),
            cursor_generation: None,
            cursor_mode_key: None,
        }
    }
}

impl KmsDispatchModels {
    pub(crate) fn model_mut(&mut self, class: KmsServiceClass) -> &mut KmsWorkerDispatchModel {
        match class {
            KmsServiceClass::Primary => &mut self.primary,
            KmsServiceClass::CursorPlaneDelta => &mut self.cursor,
            KmsServiceClass::DirectScanout => &mut self.direct,
        }
    }

    pub(crate) fn model(&self, class: KmsServiceClass) -> &KmsWorkerDispatchModel {
        match class {
            KmsServiceClass::Primary => &self.primary,
            KmsServiceClass::CursorPlaneDelta => &self.cursor,
            KmsServiceClass::DirectScanout => &self.direct,
        }
    }

    pub(crate) fn rebind_cursor_generation(&mut self, generation: u64) {
        self.rebind_cursor_timing_domain(generation, None);
    }

    pub(crate) fn rebind_cursor_timing_domain(
        &mut self,
        generation: u64,
        mode_key: Option<KmsModeTimingKey>,
    ) {
        if self.cursor_generation != Some(generation) || self.cursor_mode_key != mode_key {
            self.cursor.reset_cursor_state();
            self.cursor_generation = Some(generation);
            self.cursor_mode_key = mode_key;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_and_cursor_dispatch_histories_are_independent() {
        let mut models = KmsDispatchModels::default();
        let cursor_budget = models
            .model(KmsServiceClass::CursorPlaneDelta)
            .budget()
            .dispatch_budget_ns;
        let primary_budget = models
            .model(KmsServiceClass::Primary)
            .budget()
            .dispatch_budget_ns;

        for _ in 0..16 {
            let primary = models.model_mut(KmsServiceClass::Primary);
            primary.record_fixed_timed_wake(25_000_000);
            primary.record_worker_service(10_000_000, 10_000_000, 25_000_000);
            primary.observe_submission_deadline(1_000_000, 26_000_000, true, true);
        }

        assert_eq!(
            models
                .model(KmsServiceClass::CursorPlaneDelta)
                .budget()
                .dispatch_budget_ns,
            cursor_budget
        );
        assert!(
            models
                .model(KmsServiceClass::Primary)
                .budget()
                .dispatch_budget_ns
                > primary_budget
        );
    }

    #[test]
    fn output_generation_rebind_resets_cursor_dispatch_history() {
        let mut models = KmsDispatchModels::default();
        models.rebind_cursor_generation(3);
        let cursor = models.model_mut(KmsServiceClass::CursorPlaneDelta);
        cursor.record_fixed_timed_wake(10_000_000);
        cursor.record_worker_service(2_000_000, 3_000_000, 4_000_000);
        cursor.observe_submission_deadline(1_000_000, 2_000_000, true, true);
        assert!(
            models
                .model(KmsServiceClass::CursorPlaneDelta)
                .adaptive_tail_guard_ns()
                > 0
        );

        models.rebind_cursor_generation(4);

        let cursor = models.model(KmsServiceClass::CursorPlaneDelta);
        assert_eq!(cursor.adaptive_tail_guard_ns(), 0);
        assert_eq!(cursor.budget().wake_lateness_ns, 0);
        assert_eq!(cursor.budget().dispatch_budget_ns, DISPATCH_GUARD_NS);
    }

    #[test]
    fn mode_rebind_resets_cursor_dispatch_history_without_changing_generation() {
        use crate::native_output::presentation::kms_timing::KmsModeTiming;
        use drm_sys::drm_mode_modeinfo;

        let first_mode = KmsModeTiming::from_mode(&drm_mode_modeinfo::default(), 16_000_000).key();
        let mut changed_mode = drm_mode_modeinfo::default();
        changed_mode.hdisplay = 1920;
        let second_mode = KmsModeTiming::from_mode(&changed_mode, 16_000_000).key();

        let mut models = KmsDispatchModels::default();
        models.rebind_cursor_timing_domain(3, Some(first_mode));
        let cursor = models.model_mut(KmsServiceClass::CursorPlaneDelta);
        cursor.record_fixed_timed_wake(10_000_000);
        cursor.record_worker_service(2_000_000, 3_000_000, 4_000_000);
        cursor.observe_submission_deadline(1_000_000, 2_000_000, true, true);
        assert!(
            models
                .model(KmsServiceClass::CursorPlaneDelta)
                .adaptive_tail_guard_ns()
                > 0
        );

        models.rebind_cursor_timing_domain(3, Some(second_mode));

        let cursor = models.model(KmsServiceClass::CursorPlaneDelta);
        assert_eq!(cursor.adaptive_tail_guard_ns(), 0);
        assert_eq!(cursor.budget().wake_lateness_ns, 0);
        assert_eq!(cursor.budget().dispatch_budget_ns, DISPATCH_GUARD_NS);
    }
}
