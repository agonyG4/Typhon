use std::sync::Arc;

use crate::native_output::{
    CursorPlaneAssignment, OutputTransaction, OutputTransactionId,
    presentation::plane_policy::CursorCapabilityKey,
    presentation::{
        cursor_trace::CursorRevealTraceSnapshot,
        plane::{CursorRevision, CursorSidecarId},
    },
    runtime::AtomicCommitKind,
};
use oblivion_one::native::kms::PageFlipToken;

#[derive(Debug, Clone)]
pub(crate) struct KmsPrimaryOwner {
    pub(crate) transaction: Arc<OutputTransaction>,
    pub(crate) trace_reveal: Option<CursorRevealTraceSnapshot>,
}

#[derive(Debug, Clone)]
pub(crate) struct KmsCursorOwner {
    pub(crate) transaction: Arc<OutputTransaction>,
    pub(crate) sidecar_id: Option<CursorSidecarId>,
    pub(crate) revision: CursorRevision,
    pub(crate) capability_key: Option<CursorCapabilityKey>,
    pub(crate) trace_reveal: Option<CursorRevealTraceSnapshot>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct KmsBundleOwners {
    primary: Option<KmsPrimaryOwner>,
    cursor: Option<KmsCursorOwner>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsBundleOwnerError {
    Empty,
    OutputMismatch,
    GenerationMismatch,
    TargetMismatch,
    CursorRevisionMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KmsCommitBundleIdentity {
    pub(crate) id: crate::native_output::presentation::plane::KmsCommitBundleId,
    pub(crate) token: PageFlipToken,
    pub(crate) output_id: oblivion_one::core::OutputId,
    pub(crate) output_generation: u64,
    pub(crate) crtc_id: u32,
    pub(crate) primary_transaction_id: Option<OutputTransactionId>,
    pub(crate) cursor_transaction_id: Option<OutputTransactionId>,
}

impl KmsBundleOwners {
    pub(crate) fn new(
        primary: Option<KmsPrimaryOwner>,
        cursor: Option<KmsCursorOwner>,
    ) -> Result<Self, KmsBundleOwnerError> {
        let owners = Self { primary, cursor };
        let Some(first) = owners
            .primary
            .as_ref()
            .map(|owner| owner.transaction.as_ref())
            .or_else(|| {
                owners
                    .cursor
                    .as_ref()
                    .map(|owner| owner.transaction.as_ref())
            })
        else {
            return Err(KmsBundleOwnerError::Empty);
        };
        if owners
            .cursor
            .as_ref()
            .is_some_and(|owner| owner.transaction.output_id() != first.output_id())
        {
            return Err(KmsBundleOwnerError::OutputMismatch);
        }
        if owners
            .cursor
            .as_ref()
            .is_some_and(|owner| owner.transaction.output_generation() != first.output_generation())
        {
            return Err(KmsBundleOwnerError::GenerationMismatch);
        }
        if owners
            .cursor
            .as_ref()
            .is_some_and(|owner| owner.transaction.bound_target() != first.bound_target())
        {
            return Err(KmsBundleOwnerError::TargetMismatch);
        }
        Ok(owners)
    }

    pub(crate) const fn legacy_unchecked() -> Self {
        Self {
            primary: None,
            cursor: None,
        }
    }

    pub(crate) fn for_transaction(
        kind: AtomicCommitKind,
        transaction: Arc<OutputTransaction>,
        cursor_revision: Option<CursorRevision>,
        capability_key: Option<CursorCapabilityKey>,
    ) -> Result<Self, KmsBundleOwnerError> {
        let primary =
            (!matches!(kind, AtomicCommitKind::PlaneDelta { .. })).then(|| KmsPrimaryOwner {
                transaction: Arc::clone(&transaction),
                trace_reveal: None,
            });
        let cursor = match transaction.planes().cursor() {
            CursorPlaneAssignment::Unchanged => {
                if cursor_revision.is_some() || capability_key.is_some() {
                    return Err(KmsBundleOwnerError::CursorRevisionMismatch);
                }
                None
            }
            CursorPlaneAssignment::Atomic { .. } | CursorPlaneAssignment::Disabled => {
                Some(KmsCursorOwner {
                    revision: cursor_revision.ok_or(KmsBundleOwnerError::CursorRevisionMismatch)?,
                    transaction,
                    sidecar_id: None,
                    capability_key,
                    trace_reveal: None,
                })
            }
        };
        Self::new(primary, cursor)
    }

    pub(crate) fn primary(&self) -> Option<&KmsPrimaryOwner> {
        self.primary.as_ref()
    }

    pub(crate) fn cursor(&self) -> Option<&KmsCursorOwner> {
        self.cursor.as_ref()
    }

    pub(crate) fn set_cursor_trace_reveal(&mut self, snapshot: Option<CursorRevealTraceSnapshot>) {
        if let Some(cursor) = self.cursor.as_mut() {
            cursor.trace_reveal = snapshot;
        } else if let Some(primary) = self.primary.as_mut() {
            primary.trace_reveal = snapshot;
        }
    }

    pub(crate) fn trace_reveal(&self) -> Option<CursorRevealTraceSnapshot> {
        self.cursor
            .as_ref()
            .and_then(|owner| owner.trace_reveal)
            .or_else(|| self.primary.as_ref().and_then(|owner| owner.trace_reveal))
    }

    pub(crate) fn replace_cursor(&mut self, cursor: KmsCursorOwner) -> Option<KmsCursorOwner> {
        self.cursor.replace(cursor)
    }

    pub(crate) fn primary_transaction_id(&self) -> Option<OutputTransactionId> {
        self.primary.as_ref().map(|owner| owner.transaction.id())
    }

    pub(crate) fn cursor_transaction_id(&self) -> Option<OutputTransactionId> {
        self.cursor.as_ref().map(|owner| owner.transaction.id())
    }

    pub(crate) const fn is_legacy_unchecked(&self) -> bool {
        self.primary.is_none() && self.cursor.is_none()
    }
}
