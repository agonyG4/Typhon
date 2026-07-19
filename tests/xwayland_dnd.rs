use std::num::NonZeroU64;

use oblivion_one::xwayland::xwm::data_bridge::{
    BridgeGeneration,
    dnd::{DndManager, DndPhase, XdndAction},
};

#[test]
fn xdnd_target_change_and_terminal_cleanup_are_bounded() {
    let generation = BridgeGeneration::new(NonZeroU64::new(7).expect("nonzero"));
    let mut manager = DndManager::default();
    let id = manager.begin(generation, 11, 100);
    assert!(manager.position(id, 12, 40, 50, XdndAction::Copy));
    assert_eq!(manager.position(id, 13, 60, 70, XdndAction::Move), true);
    assert_eq!(manager.drop(id), true);
    assert_eq!(manager.finish(id, true), true);
    assert!(!manager.finish(id, false));
    assert_eq!(manager.phase(id), Some(DndPhase::Finished));
}
