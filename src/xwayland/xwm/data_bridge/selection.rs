use std::{
    collections::{HashMap, HashSet},
    num::NonZeroU64,
};

use x11rb::protocol::xproto::{Atom, Window};

use super::{BridgeGeneration, SelectionKind, SelectionOrigin};

pub const MAX_SELECTION_TARGETS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SelectionRevision(NonZeroU64);

impl SelectionRevision {
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetsDiscoveryState {
    Idle,
    AwaitingSelectionNotify,
    AwaitingProperty,
    Resolved,
    Failed,
    Inactive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SelectionIdentity {
    pub generation: BridgeGeneration,
    pub kind: SelectionKind,
    pub revision: SelectionRevision,
    pub owner: Window,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionSnapshot {
    pub generation: BridgeGeneration,
    pub kind: SelectionKind,
    pub revision: Option<SelectionRevision>,
    pub origin: Option<SelectionOrigin>,
    pub owner: Option<Window>,
    pub targets: Vec<Atom>,
    pub timestamp: u32,
    pub targets_state: TargetsDiscoveryState,
}

impl SelectionSnapshot {
    pub fn identity(&self) -> Option<SelectionIdentity> {
        Some(SelectionIdentity {
            generation: self.generation,
            kind: self.kind,
            revision: self.revision?,
            owner: self.owner?,
        })
    }
}

/// One current X11 ownership record per channel.
///
/// This is adapter state only.  Wayland's `SelectionState` remains the
/// compositor's canonical selection authority.
#[derive(Debug, Default)]
pub struct SelectionBridge {
    current: HashMap<SelectionKind, SelectionSnapshot>,
    active_generation: Option<BridgeGeneration>,
    next_revision: u64,
}

impl SelectionBridge {
    pub fn initialize_generation(&mut self, generation: BridgeGeneration) {
        if self.active_generation == Some(generation) {
            return;
        }
        self.current.clear();
        self.active_generation = Some(generation);
        for kind in [SelectionKind::Clipboard, SelectionKind::Primary] {
            self.current.insert(
                kind,
                SelectionSnapshot {
                    generation,
                    kind,
                    revision: None,
                    origin: None,
                    owner: None,
                    targets: Vec::new(),
                    timestamp: 0,
                    targets_state: TargetsDiscoveryState::Idle,
                },
            );
        }
        self.debug_assert_invariants();
    }

    /// Records one owner transition and returns its identity when it changed.
    /// Duplicate XFixes notifications do not create a new revision.
    pub fn observe_owner(
        &mut self,
        generation: BridgeGeneration,
        kind: SelectionKind,
        owner: Option<Window>,
        origin: Option<SelectionOrigin>,
        timestamp: u32,
    ) -> Option<SelectionRevision> {
        if self.active_generation != Some(generation) {
            return None;
        }
        let current = self.current.get(&kind)?;
        if current.owner == owner && current.origin == origin && current.timestamp == timestamp {
            return None;
        }

        self.next_revision = self
            .next_revision
            .checked_add(1)
            .expect("X11 selection revision space exhausted");
        let revision = SelectionRevision(
            NonZeroU64::new(self.next_revision).expect("selection revisions are nonzero"),
        );
        let current = self.current.get_mut(&kind).expect("channel initialized");
        current.revision = Some(revision);
        current.origin = origin;
        current.owner = owner;
        current.targets.clear();
        current.timestamp = timestamp;
        current.targets_state = TargetsDiscoveryState::Idle;
        self.debug_assert_invariants();
        Some(revision)
    }

    pub fn current(&self, kind: SelectionKind) -> Option<&SelectionSnapshot> {
        self.current.get(&kind)
    }

    pub fn should_reflect(&self, kind: SelectionKind, origin: SelectionOrigin) -> bool {
        self.current(kind)
            .is_some_and(|selection| selection.owner.is_some() && selection.origin != Some(origin))
    }

    pub fn mark_discovery_state(
        &mut self,
        identity: SelectionIdentity,
        state: TargetsDiscoveryState,
    ) -> bool {
        let Some(current) = self.current.get_mut(&identity.kind) else {
            return false;
        };
        if current.identity() != Some(identity) {
            return false;
        }
        current.targets_state = state;
        if state != TargetsDiscoveryState::Resolved {
            current.targets.clear();
        }
        self.debug_assert_invariants();
        true
    }

    pub fn resolve_targets(&mut self, identity: SelectionIdentity, targets: &[Atom]) -> bool {
        let Some(current) = self.current.get_mut(&identity.kind) else {
            return false;
        };
        if current.identity() != Some(identity)
            || current.targets_state != TargetsDiscoveryState::AwaitingProperty
        {
            return false;
        }
        if targets.len() > MAX_SELECTION_TARGETS {
            current.targets.clear();
            current.targets_state = TargetsDiscoveryState::Failed;
            self.debug_assert_invariants();
            return false;
        }
        let mut seen = HashSet::with_capacity(targets.len());
        current.targets = targets
            .iter()
            .copied()
            .filter(|target| seen.insert(*target))
            .collect();
        current.targets_state = TargetsDiscoveryState::Resolved;
        self.debug_assert_invariants();
        true
    }

    /// Clears only state belonging to a retired XWayland process generation.
    pub fn clear_generation(&mut self, generation: BridgeGeneration) {
        self.current
            .retain(|_, selection| selection.generation != generation);
        if self.active_generation == Some(generation) {
            self.active_generation = None;
        }
        self.debug_assert_invariants();
    }

    fn debug_assert_invariants(&self) {
        debug_assert!(self.current.iter().all(|(kind, selection)| {
            *kind == selection.kind
                && self.active_generation == Some(selection.generation)
                && selection.owner.is_some() == selection.origin.is_some()
                && (selection.owner.is_none() || selection.revision.is_some())
                && (selection.targets_state == TargetsDiscoveryState::Resolved
                    || selection.targets.is_empty())
                && (selection.owner.is_some()
                    || selection.targets_state == TargetsDiscoveryState::Idle)
        }));
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use super::*;

    fn generation() -> BridgeGeneration {
        BridgeGeneration::new(NonZeroU64::new(1).expect("nonzero"))
    }

    fn identity(bridge: &mut SelectionBridge, owner: Window, timestamp: u32) -> SelectionIdentity {
        bridge.initialize_generation(generation());
        bridge
            .observe_owner(
                generation(),
                SelectionKind::Clipboard,
                Some(owner),
                Some(SelectionOrigin::X11),
                timestamp,
            )
            .expect("owner revision");
        bridge
            .current(SelectionKind::Clipboard)
            .and_then(SelectionSnapshot::identity)
            .expect("owner identity")
    }

    #[test]
    fn owner_replacement_advances_revision_within_one_generation() {
        let mut bridge = SelectionBridge::default();
        let owner_a = identity(&mut bridge, 10, 35);
        let owner_b = bridge
            .observe_owner(
                generation(),
                SelectionKind::Clipboard,
                Some(11),
                Some(SelectionOrigin::X11),
                35,
            )
            .expect("replacement identity");

        let snapshot_b = bridge.current(SelectionKind::Clipboard).unwrap();
        assert_eq!(owner_a.generation, snapshot_b.generation);
        assert_ne!(owner_a.revision, owner_b);
        assert_ne!(Some(owner_a.owner), snapshot_b.owner);
    }

    #[test]
    fn duplicate_owner_notification_keeps_revision() {
        let mut bridge = SelectionBridge::default();
        let first = identity(&mut bridge, 10, 35);
        assert_eq!(
            bridge.observe_owner(
                generation(),
                SelectionKind::Clipboard,
                Some(10),
                Some(SelectionOrigin::X11),
                35,
            ),
            None
        );
        assert_eq!(
            bridge.current(SelectionKind::Clipboard).unwrap().identity(),
            Some(first)
        );
    }

    #[test]
    fn wayland_owned_selection_is_not_reflected_back_to_x11() {
        let mut bridge = SelectionBridge::default();
        bridge.initialize_generation(generation());
        bridge
            .observe_owner(
                generation(),
                SelectionKind::Clipboard,
                Some(10),
                Some(SelectionOrigin::Wayland),
                35,
            )
            .expect("internal owner revision");

        assert!(!bridge.should_reflect(SelectionKind::Clipboard, SelectionOrigin::Wayland));
        assert!(bridge.should_reflect(SelectionKind::Clipboard, SelectionOrigin::X11));
    }

    #[test]
    fn owner_none_clears_targets_and_pending_state() {
        let mut bridge = SelectionBridge::default();
        let owner = identity(&mut bridge, 10, 35);
        assert!(bridge.mark_discovery_state(owner, TargetsDiscoveryState::AwaitingProperty));
        assert!(bridge.resolve_targets(owner, &[1, 2]));

        assert!(
            bridge
                .observe_owner(generation(), SelectionKind::Clipboard, None, None, 36)
                .is_some()
        );
        let current = bridge.current(SelectionKind::Clipboard).unwrap();
        assert_eq!(current.owner, None);
        assert!(current.targets.is_empty());
        assert_eq!(current.targets_state, TargetsDiscoveryState::Idle);
    }

    #[test]
    fn target_atoms_are_bounded_and_deduplicated_in_order() {
        let mut bridge = SelectionBridge::default();
        let identity = identity(&mut bridge, 10, 35);
        assert!(bridge.mark_discovery_state(identity, TargetsDiscoveryState::AwaitingProperty));
        assert!(bridge.resolve_targets(identity, &[9, 4, 9, 3]));
        assert_eq!(
            bridge.current(SelectionKind::Clipboard).unwrap().targets,
            vec![9, 4, 3]
        );

        bridge
            .observe_owner(
                generation(),
                SelectionKind::Clipboard,
                Some(12),
                Some(SelectionOrigin::X11),
                36,
            )
            .expect("new owner revision");
        let next = bridge
            .current(SelectionKind::Clipboard)
            .and_then(SelectionSnapshot::identity)
            .expect("new owner identity");
        assert!(bridge.mark_discovery_state(next, TargetsDiscoveryState::AwaitingProperty));
        let oversized = (0..=MAX_SELECTION_TARGETS as Atom).collect::<Vec<_>>();
        assert!(!bridge.resolve_targets(next, &oversized));
        let current = bridge.current(SelectionKind::Clipboard).unwrap();
        assert!(current.targets.is_empty());
        assert_eq!(current.targets_state, TargetsDiscoveryState::Failed);
    }

    #[test]
    fn stale_owner_identity_cannot_resolve_targets() {
        let mut bridge = SelectionBridge::default();
        let owner_a = identity(&mut bridge, 10, 35);
        assert!(bridge.mark_discovery_state(owner_a, TargetsDiscoveryState::AwaitingProperty));
        bridge
            .observe_owner(
                generation(),
                SelectionKind::Clipboard,
                Some(11),
                Some(SelectionOrigin::X11),
                36,
            )
            .expect("replacement revision");
        let owner_b = bridge
            .current(SelectionKind::Clipboard)
            .and_then(SelectionSnapshot::identity)
            .expect("replacement identity");
        assert!(bridge.mark_discovery_state(owner_b, TargetsDiscoveryState::AwaitingProperty));
        assert!(!bridge.resolve_targets(owner_a, &[1]));
        assert_eq!(
            bridge.current(SelectionKind::Clipboard).unwrap().identity(),
            Some(owner_b)
        );
        assert!(
            bridge
                .current(SelectionKind::Clipboard)
                .unwrap()
                .targets
                .is_empty()
        );
    }
}
