use std::num::NonZeroU64;

use oblivion_one::xwayland::xwm::data_bridge::{
    BridgeGeneration,
    dnd::{DndManager, DndPhase, XdndAction},
};

#[test]
fn xdnd_foundation_bounds_target_change_and_terminal_cleanup() {
    let generation = BridgeGeneration::new(NonZeroU64::new(7).expect("nonzero"));
    let mut manager = DndManager::default();
    let id = manager.begin(generation, 11, 100);
    assert!(manager.position(id, 12, 40, 50, XdndAction::Copy));
    assert!(manager.position(id, 13, 60, 70, XdndAction::Move));
    assert!(manager.drop(id));
    assert!(manager.finish(id, true));
    assert!(!manager.finish(id, false));
    assert_eq!(manager.phase(id), Some(DndPhase::Finished));
}
