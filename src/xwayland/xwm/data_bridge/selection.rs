use std::collections::HashMap;

use super::{BridgeGeneration, SelectionKind, SelectionOrigin};

pub const MAX_SELECTION_TARGETS: usize = 128;
const MAX_TARGET_LENGTH: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionSnapshot {
    pub generation: BridgeGeneration,
    pub kind: SelectionKind,
    pub origin: SelectionOrigin,
    pub targets: Vec<String>,
    pub timestamp: u32,
}

#[derive(Debug, Default)]
pub struct SelectionBridge {
    current: HashMap<SelectionKind, SelectionSnapshot>,
    next_generation: u64,
}

impl SelectionBridge {
    pub fn replace(
        &mut self,
        generation: BridgeGeneration,
        kind: SelectionKind,
        origin: SelectionOrigin,
        targets: impl IntoIterator<Item = String>,
        timestamp: u32,
    ) -> Option<SelectionSnapshot> {
        let mut normalized = Vec::new();
        for target in targets {
            if target.is_empty()
                || target.len() > MAX_TARGET_LENGTH
                || normalized.iter().any(|existing| existing == &target)
            {
                continue;
            }
            if normalized.len() == MAX_SELECTION_TARGETS {
                break;
            }
            normalized.push(target);
        }
        if normalized.is_empty() {
            return None;
        }
        self.next_generation = self.next_generation.saturating_add(1).max(1);
        let snapshot = SelectionSnapshot {
            generation,
            kind,
            origin,
            targets: normalized,
            timestamp,
        };
        self.current.insert(kind, snapshot.clone());
        Some(snapshot)
    }

    pub fn current(&self, kind: SelectionKind) -> Option<&SelectionSnapshot> {
        self.current.get(&kind)
    }

    pub fn should_reflect(&self, kind: SelectionKind, origin: SelectionOrigin) -> bool {
        self.current(kind)
            .is_some_and(|selection| selection.origin != origin)
    }

    pub fn clear_generation(&mut self, generation: BridgeGeneration) {
        self.current
            .retain(|_, selection| selection.generation != generation);
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use super::*;

    fn generation() -> BridgeGeneration {
        BridgeGeneration::new(NonZeroU64::new(1).expect("nonzero"))
    }

    #[test]
    fn ownership_replacement_prevents_reflection_loop() {
        let mut bridge = SelectionBridge::default();
        bridge
            .replace(
                generation(),
                SelectionKind::Clipboard,
                SelectionOrigin::Wayland,
                ["TARGETS".to_owned(), "UTF8_STRING".to_owned()],
                1,
            )
            .expect("selection");
        assert!(!bridge.should_reflect(SelectionKind::Clipboard, SelectionOrigin::Wayland));
        assert!(bridge.should_reflect(SelectionKind::Clipboard, SelectionOrigin::X11));
    }

    #[test]
    fn target_list_is_bounded() {
        let mut bridge = SelectionBridge::default();
        let targets = (0..MAX_SELECTION_TARGETS + 10).map(|index| format!("target/{index}"));
        let snapshot = bridge
            .replace(
                generation(),
                SelectionKind::Primary,
                SelectionOrigin::X11,
                targets,
                2,
            )
            .expect("selection");
        assert_eq!(snapshot.targets.len(), MAX_SELECTION_TARGETS);
    }
}
