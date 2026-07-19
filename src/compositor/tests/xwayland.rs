use crate::xwayland::xwm::X11WindowLifecycle;

#[test]
fn xwayland_map_lifecycle_keeps_rendering_after_x11_map() {
    assert_ne!(
        X11WindowLifecycle::MapCommanded,
        X11WindowLifecycle::Renderable
    );
    assert_ne!(
        X11WindowLifecycle::MappedAwaitingAssociation,
        X11WindowLifecycle::Renderable
    );
}
