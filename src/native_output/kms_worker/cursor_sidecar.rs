#![allow(dead_code)]

use std::sync::Arc;

use crate::native_output::{
    CursorFramebufferPin, CursorPlaneAssignment, OutputTransaction, OutputTransactionId,
    presentation::plane::{CursorRevision, CursorSidecarId, PresentedCursorDelivery},
    presentation::plane_policy::CursorCapabilityKey,
};
use oblivion_one::native::presentation_deadline::{MonotonicTimestampNs, PresentationTarget};

use super::KmsTestOnlyPolicy;
use super::KmsValidationBase;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorSidecarCoupling {
    Independent,
    MustBundleWith(OutputTransactionId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorSidecarReturnReason {
    RequiredPrimaryPassedFreeze,
    RequiredPrimaryTerminal,
    GenerationInvalidated,
    Quiesced,
}

#[derive(Debug, Clone)]
pub(crate) struct CursorSidecar {
    pub(crate) id: CursorSidecarId,
    pub(crate) transaction: Arc<OutputTransaction>,
    pub(crate) revision: CursorRevision,
    pub(crate) assignment: CursorPlaneAssignment,
    pub(crate) lease: Option<CursorFramebufferPin>,
    pub(crate) coupling: CursorSidecarCoupling,
    pub(crate) created_at: MonotonicTimestampNs,
    pub(crate) deadline: PresentationTarget,
    pub(crate) crtc_id: u32,
    pub(crate) test_policy: KmsTestOnlyPolicy,
    pub(crate) cursor_delivery: PresentedCursorDelivery,
    pub(crate) capability_key: Option<CursorCapabilityKey>,
    pub(crate) validation_base: KmsValidationBase,
}

#[derive(Debug, Default)]
pub(crate) struct CursorSidecarMailbox {
    pending: Option<CursorSidecar>,
}

impl CursorSidecarMailbox {
    pub(crate) fn offer(&mut self, sidecar: CursorSidecar) -> Option<CursorSidecar> {
        self.pending.replace(sidecar)
    }

    pub(crate) fn pending(&self) -> Option<&CursorSidecar> {
        self.pending.as_ref()
    }

    pub(crate) fn take(&mut self) -> Option<CursorSidecar> {
        self.pending.take()
    }

    pub(crate) fn take_must_bundle_with(
        &mut self,
        transaction_id: OutputTransactionId,
    ) -> Option<CursorSidecar> {
        self.pending
            .as_ref()
            .is_some_and(|sidecar| {
                sidecar.coupling == CursorSidecarCoupling::MustBundleWith(transaction_id)
            })
            .then(|| self.pending.take())
            .flatten()
    }

    pub(crate) fn take_independent_due(
        &mut self,
        output_generation: u64,
        crtc_id: u32,
        target: PresentationTarget,
    ) -> Option<CursorSidecar> {
        let promotable = self.pending.as_ref().is_some_and(|sidecar| {
            sidecar.transaction.output_generation() == output_generation
                && sidecar.crtc_id == crtc_id
                && sidecar.deadline.clock_generation == target.clock_generation
                && sidecar.deadline.presentation_time.get() <= target.presentation_time.get()
                && matches!(sidecar.coupling, CursorSidecarCoupling::Independent)
        });
        promotable.then(|| self.pending.take()).flatten()
    }

    pub(crate) fn claim_for(
        &mut self,
        output_generation: u64,
        crtc_id: u32,
        target: PresentationTarget,
        primary_transaction_id: Option<OutputTransactionId>,
    ) -> Option<CursorSidecar> {
        let eligible = self.pending.as_ref().is_some_and(|sidecar| {
            sidecar.transaction.output_generation() == output_generation
                && sidecar.crtc_id == crtc_id
                && sidecar.deadline == target
                && match sidecar.coupling {
                    CursorSidecarCoupling::Independent => true,
                    CursorSidecarCoupling::MustBundleWith(required) => {
                        primary_transaction_id == Some(required)
                    }
                }
        });
        eligible.then(|| self.pending.take()).flatten()
    }

    pub(crate) fn len(&self) -> usize {
        usize::from(self.pending.is_some())
    }
}
