use std::sync::Arc;

use crate::native_output::{
    CursorPlaneAssignment, OutputTransaction, OutputTransactionId,
    presentation::plane::{CursorRevision, CursorSidecarId},
    presentation::plane_policy::CursorCapabilityKey,
    runtime::AtomicCommitKind,
};
use oblivion_one::native::kms::PageFlipToken;

#[derive(Debug, Clone)]
pub(crate) struct KmsPrimaryOwner {
    pub(crate) transaction: Arc<OutputTransaction>,
}

#[derive(Debug, Clone)]
pub(crate) struct KmsCursorOwner {
    pub(crate) transaction: Arc<OutputTransaction>,
    pub(crate) sidecar_id: Option<CursorSidecarId>,
    pub(crate) revision: CursorRevision,
    pub(crate) capability_key: Option<CursorCapabilityKey>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct KmsBundleOwners {
    primary: Option<KmsPrimaryOwner>,
    cursor: Option<KmsCursorOwner>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsBundleOwnerError {
    Empty,
    GenerationMismatch,
    TargetMismatch,
    CursorRevisionMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KmsCommitBundleIdentity {
    pub(crate) id: crate::native_output::presentation::plane::KmsCommitBundleId,
    pub(crate) token: PageFlipToken,
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
            .is_some_and(|owner| owner.transaction.output_generation() != first.output_generation())
        {
            return Err(KmsBundleOwnerError::GenerationMismatch);
        }
        if owners
            .cursor
            .as_ref()
            .is_some_and(|owner| owner.transaction.target() != first.target())
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
