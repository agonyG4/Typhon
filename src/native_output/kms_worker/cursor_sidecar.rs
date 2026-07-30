#![allow(dead_code)]

use std::sync::Arc;

use crate::native_output::{
    CursorFramebufferPin, CursorPlaneAssignment, OutputTransaction, OutputTransactionId,
    presentation::plane::{CursorRevision, CursorSidecarId},
};
use oblivion_one::native::presentation_deadline::{MonotonicTimestampNs, PresentationTarget};

use super::KmsTestOnlyPolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorSidecarCoupling {
    Independent,
    MustBundleWith(OutputTransactionId),
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
}

#[derive(Debug, Default)]
pub(crate) struct CursorSidecarMailbox {
    pending: Option<CursorSidecar>,
}

impl CursorSidecarMailbox {
    pub(crate) fn offer(&mut self, mut sidecar: CursorSidecar) -> Option<CursorSidecar> {
        if let Some(existing) = self.pending.as_ref()
            && let CursorSidecarCoupling::MustBundleWith(required) = existing.coupling
            && matches!(sidecar.coupling, CursorSidecarCoupling::Independent)
        {
            sidecar.coupling = CursorSidecarCoupling::MustBundleWith(required);
        }
        self.pending.replace(sidecar)
    }

    pub(crate) fn pending(&self) -> Option<&CursorSidecar> {
        self.pending.as_ref()
    }

    pub(crate) fn take(&mut self) -> Option<CursorSidecar> {
        self.pending.take()
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
