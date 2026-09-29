//! Bounded XDND wire-adapter bookkeeping.
//!
//! The compositor supplies canonical session identities and owns drag
//! lifecycle. This manager keeps one generation-qualified adapter view and
//! tracks only the bounded wire progress needed by the live XDND adapter.

use std::collections::{BTreeMap, HashSet, VecDeque};

use x11rb::connection::SequenceNumber;

use super::super::{X11WindowHandle, XwaylandGeneration};
use crate::xwayland::{
    CanonicalDndSessionId, XwaylandDndAction, XwaylandDndAdapterId, XwaylandDndMimeCatalog,
    XwaylandDndVersion,
};

pub use crate::xwayland::XwaylandDndAction as XdndAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DndWireProgress {
    AwaitingEnter,
    Entered,
    Positioned,
    AwaitingStatus,
    DropPending,
    DropPendingAwaitingStatus,
    AwaitingFinished,
    TerminalConsumed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DndSession {
    pub id: XwaylandDndAdapterId,
    pub source: Option<X11WindowHandle>,
    /// Per-session Typhon-owned source proxy; never a DesktopWindow.
    pub source_proxy: Option<u32>,
    pub ownership_timestamp: Option<u32>,
    pub ownership_confirmed: bool,
    pub ownership_claim_issued: bool,
    pub discovery_target: Option<X11WindowHandle>,
    pub discovery_recipient: Option<u32>,
    pub target: Option<X11WindowHandle>,
    /// The actual target remains canonical; XDND messages may go to its
    /// separately validated proxy recipient.
    pub wire_recipient: Option<u32>,
    pub target_version: Option<XwaylandDndVersion>,
    pub progress: DndWireProgress,
    pub action: Option<XwaylandDndAction>,
    pub x: i32,
    pub y: i32,
    /// Exact Position currently awaiting the one corresponding Status.
    pub outstanding_position: Option<CoalescedPosition>,
    pub coalesced_position: Option<CoalescedPosition>,
    pub latest_position: Option<CoalescedPosition>,
    pub(super) last_status: Option<DndStatusResult>,
    pub pending_drop_action: Option<XwaylandDndAction>,
    pub authorized_drop_action: Option<XwaylandDndAction>,
    pub status_deadline_ns: Option<u64>,
    pub finished_deadline_ns: Option<u64>,
    pub source_proxy_destroyed: bool,
    pub mime_types: XwaylandDndMimeCatalog,
    pub source_actions: Vec<XwaylandDndAction>,
    pub mime_atoms: Vec<Option<u32>>,
    pub timestamp_deadline_ns: Option<u64>,
    pub ownership_deadline_ns: Option<u64>,
    pub discovery_deadline_ns: Option<u64>,
    pub(super) discovery_serial: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoalescedPosition {
    pub x: f64,
    pub y: f64,
    pub action: Option<XwaylandDndAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DndStatusResult {
    pub accepted: bool,
    pub action: Option<XwaylandDndAction>,
    pub requested_action: Option<XwaylandDndAction>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct StatusAcknowledgement {
    pub next_position: Option<CoalescedPosition>,
    pub pending_drop_action: Option<XwaylandDndAction>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PositionDisposition {
    SendNow(CoalescedPosition),
    Coalesced,
    Stale,
}

#[derive(Debug, Default)]
pub struct DndManager {
    pub(super) active: Option<DndSession>,
    pub(super) internal_windows: HashSet<u32>,
    pub(super) pending_replies: BTreeMap<SequenceNumber, DndPendingReply>,
    pub(super) feedback: VecDeque<DndFeedback>,
    pub(super) next_discovery_serial: u64,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum DndPendingReply {
    TargetProxy {
        id: XwaylandDndAdapterId,
        actual: X11WindowHandle,
        serial: u64,
        deadline_ns: u64,
    },
    ProxySelf {
        id: XwaylandDndAdapterId,
        actual: X11WindowHandle,
        proxy: u32,
        serial: u64,
        deadline_ns: u64,
    },
    Aware {
        id: XwaylandDndAdapterId,
        actual: X11WindowHandle,
        recipient: u32,
        serial: u64,
        deadline_ns: u64,
    },
    MimeAtom {
        id: XwaylandDndAdapterId,
        ordinal: usize,
        deadline_ns: u64,
    },
    Ownership {
        id: XwaylandDndAdapterId,
        source_proxy: u32,
        timestamp: u32,
        deadline_ns: u64,
    },
    MultipleRead {
        id: XwaylandDndAdapterId,
        source_proxy: u32,
        target: X11WindowHandle,
        requestor: u32,
        property: u32,
        request_time: u32,
        deadline_ns: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DndStatusFeedback {
    pub id: XwaylandDndAdapterId,
    pub target: X11WindowHandle,
    pub accepted: bool,
    pub action: Option<XwaylandDndAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DndTerminalFeedback {
    pub id: XwaylandDndAdapterId,
    pub target: X11WindowHandle,
    pub accepted: bool,
    pub action: Option<XwaylandDndAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DndFeedback {
    Status(DndStatusFeedback),
    Terminal(DndTerminalFeedback),
}

pub(super) const SOURCE_TIMESTAMP_TIMEOUT_NS: u64 = 2_000_000_000;
pub(super) const SOURCE_OWNERSHIP_TIMEOUT_NS: u64 = 2_000_000_000;
pub(super) const TARGET_DISCOVERY_TIMEOUT_NS: u64 = 2_000_000_000;
pub(super) const TARGET_STATUS_TIMEOUT_NS: u64 = 1_000_000_000;
/// Twice the 30-second idle transfer deadline, allowing a normal local direct
/// or paced INCR read to finish before a malfunctioning target is cancelled.
pub(super) const TARGET_FINISHED_TIMEOUT_NS: u64 = 60_000_000_000;
pub(super) const DND_REPLY_BUDGET: usize = 64;
pub(super) const MAX_PENDING_DND_REPLIES: usize = 128;
pub(super) const MAX_MULTIPLE_PAIRS: usize = 64;
const MAX_DND_FEEDBACK: usize = 2;

impl DndManager {
    pub(super) fn push_status_feedback(&mut self, feedback: DndStatusFeedback) {
        if !self.active.as_ref().is_some_and(|session| {
            session.id == feedback.id
                && (session.target == Some(feedback.target)
                    || session.discovery_target == Some(feedback.target))
        }) {
            return;
        }
        if self
            .feedback
            .iter()
            .any(|entry| matches!(entry, DndFeedback::Terminal(_)))
        {
            return;
        }
        if let Some(existing) = self
            .feedback
            .iter_mut()
            .find(|entry| matches!(entry, DndFeedback::Status(_)))
        {
            *existing = DndFeedback::Status(feedback);
            return;
        }
        debug_assert!(self.feedback.len() < MAX_DND_FEEDBACK);
        self.feedback.push_back(DndFeedback::Status(feedback));
    }

    /// Terminal feedback has a reserved position behind the latest Status.
    /// The one-slot DND manager refuses replacement after a terminal edge, so
    /// this bounded queue cannot silently discard terminal authority.
    pub(super) fn push_terminal_feedback(&mut self, feedback: DndTerminalFeedback) -> bool {
        if self.feedback.iter().any(
            |entry| matches!(entry, DndFeedback::Terminal(existing) if existing.id == feedback.id),
        ) {
            return true;
        }
        while self.feedback.len() >= MAX_DND_FEEDBACK {
            let Some(status_index) = self
                .feedback
                .iter()
                .position(|entry| matches!(entry, DndFeedback::Status(_)))
            else {
                return false;
            };
            // Only Status entries coalesce. Keep the newest semantic Status
            // before Terminal so an impossible duplicate-status overflow can
            // never discard terminal authority or reverse the terminal edge.
            self.feedback.remove(status_index);
        }
        self.feedback.push_back(DndFeedback::Terminal(feedback));
        true
    }

    /// Install the canonical session identity for one XWM adapter view.
    /// Replacing it drops the old exact identity from this one-slot manager.
    pub fn install_canonical_session(
        &mut self,
        id: XwaylandDndAdapterId,
        source: Option<X11WindowHandle>,
    ) -> bool {
        if self
            .feedback
            .iter()
            .any(|entry| matches!(entry, DndFeedback::Terminal(_)))
            || self.active.as_ref().is_some_and(|session| {
                session.id == id
                    || matches!(
                        session.progress,
                        DndWireProgress::DropPending
                            | DndWireProgress::DropPendingAwaitingStatus
                            | DndWireProgress::AwaitingFinished
                            | DndWireProgress::TerminalConsumed
                    )
            })
        {
            return false;
        }
        let source_matches = match (id.session_id(), source) {
            (CanonicalDndSessionId::Wayland(_), None) => true,
            (CanonicalDndSessionId::Xwayland(offer_id), Some(source)) => {
                offer_id.generation() == id.generation() && source.generation() == id.generation()
            }
            (CanonicalDndSessionId::Wayland(_), Some(_))
            | (CanonicalDndSessionId::Xwayland(_), None) => false,
        };
        if !source_matches {
            return false;
        }
        self.active = Some(DndSession {
            id,
            source,
            source_proxy: None,
            ownership_timestamp: None,
            ownership_confirmed: false,
            ownership_claim_issued: false,
            discovery_target: None,
            discovery_recipient: None,
            target: None,
            wire_recipient: None,
            target_version: None,
            progress: DndWireProgress::AwaitingEnter,
            action: None,
            x: 0,
            y: 0,
            outstanding_position: None,
            coalesced_position: None,
            latest_position: None,
            last_status: None,
            pending_drop_action: None,
            authorized_drop_action: None,
            status_deadline_ns: None,
            finished_deadline_ns: None,
            source_proxy_destroyed: false,
            mime_types: XwaylandDndMimeCatalog::default(),
            source_actions: Vec::new(),
            mime_atoms: Vec::new(),
            timestamp_deadline_ns: None,
            ownership_deadline_ns: None,
            discovery_deadline_ns: None,
            discovery_serial: 0,
        });
        self.feedback.clear();
        true
    }

    pub fn install_wayland_session(
        &mut self,
        id: XwaylandDndAdapterId,
        mime_types: XwaylandDndMimeCatalog,
        source_actions: Vec<XwaylandDndAction>,
    ) -> bool {
        if !matches!(id.session_id(), CanonicalDndSessionId::Wayland(_)) {
            return false;
        }
        if self.active.as_ref().is_some_and(|session| session.id == id) {
            return true;
        }
        if !self.install_canonical_session(id, None) {
            return false;
        }
        if let Some(session) = self.active.as_mut() {
            session.mime_types = mime_types;
            session.source_actions = super::dnd_wire::wayland_actions(&source_actions)
                .into_iter()
                .filter(|action| {
                    *action != crate::xwayland::WaylandDndAction::Ask
                        || source_actions.iter().any(|candidate| {
                            matches!(candidate, XwaylandDndAction::Copy | XwaylandDndAction::Move)
                        })
                })
                .map(|action| match action {
                    crate::xwayland::WaylandDndAction::Copy => XwaylandDndAction::Copy,
                    crate::xwayland::WaylandDndAction::Move => XwaylandDndAction::Move,
                    crate::xwayland::WaylandDndAction::Ask => XwaylandDndAction::Ask,
                })
                .collect();
            session.mime_atoms = vec![None; session.mime_types.as_slice().len()];
        }
        true
    }

    pub(crate) fn take_feedback(&mut self) -> Vec<DndFeedback> {
        self.feedback.drain(..).collect()
    }

    pub fn next_deadline_ns(&self) -> Option<u64> {
        let session_deadline = self.active.as_ref().and_then(|session| {
            [
                session.timestamp_deadline_ns,
                session.ownership_deadline_ns,
                session.discovery_deadline_ns,
                session.status_deadline_ns,
                session.finished_deadline_ns,
            ]
            .into_iter()
            .flatten()
            .min()
        });
        let reply_deadline = self
            .pending_replies
            .values()
            .map(|reply| match reply {
                DndPendingReply::TargetProxy { deadline_ns, .. }
                | DndPendingReply::ProxySelf { deadline_ns, .. }
                | DndPendingReply::Aware { deadline_ns, .. }
                | DndPendingReply::MimeAtom { deadline_ns, .. }
                | DndPendingReply::Ownership { deadline_ns, .. }
                | DndPendingReply::MultipleRead { deadline_ns, .. } => *deadline_ns,
            })
            .min();
        session_deadline.into_iter().chain(reply_deadline).min()
    }

    pub fn bind_source_proxy(&mut self, id: XwaylandDndAdapterId, window: u32) -> bool {
        let Some(session) = self.active.as_mut().filter(|session| session.id == id) else {
            return false;
        };
        if !matches!(id.session_id(), CanonicalDndSessionId::Wayland(_))
            || window == 0
            || session.source_proxy.is_some()
        {
            return false;
        }
        session.source_proxy = Some(window);
        true
    }

    pub fn confirm_source_ownership(
        &mut self,
        id: XwaylandDndAdapterId,
        window: u32,
        timestamp: u32,
    ) -> bool {
        let Some(session) = self.active.as_mut().filter(|session| session.id == id) else {
            return false;
        };
        if session.source_proxy != Some(window) || timestamp == 0 || session.ownership_confirmed {
            return false;
        }
        session.ownership_timestamp = Some(timestamp);
        session.ownership_confirmed = true;
        true
    }

    pub fn set_discovered_target(
        &mut self,
        id: XwaylandDndAdapterId,
        actual: X11WindowHandle,
        recipient: u32,
        version: XwaylandDndVersion,
    ) -> bool {
        let Some(session) = self.active.as_mut().filter(|session| session.id == id) else {
            return false;
        };
        if actual.generation() != id.generation()
            || actual.xid() == 0
            || recipient == 0
            || session.target.is_some()
            || session.progress != DndWireProgress::AwaitingEnter
        {
            return false;
        }
        session.target = Some(actual);
        session.wire_recipient = Some(recipient);
        session.target_version = Some(version);
        true
    }

    pub fn mark_entered(&mut self, id: XwaylandDndAdapterId) -> bool {
        let Some(session) = self.active.as_mut().filter(|session| session.id == id) else {
            return false;
        };
        if session.progress != DndWireProgress::AwaitingEnter {
            return false;
        }
        session.progress = DndWireProgress::Entered;
        true
    }

    pub fn position(
        &mut self,
        id: XwaylandDndAdapterId,
        target: X11WindowHandle,
        x: i32,
        y: i32,
        action: Option<XwaylandDndAction>,
    ) -> bool {
        let Some(session) = self.active.as_mut().filter(|session| session.id == id) else {
            return false;
        };
        if target.generation() != id.generation()
            || !matches!(
                session.progress,
                DndWireProgress::Entered | DndWireProgress::Positioned
            )
            || (session.progress == DndWireProgress::Positioned && session.target != Some(target))
            || matches!(
                session.progress,
                DndWireProgress::DropPending
                    | DndWireProgress::DropPendingAwaitingStatus
                    | DndWireProgress::AwaitingFinished
                    | DndWireProgress::TerminalConsumed
            )
        {
            return false;
        }
        session.target = Some(target);
        session.x = x;
        session.y = y;
        session.action = action;
        session.progress = DndWireProgress::Positioned;
        true
    }

    /// One XdndPosition may be outstanding. Further motion is reduced to the
    /// latest point until the exact target acknowledges that position.
    pub fn queue_position(
        &mut self,
        id: XwaylandDndAdapterId,
        target: X11WindowHandle,
        position: CoalescedPosition,
    ) -> PositionDisposition {
        let Some(session) = self.active.as_mut().filter(|session| session.id == id) else {
            return PositionDisposition::Stale;
        };
        if session.target != Some(target) {
            return PositionDisposition::Stale;
        }
        let position = CoalescedPosition {
            action: position
                .action
                .or_else(|| super::dnd_wire::requested_action(&session.source_actions)),
            ..position
        };
        match session.progress {
            DndWireProgress::AwaitingStatus => {
                session.coalesced_position = Some(position);
                PositionDisposition::Coalesced
            }
            DndWireProgress::Positioned => {
                session.x = position.x.round() as i32;
                session.y = position.y.round() as i32;
                session.action = position.action;
                session.outstanding_position = Some(position);
                session.progress = DndWireProgress::AwaitingStatus;
                PositionDisposition::SendNow(position)
            }
            _ => PositionDisposition::Stale,
        }
    }

    pub fn mark_initial_position_sent(
        &mut self,
        id: XwaylandDndAdapterId,
        position: CoalescedPosition,
    ) -> bool {
        let Some(session) = self.active.as_mut().filter(|session| session.id == id) else {
            return false;
        };
        if session.progress != DndWireProgress::Positioned || session.outstanding_position.is_some()
        {
            return false;
        }
        session.outstanding_position = Some(position);
        session.progress = DndWireProgress::AwaitingStatus;
        true
    }

    /// A status reply is useful only for the exact source proxy, actual target,
    /// validated recipient, and outstanding Position.
    pub(super) fn acknowledge_status(
        &mut self,
        id: XwaylandDndAdapterId,
        source_proxy: u32,
        actual_target: X11WindowHandle,
        recipient: u32,
        status: DndStatusResult,
    ) -> Option<StatusAcknowledgement> {
        let session = self.active.as_mut().filter(|session| {
            session.id == id
                && session.source_proxy == Some(source_proxy)
                && session.target == Some(actual_target)
                && session.wire_recipient == Some(recipient)
                && matches!(
                    session.progress,
                    DndWireProgress::AwaitingStatus | DndWireProgress::DropPendingAwaitingStatus
                )
                && session.outstanding_position.is_some()
        })?;
        let drop_pending = session.progress == DndWireProgress::DropPendingAwaitingStatus;
        session.last_status = Some(status);
        session.status_deadline_ns = None;
        let next = session.coalesced_position.take();
        let pending_drop_action = if drop_pending {
            session.pending_drop_action
        } else {
            None
        };
        if let Some(position) = next {
            session.outstanding_position = Some(position);
            return Some(StatusAcknowledgement {
                next_position: Some(position),
                pending_drop_action,
            });
        }
        session.outstanding_position = None;
        session.progress = if drop_pending {
            DndWireProgress::DropPending
        } else {
            DndWireProgress::Positioned
        };
        Some(StatusAcknowledgement {
            next_position: None,
            pending_drop_action,
        })
    }

    /// Leave one exact X11 target while keeping the canonical adapter session
    /// active for a later target enter.
    pub fn leave_target(&mut self, id: XwaylandDndAdapterId, target: X11WindowHandle) -> bool {
        let Some(session) = self
            .active
            .as_mut()
            .filter(|session| session.id == id && session.target == Some(target))
        else {
            return false;
        };
        if target.generation() != id.generation()
            || matches!(
                session.progress,
                DndWireProgress::DropPending
                    | DndWireProgress::DropPendingAwaitingStatus
                    | DndWireProgress::AwaitingFinished
                    | DndWireProgress::TerminalConsumed
            )
        {
            return false;
        }
        session.target = None;
        session.wire_recipient = None;
        session.target_version = None;
        session.progress = DndWireProgress::AwaitingEnter;
        session.action = None;
        session.x = 0;
        session.y = 0;
        session.outstanding_position = None;
        session.coalesced_position = None;
        session.last_status = None;
        session.pending_drop_action = None;
        session.authorized_drop_action = None;
        session.status_deadline_ns = None;
        session.finished_deadline_ns = None;
        true
    }

    pub fn status_timed_out(&mut self, id: XwaylandDndAdapterId, now_ns: u64) -> bool {
        let Some(session) = self.active.as_mut().filter(|session| session.id == id) else {
            return false;
        };
        if !matches!(
            session.progress,
            DndWireProgress::AwaitingStatus | DndWireProgress::DropPendingAwaitingStatus
        ) || session
            .status_deadline_ns
            .is_none_or(|deadline| now_ns < deadline)
        {
            return false;
        }
        session.outstanding_position = None;
        session.coalesced_position = None;
        session.status_deadline_ns = None;
        true
    }

    pub fn set_status_deadline(&mut self, id: XwaylandDndAdapterId, deadline_ns: u64) -> bool {
        let Some(session) = self.active.as_mut().filter(|session| {
            session.id == id
                && matches!(
                    session.progress,
                    DndWireProgress::AwaitingStatus | DndWireProgress::DropPendingAwaitingStatus
                )
        }) else {
            return false;
        };
        session.status_deadline_ns = Some(deadline_ns);
        true
    }

    pub fn request_drop(
        &mut self,
        id: XwaylandDndAdapterId,
        target: X11WindowHandle,
        action: XwaylandDndAction,
    ) -> bool {
        let Some(session) = self.active.as_mut().filter(|session| session.id == id) else {
            return false;
        };
        let awaiting_status = session.progress == DndWireProgress::AwaitingStatus;
        if session.target != Some(target)
            || !matches!(
                session.progress,
                DndWireProgress::Positioned | DndWireProgress::AwaitingStatus
            )
            || session.source_proxy.is_none()
            || !session.ownership_confirmed
            || !session.source_actions.contains(&action)
            || action.to_wayland_action().is_none()
            || (!awaiting_status
                && !session
                    .last_status
                    .is_some_and(|status| status.accepted && status.action == Some(action)))
        {
            return false;
        }
        session.pending_drop_action = Some(action);
        session.progress = if awaiting_status {
            DndWireProgress::DropPendingAwaitingStatus
        } else {
            DndWireProgress::DropPending
        };
        true
    }

    pub fn mark_awaiting_finished(
        &mut self,
        id: XwaylandDndAdapterId,
        accepted_action: XwaylandDndAction,
        deadline_ns: u64,
    ) -> bool {
        let Some(session) = self.active.as_mut().filter(|session| session.id == id) else {
            return false;
        };
        if session.progress != DndWireProgress::DropPending {
            return false;
        }
        session.authorized_drop_action = Some(accepted_action);
        session.finished_deadline_ns = Some(deadline_ns);
        session.progress = DndWireProgress::AwaitingFinished;
        true
    }

    /// Consume one exact wire terminal edge without completing canonical drag
    /// state. Runtime submits the result back to the compositor for authority.
    pub fn consume_terminal_result(&mut self, id: XwaylandDndAdapterId) -> bool {
        let Some(session) = self.active.as_mut().filter(|session| session.id == id) else {
            return false;
        };
        if !matches!(
            session.progress,
            DndWireProgress::DropPending
                | DndWireProgress::DropPendingAwaitingStatus
                | DndWireProgress::AwaitingFinished
        ) {
            return false;
        }
        session.progress = DndWireProgress::TerminalConsumed;
        session.pending_drop_action = None;
        session.outstanding_position = None;
        session.coalesced_position = None;
        session.status_deadline_ns = None;
        session.finished_deadline_ns = None;
        true
    }

    /// Consume an exact DropRequested edge that cannot be reconciled with the
    /// current wire hover. This is a fail-closed adapter result, not canonical
    /// completion; the compositor still validates the feedback identity.
    pub fn consume_drop_rejection(&mut self, id: XwaylandDndAdapterId) -> bool {
        let Some(session) = self.active.as_mut().filter(|session| session.id == id) else {
            return false;
        };
        if session.progress == DndWireProgress::TerminalConsumed {
            return false;
        }
        session.progress = DndWireProgress::TerminalConsumed;
        session.pending_drop_action = None;
        session.outstanding_position = None;
        session.coalesced_position = None;
        session.status_deadline_ns = None;
        session.finished_deadline_ns = None;
        true
    }

    pub fn mark_source_proxy_destroyed(&mut self, id: XwaylandDndAdapterId, window: u32) -> bool {
        let Some(session) = self
            .active
            .as_mut()
            .filter(|session| session.id == id && session.source_proxy == Some(window))
        else {
            return false;
        };
        session.source_proxy_destroyed = true;
        true
    }

    pub fn awaiting_status(&self, id: XwaylandDndAdapterId) -> bool {
        self.active.as_ref().is_some_and(|session| {
            session.id == id
                && matches!(
                    session.progress,
                    DndWireProgress::AwaitingStatus | DndWireProgress::DropPendingAwaitingStatus
                )
        })
    }

    pub fn retire(&mut self, id: XwaylandDndAdapterId) -> bool {
        if self.active.as_ref().is_some_and(|session| session.id == id) {
            self.active = None;
            true
        } else {
            false
        }
    }

    pub fn active_id(&self) -> Option<XwaylandDndAdapterId> {
        self.active.as_ref().map(|session| session.id)
    }

    pub fn active_session(&self) -> Option<&DndSession> {
        self.active.as_ref()
    }

    pub fn is_internal_window(&self, window: u32) -> bool {
        self.active
            .as_ref()
            .is_some_and(|session| session.source_proxy == Some(window))
    }

    pub fn progress(&self, id: XwaylandDndAdapterId) -> Option<DndWireProgress> {
        self.active
            .as_ref()
            .filter(|session| session.id == id)
            .map(|session| session.progress)
    }

    pub fn terminal_event_consumed(&self, id: XwaylandDndAdapterId) -> bool {
        self.active
            .as_ref()
            .filter(|session| session.id == id)
            .is_some_and(|session| session.progress == DndWireProgress::TerminalConsumed)
    }

    pub fn clear_generation(&mut self, generation: XwaylandGeneration) {
        if self
            .active
            .as_ref()
            .is_some_and(|session| session.id.generation() == generation)
        {
            self.active = None;
        }
        self.pending_replies.retain(|_, reply| match reply {
            DndPendingReply::TargetProxy { id, .. }
            | DndPendingReply::ProxySelf { id, .. }
            | DndPendingReply::Aware { id, .. }
            | DndPendingReply::MimeAtom { id, .. }
            | DndPendingReply::Ownership { id, .. }
            | DndPendingReply::MultipleRead { id, .. } => id.generation() != generation,
        });
        self.feedback.retain(|feedback| match feedback {
            DndFeedback::Status(feedback) => feedback.id.generation() != generation,
            DndFeedback::Terminal(feedback) => feedback.id.generation() != generation,
        });
        self.internal_windows.clear();
    }
}

pub(crate) use super::dnd_adapter::{
    apply_transitions, client_message, destroy_notify, handle_deadline, is_internal_window,
    poll_replies, property_notify, retire_generation, selection_clear, take_feedback,
};
pub(crate) use super::dnd_selection::selection_request;
#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;

    fn new_manager() -> (DndManager, XwaylandDndAdapterId, X11WindowHandle) {
        let generation = XwaylandGeneration::new(NonZeroU64::new(71).unwrap());
        let session_id = CanonicalDndSessionId::Wayland(NonZeroU64::new(9001).unwrap());
        let id = XwaylandDndAdapterId::new(session_id, generation).unwrap();
        let target = X11WindowHandle::new(generation, 0x440);
        let catalog = XwaylandDndMimeCatalog::try_new(vec![
            "text/plain".to_owned(),
            "image/png".to_owned(),
            "application/json".to_owned(),
            "text/uri-list".to_owned(),
        ])
        .unwrap();
        let mut manager = DndManager::default();
        assert!(manager.install_wayland_session(
            id,
            catalog,
            vec![
                XwaylandDndAction::Ask,
                XwaylandDndAction::Link,
                XwaylandDndAction::Private,
                XwaylandDndAction::Move,
                XwaylandDndAction::Copy,
            ],
        ));
        assert_eq!(
            manager.active_session().unwrap().source_actions,
            vec![
                XwaylandDndAction::Copy,
                XwaylandDndAction::Move,
                XwaylandDndAction::Ask,
            ]
        );
        assert!(manager.bind_source_proxy(id, 0x880));
        assert!(manager.confirm_source_ownership(id, 0x880, 1234));
        assert!(manager.set_discovered_target(
            id,
            target,
            0x441,
            XwaylandDndVersion::new(5).unwrap(),
        ));
        assert!(manager.mark_entered(id));
        assert!(manager.position(id, target, 1, 2, Some(XwaylandDndAction::Copy)));
        (manager, id, target)
    }

    #[test]
    fn position_status_backpressure_coalesces_to_one_latest_position() {
        let (mut manager, id, target) = new_manager();
        let first = CoalescedPosition {
            x: 10.0,
            y: 20.0,
            action: Some(XwaylandDndAction::Copy),
        };
        assert_eq!(
            manager.queue_position(id, target, first),
            PositionDisposition::SendNow(first)
        );
        assert!(manager.set_status_deadline(id, 100));
        let mut latest = first;
        for index in 0..1000 {
            latest = CoalescedPosition {
                x: f64::from(index),
                y: f64::from(index + 1),
                action: Some(XwaylandDndAction::Move),
            };
            assert_eq!(
                manager.queue_position(id, target, latest),
                PositionDisposition::Coalesced
            );
        }
        assert_eq!(
            manager.active_session().unwrap().coalesced_position,
            Some(latest)
        );
        let status = DndStatusResult {
            accepted: true,
            action: Some(XwaylandDndAction::Copy),
            requested_action: Some(XwaylandDndAction::Copy),
        };
        assert_eq!(
            manager.acknowledge_status(id, 0x881, target, 0x441, status),
            None
        );
        assert_eq!(
            manager.acknowledge_status(
                id,
                0x880,
                X11WindowHandle::new(target.generation(), 0x442),
                0x441,
                status,
            ),
            None
        );
        assert_eq!(
            manager.acknowledge_status(id, 0x880, target, 0x441, status),
            Some(StatusAcknowledgement {
                next_position: Some(latest),
                pending_drop_action: None,
            })
        );
        assert_eq!(manager.active_session().unwrap().coalesced_position, None);
        assert_eq!(
            manager.acknowledge_status(id, 0x880, target, 0x441, status),
            Some(StatusAcknowledgement {
                next_position: None,
                pending_drop_action: None,
            })
        );
        assert_eq!(
            manager.active_session().unwrap().progress,
            DndWireProgress::Positioned
        );
    }

    #[test]
    fn target_leave_preserves_the_exact_per_drag_source_proxy() {
        let (mut manager, id, target_a) = new_manager();
        let source_proxy = manager.active_session().unwrap().source_proxy;
        assert!(manager.leave_target(id, target_a));
        let session = manager.active_session().unwrap();
        assert_eq!(session.id, id);
        assert_eq!(session.source_proxy, source_proxy);
        assert!(session.ownership_confirmed);
        assert_eq!(session.ownership_timestamp, Some(1234));
        let target_b = X11WindowHandle::new(target_a.generation(), target_a.xid() + 1);
        assert!(manager.set_discovered_target(
            id,
            target_b,
            target_b.xid() + 1,
            XwaylandDndVersion::new(4).unwrap(),
        ));
        assert!(manager.mark_entered(id));
        assert!(manager.position(id, target_b, 3, 4, Some(XwaylandDndAction::Copy)));
        assert_eq!(manager.active_session().unwrap().source_proxy, source_proxy);
    }

    #[test]
    fn target_left_before_enter_clears_discovered_target_for_reentry() {
        let (mut manager, id, target_a) = new_manager();
        let session = manager.active.as_mut().unwrap();
        session.progress = DndWireProgress::AwaitingEnter;
        session.target = None;
        assert!(manager.set_discovered_target(
            id,
            target_a,
            target_a.xid() + 1,
            XwaylandDndVersion::new(5).unwrap(),
        ));

        assert!(manager.leave_target(id, target_a));
        assert_eq!(manager.active_session().unwrap().target, None);
        let target_b = X11WindowHandle::new(target_a.generation(), target_a.xid() + 2);
        assert!(manager.set_discovered_target(
            id,
            target_b,
            target_b.xid() + 1,
            XwaylandDndVersion::new(5).unwrap(),
        ));
    }

    #[test]
    fn ask_is_not_advertised_without_a_concrete_wayland_action() {
        let generation = XwaylandGeneration::new(NonZeroU64::new(72).unwrap());
        let id = XwaylandDndAdapterId::new(
            CanonicalDndSessionId::Wayland(NonZeroU64::new(9002).unwrap()),
            generation,
        )
        .unwrap();
        let mut manager = DndManager::default();
        assert!(manager.install_wayland_session(
            id,
            XwaylandDndMimeCatalog::default(),
            vec![XwaylandDndAction::Ask, XwaylandDndAction::Link],
        ));
        assert!(manager.active_session().unwrap().source_actions.is_empty());
    }

    #[test]
    fn feedback_for_one_target_keeps_only_its_latest_status() {
        let (mut manager, id, target) = new_manager();
        manager.push_status_feedback(DndStatusFeedback {
            id,
            target,
            accepted: true,
            action: Some(XwaylandDndAction::Copy),
        });
        manager.push_status_feedback(DndStatusFeedback {
            id,
            target,
            accepted: false,
            action: None,
        });
        assert_eq!(
            manager.take_feedback(),
            [DndFeedback::Status(DndStatusFeedback {
                id,
                target,
                accepted: false,
                action: None,
            })]
        );
    }

    #[test]
    fn terminal_feedback_remains_after_the_latest_status_at_capacity() {
        let (mut manager, id, target) = new_manager();
        let earlier = DndStatusFeedback {
            id,
            target,
            accepted: false,
            action: None,
        };
        let latest = DndStatusFeedback {
            id,
            target,
            accepted: true,
            action: Some(XwaylandDndAction::Move),
        };
        // Manufacture the otherwise unreachable duplicate Status entries to
        // exercise the hard bounded-overflow behavior.
        manager.feedback.push_back(DndFeedback::Status(earlier));
        manager.feedback.push_back(DndFeedback::Status(latest));
        let terminal = DndTerminalFeedback {
            id,
            target,
            accepted: true,
            action: Some(XwaylandDndAction::Move),
        };
        assert!(manager.push_terminal_feedback(terminal));
        assert_eq!(
            manager.take_feedback(),
            [DndFeedback::Status(latest), DndFeedback::Terminal(terminal)]
        );
    }
}
