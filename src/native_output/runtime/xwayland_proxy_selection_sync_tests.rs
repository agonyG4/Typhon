use std::num::NonZeroU64;

use oblivion_one::xwayland::{
    XwaylandGeneration, XwaylandProxySelectionId, XwaylandProxySelectionOffer,
    XwaylandProxySelectionSnapshot, XwaylandSelectionKind,
};

use super::XwaylandProxySelectionSyncState;

fn generation(value: u64) -> XwaylandGeneration {
    XwaylandGeneration::new(NonZeroU64::new(value).unwrap())
}

fn clipboard_snapshot() -> XwaylandProxySelectionSnapshot {
    XwaylandProxySelectionSnapshot {
        kind: XwaylandSelectionKind::Clipboard,
        selection_generation: 7,
        offer: Some(XwaylandProxySelectionOffer {
            id: XwaylandProxySelectionId {
                kind: XwaylandSelectionKind::Clipboard,
                selection_generation: 7,
                source_key: oblivion_one::compositor::SelectionSourceKey(700),
            },
            mime_types: vec!["image/png".to_owned()],
        }),
    }
}

#[test]
fn unchanged_canonical_selection_is_replayed_to_a_restarted_xwayland_generation() {
    let mut sync = XwaylandProxySelectionSyncState::default();
    let clipboard = clipboard_snapshot();
    let initial = sync.snapshots_to_submit(generation(1), [clipboard.clone()]);
    assert_eq!(initial, [clipboard.clone()]);
    sync.mark_submitted(generation(1), &initial);
    assert!(
        sync.snapshots_to_submit(generation(1), [clipboard.clone()])
            .is_empty()
    );

    assert_eq!(
        sync.snapshots_to_submit(generation(2), [clipboard.clone()]),
        [clipboard]
    );
}
