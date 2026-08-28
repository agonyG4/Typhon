use super::*;
use crate::wm::WorkspaceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XwmDrain {
    pub processed: usize,
    pub budget_exhausted: bool,
    pub events_processed: usize,
    pub property_replies_processed: usize,
    pub events_quiescent: bool,
    pub property_replies_quiescent: bool,
    pub quiescent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11AdmissionCancellationReason {
    Unmap,
    Destroy,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum XwmEvent {
    WindowMapRequested(X11WindowHandle),
    WindowReady(X11WindowSnapshot),
    WindowAdmissionCancelled {
        window: X11WindowHandle,
        reason: X11AdmissionCancellationReason,
    },
    WindowWithdrawn(X11WindowHandle),
    WindowDestroyed(X11WindowHandle),
    MetadataChanged {
        window: X11WindowHandle,
        delta: X11MetadataDelta,
    },
    ConfigureRequested {
        window: X11WindowHandle,
        request: X11ConfigureRequest,
    },
    MoveResizeRequested {
        window: X11WindowHandle,
        request: X11MoveResizeRequest,
    },
    ConfigureNotify {
        window: X11WindowHandle,
        geometry: X11Geometry,
        above_sibling: Option<X11WindowHandle>,
    },
    /// Current X root-tree order for live override-redirect windows.
    ///
    /// QueryTree reports children from bottom to top.  This event preserves
    /// that order without asking the compositor to echo it back to X.
    OverrideRedirectStackSnapshot {
        generation: XwaylandGeneration,
        epoch: u64,
        bottom_to_top: Vec<X11WindowHandle>,
    },
    StateRequested {
        window: X11WindowHandle,
        request: X11StateRequest,
    },
    FocusRequested {
        window: X11WindowHandle,
        source: u32,
        timestamp: u32,
        current_time: u32,
        user_time: Option<u32>,
    },
    CurrentDesktopRequested(WorkspaceId),
    WindowWorkspaceRequested {
        window: X11WindowHandle,
        workspace: WorkspaceId,
    },
    CloseRequestedByClient(X11WindowHandle),
    ResizeSyncAckObserved {
        window: X11WindowHandle,
        counter_value: u64,
    },
    ResizeSyncPresented {
        window: X11WindowHandle,
        transaction_id: u64,
        geometry: X11Geometry,
    },
    /// A transaction presented while another desired geometry still belongs to
    /// the same interactive resize chain, or while the transaction is not the
    /// final release configure.  The compositor must keep its preview active
    /// and only advance the XSync state machine.
    ResizeSyncPresentedIntermediate {
        window: X11WindowHandle,
        transaction_id: u64,
        geometry: X11Geometry,
    },
    ResizeSyncImmediate {
        window: X11WindowHandle,
        geometry: X11Geometry,
    },
    ResizeSyncTimedOut(X11WindowHandle),
    ResizeSyncTimedOutWithFollowup(X11WindowHandle),
}
