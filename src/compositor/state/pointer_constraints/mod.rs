use super::*;
use crate::compositor::PointerWarpOrigin;
use crate::compositor::input::ResolvedPointerConstraintBackendRequest;
use crate::compositor::subsurface::{
    CapturedPointerConstraintCommit, CapturedPointerConstraintSurfaceState,
    PointerConstraintHintCommit,
};

const WL_POINTER_WARP_SINCE: u32 = 11;

mod backend;
mod lifecycle;
mod reveal;
mod routing;
mod runtime;

use backend::PointerConstraintRestorePolicy;

#[derive(Debug, Clone, Copy)]
pub(in crate::compositor) enum PointerConstraintDeactivationReason {
    WorkspaceDeparture,
    WindowMovedOffScene,
    WindowMinimized,
}

#[cfg(test)]
pub(in crate::compositor) use runtime::PointerConstraintRuntimeSnapshot;
pub(in crate::compositor) use runtime::{
    ActiveConfinedPointerRouting, ActiveLockedPointerRouting, PointerConstraintRegistration,
    PointerConstraintRuntimeState,
};
use runtime::{PendingLockedPointerReveal, PointerConstraint};
