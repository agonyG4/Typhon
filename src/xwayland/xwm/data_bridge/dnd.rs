//! Bounded XDND wire-adapter bookkeeping.
//!
//! The compositor supplies canonical session identities and owns drag
//! lifecycle. This manager keeps one generation-qualified adapter view and
//! tracks only wire progress needed by a future XDND adapter.

use super::super::super::{X11WindowHandle, XwaylandGeneration};
use crate::xwayland::{CanonicalDndSessionId, XwaylandDndAction, XwaylandDndAdapterId};

pub use crate::xwayland::XwaylandDndAction as XdndAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DndWireProgress {
    AwaitingEnter,
    Entered,
    Positioned,
    DropReady,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DndSession {
    pub id: XwaylandDndAdapterId,
    pub source: Option<X11WindowHandle>,
    pub target: Option<X11WindowHandle>,
    pub progress: DndWireProgress,
    pub terminal_event_consumed: bool,
    pub action: Option<XwaylandDndAction>,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Default)]
pub struct DndManager {
    active: Option<DndSession>,
}

impl DndManager {
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
            target: None,
            progress: DndWireProgress::AwaitingEnter,
            terminal_event_consumed: false,
            action: None,
            x: 0,
            y: 0,
        });
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
        self.active.map(|session| session.id)
    }

    pub fn active_session(&self) -> Option<&DndSession> {
        self.active.as_ref()
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
            .is_some_and(|session| session.id.generation() == generation)
        {
            self.active = None;
        }
    }
}
