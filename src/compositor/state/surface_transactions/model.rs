use super::*;
use crate::compositor::state_data::SurfaceContentMapping;
use crate::compositor::subsurface::ContentUpdateRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SurfaceTreeTransactionId(u64);

impl SurfaceTreeTransactionId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug)]
pub(in crate::compositor) struct SurfaceTreeAcquireDependency {
    pub(in crate::compositor) surface_commit_id: SurfaceCommitId,
    pub(in crate::compositor) commit_id: AcquireCommitId,
    pub(in crate::compositor) surface_id: u32,
    // `None` is only used by synthetic legacy tests; production admissions
    // always capture both values and therefore fail closed if either is absent.
    pub(in crate::compositor) owner_client_id: Option<ClientId>,
    pub(in crate::compositor) surface_presentation_generation: Option<u64>,
    pub(in crate::compositor) buffer_id: u32,
    pub(in crate::compositor) acquire: ExplicitSyncPoint,
    pub(in crate::compositor) state: PendingAcquireState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::compositor) struct SurfaceTreeNodeLifetime {
    pub(in crate::compositor) surface_id: u32,
    pub(in crate::compositor) owner_client_id: ClientId,
    pub(in crate::compositor) surface_presentation_generation: u64,
}

#[derive(Debug, Clone)]
pub(in crate::compositor) enum SurfaceTreeNodeLifetimes {
    Captured(Vec<SurfaceTreeNodeLifetime>),
    #[cfg(test)]
    Synthetic,
}

impl SurfaceTreeNodeLifetimes {
    pub(in crate::compositor) fn captured(&self) -> Option<&[SurfaceTreeNodeLifetime]> {
        match self {
            Self::Captured(lifetimes) => Some(lifetimes),
            #[cfg(test)]
            Self::Synthetic => None,
        }
    }
}

