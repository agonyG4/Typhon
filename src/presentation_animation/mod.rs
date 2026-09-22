//! SceneNode-owned presentation animation primitives.
//!
//! The implementation is split by responsibility as the generalized
//! presentation engine grows. The compatibility re-export keeps existing
//! compositor imports stable during the migration.

mod clip;
mod curve;
mod engine;
mod frame;
mod geometry;
mod ids;
mod opacity;
mod retained;
mod time;
mod transaction;

pub use clip::{PresentationClip, PresentationClipRect};
pub use curve::{AnimationCurve, EasingCurve, SpringSpec};
pub use engine::{
    PresentationAnimationMetrics, PresentationEngine, PresentationTransition,
    animation_policy_enabled,
};
pub use frame::{
    FramePresentationSample, NativeFramePresentationTargets, PresentationClipTransitionEvidence,
    PresentationFrameSnapshot, PresentationGroupClip, PresentationGroupOpacity,
    PresentationGroupTransform, PresentationOpacityTransitionEvidence,
    PresentationSampleTimeSource, PresentationSceneSample, PresentationWindowTarget,
    PresentedClipAck, PresentedGeometryAck, PresentedOpacityAck, PresentedWindowGeometry,
};
pub use geometry::{
    PresentationDamageRect, PresentationGeometryTransform, PresentationRect, PresentationVelocity,
    PresentationWindowSample, presentation_damage,
};
pub use ids::{
    PresentationPropertyKind, PresentationRevisionId, PresentationTransactionId, TransitionId,
};
pub use opacity::PresentationOpacity;
pub use retained::{
    PresentationRetainedVisualActivationError, PresentationRetainedVisualIdentity,
    PresentationRetainedVisualKind, PresentationTransactionMemberKind,
};
pub use time::AnimationTime;
pub use transaction::{
    PresentationClipMutation, PresentationGeometryMutation, PresentationOpacityMutation,
    PresentationTransactionError, PresentationTransactionMember, PresentationTransactionRecord,
    PresentationTransactionRequest,
};

/// Compatibility alias retained while compositor call sites converge on the
/// v2 engine name. It is not a second production authority.
pub type PresentationAnimator = PresentationEngine;

pub(crate) use transaction::{
    PreparedClipMutation, PreparedGeometryMutation, PreparedOpacityMutation,
};

#[cfg(test)]
mod math_tests;
#[cfg(test)]
mod transactions_tests;
