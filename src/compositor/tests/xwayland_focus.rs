use super::*;
use crate::compositor::WindowId;
use crate::compositor::state::{WindowActivationOutcome, WindowFocusOutcome, WindowFocusReason};
use std::num::NonZeroU64;

fn admit_focus_pair(fixture: &mut super::StationaryPointerXwaylandFixture) -> (WindowId, WindowId) {
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).expect("generation"));
    let mut first = fake_snapshot();
    first.surface_id = fixture.parent_surface_id;
    first.geometry.x = 40;
    first.geometry.y = 40;
    let first_handle = first.handle;

    let mut second = fake_snapshot();
    second.handle = X11WindowHandle::new(generation, 101);
    second.surface_id = fixture.popup_surface_id;
    second.geometry.x = 240;
    second.geometry.y = 40;
    let second_handle = second.handle;

    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(first));
    fixture
        .server
        .apply_xwayland_window_event(XwmEvent::WindowReady(second));

    let first_id = fixture
        .server
        .state
        .window_id_for_x11_handle(first_handle)
        .expect("first managed window");
    let second_id = fixture
        .server
        .state
        .window_id_for_x11_handle(second_handle)
        .expect("second managed window");
    (first_id, second_id)
}

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

#[test]
fn m7_a_hundred_click_focus_raise_cycles_deliver_to_the_captured_window() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let (first_id, second_id) = admit_focus_pair(&mut fixture);
    let _ = fixture.server.take_xwayland_backend_commands(0);

    for cycle in 0..100 {
        let _ = fixture.server.state.raise_window_id(second_id);
        assert_eq!(
            fixture.server.state.window_stacking.last().copied(),
            Some(second_id),
            "cycle {cycle}"
        );
        let before_press = fixture.server.state.window_stacking.clone();
        let _ = fixture.server.take_xwayland_backend_commands(0);

        assert!(matches!(
            fixture
                .server
                .state
                .focus_desktop_window(second_id, WindowFocusReason::PointerEnter),
            WindowFocusOutcome::Changed | WindowFocusOutcome::NoChange
        ));
        assert_eq!(
            fixture
                .server
                .state
                .focus_desktop_window(first_id, WindowFocusReason::PointerEnter),
            WindowFocusOutcome::Changed
        );
        assert_eq!(fixture.server.state.window_stacking, before_press);
        assert_eq!(
            fixture
                .server
                .state
                .activate_desktop_window(first_id, WindowFocusReason::PointerPress),
            WindowActivationOutcome::Changed
        );

        assert_eq!(
            fixture.server.state.window_stacking.last().copied(),
            Some(first_id),
            "click activation must raise the captured window once, cycle {cycle}"
        );
        let restacks = fixture
            .server
            .take_xwayland_backend_commands(0)
            .into_iter()
            .filter(|command| matches!(command, XwmCommand::RestackExact { .. }))
            .count();
        assert_eq!(restacks, 1, "cycle {cycle}");
    }
}

#[test]
fn m7_a_hundred_hover_no_raise_cycles_preserve_focus_serial_policy() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let (first_id, second_id) = admit_focus_pair(&mut fixture);
    let _ = fixture.server.take_xwayland_backend_commands(0);
    let initial_stacking = fixture.server.state.window_stacking.clone();

    assert_eq!(
        fixture
            .server
            .state
            .focus_desktop_window(first_id, WindowFocusReason::PointerEnter),
        WindowFocusOutcome::Changed
    );
    for cycle in 0..100 {
        let first_focus_generation = fixture.server.state.focus_generation;
        assert_eq!(
            fixture
                .server
                .state
                .focus_desktop_window(first_id, WindowFocusReason::PointerEnter),
            WindowFocusOutcome::NoChange
        );
        assert_eq!(fixture.server.state.focused_window_id, Some(first_id));
        assert_eq!(
            fixture.server.state.focus_generation,
            first_focus_generation
        );
        assert_eq!(fixture.server.state.window_stacking, initial_stacking);

        assert_eq!(
            fixture
                .server
                .state
                .focus_desktop_window(second_id, WindowFocusReason::PointerEnter),
            WindowFocusOutcome::Changed
        );
        let second_focus_generation = fixture.server.state.focus_generation;
        assert_eq!(
            second_focus_generation,
            first_focus_generation + 1,
            "cycle {cycle}"
        );
        assert_eq!(fixture.server.state.focused_window_id, Some(second_id));
        assert_eq!(fixture.server.state.window_stacking, initial_stacking);

        assert_eq!(
            fixture
                .server
                .state
                .focus_desktop_window(second_id, WindowFocusReason::PointerEnter),
            WindowFocusOutcome::NoChange
        );
        assert_eq!(
            fixture.server.state.focus_generation,
            second_focus_generation
        );
        assert_eq!(fixture.server.state.window_stacking, initial_stacking);

        assert_eq!(
            fixture
                .server
                .state
                .focus_desktop_window(first_id, WindowFocusReason::PointerEnter),
            WindowFocusOutcome::Changed
        );
        assert_eq!(
            fixture.server.state.focus_generation,
            second_focus_generation + 1
        );
        assert_eq!(fixture.server.state.window_stacking, initial_stacking);
        let _ = fixture.server.take_xwayland_backend_commands(0);
    }
}

#[test]
fn m7_a_hundred_mixed_hover_and_click_cycles_keep_policy_outcomes_distinct() {
    let mut fixture = stationary_pointer_xwayland_fixture();
    let (first_id, second_id) = admit_focus_pair(&mut fixture);
    let _ = fixture.server.take_xwayland_backend_commands(0);

    for cycle in 0..100 {
        if cycle % 2 == 0 {
            let _ = fixture.server.state.raise_window_id(second_id);
            assert_eq!(
                fixture.server.state.window_stacking.last().copied(),
                Some(second_id)
            );
            let _ = fixture.server.take_xwayland_backend_commands(0);
            assert_eq!(
                fixture
                    .server
                    .state
                    .focus_desktop_window(first_id, WindowFocusReason::PointerEnter),
                WindowFocusOutcome::Changed
            );
            let before_click = fixture.server.state.window_stacking.clone();
            assert_eq!(
                fixture
                    .server
                    .state
                    .activate_desktop_window(first_id, WindowFocusReason::PointerPress),
                WindowActivationOutcome::Changed
            );
            assert_eq!(
                fixture.server.state.window_stacking.last().copied(),
                Some(first_id)
            );
            assert_ne!(fixture.server.state.window_stacking, before_click);
            let _ = fixture.server.take_xwayland_backend_commands(0);
        } else {
            let before_hover = fixture.server.state.window_stacking.clone();
            assert_eq!(
                fixture
                    .server
                    .state
                    .focus_desktop_window(second_id, WindowFocusReason::PointerEnter),
                WindowFocusOutcome::Changed
            );
            assert_eq!(
                fixture
                    .server
                    .state
                    .focus_desktop_window(second_id, WindowFocusReason::PointerEnter),
                WindowFocusOutcome::NoChange
            );
            assert_eq!(fixture.server.state.window_stacking, before_hover);
            let _ = fixture.server.take_xwayland_backend_commands(0);
        }
    }
}
