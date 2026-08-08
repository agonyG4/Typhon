use super::*;
use crate::compositor::WindowId;
use crate::compositor::state::{WindowActivationOutcome, WindowFocusOutcome, WindowFocusReason};
use std::num::NonZeroU64;

#[test]
fn xwayland_attachment_replacement_does_not_churn_window_focus_serial() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.parent_surface_id;
    snapshot.geometry.x = 37;
    snapshot.geometry.y = 42;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("admitted X11 window");
    let focus_generation_before = fixture.server.state.focus_generation;
    let focus_serial_before = fixture
        .server
        .state
        .window(window_id)
        .expect("admitted X11 window")
        .last_focus_serial;

    fixture
        .server
        .apply_xwayland_association_event(XwmAssociationEvent::Associated {
            generation: handle.generation(),
            window: handle,
            surface_id: fixture.popup_surface_id,
        });

    assert_eq!(fixture.server.state.focused_window_id, Some(window_id));
    assert_eq!(
        fixture.server.state.focus_generation,
        focus_generation_before
    );
    assert_eq!(
        fixture
            .server
            .state
            .window(window_id)
            .expect("admitted X11 window")
            .last_focus_serial,
        focus_serial_before
    );
}

#[test]
fn desktop_focus_outcomes_distinguish_change_no_change_and_unavailable() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let mut snapshot = fake_snapshot();
    snapshot.surface_id = fixture.parent_surface_id;
    let handle = snapshot.handle;
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(snapshot));
    let window_id = fixture
        .server
        .state
        .window_id_for_x11_handle(handle)
        .expect("admitted X11 window");

    fixture.server.state.focused_surface = None;
    fixture.server.state.focused_window_id = None;

    assert_eq!(
        fixture
            .server
            .state
            .focus_desktop_window(window_id, WindowFocusReason::PointerEnter),
        WindowFocusOutcome::Changed
    );
    assert_eq!(
        fixture
            .server
            .state
            .focus_desktop_window(window_id, WindowFocusReason::PointerEnter),
        WindowFocusOutcome::NoChange
    );
    assert_eq!(
        fixture.server.state.focus_desktop_window(
            WindowId::new(NonZeroU64::new(999_999).expect("nonzero window id")),
            WindowFocusReason::PointerEnter,
        ),
        WindowFocusOutcome::Unavailable
    );
    assert_eq!(
        fixture
            .server
            .state
            .activate_desktop_window(window_id, WindowFocusReason::PointerPress),
        WindowActivationOutcome::NoChange
    );
}
