use std::collections::HashMap;

use super::BridgeGeneration;

pub const DND_ACTION_COPY: u32 = 1;
pub const DND_ACTION_MOVE: u32 = 2;
pub const DND_ACTION_LINK: u32 = 4;
pub const DND_ACTION_ASK: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XdndAction {
    Copy,
    Move,
    Link,
    Ask,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DndPhase {
    Entered,
    Positioned,
    Dropped,
    Finished,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DndId {
    pub generation: BridgeGeneration,
    pub serial: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DndSession {
    pub id: DndId,
    pub source: u32,
    pub target: Option<u32>,
    pub phase: DndPhase,
    pub action: XdndAction,
    pub x: i32,
    pub y: i32,
    pub deadline_ns: u64,
}

#[derive(Debug, Default)]
pub struct DndManager {
    next_serial: u64,
    sessions: HashMap<DndId, DndSession>,
}

impl DndManager {
    pub fn begin(&mut self, generation: BridgeGeneration, source: u32, deadline_ns: u64) -> DndId {
        self.next_serial = self.next_serial.saturating_add(1).max(1);
        let id = DndId {
            generation,
            serial: self.next_serial,
        };
        self.sessions.insert(
            id,
            DndSession {
                id,
                source,
                target: None,
                phase: DndPhase::Entered,
                action: XdndAction::None,
                x: 0,
                y: 0,
                deadline_ns,
            },
        );
        id
    }

    pub fn position(&mut self, id: DndId, target: u32, x: i32, y: i32, action: XdndAction) -> bool {
        let Some(session) = self.sessions.get_mut(&id) else {
            return false;
        };
        if matches!(session.phase, DndPhase::Finished | DndPhase::Cancelled) {
            return false;
        }
        session.target = Some(target);
        session.x = x;
        session.y = y;
        session.action = action;
        session.phase = DndPhase::Positioned;
        true
    }

    pub fn drop(&mut self, id: DndId) -> bool {
        let Some(session) = self.sessions.get_mut(&id) else {
            return false;
        };
        if session.target.is_none() || session.action == XdndAction::None {
            return false;
        }
        session.phase = DndPhase::Dropped;
        true
    }

    pub fn finish(&mut self, id: DndId, accepted: bool) -> bool {
        let Some(session) = self.sessions.get_mut(&id) else {
            return false;
        };
        if !matches!(session.phase, DndPhase::Dropped | DndPhase::Positioned) {
            return false;
        }
        session.phase = if accepted {
            DndPhase::Finished
        } else {
            DndPhase::Cancelled
        };
        true
    }

    pub fn phase(&self, id: DndId) -> Option<DndPhase> {
        self.sessions.get(&id).map(|session| session.phase)
    }

    pub fn expire(&mut self, now_ns: u64) {
        self.sessions
            .retain(|_, session| now_ns < session.deadline_ns);
    }

    pub fn clear_generation(&mut self, generation: BridgeGeneration) {
        self.sessions.retain(|id, _| id.generation != generation);
    }
}

pub fn action_from_mask(mask: u32) -> XdndAction {
    if mask & DND_ACTION_COPY != 0 {
        XdndAction::Copy
    } else if mask & DND_ACTION_MOVE != 0 {
        XdndAction::Move
    } else if mask & DND_ACTION_LINK != 0 {
        XdndAction::Link
    } else if mask & DND_ACTION_ASK != 0 {
        XdndAction::Ask
    } else {
        XdndAction::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;

    #[test]
    fn dnd_requires_target_and_action_before_drop() {
        let generation = BridgeGeneration::new(NonZeroU64::new(1).expect("nonzero"));
        let mut manager = DndManager::default();
        let id = manager.begin(generation, 4, 100);
        assert!(!manager.drop(id));
        assert!(manager.position(id, 5, 10, 20, XdndAction::Copy));
        assert!(manager.drop(id));
        assert!(manager.finish(id, true));
    }
}
