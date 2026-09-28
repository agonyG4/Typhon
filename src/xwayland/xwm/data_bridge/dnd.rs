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
    DropReady,
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
    pub terminal_event_consumed: bool,
    pub action: Option<XwaylandDndAction>,
    pub x: i32,
    pub y: i32,
    pub awaiting_status: bool,
    pub coalesced_position: Option<CoalescedPosition>,
    pub latest_position: Option<CoalescedPosition>,
    pub status_deadline_ns: Option<u64>,
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
    pub(super) feedback: VecDeque<DndStatusFeedback>,
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

pub(super) const SOURCE_TIMESTAMP_TIMEOUT_NS: u64 = 2_000_000_000;
pub(super) const SOURCE_OWNERSHIP_TIMEOUT_NS: u64 = 2_000_000_000;
pub(super) const TARGET_DISCOVERY_TIMEOUT_NS: u64 = 2_000_000_000;
pub(super) const TARGET_STATUS_TIMEOUT_NS: u64 = 1_000_000_000;
pub(super) const DND_REPLY_BUDGET: usize = 64;
pub(super) const MAX_PENDING_DND_REPLIES: usize = 128;
pub(super) const MAX_MULTIPLE_PAIRS: usize = 64;

impl DndManager {
    pub(super) fn push_feedback(&mut self, feedback: DndStatusFeedback) {
        if let Some(existing) = self
            .feedback
            .iter_mut()
            .find(|existing| existing.id == feedback.id && existing.target == feedback.target)
        {
            *existing = feedback;
            return;
        }
        if self.feedback.len() == 64 {
            self.feedback.pop_front();
        }
        self.feedback.push_back(feedback);
    }

    /// Install the canonical session identity for one XWM adapter view.
    /// Replacing it drops the old exact identity from this one-slot manager.
    pub fn install_canonical_session(
        &mut self,
        id: XwaylandDndAdapterId,
        source: Option<X11WindowHandle>,
    ) -> bool {
        if self.active.as_ref().is_some_and(|session| session.id == id) {
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
            terminal_event_consumed: false,
            action: None,
            x: 0,
            y: 0,
            awaiting_status: false,
            coalesced_position: None,
            latest_position: None,
            status_deadline_ns: None,
            mime_types: XwaylandDndMimeCatalog::default(),
            source_actions: Vec::new(),
            mime_atoms: Vec::new(),
            timestamp_deadline_ns: None,
            ownership_deadline_ns: None,
            discovery_deadline_ns: None,
            discovery_serial: 0,
        });
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

    pub(crate) fn take_feedback(&mut self) -> Vec<DndStatusFeedback> {
        self.feedback.drain(..).collect()
    }

    pub fn next_deadline_ns(&self) -> Option<u64> {
        let session_deadline = self.active.as_ref().and_then(|session| {
            [
                session.timestamp_deadline_ns,
                session.ownership_deadline_ns,
                session.discovery_deadline_ns,
                session.status_deadline_ns,
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
            || session.terminal_event_consumed
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
        if session.progress != DndWireProgress::AwaitingEnter || session.terminal_event_consumed {
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
            || session.terminal_event_consumed
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
        if session.target != Some(target)
            || session.progress != DndWireProgress::Positioned
            || session.terminal_event_consumed
        {
            return PositionDisposition::Stale;
        }
        session.x = position.x.round() as i32;
        session.y = position.y.round() as i32;
        session.action = position.action;
        if session.awaiting_status {
            session.coalesced_position = Some(position);
            PositionDisposition::Coalesced
        } else {
            session.awaiting_status = true;
            PositionDisposition::SendNow(position)
        }
    }

    /// A status reply is useful only for the exact source proxy, actual target,
    /// validated recipient, and outstanding Position.
    pub fn acknowledge_status(
        &mut self,
        id: XwaylandDndAdapterId,
        source_proxy: u32,
        actual_target: X11WindowHandle,
        recipient: u32,
    ) -> Option<Option<CoalescedPosition>> {
        let session = self.active.as_mut().filter(|session| {
            session.id == id
                && session.source_proxy == Some(source_proxy)
                && session.target == Some(actual_target)
                && session.wire_recipient == Some(recipient)
                && session.awaiting_status
                && !session.terminal_event_consumed
        })?;
        session.awaiting_status = false;
        session.status_deadline_ns = None;
        let next = session.coalesced_position.take();
        if next.is_some() {
            session.awaiting_status = true;
        }
        Some(next)
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
            || session.progress == DndWireProgress::DropReady
            || session.terminal_event_consumed
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
        session.awaiting_status = false;
        session.coalesced_position = None;
        session.status_deadline_ns = None;
        true
    }

    pub fn status_timed_out(&mut self, id: XwaylandDndAdapterId, now_ns: u64) -> bool {
        let Some(session) = self.active.as_mut().filter(|session| session.id == id) else {
            return false;
        };
        if !session.awaiting_status
            || session
                .status_deadline_ns
                .is_none_or(|deadline| now_ns < deadline)
        {
            return false;
        }
        session.awaiting_status = false;
        session.coalesced_position = None;
        session.status_deadline_ns = None;
        true
    }

    pub fn set_status_deadline(&mut self, id: XwaylandDndAdapterId, deadline_ns: u64) -> bool {
        let Some(session) = self
            .active
            .as_mut()
            .filter(|session| session.id == id && session.awaiting_status)
        else {
            return false;
        };
        session.status_deadline_ns = Some(deadline_ns);
        true
    }

    pub fn mark_drop_ready(&mut self, id: XwaylandDndAdapterId) -> bool {
        let Some(session) = self.active.as_mut().filter(|session| session.id == id) else {
            return false;
        };
        if session.progress != DndWireProgress::Positioned
            || session.target.is_none()
            || session.action.is_none()
            || session.terminal_event_consumed
        {
            return false;
        }
        session.progress = DndWireProgress::DropReady;
        true
    }

    /// Claim one terminal adapter event for this exact session. This tracks
    /// wire-event consumption; it does not transition canonical drag state.
    pub fn consume_terminal_event(&mut self, id: XwaylandDndAdapterId) -> bool {
        let Some(session) = self.active.as_mut().filter(|session| session.id == id) else {
            return false;
        };
        if session.progress == DndWireProgress::AwaitingEnter || session.terminal_event_consumed {
            return false;
        }
        session.terminal_event_consumed = true;
        true
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
            .is_some_and(|session| session.terminal_event_consumed)
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
        self.feedback
            .retain(|feedback| feedback.id.generation() != generation);
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
        assert_eq!(manager.acknowledge_status(id, 0x881, target, 0x441), None);
        assert_eq!(
            manager.acknowledge_status(
                id,
                0x880,
                X11WindowHandle::new(target.generation(), 0x442),
                0x441
            ),
            None
        );
        assert_eq!(
            manager.acknowledge_status(id, 0x880, target, 0x441),
            Some(Some(latest))
        );
        assert_eq!(manager.active_session().unwrap().coalesced_position, None);
        assert_eq!(
            manager.acknowledge_status(id, 0x880, target, 0x441),
            Some(None)
        );
        assert!(!manager.active_session().unwrap().awaiting_status);
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
        manager.push_feedback(DndStatusFeedback {
            id,
            target,
            accepted: true,
            action: Some(XwaylandDndAction::Copy),
        });
        manager.push_feedback(DndStatusFeedback {
            id,
            target,
            accepted: false,
            action: None,
        });
        assert_eq!(
            manager.take_feedback(),
            [DndStatusFeedback {
                id,
                target,
                accepted: false,
                action: None,
            }]
        );
    }
}
