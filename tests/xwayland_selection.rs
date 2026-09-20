use std::num::NonZeroU64;

use oblivion_one::xwayland::xwm::data_bridge::{
    BridgeGeneration, SelectionKind, SelectionOrigin, selection::SelectionBridge,
};

#[test]
fn selection_foundation_keeps_clipboard_and_primary_generation_bound() {
    let generation = BridgeGeneration::new(NonZeroU64::new(1).expect("nonzero"));
    let mut bridge = SelectionBridge::default();
    bridge.initialize_generation(generation);
    bridge
        .observe_owner(
            generation,
            SelectionKind::Clipboard,
            Some(10),
            Some(SelectionOrigin::Wayland),
            1,
        )
        .expect("clipboard revision");
    bridge
        .observe_owner(
            generation,
            SelectionKind::Primary,
            Some(20),
            Some(SelectionOrigin::X11),
            2,
        )
        .expect("primary revision");
    assert_eq!(
        bridge.current(SelectionKind::Clipboard).unwrap().timestamp,
        1
    );
    assert_eq!(bridge.current(SelectionKind::Primary).unwrap().timestamp, 2);
    bridge.clear_generation(generation);
    assert!(bridge.current(SelectionKind::Clipboard).is_none());
}
