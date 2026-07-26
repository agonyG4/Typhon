//! Owned, immutable values that may cross into the Atomic submit thread.

use super::super::runtime::AtomicCommitKind;
use crate::native_output::{
    CursorPlaneAssignment, OutputTransaction, OutputTransactionContent, OutputTransactionId,
    PrimaryPlaneAssignment,
};
use oblivion_one::native::{
    kms::{AtomicCursorVisualState, FramebufferId, PageFlipToken},
    presentation_deadline::{MonotonicTimestampNs, PresentationTarget},
};
use std::os::fd::OwnedFd;

#[derive(Debug)]
pub(crate) struct KmsCommitJob {
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) token: PageFlipToken,
    pub(crate) output_generation: u64,
    pub(crate) crtc_id: u32,
    pub(crate) kind: AtomicCommitKind,
    pub(crate) target: PresentationTarget,
    pub(crate) queued_at: MonotonicTimestampNs,
    pub(crate) primary: KmsPrimaryUpdate,
    pub(crate) cursor: KmsCursorUpdate,
    pub(crate) test_only: KmsTestOnlyPolicy,
    pub(crate) ready_submit: bool,
}

#[derive(Debug)]
pub(crate) enum KmsPrimaryUpdate {
    Unchanged,
    Framebuffer {
        framebuffer: FramebufferId,
        in_fence: Option<OwnedFd>,
        request_out_fence: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KmsCursorUpdate {
    Unchanged,
    Disable,
    Set(AtomicCursorVisualState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsTestOnlyPolicy {
    Skip,
    Required,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsCommitPayloadError {
    TransactionIdentityMismatch,
    GenerationMismatch,
    TargetMismatch,
    PrimaryAssignmentMismatch,
    CursorAssignmentMismatch,
    MissingExplicitInputFence,
    CursorOnlyChangesPrimary,
    CursorOnlyMissingCursorUpdate,
    UnexpectedCompatibilityImmediate,
}

impl KmsCommitJob {
    pub(crate) fn validate_against(
        &self,
        transaction: &OutputTransaction,
    ) -> Result<(), KmsCommitPayloadError> {
        if self.transaction_id != transaction.id()
            || kind_transaction_id(self.kind) != self.transaction_id
        {
            return Err(KmsCommitPayloadError::TransactionIdentityMismatch);
        }
        if self.output_generation != transaction.output_generation() {
            return Err(KmsCommitPayloadError::GenerationMismatch);
        }
        if self.target != transaction.target() {
            return Err(KmsCommitPayloadError::TargetMismatch);
        }

        match (self.kind, transaction.content(), &self.primary) {
            (
                AtomicCommitKind::CompositedPrimary { framebuffer_id, .. },
                OutputTransactionContent::Composited { .. },
                KmsPrimaryUpdate::Framebuffer {
                    framebuffer,
                    in_fence,
                    ..
                },
            ) if matches!(
                transaction.planes().primary(),
                PrimaryPlaneAssignment::CompositorFramebuffer {
                    framebuffer_id: expected,
                    ..
                } if expected == framebuffer_id && framebuffer.get() == framebuffer_id
            ) && in_fence.is_some() => {}
            (
                AtomicCommitKind::CompositedPrimary { framebuffer_id, .. },
                OutputTransactionContent::Composited { .. },
                KmsPrimaryUpdate::Framebuffer {
                    framebuffer,
                    in_fence,
                    ..
                },
            ) if matches!(
                transaction.planes().primary(),
                PrimaryPlaneAssignment::CompatibilityFramebuffer {
                    framebuffer_id: expected,
                } if expected == framebuffer_id && framebuffer.get() == framebuffer_id
            ) && in_fence.is_none() => {}
            (
                AtomicCommitKind::DirectPrimary { framebuffer_id, .. },
                OutputTransactionContent::Direct { .. },
                KmsPrimaryUpdate::Framebuffer { framebuffer, .. },
            ) if matches!(
                transaction.planes().primary(),
                PrimaryPlaneAssignment::ClientFramebuffer {
                    framebuffer_id: expected,
                    ..
                } if expected == framebuffer_id && framebuffer.get() == framebuffer_id
            ) => {}
            (
                AtomicCommitKind::CursorOnly { .. },
                OutputTransactionContent::CursorOnly { .. },
                KmsPrimaryUpdate::Unchanged,
            ) => {}
            (
                AtomicCommitKind::CompositedPrimary { .. } | AtomicCommitKind::DirectPrimary { .. },
                OutputTransactionContent::CompatibilityImmediate { .. },
                _,
            ) => return Err(KmsCommitPayloadError::UnexpectedCompatibilityImmediate),
            (AtomicCommitKind::CursorOnly { .. }, _, KmsPrimaryUpdate::Framebuffer { .. }) => {
                return Err(KmsCommitPayloadError::CursorOnlyChangesPrimary);
            }
            _ => return Err(KmsCommitPayloadError::PrimaryAssignmentMismatch),
        }

        if matches!(self.kind, AtomicCommitKind::CursorOnly { .. })
            && matches!(self.cursor, KmsCursorUpdate::Unchanged)
        {
            return Err(KmsCommitPayloadError::CursorOnlyMissingCursorUpdate);
        }
        let cursor_matches = match (&self.cursor, transaction.planes().cursor()) {
            (KmsCursorUpdate::Unchanged, CursorPlaneAssignment::Unchanged) => true,
            (
                KmsCursorUpdate::Disable,
                CursorPlaneAssignment::Atomic {
                    framebuffer_id,
                    visible,
                    ..
                },
            ) => framebuffer_id.is_none() && !visible,
            (
                KmsCursorUpdate::Set(state),
                CursorPlaneAssignment::Atomic {
                    framebuffer_id,
                    visible,
                    ..
                },
            ) => state.framebuffer_id == framebuffer_id && state.visible == visible,
            _ => false,
        };
        if !cursor_matches {
            return Err(KmsCommitPayloadError::CursorAssignmentMismatch);
        }
        if let AtomicCommitKind::CursorOnly { cursor_epoch, .. } = self.kind {
            if !matches!(
                transaction.planes().primary(),
                PrimaryPlaneAssignment::Unchanged
            ) {
                return Err(KmsCommitPayloadError::CursorOnlyChangesPrimary);
            }
            if !matches!(
                transaction.planes().cursor(),
                CursorPlaneAssignment::Atomic {
                    desired_epoch,
                    ..
                } if desired_epoch == cursor_epoch
            ) {
                return Err(KmsCommitPayloadError::CursorAssignmentMismatch);
            }
        }
        Ok(())
    }
}

fn kind_transaction_id(kind: AtomicCommitKind) -> OutputTransactionId {
    match kind {
        AtomicCommitKind::CompositedPrimary { transaction_id, .. }
        | AtomicCommitKind::DirectPrimary { transaction_id, .. }
        | AtomicCommitKind::CursorOnly { transaction_id, .. } => transaction_id,
    }
}

fn _assert_send<T: Send>() {}
fn _assert_sync<T: Sync>() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_worker_payload_types_are_send() {
        _assert_send::<KmsCommitJob>();
        _assert_send::<KmsPrimaryUpdate>();
        _assert_send::<KmsCursorUpdate>();
        _assert_send::<KmsTestOnlyPolicy>();
    }

    #[test]
    fn worker_payload_types_with_shared_metadata_are_sync() {
        _assert_sync::<KmsTestOnlyPolicy>();
        _assert_sync::<KmsCursorUpdate>();
    }
}
