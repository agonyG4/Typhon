use oblivion_one::xwayland::xwm::X11WindowLifecycle;

#[test]
fn lifecycle_exposes_map_and_render_boundaries() {
    let waiting = [
        X11WindowLifecycle::Observed,
        X11WindowLifecycle::MapRequested,
        X11WindowLifecycle::PropertiesPending,
        X11WindowLifecycle::MapCommanded,
        X11WindowLifecycle::MappedAwaitingAssociation,
        X11WindowLifecycle::AssociatedAwaitingBuffer,
    ];
    assert_eq!(waiting.len(), 6);
    assert_ne!(waiting[3], X11WindowLifecycle::Renderable);
}
