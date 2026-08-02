//! Owned, immutable values that may cross into the Atomic submit thread.

use super::super::runtime::AtomicCommitKind;
use super::{KmsBundleOwners, KmsCommitBundleIdentity};
use crate::native_output::output::CursorFramebufferPin;
use crate::native_output::presentation::plane::{
    KmsCommitBundleId, PresentedCursorDelivery, PresentedCursorState, PresentedPlaneSnapshot,
};
use crate::native_output::scanout::DirectPrimaryLease;
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
    pub(crate) bundle_id: KmsCommitBundleId,
    pub(crate) owners: KmsBundleOwners,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) token: PageFlipToken,
    pub(crate) output_generation: u64,
    pub(crate) crtc_id: u32,
    pub(crate) kind: AtomicCommitKind,
    pub(crate) target: PresentationTarget,
    pub(crate) validation_base: KmsValidationBase,
    pub(crate) queued_at: MonotonicTimestampNs,
    pub(crate) primary: KmsPrimaryUpdate,
    pub(crate) cursor: KmsCursorUpdate,
    pub(crate) cursor_delivery: PresentedCursorDelivery,
    pub(crate) primary_cursor_presentation: KmsPrimaryCursorPresentation,
    /// A Send-only lease marker for the exact DRM cursor framebuffer named by
    /// `cursor`. The framebuffer object remains compositor-thread-owned; the
    /// lease keeps it in the cursor resource registry until this job reaches a
    /// terminal point.
    pub(crate) cursor_pin: Option<CursorFramebufferPin>,
    pub(crate) direct_primary_lease: Option<DirectPrimaryLease>,
    pub(crate) test_only_duration_ns: Option<u64>,
    pub(crate) pacing_frame_id: Option<u64>,
    pub(crate) test_policy: KmsCommitTestPolicy,
    pub(crate) ready_submit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsPrimaryCursorPresentation {
    Preserve,
    Promote(PresentedCursorState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsValidationBase {
    Presented {
        snapshot: PresentedPlaneSnapshot,
        output_generation: u64,
        crtc_id: u32,
    },
    Predecessor(KmsCommitBundleIdentity),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EstablishedKmsBase {
    Presented {
        revision: crate::native_output::presentation::plane::PlaneStateRevision,
        output_generation: u64,
        crtc_id: u32,
    },
    Pending(KmsCommitBundleIdentity),
    Bundle(KmsCommitBundleIdentity),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValidationBaseDisposition {
    Ready,
    Wait,
    Invalidated,
}

pub(crate) fn validation_base_ready(
    established: EstablishedKmsBase,
    required: KmsValidationBase,
) -> ValidationBaseDisposition {
    match (established, required) {
        (
            EstablishedKmsBase::Presented {
                revision,
                output_generation,
                crtc_id,
            },
            KmsValidationBase::Presented {
                snapshot,
                output_generation: required_generation,
                crtc_id: required_crtc,
            },
        ) if revision == snapshot.revision
            && output_generation == required_generation
            && crtc_id == required_crtc =>
        {
            ValidationBaseDisposition::Ready
        }
        (EstablishedKmsBase::Pending(pending), KmsValidationBase::Predecessor(required))
            if pending == required =>
        {
            ValidationBaseDisposition::Wait
        }
        (EstablishedKmsBase::Bundle(established), KmsValidationBase::Predecessor(required))
            if established == required =>
        {
            ValidationBaseDisposition::Ready
        }
        _ => ValidationBaseDisposition::Invalidated,
    }
}

#[derive(Debug)]
pub(crate) struct KmsSubmittedOwnership {
    pub(crate) job: KmsCommitJob,
    pub(crate) out_fence: Option<OwnedFd>,
    pub(crate) submit_started_at: MonotonicTimestampNs,
    pub(crate) submit_returned_at: MonotonicTimestampNs,
    pub(crate) queue_residency_ns: u64,
    pub(crate) submit_wake_lateness_ns: u64,
    pub(crate) submission_budget_ns: u64,
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
pub(crate) struct KmsCommitTestPolicy {
    pub(crate) primary: KmsTestOnlyPolicy,
    pub(crate) cursor: KmsTestOnlyPolicy,
}

impl KmsCommitTestPolicy {
    pub(crate) const fn from_primary(primary: KmsTestOnlyPolicy) -> Self {
        Self {
            primary,
            cursor: KmsTestOnlyPolicy::Skip,
        }
    }

    pub(crate) const fn from_cursor(cursor: KmsTestOnlyPolicy) -> Self {
        Self {
            primary: KmsTestOnlyPolicy::Skip,
            cursor,
        }
    }

    pub(crate) const fn effective(self) -> KmsTestOnlyPolicy {
        if matches!(self.primary, KmsTestOnlyPolicy::Required)
            || matches!(self.cursor, KmsTestOnlyPolicy::Required)
        {
            KmsTestOnlyPolicy::Required
        } else {
            KmsTestOnlyPolicy::Skip
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsCommitPayloadError {
    TransactionIdentityMismatch,
    GenerationMismatch,
    TargetMismatch,
    PrimaryAssignmentMismatch,
    CursorAssignmentMismatch,
    MissingExplicitInputFence,
    PlaneDeltaChangesPrimary,
    PlaneDeltaMissingCursorUpdate,
    UnexpectedCompatibilityImmediate,
    CursorResourceMismatch,
    DirectPrimaryResourceMismatch,
    MissingPrimaryOwner,
    MissingCursorOwner,
    CursorDeliveryMismatch,
    OwnerIdentityMismatch,
    OwnerGenerationMismatch,
    OwnerTargetMismatch,
    ValidationBaseMismatch,
}

impl KmsCommitJob {
    pub(crate) fn identity(&self) -> KmsCommitBundleIdentity {
        KmsCommitBundleIdentity {
            id: self.bundle_id,
            token: self.token,
            output_generation: self.output_generation,
            crtc_id: self.crtc_id,
            primary_transaction_id: self.owners.primary_transaction_id(),
            cursor_transaction_id: self.owners.cursor_transaction_id(),
        }
    }

    pub(crate) fn validate_against(
        &self,
        transaction: &OutputTransaction,
    ) -> Result<(), KmsCommitPayloadError> {
        self.validate_against_mode(transaction, false)
    }

    pub(crate) fn validate_submitted_against(
        &self,
        transaction: &OutputTransaction,
    ) -> Result<(), KmsCommitPayloadError> {
        self.validate_against_mode(transaction, true)
    }

    fn validate_against_mode(
        &self,
        transaction: &OutputTransaction,
        submitted: bool,
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
        match self.validation_base {
            KmsValidationBase::Presented {
                output_generation,
                crtc_id,
                ..
            }
            | KmsValidationBase::Predecessor(KmsCommitBundleIdentity {
                output_generation,
                crtc_id,
                ..
            }) if output_generation != self.output_generation || crtc_id != self.crtc_id => {
                return Err(KmsCommitPayloadError::ValidationBaseMismatch);
            }
            _ => {}
        }
        if !self.owners.is_legacy_unchecked() {
            let primary_changed = !matches!(self.primary, KmsPrimaryUpdate::Unchanged);
            let cursor_changed = !matches!(self.cursor, KmsCursorUpdate::Unchanged);
            if primary_changed && self.owners.primary().is_none() {
                return Err(KmsCommitPayloadError::MissingPrimaryOwner);
            }
            if cursor_changed && self.owners.cursor().is_none() {
                return Err(KmsCommitPayloadError::MissingCursorOwner);
            }
            for owner in [
                self.owners
                    .primary()
                    .map(|owner| owner.transaction.as_ref()),
                self.owners.cursor().map(|owner| owner.transaction.as_ref()),
            ]
            .into_iter()
            .flatten()
            {
                if owner.output_generation() != self.output_generation {
                    return Err(KmsCommitPayloadError::OwnerGenerationMismatch);
                }
                if owner.target() != self.target {
                    return Err(KmsCommitPayloadError::OwnerTargetMismatch);
                }
            }
            let expected_legacy_owner = if matches!(self.kind, AtomicCommitKind::PlaneDelta { .. })
            {
                self.owners.cursor_transaction_id()
            } else {
                self.owners.primary_transaction_id()
            };
            if expected_legacy_owner != Some(self.transaction_id) {
                return Err(KmsCommitPayloadError::OwnerIdentityMismatch);
            }
        }

        match (self.kind, self.direct_primary_lease.as_ref()) {
            (AtomicCommitKind::DirectPrimary { framebuffer_id, .. }, Some(lease)) => {
                let OutputTransactionContent::Direct {
                    key: expected_key, ..
                } = transaction.content()
                else {
                    return Err(KmsCommitPayloadError::DirectPrimaryResourceMismatch);
                };
                let Some(expected_surface_id) = transaction.obligations().direct_surface_id()
                else {
                    return Err(KmsCommitPayloadError::DirectPrimaryResourceMismatch);
                };
                if !lease.validate_against(expected_key, expected_surface_id, framebuffer_id)
                    || !matches!(
                        transaction.planes().primary(),
                        PrimaryPlaneAssignment::ClientFramebuffer {
                            key: primary_key,
                            framebuffer_id: expected_framebuffer,
                        } if primary_key == expected_key && expected_framebuffer == framebuffer_id
                    )
                {
                    return Err(KmsCommitPayloadError::DirectPrimaryResourceMismatch);
                }
            }
            (AtomicCommitKind::DirectPrimary { .. }, _) => {
                return Err(KmsCommitPayloadError::DirectPrimaryResourceMismatch);
            }
            (_, None) => {}
            (_, Some(_)) => {
                return Err(KmsCommitPayloadError::DirectPrimaryResourceMismatch);
            }
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
            ) && (in_fence.is_some() || submitted) => {}
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
                AtomicCommitKind::PlaneDelta { .. },
                OutputTransactionContent::PlaneDelta { .. },
                KmsPrimaryUpdate::Unchanged,
            ) => {}
            (
                AtomicCommitKind::CompositedPrimary { .. } | AtomicCommitKind::DirectPrimary { .. },
                OutputTransactionContent::CompatibilityImmediate { .. },
                _,
            ) => return Err(KmsCommitPayloadError::UnexpectedCompatibilityImmediate),
            (AtomicCommitKind::PlaneDelta { .. }, _, KmsPrimaryUpdate::Framebuffer { .. }) => {
                return Err(KmsCommitPayloadError::PlaneDeltaChangesPrimary);
            }
            _ => return Err(KmsCommitPayloadError::PrimaryAssignmentMismatch),
        }

        if matches!(self.kind, AtomicCommitKind::PlaneDelta { .. })
            && matches!(self.cursor, KmsCursorUpdate::Unchanged)
        {
            return Err(KmsCommitPayloadError::PlaneDeltaMissingCursorUpdate);
        }
        let cursor_transaction = self
            .owners
            .cursor()
            .map_or(transaction, |owner| owner.transaction.as_ref());
        let cursor_matches = match (&self.cursor, cursor_transaction.planes().cursor()) {
            (KmsCursorUpdate::Unchanged, CursorPlaneAssignment::Unchanged) => true,
            (KmsCursorUpdate::Disable, CursorPlaneAssignment::Atomic { state: None, .. }) => true,
            (
                KmsCursorUpdate::Set(state),
                CursorPlaneAssignment::Atomic {
                    state: Some(planned),
                    ..
                },
            ) => state == planned,
            _ => false,
        };
        if !cursor_matches {
            return Err(KmsCommitPayloadError::CursorAssignmentMismatch);
        }
        match (&self.kind, &self.cursor, self.cursor_delivery) {
            (_, KmsCursorUpdate::Set(state), delivery)
                if state.visible && delivery != PresentedCursorDelivery::Hardware =>
            {
                return Err(KmsCommitPayloadError::CursorDeliveryMismatch);
            }
            (
                AtomicCommitKind::PlaneDelta { .. },
                KmsCursorUpdate::Disable,
                PresentedCursorDelivery::Hardware,
            )
            | (AtomicCommitKind::PlaneDelta { .. }, _, PresentedCursorDelivery::Software)
            | (AtomicCommitKind::DirectPrimary { .. }, _, PresentedCursorDelivery::Software) => {
                return Err(KmsCommitPayloadError::CursorDeliveryMismatch);
            }
            _ => {}
        }
        match &self.cursor {
            KmsCursorUpdate::Set(state) if state.framebuffer_id.is_some() => {
                let Some(pin) = self.cursor_pin.as_ref() else {
                    return Err(KmsCommitPayloadError::CursorResourceMismatch);
                };
                if Some(pin.framebuffer_id().get()) != state.framebuffer_id {
                    return Err(KmsCommitPayloadError::CursorResourceMismatch);
                }
            }
            KmsCursorUpdate::Set(_) | KmsCursorUpdate::Disable | KmsCursorUpdate::Unchanged => {
                if self.cursor_pin.is_some() {
                    return Err(KmsCommitPayloadError::CursorResourceMismatch);
                }
            }
        }
        if let AtomicCommitKind::PlaneDelta { cursor_epoch, .. } = self.kind {
            if !matches!(
                transaction.planes().primary(),
                PrimaryPlaneAssignment::Unchanged
            ) {
                return Err(KmsCommitPayloadError::PlaneDeltaChangesPrimary);
            }
            if !matches!(
                transaction.planes().cursor(),
                CursorPlaneAssignment::Atomic {
                    desired_epoch,
                    ..
                } if *desired_epoch == cursor_epoch
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
        | AtomicCommitKind::PlaneDelta { transaction_id, .. } => transaction_id,
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
        _assert_send::<KmsSubmittedOwnership>();
        _assert_send::<KmsPrimaryUpdate>();
        _assert_send::<KmsCursorUpdate>();
        _assert_send::<CursorFramebufferPin>();
        _assert_send::<KmsTestOnlyPolicy>();
        _assert_send::<DirectPrimaryLease>();
        _assert_send::<KmsCommitJob>();
    }

    #[test]
    fn worker_payload_types_with_shared_metadata_are_sync() {
        _assert_sync::<KmsTestOnlyPolicy>();
        _assert_sync::<KmsCursorUpdate>();
    }
}
