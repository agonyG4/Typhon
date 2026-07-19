use std::num::NonZeroU64;

use oblivion_one::xwayland::xwm::data_bridge::{
    BridgeGeneration, SelectionKind, SelectionOrigin, selection::SelectionBridge,
};

#[test]
fn clipboard_and_primary_are_separate_generation_bound_selections() {
    let generation = BridgeGeneration::new(NonZeroU64::new(1).expect("nonzero"));
    let mut bridge = SelectionBridge::default();
    bridge
        .replace(
            generation,
            SelectionKind::Clipboard,
            SelectionOrigin::Wayland,
            ["TARGETS".to_owned(), "UTF8_STRING".to_owned()],
            1,
        )
        .expect("clipboard");
    bridge
        .replace(
            generation,
            SelectionKind::Primary,
            SelectionOrigin::X11,
            ["STRING".to_owned()],
            2,
        )
        .expect("primary");
    assert_eq!(
        bridge.current(SelectionKind::Clipboard).unwrap().timestamp,
        1
    );
    assert_eq!(bridge.current(SelectionKind::Primary).unwrap().timestamp, 2);
    bridge.clear_generation(generation);
    assert!(bridge.current(SelectionKind::Clipboard).is_none());
}