#[derive(Debug)]
pub(in crate::compositor) struct PendingSurfaceTreeTransaction {
    pub(in crate::compositor) id: SurfaceTreeTransactionId,
    pub(in crate::compositor) root_surface_id: u32,
    pub(in crate::compositor) nodes: Vec<(u32, CachedSubsurfaceCommit)>,
    pub(in crate::compositor) publication_lifetimes: SurfaceTreeNodeLifetimes,
    pub(in crate::compositor) dependencies: Vec<SurfaceTreeAcquireDependency>,
    pub(in crate::compositor) external_content_update_dependencies: Vec<ContentUpdateRef>,
    pub(in crate::compositor) commit_timing_readiness: Option<CommitTimingReadiness>,
    pub(in crate::compositor) received_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum TransactionOrdering {
    Coalescible,
    PacingProtected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum SurfaceTreeSubmissionKind {
    /// A new client Content Update subject to normal admission bounds.
    ClientAdmission,
    /// Already-admitted work materialized after a synchronization transition.
    InternalMigration,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct SurfacePublicationState {
    pub(in crate::compositor) latest_received: SurfaceCommitSequence,
    pub(in crate::compositor) latest_attachment_received: Option<SurfaceCommitSequence>,
    pub(in crate::compositor) latest_published: Option<SurfaceCommitSequence>,
    pub(in crate::compositor) latest_published_buffer_id: Option<BufferId>,
    /// The latest Content Update explicitly settled without publication.
    ///
    /// This is a surface-local high-water mark for teardown paths which
    /// retire all unpublished work for the surface. It is not publication
    /// state and must not be used as a published sequence.
    pub(in crate::compositor) latest_terminal: Option<SurfaceCommitSequence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct ActiveSurfacePresentationCommit {
    pub(in crate::compositor) surface_generation: u64,
    pub(in crate::compositor) commit_sequence: SurfaceCommitSequence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum SurfacePublicationDecision {
    Publish,
    StaleAlreadyPublished,
    SupersededByNewerAttachment,
    SurfaceGone,
    OwnerGone,
    TerminalClient,
    StaleSurfaceGeneration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum SurfacePublicationContext {
    ImmediateLatestAttachment,
    OrderedExplicitSyncQueue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(in crate::compositor) enum SurfacePublicationSource {
    Immediate,
    ExplicitSync,
    SurfaceTree,
    RemoveContent,
}

impl SurfacePublicationSource {
    pub(in crate::compositor) const fn as_str(self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::ExplicitSync => "explicit_sync",
            Self::SurfaceTree => "surface_tree",
            Self::RemoveContent => "remove_content",
        }
    }

    pub(in crate::compositor) const fn publication_context(self) -> SurfacePublicationContext {
        match self {
            Self::ExplicitSync | Self::SurfaceTree => {
                SurfacePublicationContext::OrderedExplicitSyncQueue
            }
            Self::Immediate | Self::RemoveContent => {
                SurfacePublicationContext::ImmediateLatestAttachment
            }
        }
    }
}

#[derive(Debug)]
pub(in crate::compositor) struct ReleasedSurfaceTreeState {
    pub(in crate::compositor) callbacks: Vec<wl_callback::WlCallback>,
    pub(in crate::compositor) resize_commit: Option<ResizeCommitSnapshot>,
}

#[derive(Debug, Default)]
pub(in crate::compositor) struct SurfaceTreeMergeStats {
    pub(in crate::compositor) incoming_nodes: usize,
    pub(in crate::compositor) existing_nodes: usize,
    pub(in crate::compositor) bufferless_nodes: usize,
    pub(in crate::compositor) attachments_replaced: usize,
    pub(in crate::compositor) explicit_detaches: usize,
    pub(in crate::compositor) dependencies_preserved: usize,
    pub(in crate::compositor) dependencies_replaced: usize,
    pub(in crate::compositor) callbacks_merged: usize,
    pub(in crate::compositor) feedbacks_merged: usize,
    pub(in crate::compositor) resize_snapshots_preserved: usize,
    pub(in crate::compositor) resize_snapshots_replaced: usize,
}

pub(in crate::compositor) struct BufferlessSurfaceCommitState {
    pub(in crate::compositor) commit_sequence: SurfaceCommitSequence,
    pub(in crate::compositor) damage: Option<RenderableSurfaceDamage>,
    pub(in crate::compositor) mapping: Option<SurfaceContentMapping>,
    pub(in crate::compositor) resize_commit: Option<ResizeCommitSnapshot>,
    pub(in crate::compositor) resize_capture_finalized: bool,
    pub(in crate::compositor) window_geometry: Option<XdgWindowGeometry>,
}

impl PendingSurfaceTreeTransaction {
    pub(in crate::compositor) fn commit_timing_request(&self) -> Option<CommitTimingConstraint> {
        self.nodes
            .iter()
            .filter_map(|(_, commit)| commit.pacing.commit_timing)
            .max_by(|left, right| left.ordering_key().cmp(&right.ordering_key()))
    }

    pub(in crate::compositor) fn ordering(&self) -> TransactionOrdering {
        if self
            .nodes
            .iter()
            .any(|(_, commit)| commit.pacing.is_boundary() || commit.lineage.merge_frozen)
        {
            TransactionOrdering::PacingProtected
        } else {
            TransactionOrdering::Coalescible
        }
    }

    pub(in crate::compositor) fn is_pacing_protected(&self) -> bool {
        self.ordering() == TransactionOrdering::PacingProtected
    }
}

impl SurfacePublicationDecision {
    pub(in crate::compositor) const fn pipeline_rejection_reason(
        self,
    ) -> Option<SurfacePipelineRejectionReason> {
        match self {
            Self::Publish => None,
            Self::StaleAlreadyPublished => Some(SurfacePipelineRejectionReason::AlreadyPublished),
            Self::SupersededByNewerAttachment => {
                Some(SurfacePipelineRejectionReason::SupersededByNewerAttachment)
            }
            Self::SurfaceGone => Some(SurfacePipelineRejectionReason::SurfaceGone),
            Self::OwnerGone => Some(SurfacePipelineRejectionReason::OwnerGone),
            Self::TerminalClient => Some(SurfacePipelineRejectionReason::TerminalClient),
            Self::StaleSurfaceGeneration => {
                Some(SurfacePipelineRejectionReason::StaleSurfaceGeneration)
            }
        }
    }
}
