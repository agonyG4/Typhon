use std::sync::Arc;

use crate::native_output::{
    CursorPlaneAssignment, OutputTransaction, OutputTransactionId,
    presentation::plane::{CursorRevision, CursorSidecarId},
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

    pub(crate) fn for_legacy_transaction(
        kind: AtomicCommitKind,
        transaction: Arc<OutputTransaction>,
    ) -> Self {
        let primary =
            (!matches!(kind, AtomicCommitKind::CursorOnly { .. })).then(|| KmsPrimaryOwner {
                transaction: Arc::clone(&transaction),
            });
        let cursor = (!matches!(
            transaction.planes().cursor(),
            CursorPlaneAssignment::Unchanged
        ))
        .then(|| KmsCursorOwner {
            revision: match transaction.planes().cursor() {
                CursorPlaneAssignment::Atomic { desired_epoch, .. } => {
                    let epoch = std::num::NonZeroU64::new(*desired_epoch)
                        .expect("cursor transaction epoch is nonzero");
                    CursorRevision::from_legacy_epoch(epoch)
                }
                CursorPlaneAssignment::Unchanged | CursorPlaneAssignment::Disabled => {
                    CursorRevision::initial()
                }
            },
            transaction,
            sidecar_id: None,
        });
        Self { primary, cursor }
    }

    pub(crate) fn primary(&self) -> Option<&KmsPrimaryOwner> {
        self.primary.as_ref()
    }

    pub(crate) fn cursor(&self) -> Option<&KmsCursorOwner> {
        self.cursor.as_ref()
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
