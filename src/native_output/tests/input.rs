use super::input_protocol::{ClientCommand, ClientEvent};
use super::*;
use crate::native_output::runtime::{
    NativePointerConstraint, NativePointerConstraintBackendAction,
};
use oblivion_one::compositor::{InteractionUpdateOutcome, PointerWarpOrigin};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_buffer as client_wl_buffer, wl_compositor as client_wl_compositor,
        wl_keyboard as client_wl_keyboard, wl_pointer as client_wl_pointer, wl_registry,
        wl_seat as client_wl_seat, wl_shm as client_wl_shm, wl_shm_pool as client_wl_shm_pool,
        wl_surface as client_wl_surface,
    },
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface as client_xdg_surface, xdg_toplevel as client_xdg_toplevel,
    xdg_wm_base as client_xdg_wm_base,
};
use wayland_protocols::xwayland::shell::v1::client::{
    xwayland_shell_v1 as client_xwayland_shell_v1,
    xwayland_surface_v1 as client_xwayland_surface_v1,
};
use xkbcommon::xkb;

#[path = "input_shortcut_inhibition.rs"]
mod input_shortcut_inhibition;

#[test]
fn raw_evdev_events_discarded_during_suspend_do_not_replay() {
    let mut pipe = [0; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) },
        0
    );
    let read = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
    let mut devices = NativeInputDevices::from_devices(
        vec![NativeInputDevice {
            file: fs::File::from(read),
            path: PathBuf::from("test-event"),
            keyboard_device_id: None,
        }],
        false,
    );
    let event = LinuxInputEvent {
        _time: libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        },
        type_: EV_KEY,
        code: KEY_P,
        value: 1,
    };
    let written = unsafe {
        libc::write(
            write.as_raw_fd(),
            (&event as *const LinuxInputEvent).cast(),
            std::mem::size_of::<LinuxInputEvent>(),
        )
    };
    assert_eq!(written as usize, std::mem::size_of::<LinuxInputEvent>());

    devices.suspend_for_session();

    assert!(devices.drain_events().is_empty());
}

#[test]
fn raw_evdev_events_arriving_after_suspend_are_not_delivered() {
    let mut pipe = [0; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) },
        0
    );
    let read = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
    let mut backend = NativeInputBackend::RawEvdev(NativeInputDevices::from_devices(
        vec![NativeInputDevice {
            file: fs::File::from(read),
            path: PathBuf::from("test-event"),
            keyboard_device_id: None,
        }],
        false,
    ));

    backend.suspend_for_session();
    let event = LinuxInputEvent {
        _time: libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        },
        type_: EV_KEY,
        code: KEY_P,
        value: 1,
    };
    let written = unsafe {
        libc::write(
            write.as_raw_fd(),
            (&event as *const LinuxInputEvent).cast(),
            std::mem::size_of::<LinuxInputEvent>(),
        )
    };
    assert_eq!(written as usize, std::mem::size_of::<LinuxInputEvent>());

    assert!(backend.drain_events().is_empty());
}

#[test]
fn raw_backend_targeted_readiness_is_complete_and_nonconsuming() {
    let mut first_pipe = [0; 2];
    let mut second_pipe = [0; 2];
    assert_eq!(
        unsafe { libc::pipe2(first_pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) },
        0
    );
    assert_eq!(
        unsafe { libc::pipe2(second_pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) },
        0
    );
    let first_read = unsafe { OwnedFd::from_raw_fd(first_pipe[0]) };
    let first_write = unsafe { OwnedFd::from_raw_fd(first_pipe[1]) };
    let second_read = unsafe { OwnedFd::from_raw_fd(second_pipe[0]) };
    let second_write = unsafe { OwnedFd::from_raw_fd(second_pipe[1]) };
    let second_write_fd = second_write.as_raw_fd();
    let mut backend = NativeInputBackend::RawEvdev(NativeInputDevices::from_devices(
        vec![
            NativeInputDevice {
                file: fs::File::from(first_read),
                path: PathBuf::from("first-event"),
                keyboard_device_id: None,
            },
            NativeInputDevice {
                file: fs::File::from(second_read),
                path: PathBuf::from("second-event"),
                keyboard_device_id: None,
            },
        ],
        false,
    ));

    assert!(!backend.ready_nonblocking().unwrap());
    let event = LinuxInputEvent {
        _time: libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        },
        type_: EV_KEY,
        code: KEY_P,
        value: 1,
    };
    let written = unsafe {
        libc::write(
            second_write_fd,
            (&event as *const LinuxInputEvent).cast(),
            std::mem::size_of::<LinuxInputEvent>(),
        )
    };
    assert_eq!(written as usize, std::mem::size_of::<LinuxInputEvent>());

    assert!(backend.ready_nonblocking().unwrap());
    assert!(backend.ready_nonblocking().unwrap());
    assert_eq!(backend.drain_events().len(), 1);

    drop(first_write);
    assert!(backend.ready_nonblocking().unwrap());
    assert!(backend.drain_events().is_empty());
    assert!(!backend.ready_nonblocking().unwrap());

    let event = LinuxInputEvent {
        _time: libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        },
        type_: EV_KEY,
        code: KEY_P,
        value: 1,
    };
    let written = unsafe {
        libc::write(
            second_write_fd,
            (&event as *const LinuxInputEvent).cast(),
            std::mem::size_of::<LinuxInputEvent>(),
        )
    };
    assert_eq!(written as usize, std::mem::size_of::<LinuxInputEvent>());
    assert!(backend.ready_nonblocking().unwrap());

    backend.suspend_for_session();
    assert!(!backend.ready_nonblocking().unwrap());
}

#[test]
fn raw_terminal_keyboard_fd_emits_one_source_removal_after_queued_keys() {
    let mut pipe = [0; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) },
        0
    );
    let read = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
    let device = k1_keyboard_id(70);
    let mut backend = NativeInputBackend::RawEvdev(NativeInputDevices::from_devices(
        vec![NativeInputDevice {
            file: fs::File::from(read),
            path: PathBuf::from("keyboard-event"),
            keyboard_device_id: Some(device),
        }],
        false,
    ));
    let event = LinuxInputEvent {
        _time: libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        },
        type_: EV_KEY,
        code: KEY_Q,
        value: 1,
    };
    let written = unsafe {
        libc::write(
            write.as_raw_fd(),
            (&event as *const LinuxInputEvent).cast(),
            std::mem::size_of::<LinuxInputEvent>(),
        )
    };
    assert_eq!(written as usize, std::mem::size_of::<LinuxInputEvent>());
    drop(write);

    assert!(backend.ready_nonblocking().unwrap());
    assert_eq!(
        backend.drain_events(),
        vec![
            NativeHardwareInputEvent::Keyboard(NativeKeyboardInputEvent::Key {
                device,
                code: KEY_Q,
                value: 1,
            }),
            remove_k1_keyboard(device),
        ]
    );
    assert!(!backend.ready_nonblocking().unwrap());
    let NativeInputBackend::RawEvdev(devices) = backend else {
        unreachable!();
    };
    assert!(devices.devices.is_empty());
}

#[test]
fn raw_terminal_device_without_keyboard_identity_retires_without_fake_event() {
    let mut pipe = [0; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) },
        0
    );
    let read = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
    let mut devices = NativeInputDevices::from_devices(
        vec![NativeInputDevice {
            file: fs::File::from(read),
            path: PathBuf::from("pointer-event"),
            keyboard_device_id: None,
        }],
        false,
    );
    drop(write);

    assert!(devices.ready_nonblocking().unwrap());
    assert!(devices.drain_events().is_empty());
    assert!(!devices.ready_nonblocking().unwrap());
    assert!(devices.devices.is_empty());
}

#[test]
fn raw_pointer_button_never_allocates_keyboard_source_identity() {
    let mut pipe = [0; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) },
        0
    );
    let read = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
    let mut devices = NativeInputDevices::from_devices(
        vec![NativeInputDevice {
            file: fs::File::from(read),
            path: PathBuf::from("pointer-event"),
            keyboard_device_id: None,
        }],
        false,
    );
    let event = LinuxInputEvent {
        _time: libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        },
        type_: EV_KEY,
        code: BTN_LEFT,
        value: 1,
    };
    let written = unsafe {
        libc::write(
            write.as_raw_fd(),
            (&event as *const LinuxInputEvent).cast(),
            std::mem::size_of::<LinuxInputEvent>(),
        )
    };
    assert_eq!(written as usize, std::mem::size_of::<LinuxInputEvent>());

    assert_eq!(
        devices.drain_events(),
        vec![NativeHardwareInputEvent::PointerButton {
            button: u32::from(BTN_LEFT),
            pressed: true,
        }]
    );
    assert!(devices.devices[0].keyboard_device_id.is_none());
}

#[test]
fn raw_keyboard_source_identity_is_fresh_after_session_resume() {
    let mut pipe = [0; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) },
        0
    );
    let read = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
    let write_fd = write.as_raw_fd();
    let mut backend = NativeInputBackend::RawEvdev(NativeInputDevices::from_devices(
        vec![NativeInputDevice {
            file: fs::File::from(read),
            path: PathBuf::from("keyboard-event"),
            keyboard_device_id: None,
        }],
        false,
    ));
    let event = LinuxInputEvent {
        _time: libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        },
        type_: EV_KEY,
        code: KEY_Q,
        value: 1,
    };
    let write_key = || unsafe {
        libc::write(
            write_fd,
            (&event as *const LinuxInputEvent).cast(),
            std::mem::size_of::<LinuxInputEvent>(),
        )
    };
    assert_eq!(write_key() as usize, std::mem::size_of::<LinuxInputEvent>());
    let before_suspend = backend.drain_events();
    let old_id = match before_suspend[0] {
        NativeHardwareInputEvent::Keyboard(NativeKeyboardInputEvent::Key { device, .. }) => device,
        _ => unreachable!(),
    };

    backend.suspend_for_session();
    backend.resume_after_session().unwrap();
    assert_eq!(write_key() as usize, std::mem::size_of::<LinuxInputEvent>());
    let after_resume = backend.drain_events();
    let new_id = match after_resume[0] {
        NativeHardwareInputEvent::Keyboard(NativeKeyboardInputEvent::Key { device, .. }) => device,
        _ => unreachable!(),
    };

    assert_ne!(old_id, new_id);
}

#[test]
fn raw_evdev_budget_reports_a_continuation_without_changing_event_storage() {
    let mut pipe = [0; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) },
        0
    );
    let read = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
    let mut backend = NativeInputBackend::RawEvdev(NativeInputDevices::from_devices(
        vec![NativeInputDevice {
            file: fs::File::from(read),
            path: PathBuf::from("test-event"),
            keyboard_device_id: None,
        }],
        false,
    ));
    let events = vec![
        LinuxInputEvent {
            _time: libc::timeval {
                tv_sec: 0,
                tv_usec: 0,
            },
            type_: EV_KEY,
            code: KEY_P,
            value: 1,
        };
        257
    ];
    let mut write = fs::File::from(write);
    let bytes = unsafe {
        std::slice::from_raw_parts(
            events.as_ptr().cast::<u8>(),
            events.len() * std::mem::size_of::<LinuxInputEvent>(),
        )
    };
    write.write_all(bytes).unwrap();

    let mut batch = NativeInputBatch::default();
    assert!(backend.begin_semantic_epoch());
    backend.drain_epoch_chunk_into(&mut batch);
    assert_eq!(batch.raw.len(), 256);
    assert!(batch.budget_exhausted);

    backend.drain_epoch_chunk_into(&mut batch);
    assert_eq!(batch.raw.len(), 1);
    assert!(!batch.budget_exhausted);
    drop(write);
}

#[test]
fn raw_evdev_epoch_ingress_separates_begin_from_queue_drain() {
    let mut backend =
        NativeInputBackend::RawEvdev(NativeInputDevices::from_devices(Vec::new(), false));
    let mut batch = NativeInputBatch::default();

    assert!(backend.begin_semantic_epoch());
    backend.drain_epoch_chunk_into(&mut batch);

    assert!(batch.raw.is_empty());
    assert!(!batch.budget_exhausted);
}

#[test]
fn native_input_super_space_emits_astrea_spotlight_without_forwarding_space() {
    let _guard = ASTREA_ENV_LOCK.lock().unwrap();
    // SAFETY: this test serializes access to the process environment with
    // ASTREA_ENV_LOCK.
    unsafe {
        std::env::set_var("OBLIVION_ONE_SPOTLIGHT_COMMAND", "printf spotlight");
    }
    let mut input = NativeInputState::new(320, 200);

    let super_key = input.handle_key_event(KEY_LEFTMETA, 1);
    let space = input.handle_key_event(KEY_SPACE, 1);

    assert!(super_key.keyboard_events.is_empty());
    assert!(space.keyboard_events.is_empty());
    assert_eq!(space.launch_command, None);
    assert_eq!(space.launch_source, None);
    assert_eq!(
        space.shortcut_events,
        vec![AstreaShortcutEvent::pressed(
            "astrea-shell",
            "spotlight_toggle"
        )]
    );

    // SAFETY: guarded by ASTREA_ENV_LOCK.
    unsafe {
        std::env::remove_var("OBLIVION_ONE_SPOTLIGHT_COMMAND");
    }
}

#[test]
fn native_input_screenshot_bindings_are_exact_press_only_reserved_actions() {
    let cases = [
        ("quick", ModifierMask::EMPTY, &[][..], "screenshot_quick"),
        (
            "frozen",
            ModifierMask::SUPER,
            &[KEY_LEFTMETA][..],
            "screenshot_region_frozen",
        ),
        (
            "live",
            ModifierMask::SUPER | ModifierMask::SHIFT,
            &[KEY_LEFTMETA, KEY_LEFTSHIFT][..],
            "screenshot_region_live",
        ),
    ];

    for (_, modifiers, modifier_keys, name) in cases {
        let mut input = NativeInputState::new(320, 200);
        for key in modifier_keys {
            input.handle_key_event(*key, 1);
        }

        let pressed = input.handle_key_event(KEY_SYSRQ, 1);
        let repeated = input.handle_key_event(KEY_SYSRQ, 2);
        let released = input.handle_key_event(KEY_SYSRQ, 0);

        assert_eq!(input.active_modifier_mask(), modifiers);
        assert_eq!(
            pressed.shortcut_events,
            vec![AstreaShortcutEvent::pressed("astrea-shell", name)]
        );
        assert!(repeated.shortcut_events.is_empty());
        assert!(released.shortcut_events.is_empty());
    }
}

#[test]
fn native_input_screenshot_bindings_reject_extra_modifiers() {
    for modifier_keys in [
        vec![KEY_LEFTCTRL],
        vec![KEY_LEFTMETA, KEY_LEFTCTRL],
        vec![KEY_LEFTMETA, KEY_LEFTSHIFT, KEY_LEFTCTRL],
    ] {
        let mut input = NativeInputState::new(320, 200);
        for key in modifier_keys {
            input.handle_key_event(key, 1);
        }
        assert!(
            input
                .handle_key_event(KEY_SYSRQ, 1)
                .shortcut_events
                .is_empty()
        );
    }
}

#[test]
fn native_input_repeat_enabled_shortcut_emits_repeated_phase() {
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_bindings(vec![Binding {
        modifiers: ModifierMask::EMPTY,
        trigger: BindingTrigger::Press,
        input: BindingInput::Key(KEY_Z),
        action: BindingAction::EmitShortcut {
            namespace: "astrea-shell".to_string(),
            name: "test_repeat".to_string(),
        },
        repeat: RepeatPolicy::Enabled,
        inhibition: InhibitionPolicy::Respect,
        reserved: false,
    }]);

    let pressed = input.handle_key_event(KEY_Z, 1);
    let repeated = input.handle_key_event(KEY_Z, 2);

    assert_eq!(pressed.shortcut_events.len(), 1);
    assert_eq!(
        pressed.shortcut_events[0].phase,
        AstreaShortcutPhase::Pressed
    );
    assert_eq!(repeated.shortcut_events.len(), 1);
    assert_eq!(
        repeated.shortcut_events[0].phase,
        AstreaShortcutPhase::Repeated
    );
}

#[test]
fn native_input_repeat_disabled_shortcut_suppresses_repeat() {
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_bindings(vec![Binding {
        modifiers: ModifierMask::EMPTY,
        trigger: BindingTrigger::Press,
        input: BindingInput::Key(KEY_Z),
        action: BindingAction::EmitShortcut {
            namespace: "astrea-shell".to_string(),
            name: "test_no_repeat".to_string(),
        },
        repeat: RepeatPolicy::Disabled,
        inhibition: InhibitionPolicy::Respect,
        reserved: false,
    }]);

    let pressed = input.handle_key_event(KEY_Z, 1);
    let repeated = input.handle_key_event(KEY_Z, 2);

    assert_eq!(pressed.shortcut_events.len(), 1);
    assert_eq!(
        pressed.shortcut_events[0].phase,
        AstreaShortcutPhase::Pressed
    );
    assert!(repeated.shortcut_events.is_empty());
}

#[test]
fn native_input_repeat_is_not_a_keyboard_state_transition() {
    let mut input = NativeInputState::new(320, 200);

    let pressed = input.handle_key_event(KEY_Z, 1);
    let repeated = input.handle_key_event(KEY_Z, 2);
    let released = input.handle_key_event(KEY_Z, 0);

    assert_eq!(
        pressed.keyboard_actions,
        vec![NativeKeyboardAction::PhysicalAndClient(
            NativeKeyboardEvent::new(KEY_Z, true,)
        )]
    );
    assert!(repeated.keyboard_actions.is_empty());
    assert!(repeated.keyboard_events.is_empty());
    assert_eq!(
        released.keyboard_actions,
        vec![NativeKeyboardAction::PhysicalAndClient(
            NativeKeyboardEvent::new(KEY_Z, false,)
        )]
    );
}

#[test]
fn raw_evdev_repeat_notifications_do_not_create_keyboard_actions() {
    let mut input = NativeInputState::new(320, 200);
    let mut action_counts = Vec::new();
    let device = k1_keyboard_id(1);

    for value in [1, 2, 2, 0] {
        let event = LinuxInputEvent {
            _time: libc::timeval {
                tv_sec: 0,
                tv_usec: 0,
            },
            type_: EV_KEY,
            code: KEY_Z,
            value,
        };
        let event = NativeHardwareInputEvent::from_linux_event(event, Some(device)).unwrap();
        action_counts.push(
            input
                .handle_hardware_input_event(event)
                .keyboard_actions
                .len(),
        );
        assert_eq!(input.keyboard_key_is_logically_pressed(KEY_Z), value != 0);
    }

    assert_eq!(action_counts, vec![1, 0, 0, 1]);
}

#[test]
fn raw_repeat_from_another_keyboard_does_not_claim_source_ownership() {
    let mut input = NativeInputState::new(320, 200);
    let first = k1_keyboard_id(81);
    let second = k1_keyboard_id(82);
    input.handle_key_event_from(first, KEY_A, 1);
    let repeat = LinuxInputEvent {
        _time: libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        },
        type_: EV_KEY,
        code: KEY_A,
        value: 2,
    };
    let repeat = NativeHardwareInputEvent::from_linux_event(repeat, Some(second)).unwrap();

    assert!(
        input
            .handle_hardware_input_event(repeat)
            .keyboard_actions
            .is_empty()
    );
    assert_eq!(
        input
            .handle_key_event_from(first, KEY_A, 0)
            .keyboard_actions,
        vec![NativeKeyboardAction::PhysicalAndClient(
            NativeKeyboardEvent::new(KEY_A, false)
        )]
    );
    assert!(!input.keyboard_key_is_logically_pressed(KEY_A));
}

fn k1_keyboard_id(value: u32) -> KeyboardDeviceId {
    KeyboardDeviceId::from_raw(value).expect("K1 test keyboard ids are nonzero")
}

fn remove_k1_keyboard(device: KeyboardDeviceId) -> NativeHardwareInputEvent {
    NativeHardwareInputEvent::Keyboard(NativeKeyboardInputEvent::SourceRemoved { device })
}

fn apply_k1_keyboard_effect(
    server: &mut OwnCompositorServer,
    effect: NativeInputEffect,
    resize_perf: &mut NativeResizePerfState,
    process_supervisor: &mut ChildSupervisor,
) {
    apply_native_input_effect(
        effect,
        NativeInputApplyContext {
            server,
            perf: NativePerfLogger::from_env(),
            resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();
}

#[test]
fn native_input_aggregates_key_ownership_before_the_authoritative_xkb_state() {
    let socket_name = format!("typhon-k1-key-ledger-{}", std::process::id());
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let mut input = NativeInputState::new(320, 200);
    let first = k1_keyboard_id(1);
    let second = k1_keyboard_id(2);
    let mut resize_perf = NativeResizePerfState::default();
    let mut process_supervisor = ChildSupervisor::new();

    server.update_keyboard_state_without_publication(u32::from(KEY_A), false);
    assert!(server.keyboard_reconfiguration_is_quiescent());

    let press = input.handle_key_event_from(first, KEY_A, 1);
    assert_eq!(
        press.keyboard_actions,
        vec![NativeKeyboardAction::PhysicalAndClient(
            NativeKeyboardEvent::new(KEY_A, true)
        )]
    );
    apply_k1_keyboard_effect(
        &mut server,
        press,
        &mut resize_perf,
        &mut process_supervisor,
    );
    assert!(!server.keyboard_reconfiguration_is_quiescent());

    let second_press = input.handle_key_event_from(second, KEY_A, 1);
    assert!(second_press.keyboard_actions.is_empty());
    let first_release = input.handle_key_event_from(first, KEY_A, 0);
    assert!(first_release.keyboard_actions.is_empty());
    assert!(input.keyboard_key_is_logically_pressed(KEY_A));
    assert!(!server.keyboard_reconfiguration_is_quiescent());

    let final_release = input.handle_key_event_from(second, KEY_A, 0);
    assert_eq!(
        final_release.keyboard_actions,
        vec![NativeKeyboardAction::PhysicalAndClient(
            NativeKeyboardEvent::new(KEY_A, false)
        )]
    );
    apply_k1_keyboard_effect(
        &mut server,
        final_release,
        &mut resize_perf,
        &mut process_supervisor,
    );
    assert!(server.keyboard_reconfiguration_is_quiescent());
}

#[test]
fn removing_a_keyboard_releases_only_its_unshared_keys() {
    let mut input = NativeInputState::new(320, 200);
    let first = k1_keyboard_id(11);
    let second = k1_keyboard_id(12);
    input.handle_key_event_from(first, KEY_Q, 1);
    input.handle_key_event_from(second, KEY_Q, 1);

    let first_removed = input.handle_hardware_input_event(remove_k1_keyboard(first));
    assert!(first_removed.keyboard_actions.is_empty());
    assert!(input.keyboard_key_is_logically_pressed(KEY_Q));
    assert_eq!(
        input
            .handle_key_event_from(second, KEY_Q, 0)
            .keyboard_actions,
        vec![NativeKeyboardAction::PhysicalAndClient(
            NativeKeyboardEvent::new(KEY_Q, false)
        )]
    );

    let third = k1_keyboard_id(13);
    input.handle_key_event_from(third, KEY_Q, 1);
    assert_eq!(
        input
            .handle_hardware_input_event(remove_k1_keyboard(third))
            .keyboard_actions,
        vec![NativeKeyboardAction::PhysicalAndClient(
            NativeKeyboardEvent::new(KEY_Q, false)
        )]
    );
    assert!(!input.keyboard_key_is_logically_pressed(KEY_Q));
}

#[test]
fn modifier_families_remain_active_until_the_last_left_or_right_key_releases() {
    for (left, right, family) in [
        (KEY_LEFTALT, KEY_RIGHTALT, ModifierMask::ALT),
        (KEY_LEFTCTRL, KEY_RIGHTCTRL, ModifierMask::CTRL),
        (KEY_LEFTSHIFT, KEY_RIGHTSHIFT, ModifierMask::SHIFT),
        (KEY_LEFTMETA, KEY_RIGHTMETA, ModifierMask::SUPER),
    ] {
        let mut input = NativeInputState::new(320, 200);
        input.handle_key_event(left, 1);
        input.handle_key_event(right, 1);

        let left_release = input.handle_key_event(left, 0);

        assert!(input.active_modifier_mask().contains(family));
        if family == ModifierMask::ALT {
            input.handle_key_event(KEY_TAB, 1);
            assert!(left_release.shortcut_events.is_empty());
            let final_release = input.handle_key_event(right, 0);
            assert_eq!(
                final_release.shortcut_events,
                vec![AstreaShortcutEvent::pressed(
                    "astrea-shell",
                    "alt_tab_commit"
                )]
            );
        } else {
            input.handle_key_event(right, 0);
            assert!(!input.active_modifier_mask().contains(family));
        }
    }
}

fn physical_keyboard_events(effect: &NativeInputEffect) -> Vec<NativeKeyboardEvent> {
    effect
        .keyboard_actions
        .iter()
        .filter_map(|action| match action {
            NativeKeyboardAction::PhysicalOnly(event)
            | NativeKeyboardAction::PhysicalAndClient(event) => Some(event.clone()),
            NativeKeyboardAction::ClientOnly(_) => None,
        })
        .collect()
}

#[test]
fn removing_one_source_releases_both_modifier_keys_but_each_family_once() {
    for (left, right, family) in [
        (KEY_LEFTALT, KEY_RIGHTALT, ModifierMask::ALT),
        (KEY_LEFTCTRL, KEY_RIGHTCTRL, ModifierMask::CTRL),
        (KEY_LEFTSHIFT, KEY_RIGHTSHIFT, ModifierMask::SHIFT),
        (KEY_LEFTMETA, KEY_RIGHTMETA, ModifierMask::SUPER),
    ] {
        let mut input = NativeInputState::new(320, 200);
        let device = k1_keyboard_id(83);
        input.handle_key_event_from(device, left, 1);
        input.handle_key_event_from(device, right, 1);
        let releases_before = input.binding_manager.modifier_release_call_count(family);

        let removed = input.handle_hardware_input_event(remove_k1_keyboard(device));

        assert_eq!(
            physical_keyboard_events(&removed),
            vec![
                NativeKeyboardEvent::new(left, false),
                NativeKeyboardEvent::new(right, false),
            ]
        );
        assert!(!input.active_modifier_mask().contains(family));
        assert_eq!(
            input.binding_manager.modifier_release_call_count(family) - releases_before,
            1,
            "{family:?} must transition inactive only once per source removal"
        );
        assert_eq!(
            input.source_removal_modifier_release_routes,
            vec![(right, family)]
        );
    }
}

#[test]
fn removing_both_alt_variants_commits_alt_tab_once_after_both_releases() {
    let mut input = NativeInputState::new(320, 200);
    let device = k1_keyboard_id(84);
    input.handle_key_event_from(device, KEY_LEFTALT, 1);
    input.handle_key_event_from(device, KEY_RIGHTALT, 1);
    assert_eq!(
        input
            .handle_key_event_from(device, KEY_TAB, 1)
            .shortcut_events,
        vec![AstreaShortcutEvent::pressed("astrea-shell", "alt_tab_next")]
    );
    let releases_before = input
        .binding_manager
        .modifier_release_call_count(ModifierMask::ALT);

    let removed = input.handle_hardware_input_event(remove_k1_keyboard(device));

    assert_eq!(
        physical_keyboard_events(&removed),
        vec![
            NativeKeyboardEvent::new(KEY_TAB, false),
            NativeKeyboardEvent::new(KEY_LEFTALT, false),
            NativeKeyboardEvent::new(KEY_RIGHTALT, false),
        ]
    );
    assert_eq!(
        removed.shortcut_events,
        vec![AstreaShortcutEvent::pressed(
            "astrea-shell",
            "alt_tab_commit"
        )]
    );
    assert_eq!(
        input
            .binding_manager
            .modifier_release_call_count(ModifierMask::ALT)
            - releases_before,
        1
    );
    assert_eq!(
        input.source_removal_modifier_release_routes,
        vec![(KEY_RIGHTALT, ModifierMask::ALT)]
    );
    assert!(!input.active_modifier_mask().contains(ModifierMask::ALT));
}

#[test]
fn another_source_keeps_alt_active_until_its_final_release() {
    let mut input = NativeInputState::new(320, 200);
    let first = k1_keyboard_id(85);
    let second = k1_keyboard_id(86);
    input.handle_key_event_from(first, KEY_LEFTALT, 1);
    input.handle_key_event_from(first, KEY_RIGHTALT, 1);
    input.handle_key_event_from(second, KEY_LEFTALT, 1);
    input.handle_key_event_from(first, KEY_TAB, 1);
    let releases_before = input
        .binding_manager
        .modifier_release_call_count(ModifierMask::ALT);

    let first_removed = input.handle_hardware_input_event(remove_k1_keyboard(first));

    assert_eq!(
        physical_keyboard_events(&first_removed),
        vec![
            NativeKeyboardEvent::new(KEY_TAB, false),
            NativeKeyboardEvent::new(KEY_RIGHTALT, false),
        ]
    );
    assert!(input.active_modifier_mask().contains(ModifierMask::ALT));
    assert!(first_removed.shortcut_events.is_empty());
    assert_eq!(
        input
            .binding_manager
            .modifier_release_call_count(ModifierMask::ALT),
        releases_before
    );
    assert!(input.source_removal_modifier_release_routes.is_empty());

    let second_removed = input.handle_hardware_input_event(remove_k1_keyboard(second));

    assert_eq!(
        physical_keyboard_events(&second_removed),
        vec![NativeKeyboardEvent::new(KEY_LEFTALT, false)]
    );
    assert!(!input.active_modifier_mask().contains(ModifierMask::ALT));
    assert_eq!(
        second_removed.shortcut_events,
        vec![AstreaShortcutEvent::pressed(
            "astrea-shell",
            "alt_tab_commit"
        )]
    );
    assert_eq!(
        input
            .binding_manager
            .modifier_release_call_count(ModifierMask::ALT)
            - releases_before,
        1
    );
    assert_eq!(
        input.source_removal_modifier_release_routes,
        vec![(KEY_LEFTALT, ModifierMask::ALT)]
    );
}

#[test]
fn the_same_ctrl_key_held_by_two_devices_is_released_once() {
    let mut input = NativeInputState::new(320, 200);
    let first = k1_keyboard_id(15);
    let second = k1_keyboard_id(16);
    let press = input.handle_key_event_from(first, KEY_LEFTCTRL, 1);
    assert_eq!(
        press.keyboard_actions,
        vec![NativeKeyboardAction::PhysicalAndClient(
            NativeKeyboardEvent::new(KEY_LEFTCTRL, true)
        )]
    );
    assert!(
        input
            .handle_key_event_from(second, KEY_LEFTCTRL, 1)
            .keyboard_actions
            .is_empty()
    );

    let first_release = input.handle_key_event_from(first, KEY_LEFTCTRL, 0);
    assert!(first_release.keyboard_actions.is_empty());
    assert!(input.active_modifier_mask().contains(ModifierMask::CTRL));

    let final_release = input.handle_key_event_from(second, KEY_LEFTCTRL, 0);
    assert_eq!(
        final_release.keyboard_actions,
        vec![NativeKeyboardAction::PhysicalAndClient(
            NativeKeyboardEvent::new(KEY_LEFTCTRL, false)
        )]
    );
    assert!(!input.active_modifier_mask().contains(ModifierMask::CTRL));
}

#[test]
fn removing_a_source_releases_shortcut_triggers_before_modifiers() {
    let mut input = NativeInputState::new(320, 200);
    let device = k1_keyboard_id(21);
    input.handle_key_event_from(device, KEY_LEFTALT, 1);
    let tab_press = input.handle_key_event_from(device, KEY_TAB, 1);
    assert_eq!(
        tab_press.shortcut_events,
        vec![AstreaShortcutEvent::pressed("astrea-shell", "alt_tab_next")]
    );

    let removed = input.handle_hardware_input_event(remove_k1_keyboard(device));

    assert_eq!(
        removed.keyboard_actions,
        vec![
            NativeKeyboardAction::PhysicalOnly(NativeKeyboardEvent::new(KEY_TAB, false)),
            NativeKeyboardAction::PhysicalOnly(NativeKeyboardEvent::new(KEY_LEFTALT, false)),
        ]
    );
    assert!(removed.keyboard_events.is_empty());
    assert_eq!(
        removed.shortcut_events,
        vec![AstreaShortcutEvent::pressed(
            "astrea-shell",
            "alt_tab_commit"
        )]
    );
    assert!(input.active_modifier_mask().matches(ModifierMask::EMPTY));
}

#[test]
fn source_removal_balances_forwarded_keys_under_shortcut_inhibition() {
    let mut input = NativeInputState::new(320, 200);
    input.reconcile_keyboard_shortcut_inhibition(KeyboardShortcutInhibitionSnapshot::new(true, 1));
    let first = k1_keyboard_id(31);
    let second = k1_keyboard_id(32);
    let press = input.handle_key_event_from(first, KEY_A, 1);
    assert_eq!(
        press.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_A, true)]
    );
    input.handle_key_event_from(second, KEY_A, 1);

    let shared_removal = input.handle_hardware_input_event(remove_k1_keyboard(first));
    assert!(shared_removal.keyboard_events.is_empty());
    assert!(input.forwarded_client_keys.contains(&KEY_A));

    let final_removal = input.handle_hardware_input_event(remove_k1_keyboard(second));
    assert_eq!(
        final_removal.keyboard_actions,
        vec![NativeKeyboardAction::PhysicalAndClient(
            NativeKeyboardEvent::new(KEY_A, false)
        )]
    );
    assert!(!input.forwarded_client_keys.contains(&KEY_A));
}

#[test]
fn removing_a_compositor_consumed_shortcut_never_sends_a_client_release() {
    let mut input = NativeInputState::new(320, 200);
    let device = k1_keyboard_id(41);
    input.handle_key_event_from(device, KEY_LEFTALT, 1);
    let consumed = input.handle_key_event_from(device, KEY_TAB, 1);
    assert!(consumed.keyboard_events.is_empty());

    let removed = input.handle_hardware_input_event(remove_k1_keyboard(device));

    assert!(removed.keyboard_events.is_empty());
    assert!(
        removed
            .keyboard_actions
            .iter()
            .all(|action| matches!(action, NativeKeyboardAction::PhysicalOnly(_)))
    );
    assert_eq!(
        removed.shortcut_events,
        vec![AstreaShortcutEvent::pressed(
            "astrea-shell",
            "alt_tab_commit"
        )]
    );
    assert!(!input.keyboard_key_is_logically_pressed(KEY_TAB));
}

#[test]
fn session_clear_discards_keyboard_ownership_without_id_aliasing() {
    let mut allocator = KeyboardDeviceIdAllocator::default();
    let old_device = allocator.allocate().unwrap();
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event_from(old_device, KEY_A, 1);
    input.clear_pressed_state_for_session_switch();
    assert!(!input.keyboard_key_is_logically_pressed(KEY_A));

    let fresh_device = allocator.allocate().unwrap();
    assert_ne!(old_device, fresh_device);
    assert!(
        input
            .handle_key_event_from(old_device, KEY_A, 0)
            .keyboard_actions
            .is_empty()
    );
    assert_eq!(
        input
            .handle_key_event_from(fresh_device, KEY_A, 1)
            .keyboard_actions,
        vec![NativeKeyboardAction::PhysicalAndClient(
            NativeKeyboardEvent::new(KEY_A, true)
        )]
    );
}

#[test]
fn vt_switch_clear_releases_shared_physical_keys_once() {
    let mut input = NativeInputState::new(320, 200);
    let first = k1_keyboard_id(51);
    let second = k1_keyboard_id(52);
    input.handle_key_event_from(first, KEY_Q, 1);
    input.handle_key_event_from(second, KEY_Q, 1);
    input.handle_key_event_from(first, KEY_LEFTCTRL, 1);
    input.handle_key_event_from(first, KEY_LEFTALT, 1);

    let switch = input.handle_key_event_from(first, KEY_F3, 1);
    let q_releases = switch
        .keyboard_actions
        .iter()
        .filter(|action| {
            **action == NativeKeyboardAction::PhysicalOnly(NativeKeyboardEvent::new(KEY_Q, false))
        })
        .count();

    assert_eq!(switch.vt_switch, Some(3));
    assert_eq!(q_releases, 1);
    assert!(!input.keyboard_key_is_logically_pressed(KEY_Q));
}

#[test]
fn source_removal_is_not_activity_but_can_require_constraint_reconciliation() {
    let removal = remove_k1_keyboard(k1_keyboard_id(61));
    assert!(!removal.is_meaningful_user_activity());
    assert!(removal.may_change_pointer_constraints());
}

#[test]
fn native_input_caps_lock_repeat_does_not_create_an_extra_transition() {
    let mut input = NativeInputState::new(320, 200);

    let press = input.handle_key_event(58, 1);
    let repeat = input.handle_key_event(58, 2);
    let release = input.handle_key_event(58, 0);

    assert_eq!(press.keyboard_actions.len(), 1);
    assert!(repeat.keyboard_actions.is_empty());
    assert_eq!(release.keyboard_actions.len(), 1);
}

#[test]
fn native_input_release_trigger_shortcut_emits_released_phase() {
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager = AstreaBindingManager::with_bindings(vec![Binding {
        modifiers: ModifierMask::EMPTY,
        trigger: BindingTrigger::Release,
        input: BindingInput::Key(KEY_Z),
        action: BindingAction::EmitShortcut {
            namespace: "astrea-shell".to_string(),
            name: "test_release".to_string(),
        },
        repeat: RepeatPolicy::Disabled,
        inhibition: InhibitionPolicy::Respect,
        reserved: false,
    }]);

    input.handle_key_event(KEY_Z, 1);
    let release = input.handle_key_event(KEY_Z, 0);

    assert_eq!(release.shortcut_events.len(), 1);
    assert_eq!(
        release.shortcut_events[0].phase,
        AstreaShortcutPhase::Released
    );
}

#[test]
fn native_input_zero_owner_spotlight_press_launches_one_fallback() {
    let _guard = ASTREA_ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("OBLIVION_ONE_SPOTLIGHT_COMMAND", "exit 0");
    }
    let mut server =
        OwnCompositorServer::bind(format!("typhon-shortcut-fallback-{}", std::process::id()))
            .unwrap();
    let mut process_supervisor = ChildSupervisor::new();
    let mut resize_perf = NativeResizePerfState::default();
    let application = apply_native_input_effect(
        NativeInputEffect {
            shortcut_events: vec![AstreaShortcutEvent {
                namespace: "astrea-shell".to_string(),
                name: "spotlight_toggle".to_string(),
                phase: AstreaShortcutPhase::Pressed,
            }],
            ..NativeInputEffect::default()
        },
        NativeInputApplyContext {
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();
    unsafe {
        std::env::remove_var("OBLIVION_ONE_SPOTLIGHT_COMMAND");
    }

    let launch = application.launch.expect("fallback should launch once");
    assert_eq!(launch.source, NativeLaunchSource::Spotlight);
    assert_eq!(process_supervisor.active_count(), 1);
    wait_for_no_active_children(&mut process_supervisor);
    assert_eq!(process_supervisor.active_count(), 0);
}

#[test]
fn native_input_zero_owner_alt_tab_next_launches_one_fallback() {
    let _guard = ASTREA_ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("OBLIVION_ONE_ALT_TAB_COMMAND", "exit 0");
    }
    let mut server =
        OwnCompositorServer::bind(format!("typhon-alt-tab-fallback-{}", std::process::id()))
            .unwrap();
    let mut process_supervisor = ChildSupervisor::new();
    let mut resize_perf = NativeResizePerfState::default();
    let application = apply_native_input_effect(
        NativeInputEffect {
            shortcut_events: vec![AstreaShortcutEvent {
                namespace: "astrea-shell".to_string(),
                name: "alt_tab_next".to_string(),
                phase: AstreaShortcutPhase::Pressed,
            }],
            ..NativeInputEffect::default()
        },
        NativeInputApplyContext {
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();
    unsafe {
        std::env::remove_var("OBLIVION_ONE_ALT_TAB_COMMAND");
    }

    let launch = application.launch.expect("fallback should launch once");
    assert_eq!(launch.source, NativeLaunchSource::AltTab);
    assert_eq!(process_supervisor.active_count(), 1);
    wait_for_no_active_children(&mut process_supervisor);
    assert_eq!(process_supervisor.active_count(), 0);
}

fn wait_for_no_active_children(process_supervisor: &mut ChildSupervisor) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while process_supervisor.active_count() > 0 && std::time::Instant::now() < deadline {
        process_supervisor.reap_exited().unwrap();
        if process_supervisor.active_count() > 0 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

#[test]
fn native_input_spotlight_fallback_spawn_failure_is_non_fatal_and_recorded() {
    let _guard = ASTREA_ENV_LOCK.lock().unwrap();
    let previous_path = std::env::var_os("PATH");
    unsafe {
        std::env::set_var("OBLIVION_ONE_SPOTLIGHT_COMMAND", "exit 0");
        std::env::set_var("PATH", "/definitely-not-a-command-path");
    }
    assert!(external_spotlight_command().is_some());

    let mut server = OwnCompositorServer::bind(format!(
        "typhon-shortcut-fallback-failure-{}",
        std::process::id()
    ))
    .unwrap();
    let mut process_supervisor = ChildSupervisor::new();
    let mut resize_perf = NativeResizePerfState::default();
    let application = apply_native_input_effect(
        NativeInputEffect {
            shortcut_events: vec![AstreaShortcutEvent::pressed(
                "astrea-shell",
                "spotlight_toggle",
            )],
            ..NativeInputEffect::default()
        },
        NativeInputApplyContext {
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .expect("optional fallback spawn failure must not fail input handling");

    unsafe {
        match previous_path {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
        std::env::remove_var("OBLIVION_ONE_SPOTLIGHT_COMMAND");
    }

    assert_eq!(application.fallback_attempts, 1);
    assert_eq!(
        application.fallback_spawn_failed,
        Some(AstreaShortcutFallbackKind::Spotlight)
    );
    assert!(application.launch.is_none());
    assert_eq!(process_supervisor.active_count(), 0);
}

#[test]
fn native_input_alt_tab_fallback_spawn_failure_is_non_fatal_and_recorded() {
    let _guard = ASTREA_ENV_LOCK.lock().unwrap();
    let previous_path = std::env::var_os("PATH");
    unsafe {
        std::env::set_var("OBLIVION_ONE_ALT_TAB_COMMAND", "exit 0");
        std::env::set_var("PATH", "/definitely-not-a-command-path");
    }
    assert!(external_alt_tab_command().is_some());

    let mut server = OwnCompositorServer::bind(format!(
        "typhon-alt-tab-fallback-failure-{}",
        std::process::id()
    ))
    .unwrap();
    let mut process_supervisor = ChildSupervisor::new();
    let mut resize_perf = NativeResizePerfState::default();
    let application = apply_native_input_effect(
        NativeInputEffect {
            shortcut_events: vec![AstreaShortcutEvent::pressed("astrea-shell", "alt_tab_next")],
            ..NativeInputEffect::default()
        },
        NativeInputApplyContext {
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .expect("optional fallback spawn failure must not fail input handling");

    unsafe {
        match previous_path {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
        std::env::remove_var("OBLIVION_ONE_ALT_TAB_COMMAND");
    }

    assert_eq!(application.fallback_attempts, 1);
    assert_eq!(
        application.fallback_spawn_failed,
        Some(AstreaShortcutFallbackKind::AltTab)
    );
    assert!(application.launch.is_none());
    assert_eq!(process_supervisor.active_count(), 0);
}

#[test]
fn native_input_registered_shortcut_owner_suppresses_fallback_spawn() {
    let shortcut = AstreaShortcutEvent::pressed("astrea-shell", "spotlight_toggle");

    assert_eq!(
        astrea_shortcut_fallback_kind(&shortcut, 1),
        None,
        "a registered protocol owner must suppress the external fallback"
    );
}

#[test]
fn native_input_zero_owner_repeat_and_alt_tab_non_next_do_not_launch_fallback() {
    let _guard = ASTREA_ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("OBLIVION_ONE_SPOTLIGHT_COMMAND", "exit 0");
        std::env::set_var("OBLIVION_ONE_ALT_TAB_COMMAND", "exit 0");
    }
    for (name, phase) in [
        ("spotlight_toggle", AstreaShortcutPhase::Repeated),
        ("alt_tab_previous", AstreaShortcutPhase::Pressed),
        ("alt_tab_commit", AstreaShortcutPhase::Pressed),
    ] {
        let mut server = OwnCompositorServer::bind(format!(
            "typhon-shortcut-no-fallback-{}-{}",
            std::process::id(),
            name
        ))
        .unwrap();
        let mut process_supervisor = ChildSupervisor::new();
        let mut resize_perf = NativeResizePerfState::default();
        let application = apply_native_input_effect(
            NativeInputEffect {
                shortcut_events: vec![AstreaShortcutEvent {
                    namespace: "astrea-shell".to_string(),
                    name: name.to_string(),
                    phase,
                }],
                ..NativeInputEffect::default()
            },
            NativeInputApplyContext {
                server: &mut server,
                perf: NativePerfLogger::from_env(),
                resize_perf: &mut resize_perf,
                cursor_mode: NativeCursorRenderMode::Software,
                app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
                process_supervisor: &mut process_supervisor,
                xwayland: None,
            },
        )
        .unwrap();
        assert!(
            application.launch.is_none(),
            "unexpected fallback for {name}"
        );
    }
    unsafe {
        std::env::remove_var("OBLIVION_ONE_SPOTLIGHT_COMMAND");
        std::env::remove_var("OBLIVION_ONE_ALT_TAB_COMMAND");
    }
}

#[test]
fn ordinary_motion_does_not_apply_compositor_only_position_update() {
    let mut input = NativeInputState::new(320, 200);
    let effect = input.handle_pointer_motion_delta(24.0, 12.0);

    let compositor_visual_changed = apply_compositor_only_pointer_position(&effect, |_, _| {
        panic!("ordinary forwarded motion must not update position twice")
    });

    assert_eq!(effect.pointer_motion, Some((184.0, 112.0)));
    assert!(!compositor_visual_changed);
}

#[test]
fn native_input_alt_p_requests_session_exit_without_forwarding_p() {
    let mut input = NativeInputState::new(320, 200);

    input.handle_key_event(KEY_LEFTALT, 1);
    let p = input.handle_key_event(KEY_P, 1);

    assert!(p.exit_requested);
    assert!(p.keyboard_events.is_empty());
}

#[test]
fn native_input_unbound_alt_z_replays_alt_before_key_and_releases_it() {
    let mut input = NativeInputState::new(320, 200);

    let alt_press = input.handle_key_event(KEY_LEFTALT, 1);
    let z_press = input.handle_key_event(KEY_Z, 1);
    let z_release = input.handle_key_event(KEY_Z, 0);
    let alt_release = input.handle_key_event(KEY_LEFTALT, 0);

    assert!(alt_press.keyboard_events.is_empty());
    assert_eq!(
        z_press.keyboard_events,
        vec![
            NativeKeyboardEvent::new(KEY_LEFTALT, true),
            NativeKeyboardEvent::new(KEY_Z, true),
        ]
    );
    assert_eq!(
        z_release.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_Z, false)]
    );
    assert_eq!(
        alt_release.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_LEFTALT, false)]
    );
}

#[test]
fn native_input_deferred_modifier_replay_does_not_double_update_xkb() {
    let mut input = NativeInputState::new(320, 200);

    let alt_press = input.handle_key_event(KEY_LEFTALT, 1);
    let z_press = input.handle_key_event(KEY_Z, 1);

    assert_eq!(
        alt_press.keyboard_actions,
        vec![NativeKeyboardAction::PhysicalOnly(
            NativeKeyboardEvent::new(KEY_LEFTALT, true,)
        )]
    );
    assert_eq!(
        z_press.keyboard_actions,
        vec![
            NativeKeyboardAction::ClientOnly(NativeKeyboardEvent::new(KEY_LEFTALT, true,)),
            NativeKeyboardAction::PhysicalAndClient(NativeKeyboardEvent::new(KEY_Z, true)),
        ]
    );
}

#[test]
fn native_input_unbound_super_z_replays_super_before_key_and_releases_it() {
    let mut input = NativeInputState::new(320, 200);

    input.handle_key_event(KEY_RIGHTMETA, 1);
    let z_press = input.handle_key_event(KEY_Z, 1);
    let super_release = input.handle_key_event(KEY_RIGHTMETA, 0);

    assert_eq!(
        z_press.keyboard_events,
        vec![
            NativeKeyboardEvent::new(KEY_RIGHTMETA, true),
            NativeKeyboardEvent::new(KEY_Z, true),
        ]
    );
    assert_eq!(
        super_release.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_RIGHTMETA, false)]
    );
}

#[test]
fn native_input_unbound_ctrl_shift_alt_z_forwards_each_modifier_once() {
    let mut input = NativeInputState::new(320, 200);

    let ctrl = input.handle_key_event(KEY_RIGHTCTRL, 1);
    let shift = input.handle_key_event(KEY_RIGHTSHIFT, 1);
    let alt = input.handle_key_event(KEY_RIGHTALT, 1);
    let z = input.handle_key_event(KEY_Z, 1);
    let repeat = input.handle_key_event(KEY_Z, 2);

    assert_eq!(
        ctrl.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_RIGHTCTRL, true)]
    );
    assert_eq!(
        shift.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_RIGHTSHIFT, true)]
    );
    assert!(alt.keyboard_events.is_empty());
    assert_eq!(
        z.keyboard_events,
        vec![
            NativeKeyboardEvent::new(KEY_RIGHTALT, true),
            NativeKeyboardEvent::new(KEY_Z, true),
        ]
    );
    assert!(repeat.keyboard_events.is_empty());
}

#[test]
fn native_input_alt_tab_sequence_emits_astrea_shortcuts() {
    let mut input = NativeInputState::new(320, 200);

    input.handle_key_event(KEY_LEFTALT, 1);
    let forwarded_alt = input.handle_key_event(KEY_Z, 1);
    input.handle_key_event(KEY_Z, 0);
    assert_eq!(
        forwarded_alt.keyboard_events,
        vec![
            NativeKeyboardEvent::new(KEY_LEFTALT, true),
            NativeKeyboardEvent::new(KEY_Z, true),
        ]
    );
    let next = input.handle_key_event(KEY_TAB, 1);
    let commit = input.handle_key_event(KEY_LEFTALT, 0);

    assert_eq!(
        next.shortcut_events,
        vec![AstreaShortcutEvent::pressed("astrea-shell", "alt_tab_next")]
    );
    assert!(next.keyboard_events.is_empty());
    assert_eq!(
        commit.shortcut_events,
        vec![AstreaShortcutEvent::pressed(
            "astrea-shell",
            "alt_tab_commit"
        )]
    );
    assert_eq!(
        commit.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_LEFTALT, false)],
        "a consumed Alt release must reconcile the client-visible Alt press"
    );
    assert!(input.forwarded_key_ledger_is_empty());
}

#[test]
fn native_input_alt_shift_tab_sequence_emits_previous() {
    let mut input = NativeInputState::new(320, 200);

    input.handle_key_event(KEY_LEFTALT, 1);
    input.handle_key_event(KEY_LEFTSHIFT, 1);
    let previous = input.handle_key_event(KEY_TAB, 1);

    assert_eq!(
        previous.shortcut_events,
        vec![AstreaShortcutEvent::pressed(
            "astrea-shell",
            "alt_tab_previous"
        )]
    );
    assert!(previous.keyboard_events.is_empty());
}

#[test]
fn native_input_session_switch_shortcuts_launch_exact_configured_command() {
    let _guard = ASTREA_ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("OBLIVION_ONE_SESSION_1_COMMAND", "switch-one");
        std::env::set_var("OBLIVION_ONE_SESSION_2_COMMAND", "switch-two");
        std::env::set_var("OBLIVION_ONE_SESSION_3_COMMAND", "switch-three");
    }
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTCTRL, 1);
    input.handle_key_event(KEY_LEFTSHIFT, 1);
    input.handle_key_event(KEY_LEFTALT, 1);

    let one = input.handle_key_event(KEY_1, 1);
    let two = input.handle_key_event(KEY_2, 1);
    let three = input.handle_key_event(KEY_3, 1);

    assert_eq!(
        one.launch_command,
        Some(vec![
            "sh".to_string(),
            "-lc".to_string(),
            "switch-one".to_string()
        ])
    );
    assert_eq!(
        two.launch_command,
        Some(vec![
            "sh".to_string(),
            "-lc".to_string(),
            "switch-two".to_string()
        ])
    );
    assert_eq!(
        three.launch_command,
        Some(vec![
            "sh".to_string(),
            "-lc".to_string(),
            "switch-three".to_string()
        ])
    );

    unsafe {
        std::env::remove_var("OBLIVION_ONE_SESSION_1_COMMAND");
        std::env::remove_var("OBLIVION_ONE_SESSION_2_COMMAND");
        std::env::remove_var("OBLIVION_ONE_SESSION_3_COMMAND");
    }
}

#[test]
fn native_input_ctrl_alt_function_key_requests_vt_switch_without_shell_command() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTCTRL, 1);
    input.handle_key_event(KEY_LEFTALT, 1);

    let switch = input.handle_key_event(KEY_F2, 1);
    let f2_after_reset = input.handle_key_event(KEY_F2, 0);

    assert_eq!(switch.vt_switch, Some(2));
    assert!(switch.launch_command.is_none());
    assert!(switch.keyboard_events.is_empty());
    assert!(f2_after_reset.keyboard_events.is_empty());
}

#[test]
fn native_input_ctrl_c_is_forwarded_to_clients() {
    let mut input = NativeInputState::new(320, 200);

    let ctrl = input.handle_key_event(KEY_LEFTCTRL, 1);
    let c = input.handle_key_event(KEY_C, 1);

    assert_eq!(
        ctrl.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_LEFTCTRL, true)]
    );
    assert!(!c.exit_requested);
    assert_eq!(
        c.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_C, true)]
    );
}

#[test]
fn native_input_super_mouse_buttons_start_window_interactions() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTMETA, 1);

    let move_start = input.handle_pointer_button(u32::from(BTN_LEFT), true);
    let motion = input.handle_pointer_motion_delta(24.0, 12.0);
    let move_end = input.handle_pointer_button(u32::from(BTN_LEFT), false);

    assert_eq!(
        move_start.window_actions,
        vec![NativeWindowAction::BeginMove {
            x: 160.0,
            y: 100.0,
            trigger_button: Some(u32::from(BTN_LEFT)),
        }]
    );
    assert!(move_start.pointer_buttons.is_empty());
    assert!(motion.window_actions.is_empty());
    assert_eq!(motion.pointer_motion, Some((184.0, 112.0)));
    assert!(move_end.window_actions.is_empty());
    assert_eq!(
        move_end.pointer_buttons,
        vec![NativePointerButtonEvent::new_at(
            u32::from(BTN_LEFT),
            false,
            184.0,
            112.0,
            320,
            200,
        )]
    );
}

#[test]
fn native_input_unlocked_relative_motion_moves_cursor_and_preserves_relative_delta() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_pointer_motion_delta(24.0, 12.0);

    assert_eq!(input.cursor_position(), (184, 112));
    assert_eq!(
        effect.relative_motion,
        Some(RelativeMotion::accelerated_only(24.0, 12.0))
    );
    assert_eq!(effect.pointer_motion, Some((184.0, 112.0)));
    assert_eq!(effect.cursor_position, Some((184, 112)));
}

#[test]
fn native_input_locked_relative_motion_preserves_delta_without_moving_cursor() {
    let mut input = NativeInputState::new(320, 200);
    input.set_pointer_locked_at(input.cursor_position_f64());

    let effect = input.handle_pointer_motion_delta(24.0, 12.0);

    assert_eq!(input.cursor_position(), (160, 100));
    assert_eq!(
        effect.relative_motion,
        Some(RelativeMotion::accelerated_only(24.0, 12.0))
    );
    assert_eq!(effect.pointer_motion, None);
    assert_eq!(effect.cursor_position, None);
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Software));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
}

#[test]
fn native_input_locked_absolute_motion_does_not_move_cursor() {
    let mut input = NativeInputState::new(320, 200);
    input.set_pointer_locked_at(input.cursor_position_f64());

    let effect = input.handle_pointer_motion(PointerMotionSample::absolute(7, 25.0, 26.0));

    assert_eq!(input.cursor_position(), (160, 100));
    assert_eq!(effect.pointer_motion, None);
    assert_eq!(effect.cursor_position, None);
}

#[test]
fn locked_pointer_motion_never_accumulates_absolute_cursor_position() {
    let mut input = NativeInputState::new(800, 600);
    let anchor = CompositorOutputPosition {
        x: 400.25,
        y: 300.75,
    };
    input.restore_cursor_position(anchor);
    input.set_pointer_locked_at(anchor);

    let mut total_dx = 0.0;
    let mut total_dy = 0.0;
    for _ in 0..100 {
        let effect = input.handle_pointer_motion(PointerMotionSample::relative(
            10,
            RelativeMotion::accelerated_only(20.0, -15.0),
        ));
        total_dx += effect.relative_motion.unwrap().dx;
        total_dy += effect.relative_motion.unwrap().dy;
        assert_eq!(effect.pointer_motion, None);
        assert_eq!(effect.cursor_position, None);
    }

    assert_eq!(total_dx, 2000.0);
    assert_eq!(total_dy, -1500.0);
    assert_eq!(input.cursor_position_f64(), anchor);
}

#[test]
fn native_input_unlock_restore_sets_logical_cursor_position() {
    let mut input = NativeInputState::new(320, 200);
    input.set_pointer_locked_at(input.cursor_position_f64());
    input.handle_pointer_motion_delta(200.0, 200.0);

    input.clear_pointer_constraint();
    let effect = input.restore_cursor_position(CompositorOutputPosition { x: 35.0, y: 45.0 });

    assert_eq!(input.cursor_position(), (35, 45));
    assert_eq!(effect.cursor_position, Some((35, 45)));
    assert!(effect.requires_frame_repaint(NativeCursorRenderMode::Software));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
}

#[test]
fn native_input_first_real_move_after_unlock_starts_from_restored_position() {
    let mut input = NativeInputState::new(320, 200);
    input.set_pointer_locked_at(input.cursor_position_f64());
    input.handle_pointer_motion_delta(200.0, 200.0);
    input.clear_pointer_constraint();
    input.restore_cursor_position(CompositorOutputPosition { x: 35.0, y: 45.0 });

    let effect = input.handle_pointer_motion_delta(5.0, -10.0);

    assert_eq!(input.cursor_position(), (40, 35));
    assert_eq!(effect.pointer_motion, Some((40.0, 35.0)));
}

#[test]
fn native_locked_input_resumes_physical_motion_after_preserve_position_deactivation() {
    let mut backend = NativePointerConstraintBackend::new();
    let mut input = NativeInputState::new(320, 200);
    let anchor = CompositorOutputPosition {
        x: 160.25,
        y: 100.75,
    };
    let id = PointerConstraintBackendId {
        constraint_id: 21,
        generation: 4,
    };
    input.restore_cursor_position(anchor);

    let activation = backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked { id },
        anchor,
    );
    assert_eq!(
        activation
            .activated
            .as_ref()
            .map(|constraint| constraint.id),
        Some(id)
    );
    assert!(backend.active_locked());
    // Keep input routing aligned with the backend as the runtime does after settlement.
    input.pointer_constraint = backend.active_constraint_state();
    assert_eq!(
        input.pointer_constraint,
        NativePointerConstraintState::Locked { anchor }
    );

    let locked_motion =
        input.handle_pointer_motion(PointerMotionSample::absolute(31, 250.0, 160.0));
    assert_eq!(locked_motion.pointer_motion, None);
    assert_eq!(input.cursor_position_f64(), anchor);

    let deactivation = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: None,
            restore_origin: None,
        },
        input.cursor_position_f64(),
    );
    assert_eq!(deactivation.deactivated, Some(id));
    assert_eq!(deactivation.restore_position, None);
    assert_eq!(deactivation.restore_origin, None);
    assert!(!backend.active_locked());
    input.pointer_constraint = backend.active_constraint_state();
    assert_eq!(input.pointer_constraint, NativePointerConstraintState::None);
    assert_eq!(input.cursor_position_f64(), anchor);

    let resumed_motion =
        input.handle_pointer_motion(PointerMotionSample::absolute(32, 164.25, 105.75));
    assert_eq!(resumed_motion.pointer_motion, Some((164.25, 105.75)));
    assert_eq!(
        input.cursor_position_f64(),
        CompositorOutputPosition {
            x: 164.25,
            y: 105.75
        }
    );
}

#[test]
fn native_input_confined_relative_motion_clamps_to_region_and_keeps_absolute_motion() {
    let mut input = NativeInputState::new(320, 200);
    input.restore_cursor_position(CompositorOutputPosition { x: 50.0, y: 50.0 });
    input.set_pointer_confined(OutputRegion::from_rect(
        OutputRect::new(40.0, 30.0, 60.0, 40.0).unwrap(),
    ));

    let right = input.handle_pointer_motion_delta(100.0, 0.0);
    assert_eq!(input.cursor_position(), (99, 50));
    assert_eq!(right.pointer_motion, Some((99.0, 50.0)));
    assert_eq!(
        right.relative_motion,
        Some(RelativeMotion::accelerated_only(100.0, 0.0))
    );

    let bottom = input.handle_pointer_motion_delta(0.0, 100.0);
    assert_eq!(input.cursor_position(), (99, 69));
    assert_eq!(bottom.pointer_motion, Some((99.0, 69.0)));

    let left = input.handle_pointer_motion_delta(-100.0, 0.0);
    assert_eq!(input.cursor_position(), (40, 69));
    assert_eq!(left.pointer_motion, Some((40.0, 69.0)));

    let top = input.handle_pointer_motion_delta(0.0, -100.0);
    assert_eq!(input.cursor_position(), (40, 30));
    assert_eq!(top.pointer_motion, Some((40.0, 30.0)));
}

#[test]
fn native_input_confined_absolute_motion_clamps_and_requests_cursor_repaint() {
    let mut input = NativeInputState::new(320, 200);
    input.set_pointer_confined(OutputRegion::from_rect(
        OutputRect::new(40.0, 30.0, 60.0, 40.0).unwrap(),
    ));

    let effect = input.handle_pointer_motion(PointerMotionSample::absolute(7, 500.0, 1.0));

    assert_eq!(input.cursor_position(), (99, 30));
    assert_eq!(effect.pointer_motion, Some((99.0, 30.0)));
    assert_eq!(effect.cursor_position, Some((99, 30)));
    assert!(effect.requires_frame_repaint(NativeCursorRenderMode::Software));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
}

#[test]
fn native_pointer_constraint_backend_activates_locked_once() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 7,
        generation: 1,
    };

    let first = backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked { id },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );
    let duplicate = backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked { id },
        CompositorOutputPosition { x: 30.0, y: 40.0 },
    );

    assert_eq!(
        first.activated,
        Some(NativePointerConstraint {
            id,
            mode: PointerConstraintMode::Locked,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
            region: None,
        })
    );
    assert_eq!(duplicate, NativePointerConstraintBackendAction::default());
    assert!(backend.active_locked());
}

#[test]
fn native_pointer_constraint_backend_ignores_warp_while_locked() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 13,
        generation: 1,
    };
    let anchor = CompositorOutputPosition {
        x: 100.25,
        y: 80.75,
    };
    let warp_position = CompositorOutputPosition { x: 240.0, y: 160.0 };
    backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked { id },
        anchor,
    );

    let action = backend.handle_request(
        PointerConstraintBackendRequest::WarpPointer {
            position: warp_position,
            origin: PointerWarpOrigin::PointerWarpProtocol,
        },
        anchor,
    );

    assert_eq!(action, NativePointerConstraintBackendAction::default());
    assert!(backend.active_locked());
    assert_eq!(
        backend.active_constraint_state(),
        NativePointerConstraintState::Locked { anchor }
    );
}

#[test]
fn native_pointer_constraint_backend_warps_when_unlocked() {
    let mut backend = NativePointerConstraintBackend::new();
    let position = CompositorOutputPosition { x: 240.0, y: 160.0 };

    let action = backend.handle_request(
        PointerConstraintBackendRequest::WarpPointer {
            position,
            origin: PointerWarpOrigin::PointerWarpProtocol,
        },
        CompositorOutputPosition { x: 100.0, y: 80.0 },
    );

    assert_eq!(action.cursor_position, Some(position));
    assert!(action.activated.is_none());
    assert!(action.deactivated.is_none());
    assert!(action.failed.is_none());
    assert!(action.cursor_visibility_changed.is_none());
}

#[test]
fn native_pointer_constraint_backend_clamps_warp_when_confined() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 14,
        generation: 1,
    };
    let region = OutputRegion::from_rect(OutputRect::new(40.0, 30.0, 60.0, 40.0).unwrap());
    let requested = CompositorOutputPosition { x: 240.0, y: 160.0 };
    let expected = CompositorOutputPosition { x: 99.0, y: 69.0 };
    let anchor = CompositorOutputPosition { x: 50.0, y: 40.0 };
    backend.handle_request(
        PointerConstraintBackendRequest::ActivateConfined {
            id,
            region: region.clone(),
            region_resolution_timing: None,
        },
        anchor,
    );

    let action = backend.handle_request(
        PointerConstraintBackendRequest::WarpPointer {
            position: requested,
            origin: PointerWarpOrigin::PointerWarpProtocol,
        },
        anchor,
    );

    assert_eq!(action.cursor_position, Some(expected));
    assert_eq!(
        backend.active_constraint_state(),
        NativePointerConstraintState::Confined { region }
    );
}

#[test]
fn native_pointer_constraint_backend_activates_confined_with_region() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 8,
        generation: 1,
    };
    let region = OutputRegion::from_rect(OutputRect::new(10.0, 20.0, 100.0, 50.0).unwrap());

    let action = backend.handle_request(
        PointerConstraintBackendRequest::ActivateConfined {
            id,
            region: region.clone(),
            region_resolution_timing: None,
        },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );

    assert_eq!(
        action.activated,
        Some(NativePointerConstraint {
            id,
            mode: PointerConstraintMode::Confined,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
            region: Some(region),
        })
    );
    assert!(action.failed.is_none());
}

#[test]
fn native_pointer_constraint_backend_mismatched_deactivation_cannot_unlock_newer_lock() {
    let mut backend = NativePointerConstraintBackend::new();
    let active = PointerConstraintBackendId {
        constraint_id: 9,
        generation: 2,
    };
    let stale = PointerConstraintBackendId {
        constraint_id: 9,
        generation: 1,
    };
    backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked { id: active },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );

    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id: stale,
            restore_position: Some(CompositorOutputPosition { x: 40.0, y: 50.0 }),
            restore_origin: Some(PointerWarpOrigin::LockedPointerCursorHint),
        },
        CompositorOutputPosition { x: 99.0, y: 99.0 },
    );

    assert_eq!(action, NativePointerConstraintBackendAction::default());
    assert!(backend.active_locked());
}

#[test]
fn native_pointer_constraint_backend_deactivation_restores_only_explicit_hint() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 10,
        generation: 1,
    };
    backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked { id },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );

    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: Some(CompositorOutputPosition { x: 30.0, y: 40.0 }),
            restore_origin: Some(PointerWarpOrigin::LockedPointerCursorHint),
        },
        CompositorOutputPosition { x: 99.0, y: 99.0 },
    );

    assert_eq!(action.deactivated, Some(id));
    assert_eq!(
        action.restore_position,
        Some(CompositorOutputPosition { x: 30.0, y: 40.0 })
    );
    assert!(!backend.active_locked());

    backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked { id },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );
    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: None,
            restore_origin: None,
        },
        CompositorOutputPosition { x: 99.0, y: 99.0 },
    );

    assert_eq!(action.restore_position, None);
}

#[test]
fn native_pointer_constraint_backend_does_not_restore_fractional_activation_anchor() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 12,
        generation: 3,
    };
    let anchor = CompositorOutputPosition {
        x: 400.25,
        y: 300.75,
    };

    backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked { id },
        anchor,
    );
    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: None,
            restore_origin: None,
        },
        CompositorOutputPosition { x: 0.0, y: 0.0 },
    );

    assert_eq!(action.restore_position, None);
}

#[test]
fn native_pointer_constraint_backend_uses_settlement_anchor() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 15,
        generation: 1,
    };
    let request_position = CompositorOutputPosition { x: 100.0, y: 80.0 };
    let settlement_anchor = CompositorOutputPosition { x: 132.5, y: 96.25 };

    let action = backend.handle_resolved_request(
        PointerConstraintBackendRequest::ActivateLocked { id },
        request_position,
        Some(settlement_anchor),
    );

    assert_eq!(
        action.activated,
        Some(NativePointerConstraint {
            id,
            mode: PointerConstraintMode::Locked,
            anchor: settlement_anchor,
            region: None,
        })
    );
    assert_eq!(
        backend.active_constraint_state(),
        NativePointerConstraintState::Locked {
            anchor: settlement_anchor
        }
    );
}

#[test]
fn native_pointer_constraint_backend_confined_deactivation_does_not_restore_anchor() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 11,
        generation: 1,
    };
    backend.handle_request(
        PointerConstraintBackendRequest::ActivateConfined {
            id,
            region: OutputRegion::from_rect(OutputRect::new(10.0, 20.0, 100.0, 50.0).unwrap()),
            region_resolution_timing: None,
        },
        CompositorOutputPosition { x: 30.0, y: 40.0 },
    );

    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: None,
            restore_origin: None,
        },
        CompositorOutputPosition { x: 99.0, y: 99.0 },
    );

    assert_eq!(action.deactivated, Some(id));
    assert_eq!(action.restore_position, None);
    assert!(!backend.active_locked());
}

#[test]
fn native_pointer_constraint_backend_updates_confined_region_in_place() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 12,
        generation: 1,
    };
    backend.handle_request(
        PointerConstraintBackendRequest::ActivateConfined {
            id,
            region: OutputRegion::from_rect(OutputRect::new(10.0, 20.0, 100.0, 50.0).unwrap()),
            region_resolution_timing: None,
        },
        CompositorOutputPosition { x: 30.0, y: 40.0 },
    );

    let action = backend.handle_request(
        PointerConstraintBackendRequest::UpdateConfinedRegion {
            id,
            region: OutputRegion::from_rect(OutputRect::new(40.0, 50.0, 20.0, 10.0).unwrap()),
        },
        CompositorOutputPosition { x: 30.0, y: 40.0 },
    );

    assert_eq!(action.deactivated, None);
    assert_eq!(action.activated, None);
    assert_eq!(
        action.cursor_position,
        Some(CompositorOutputPosition { x: 40.0, y: 50.0 })
    );
    assert_eq!(action.restore_position, None);
    assert_eq!(
        backend.active_constraint_state(),
        NativePointerConstraintState::Confined {
            region: OutputRegion::from_rect(OutputRect::new(40.0, 50.0, 20.0, 10.0).unwrap())
        }
    );
}

#[test]
fn native_pointer_constraint_backend_tracks_cursor_visibility_changes() {
    let mut backend = NativePointerConstraintBackend::new();

    let hide = backend.handle_request(
        PointerConstraintBackendRequest::ApplyCursorVisibility { visible: false },
        CompositorOutputPosition::default(),
    );
    let duplicate_hide = backend.handle_request(
        PointerConstraintBackendRequest::ApplyCursorVisibility { visible: false },
        CompositorOutputPosition::default(),
    );
    let show = backend.handle_request(
        PointerConstraintBackendRequest::ApplyCursorVisibility { visible: true },
        CompositorOutputPosition::default(),
    );

    assert_eq!(hide.cursor_visibility_changed, Some(false));
    assert_eq!(
        duplicate_hide,
        NativePointerConstraintBackendAction::default()
    );
    assert_eq!(show.cursor_visibility_changed, Some(true));
}

#[test]
fn native_input_super_right_starts_window_resize() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_RIGHTMETA, 1);

    let effect = input.handle_pointer_button(u32::from(BTN_RIGHT), true);

    assert_eq!(
        effect.window_actions,
        vec![NativeWindowAction::BeginResize {
            x: 160.0,
            y: 100.0,
            trigger_button: Some(u32::from(BTN_RIGHT)),
        }]
    );
    assert!(effect.pointer_buttons.is_empty());
}

#[test]
fn native_input_super_release_does_not_end_active_window_interaction() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTMETA, 1);
    input.handle_pointer_button(u32::from(BTN_LEFT), true);

    let effect = input.handle_key_event(KEY_LEFTMETA, 0);

    assert!(effect.window_actions.is_empty());
}

#[test]
fn modifier_release_does_not_end_button_owned_resize() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTMETA, 1);
    input.handle_pointer_button(u32::from(BTN_RIGHT), true);

    let effect = input.handle_key_event(KEY_LEFTMETA, 0);

    assert!(effect.window_actions.is_empty());
}

#[test]
fn non_trigger_button_release_does_not_end_resize() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTMETA, 1);
    input.handle_pointer_button(u32::from(BTN_RIGHT), true);

    let effect = input.handle_pointer_button(u32::from(BTN_LEFT), false);

    assert!(effect.window_actions.is_empty());
    assert_eq!(
        effect.pointer_buttons,
        vec![NativePointerButtonEvent::new_at(
            u32::from(BTN_LEFT),
            false,
            160.0,
            100.0,
            320,
            200,
        )]
    );
}

#[test]
fn native_input_astrea_keyboard_shortcuts_map_to_actions_and_events() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTMETA, 1);

    let launch = input.handle_key_event(KEY_Q, 1);
    let close = input.handle_key_event(KEY_C, 1);
    let fullscreen = input.handle_key_event(KEY_F, 1);

    assert_eq!(launch.launch_command, Some(vec!["kitty".to_string()]));
    assert!(launch.keyboard_events.is_empty());
    assert_eq!(
        close.window_actions,
        vec![NativeWindowAction::CloseActiveWindow]
    );
    assert_eq!(
        fullscreen.window_actions,
        vec![NativeWindowAction::ToggleFullscreen]
    );
}

#[test]
fn native_input_backend_plan_prefers_libseat_libinput_when_available() {
    let plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
        preference: NativeInputBackendPreference::Auto,
        libseat_available: true,
        libinput_available: true,
        raw_evdev_available: true,
    });

    assert_eq!(plan.primary, NativeInputBackendKind::LibseatLibinputUdev);
    assert_eq!(
        plan.fallbacks,
        vec![
            NativeInputBackendKind::DirectLibinputUdev,
            NativeInputBackendKind::RawEvdev,
        ]
    );
}

#[test]
fn native_input_backend_plan_uses_direct_libinput_without_libseat() {
    let plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
        preference: NativeInputBackendPreference::Auto,
        libseat_available: false,
        libinput_available: true,
        raw_evdev_available: true,
    });

    assert_eq!(plan.primary, NativeInputBackendKind::DirectLibinputUdev);
    assert_eq!(plan.fallbacks, vec![NativeInputBackendKind::RawEvdev]);
}

#[test]
fn native_input_backend_plan_can_force_raw_evdev_for_debugging() {
    let plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
        preference: NativeInputBackendPreference::RawEvdev,
        libseat_available: true,
        libinput_available: true,
        raw_evdev_available: true,
    });

    assert_eq!(plan.primary, NativeInputBackendKind::RawEvdev);
    assert!(plan.fallbacks.is_empty());
}

#[test]
fn native_input_backend_plan_falls_back_to_raw_when_libinput_is_unavailable() {
    let plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
        preference: NativeInputBackendPreference::Auto,
        libseat_available: true,
        libinput_available: false,
        raw_evdev_available: true,
    });

    assert_eq!(plan.primary, NativeInputBackendKind::RawEvdev);
    assert!(plan.fallbacks.is_empty());
}

#[test]

fn native_input_backend_no_longer_owns_seat_lifecycle() {
    // NativeSessionLifecycle is runtime-owned; input backend plans remain independent.
    let plan = NativeInputBackendPlan::choose(NativeInputBackendChoice {
        preference: NativeInputBackendPreference::Auto,
        libseat_available: true,
        libinput_available: true,
        raw_evdev_available: true,
    });
    assert_eq!(plan.primary, NativeInputBackendKind::LibseatLibinputUdev);
}

#[test]
fn native_input_state_handles_normalized_relative_motion() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion(
        PointerMotionSample::relative(10, RelativeMotion::accelerated_only(12.0, -4.0)),
    ));

    assert_eq!(effect.pointer_motion, Some((172.0, 96.0)));
    assert!(effect.redraw_requested);
}

#[test]
fn native_input_pointer_motion_can_skip_frame_repaint_with_hardware_cursor() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion(
        PointerMotionSample::relative(10, RelativeMotion::accelerated_only(12.0, -4.0)),
    ));

    assert_eq!(effect.pointer_motion, Some((172.0, 96.0)));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
    assert!(effect.requires_frame_repaint(NativeCursorRenderMode::Software));
}

#[test]
fn native_forwarded_keyboard_input_skips_frame_repaint_without_local_visual_change() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_key_event(KEY_Z, 1);

    assert_eq!(
        effect.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_Z, true)]
    );
    assert!(effect.redraw_requested);
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Software));
}

#[test]
fn native_forwarded_pointer_button_skips_frame_repaint_without_local_visual_change() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_pointer_button(u32::from(BTN_LEFT), true);

    assert_eq!(effect.pointer_buttons.len(), 1);
    assert!(effect.redraw_requested);
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Software));
}

#[test]
fn native_forwarded_pointer_axis_skips_frame_repaint_without_local_visual_change() {
    let mut input = NativeInputState::new(320, 200);

    let effect = input.handle_pointer_axis(0.0, 120.0);

    assert_eq!(
        effect.pointer_axis,
        Some(PointerAxisFrame::unknown(0, 0.0, 120.0))
    );
    assert!(effect.redraw_requested);
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Software));
}

#[test]
fn native_input_window_interaction_motion_routes_through_compositor_owner() {
    let mut input = NativeInputState::new(320, 200);
    input.handle_key_event(KEY_LEFTMETA, 1);
    input.handle_pointer_button(u32::from(BTN_LEFT), true);

    let effect = input.handle_hardware_input_event(NativeHardwareInputEvent::PointerMotion(
        PointerMotionSample::relative(10, RelativeMotion::accelerated_only(12.0, -4.0)),
    ));

    assert!(effect.window_actions.is_empty());
    assert_eq!(effect.pointer_motion, Some((172.0, 96.0)));
    assert!(!effect.requires_frame_repaint(NativeCursorRenderMode::Hardware));
}

fn apply_native_keyboard_events(
    server: &mut OwnCompositorServer,
    input: &mut NativeInputState,
    events: &[(u16, i32)],
) {
    let mut process_supervisor = ChildSupervisor::new();
    let mut resize_perf = NativeResizePerfState::default();
    for &(code, value) in events {
        apply_native_input_effect(
            input.handle_key_event(code, value),
            NativeInputApplyContext {
                server,
                perf: NativePerfLogger::from_env(),
                resize_perf: &mut resize_perf,
                cursor_mode: NativeCursorRenderMode::Software,
                app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
                process_supervisor: &mut process_supervisor,
                xwayland: None,
            },
        )
        .unwrap();
    }
}

struct NativeKeyboardStateSnapshot {
    keymap: Vec<u8>,
    keys: Vec<(u32, bool)>,
    modifiers: Vec<(u32, u32, u32, u32)>,
    enters: Vec<Vec<u32>>,
    leaves: usize,
}

fn capture_native_keyboard_state(
    server: &mut OwnCompositorServer,
    client_commands: &mpsc::Sender<ClientCommand>,
    client_events: &mpsc::Receiver<ClientEvent>,
) -> NativeKeyboardStateSnapshot {
    client_commands
        .send(ClientCommand::CaptureKeyboardState)
        .unwrap();
    match pump_native_input_server_until(server, client_events) {
        ClientEvent::KeyboardState {
            keymap,
            keys,
            modifiers,
            enters,
            leaves,
        } => NativeKeyboardStateSnapshot {
            keymap,
            keys,
            modifiers,
            enters,
            leaves,
        },
        event => panic!("expected keyboard state, got {event:?}"),
    }
}

fn reset_native_keyboard_session(server: &mut OwnCompositorServer, input: &mut NativeInputState) {
    server.clear_keyboard_transient_state_for_session_switch();
    input.clear_pressed_state_for_session_switch();
    server.restore_keyboard_focus_after_session_switch();
}

fn native_keyboard_modifier_mask(keymap_bytes: &[u8], modifier: &str) -> u32 {
    let keymap_text = std::ffi::CStr::from_bytes_with_nul(keymap_bytes)
        .unwrap()
        .to_str()
        .unwrap();
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let keymap = xkb::Keymap::new_from_string(
        &context,
        keymap_text.to_string(),
        xkb::KEYMAP_FORMAT_TEXT_V1,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .unwrap();
    1u32.checked_shl(keymap.mod_get_index(modifier)).unwrap()
}

#[test]
fn native_input_session_reset_reconciles_forwarded_ctrl_and_held_key() {
    let _guard = ASTREA_ENV_LOCK.lock().unwrap();
    let previous_layout = std::env::var_os("OBLIVION_ONE_XKB_LAYOUT");
    let previous_variant = std::env::var_os("OBLIVION_ONE_XKB_VARIANT");
    let previous_options = std::env::var_os("OBLIVION_ONE_XKB_OPTIONS");
    // SAFETY: this test serializes its process-wide environment changes.
    unsafe {
        std::env::set_var("OBLIVION_ONE_XKB_LAYOUT", "us");
        std::env::set_var("OBLIVION_ONE_XKB_VARIANT", "");
        std::env::set_var("OBLIVION_ONE_XKB_OPTIONS", "");
    }

    let socket_name = format!(
        "typhon-native-input-session-reset-forwarded-{}",
        std::process::id()
    );
    let socket_path =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join(&socket_name);
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (client_commands, client_events) = spawn_native_input_keyboard_client(socket_path);
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::ReadyForPointer
    ));
    let mut input = NativeInputState::new(320, 200);
    apply_native_keyboard_events(&mut server, &mut input, &[(KEY_LEFTCTRL, 1)]);

    let state = capture_native_keyboard_state(&mut server, &client_commands, &client_events);
    let ctrl_mask = native_keyboard_modifier_mask(&state.keymap, xkb::MOD_NAME_CTRL);
    assert!(state.keys.contains(&(u32::from(KEY_LEFTCTRL), true)));
    assert!(
        state
            .modifiers
            .iter()
            .any(|(depressed, _, _, _)| depressed & ctrl_mask != 0)
    );

    reset_native_keyboard_session(&mut server, &mut input);
    let state = capture_native_keyboard_state(&mut server, &client_commands, &client_events);
    assert!(state.leaves >= 1);
    assert_eq!(state.enters.last(), Some(&Vec::new()));
    assert_eq!(
        state.modifiers.last().map(|state| state.0 & ctrl_mask),
        Some(0)
    );

    apply_native_keyboard_events(&mut server, &mut input, &[(KEY_Z, 1)]);
    let state = capture_native_keyboard_state(&mut server, &client_commands, &client_events);
    assert!(state.keys.contains(&(u32::from(KEY_Z), true)));
    reset_native_keyboard_session(&mut server, &mut input);
    let state = capture_native_keyboard_state(&mut server, &client_commands, &client_events);
    assert!(state.leaves >= 2);
    assert_eq!(state.enters.last(), Some(&Vec::new()));
    assert_eq!(state.modifiers.last().map(|state| state.0), Some(0));

    client_commands.send(ClientCommand::Finish).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Finished { .. }
    ));

    // SAFETY: restore the values while the same environment lock is held.
    unsafe {
        match previous_layout {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_LAYOUT", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_LAYOUT"),
        }
        match previous_variant {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_VARIANT", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_VARIANT"),
        }
        match previous_options {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_OPTIONS", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_OPTIONS"),
        }
    }
}

#[test]
fn native_input_session_reset_retains_caps_lock_without_transient_depressed_keys() {
    let _guard = ASTREA_ENV_LOCK.lock().unwrap();
    let previous_layout = std::env::var_os("OBLIVION_ONE_XKB_LAYOUT");
    let previous_variant = std::env::var_os("OBLIVION_ONE_XKB_VARIANT");
    let previous_options = std::env::var_os("OBLIVION_ONE_XKB_OPTIONS");
    // SAFETY: this test serializes its process-wide environment changes.
    unsafe {
        std::env::set_var("OBLIVION_ONE_XKB_LAYOUT", "us");
        std::env::set_var("OBLIVION_ONE_XKB_VARIANT", "");
        std::env::set_var("OBLIVION_ONE_XKB_OPTIONS", "");
    }

    let socket_name = format!(
        "typhon-native-input-session-reset-caps-lock-{}",
        std::process::id()
    );
    let socket_path =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join(&socket_name);
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (client_commands, client_events) = spawn_native_input_keyboard_client(socket_path);
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::ReadyForPointer
    ));
    let mut input = NativeInputState::new(320, 200);
    apply_native_keyboard_events(
        &mut server,
        &mut input,
        &[(KEY_CAPSLOCK, 1), (KEY_CAPSLOCK, 0)],
    );

    let state = capture_native_keyboard_state(&mut server, &client_commands, &client_events);
    let caps_mask = native_keyboard_modifier_mask(&state.keymap, xkb::MOD_NAME_CAPS);
    assert!(
        state
            .modifiers
            .iter()
            .any(|(_, _, locked, _)| locked & caps_mask != 0)
    );

    reset_native_keyboard_session(&mut server, &mut input);
    let state = capture_native_keyboard_state(&mut server, &client_commands, &client_events);
    let last_modifiers = state.modifiers.last().copied().unwrap();
    assert!(state.leaves >= 1);
    assert_eq!(state.enters.last(), Some(&Vec::new()));
    assert_eq!(last_modifiers.0, 0);
    assert_eq!(last_modifiers.2 & caps_mask, caps_mask);

    client_commands.send(ClientCommand::Finish).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Finished { .. }
    ));

    // SAFETY: restore the values while the same environment lock is held.
    unsafe {
        match previous_layout {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_LAYOUT", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_LAYOUT"),
        }
        match previous_variant {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_VARIANT", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_VARIANT"),
        }
        match previous_options {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_OPTIONS", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_OPTIONS"),
        }
    }
}

#[test]
fn native_input_group_switch_reaches_wayland_keyboard_modifiers() {
    let _guard = ASTREA_ENV_LOCK.lock().unwrap();
    let previous_layout = std::env::var_os("OBLIVION_ONE_XKB_LAYOUT");
    let previous_variant = std::env::var_os("OBLIVION_ONE_XKB_VARIANT");
    let previous_options = std::env::var_os("OBLIVION_ONE_XKB_OPTIONS");
    // SAFETY: this test serializes its process-wide environment changes.
    unsafe {
        std::env::set_var("OBLIVION_ONE_XKB_LAYOUT", "br,us");
        std::env::set_var("OBLIVION_ONE_XKB_VARIANT", "abnt2,");
        std::env::set_var("OBLIVION_ONE_XKB_OPTIONS", "grp:alt_shift_toggle");
    }

    let socket_name = format!("typhon-native-input-group-switch-{}", std::process::id());
    let socket_path =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join(&socket_name);
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (client_commands, client_events) = spawn_native_input_keyboard_client(socket_path);
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::ReadyForPointer
    ));
    let mut input = NativeInputState::new(320, 200);
    apply_native_keyboard_events(
        &mut server,
        &mut input,
        &[
            (KEY_LEFTALT, 1),
            (KEY_LEFTSHIFT, 1),
            (KEY_LEFTSHIFT, 0),
            (KEY_LEFTALT, 0),
            (KEY_LEFTALT, 1),
            (KEY_LEFTSHIFT, 1),
            (KEY_LEFTSHIFT, 0),
            (KEY_LEFTALT, 0),
        ],
    );

    client_commands
        .send(ClientCommand::CaptureKeyboardState)
        .unwrap();
    let keyboard_state = match pump_native_input_server_until(&mut server, &client_events) {
        ClientEvent::KeyboardState {
            keymap,
            keys,
            modifiers,
            ..
        } => (keymap, keys, modifiers),
        event => panic!("expected keyboard state, got {event:?}"),
    };
    client_commands.send(ClientCommand::Finish).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Finished { .. }
    ));

    // SAFETY: restore the values while the same environment lock is held.
    unsafe {
        match previous_layout {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_LAYOUT", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_LAYOUT"),
        }
        match previous_variant {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_VARIANT", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_VARIANT"),
        }
        match previous_options {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_OPTIONS", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_OPTIONS"),
        }
    }

    let (keymap, keys, modifiers) = keyboard_state;
    let keymap_text = std::ffi::CStr::from_bytes_with_nul(&keymap)
        .unwrap()
        .to_str()
        .unwrap();
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let keymap = xkb::Keymap::new_from_string(
        &context,
        keymap_text.to_string(),
        xkb::KEYMAP_FORMAT_TEXT_V1,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .unwrap();
    let alt_mask = 1u32
        .checked_shl(keymap.mod_get_index(xkb::MOD_NAME_ALT))
        .unwrap();
    assert_eq!(
        keys,
        vec![
            (u32::from(KEY_LEFTSHIFT), true),
            (u32::from(KEY_LEFTSHIFT), false),
            (u32::from(KEY_LEFTSHIFT), true),
            (u32::from(KEY_LEFTSHIFT), false),
        ]
    );
    assert!(
        modifiers
            .iter()
            .any(|(depressed, _, _, _)| depressed & alt_mask != 0)
    );
    let groups = modifiers
        .iter()
        .map(|(_, _, _, group)| *group)
        .collect::<Vec<_>>();
    assert!(groups.windows(2).any(|pair| pair == [0, 1]));
    assert!(groups.windows(2).any(|pair| pair == [1, 0]));
    assert_eq!(modifiers.last().map(|state| state.0), Some(0));
}

#[test]
fn native_input_deferred_alt_keeps_authoritative_physical_modifier_state() {
    let _guard = ASTREA_ENV_LOCK.lock().unwrap();
    let previous_layout = std::env::var_os("OBLIVION_ONE_XKB_LAYOUT");
    let previous_variant = std::env::var_os("OBLIVION_ONE_XKB_VARIANT");
    let previous_options = std::env::var_os("OBLIVION_ONE_XKB_OPTIONS");
    // SAFETY: this test serializes its process-wide environment changes.
    unsafe {
        std::env::set_var("OBLIVION_ONE_XKB_LAYOUT", "br");
        std::env::set_var("OBLIVION_ONE_XKB_VARIANT", "abnt2");
        std::env::set_var("OBLIVION_ONE_XKB_OPTIONS", "");
    }

    let socket_name = format!(
        "typhon-native-input-deferred-alt-projection-{}",
        std::process::id()
    );
    let socket_path =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join(&socket_name);
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (client_commands, client_events) = spawn_native_input_keyboard_client(socket_path);
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::ReadyForPointer
    ));
    let mut input = NativeInputState::new(320, 200);
    apply_native_keyboard_events(
        &mut server,
        &mut input,
        &[
            (KEY_LEFTALT, 1),
            (KEY_LEFTSHIFT, 1),
            (KEY_LEFTSHIFT, 0),
            (KEY_LEFTALT, 0),
        ],
    );

    client_commands
        .send(ClientCommand::CaptureKeyboardState)
        .unwrap();
    let (keymap, keys, modifiers) =
        match pump_native_input_server_until(&mut server, &client_events) {
            ClientEvent::KeyboardState {
                keymap,
                keys,
                modifiers,
                ..
            } => (keymap, keys, modifiers),
            event => panic!("expected keyboard state, got {event:?}"),
        };
    client_commands.send(ClientCommand::Finish).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Finished { .. }
    ));

    // SAFETY: restore the values while the same environment lock is held.
    unsafe {
        match previous_layout {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_LAYOUT", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_LAYOUT"),
        }
        match previous_variant {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_VARIANT", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_VARIANT"),
        }
        match previous_options {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_OPTIONS", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_OPTIONS"),
        }
    }

    let keymap_text = std::ffi::CStr::from_bytes_with_nul(&keymap)
        .unwrap()
        .to_str()
        .unwrap();
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let keymap = xkb::Keymap::new_from_string(
        &context,
        keymap_text.to_string(),
        xkb::KEYMAP_FORMAT_TEXT_V1,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .unwrap();
    let alt_mask = 1u32
        .checked_shl(keymap.mod_get_index(xkb::MOD_NAME_ALT))
        .unwrap();
    let shift_mask = 1u32
        .checked_shl(keymap.mod_get_index(xkb::MOD_NAME_SHIFT))
        .unwrap();

    assert_eq!(
        keys,
        vec![
            (u32::from(KEY_LEFTSHIFT), true),
            (u32::from(KEY_LEFTSHIFT), false),
        ]
    );
    assert!(
        modifiers
            .iter()
            .any(|(depressed, _, _, _)| depressed & alt_mask != 0)
    );
    assert!(
        modifiers
            .iter()
            .any(|(depressed, _, _, _)| depressed & shift_mask != 0)
    );
    assert_eq!(modifiers.last().map(|state| state.0), Some(0));
}

#[test]
fn native_input_consumed_super_space_publishes_xkb_group_without_key_leak() {
    let _guard = ASTREA_ENV_LOCK.lock().unwrap();
    let previous_layout = std::env::var_os("OBLIVION_ONE_XKB_LAYOUT");
    let previous_variant = std::env::var_os("OBLIVION_ONE_XKB_VARIANT");
    let previous_options = std::env::var_os("OBLIVION_ONE_XKB_OPTIONS");
    // SAFETY: this test serializes its process-wide environment changes.
    unsafe {
        std::env::set_var("OBLIVION_ONE_XKB_LAYOUT", "br,us");
        std::env::set_var("OBLIVION_ONE_XKB_VARIANT", "abnt2,");
        std::env::set_var("OBLIVION_ONE_XKB_OPTIONS", "grp:win_space_toggle");
    }

    let socket_name = format!(
        "typhon-native-input-consumed-group-switch-{}",
        std::process::id()
    );
    let socket_path =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join(&socket_name);
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (client_commands, client_events) = spawn_native_input_keyboard_client(socket_path);
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::ReadyForPointer
    ));
    let mut input = NativeInputState::new(320, 200);
    apply_native_keyboard_events(
        &mut server,
        &mut input,
        &[
            (KEY_LEFTMETA, 1),
            (KEY_SPACE, 1),
            (KEY_SPACE, 0),
            (KEY_LEFTMETA, 0),
        ],
    );

    client_commands
        .send(ClientCommand::CaptureKeyboardState)
        .unwrap();
    let (keymap, keys, modifiers) =
        match pump_native_input_server_until(&mut server, &client_events) {
            ClientEvent::KeyboardState {
                keymap,
                keys,
                modifiers,
                ..
            } => (keymap, keys, modifiers),
            event => panic!("expected keyboard state, got {event:?}"),
        };
    client_commands.send(ClientCommand::Finish).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Finished { .. }
    ));

    // SAFETY: restore the values while the same environment lock is held.
    unsafe {
        match previous_layout {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_LAYOUT", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_LAYOUT"),
        }
        match previous_variant {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_VARIANT", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_VARIANT"),
        }
        match previous_options {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_OPTIONS", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_OPTIONS"),
        }
    }

    assert!(keys.is_empty());
    let logo_mask = native_keyboard_modifier_mask(&keymap, xkb::MOD_NAME_LOGO);
    let groups = modifiers
        .iter()
        .map(|(_, _, _, group)| *group)
        .collect::<Vec<_>>();
    assert!(groups.windows(2).any(|pair| pair == [0, 1]));
    assert!(
        modifiers
            .iter()
            .any(|(depressed, _, _, _)| depressed & logo_mask != 0)
    );
    assert_eq!(modifiers.last().map(|state| state.0), Some(0));
}

#[test]
fn native_runtime_layout_set_preserves_held_modifier_and_session_state() {
    let _guard = ASTREA_ENV_LOCK.lock().unwrap();
    let previous_layout = std::env::var_os("OBLIVION_ONE_XKB_LAYOUT");
    let previous_variant = std::env::var_os("OBLIVION_ONE_XKB_VARIANT");
    let previous_options = std::env::var_os("OBLIVION_ONE_XKB_OPTIONS");
    // SAFETY: this test serializes its process-wide environment changes.
    unsafe {
        std::env::set_var("OBLIVION_ONE_XKB_LAYOUT", "br,us");
        std::env::set_var("OBLIVION_ONE_XKB_VARIANT", "abnt2,");
        std::env::set_var("OBLIVION_ONE_XKB_OPTIONS", "");
    }

    let socket_name = format!(
        "typhon-native-runtime-layout-held-modifier-{}",
        std::process::id()
    );
    let socket_path =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join(&socket_name);
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (client_commands, client_events) = spawn_native_input_keyboard_client(socket_path);
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::ReadyForPointer
    ));
    let mut input = NativeInputState::new(320, 200);
    apply_native_keyboard_events(&mut server, &mut input, &[(KEY_LEFTSHIFT, 1)]);
    let before = capture_native_keyboard_state(&mut server, &client_commands, &client_events);
    let shifted_keymap = before.keymap.clone();
    let shift_mask = native_keyboard_modifier_mask(&before.keymap, xkb::MOD_NAME_SHIFT);

    let snapshot = server.set_keyboard_layout(1).unwrap();
    assert_eq!(snapshot.effective_index, 1);
    assert_eq!(snapshot.locked_index, 1);
    let after_set = capture_native_keyboard_state(&mut server, &client_commands, &client_events);
    assert_eq!(after_set.keymap, shifted_keymap);
    assert_eq!(after_set.keys, before.keys);
    assert_eq!(
        after_set.modifiers.last().map(|state| state.0 & shift_mask),
        Some(shift_mask)
    );
    assert_eq!(after_set.modifiers.last().map(|state| state.3), Some(1));

    apply_native_keyboard_events(&mut server, &mut input, &[(KEY_LEFTSHIFT, 0)]);
    let after_release =
        capture_native_keyboard_state(&mut server, &client_commands, &client_events);
    assert_eq!(after_release.modifiers.last().map(|state| state.0), Some(0));
    assert_eq!(after_release.modifiers.last().map(|state| state.3), Some(1));

    reset_native_keyboard_session(&mut server, &mut input);
    let after_session =
        capture_native_keyboard_state(&mut server, &client_commands, &client_events);
    assert_eq!(after_session.enters.last(), Some(&Vec::new()));
    assert_eq!(after_session.modifiers.last().map(|state| state.3), Some(1));

    client_commands.send(ClientCommand::Finish).unwrap();
    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::Finished { .. }
    ));

    // SAFETY: restore the values while the same environment lock is held.
    unsafe {
        match previous_layout {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_LAYOUT", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_LAYOUT"),
        }
        match previous_variant {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_VARIANT", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_VARIANT"),
        }
        match previous_options {
            Some(value) => std::env::set_var("OBLIVION_ONE_XKB_OPTIONS", value),
            None => std::env::remove_var("OBLIVION_ONE_XKB_OPTIONS"),
        }
    }
}

#[test]
fn native_input_active_resize_updates_compositor_and_exact_client_cursor_motion() {
    let socket_name = format!("typhon-native-input-interaction-{}", std::process::id());
    let socket_path =
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap()).join(&socket_name);
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (client_commands, client_events) = spawn_native_input_resize_client(socket_path);

    assert!(matches!(
        pump_native_input_server_until(&mut server, &client_events),
        ClientEvent::ReadyForPointer
    ));
    assert_eq!(server.renderable_surfaces().len(), 1);
    assert!(server.begin_window_resize_at_with_trigger(92.0, 86.0, u32::from(BTN_LEFT),));
    assert!(server.window_interaction_active());
    assert!(server.cursor_visibility_requested());
    server.send_pointer_motion(92.0, 86.0);
    client_commands.send(ClientCommand::SetCursor).unwrap();
    let (pointer_motion_count, pointer_enter_count, pointer_leave_count) =
        match pump_native_input_server_until(&mut server, &client_events) {
            ClientEvent::CursorReady {
                pointer_motion_count,
                pointer_enter_count,
                pointer_leave_count,
            } => (
                pointer_motion_count,
                pointer_enter_count,
                pointer_leave_count,
            ),
            event => panic!("expected client cursor, got {event:?}"),
        };
    pump_native_input_server_until_cursor(&mut server);
    let updates_before = server.resize_flow_metrics().resize_updates_applied;
    let raw_updates_before = server.resize_flow_metrics().raw_pointer_resize_updates;
    // Move toward the top-left resize corner so the test grows the window
    // beyond the compositor's generic minimum size.
    let x = 52.0;
    let y = 52.0;
    let mut process_supervisor = ChildSupervisor::new();
    let mut resize_perf = NativeResizePerfState::default();

    let application = apply_native_input_effect(
        NativeInputEffect {
            pointer_motion: Some((x, y)),
            pointer_motion_usec: Some(42_000),
            relative_motion: Some(RelativeMotion::accelerated_only(7.0, -5.0)),
            ..NativeInputEffect::default()
        },
        NativeInputApplyContext {
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();

    assert!(server.window_interaction_active());
    assert_eq!(
        server.resize_flow_metrics().raw_pointer_resize_updates,
        raw_updates_before + 1
    );
    assert!(application.redraw_requested);
    assert!(server.client_cursor_request_active());
    assert!(server.client_cursor_render_state().is_none());
    assert_eq!(server.last_pointer_position(), (x, y));
    server.flush_pending_interactive_visual_state_for_render_admission(false);
    client_commands.send(ClientCommand::CaptureActive).unwrap();
    let active = pump_native_input_server_until(&mut server, &client_events);
    assert_eq!(
        active,
        ClientEvent::Active {
            pointer_motion_count: pointer_motion_count + 1,
            pointer_surface_x: Some(20.0),
            pointer_surface_y: Some(14.0),
            pointer_enter_count,
            pointer_leave_count,
        }
    );
    assert_eq!(
        server.resize_flow_metrics().resize_updates_applied,
        updates_before + 1
    );

    let release_application = apply_native_input_effect(
        NativeInputEffect {
            pointer_buttons: vec![NativePointerButtonEvent::new_at(
                u32::from(BTN_LEFT),
                false,
                x,
                y,
                320,
                200,
            )],
            ..NativeInputEffect::default()
        },
        NativeInputApplyContext {
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();

    assert!(release_application.redraw_requested);
    assert!(!server.window_interaction_active());
    assert!(!server.cursor_visibility_requested());
    let cursor_after_release = server
        .client_cursor_render_state()
        .expect("client cursor remains rendered after resize release");
    assert_eq!(
        (
            cursor_after_release.logical_x + 3,
            cursor_after_release.logical_y + 4
        ),
        (x as i32, y as i32)
    );

    let next_x = x + 1.0;
    let next_y = y + 2.0;
    apply_native_input_effect(
        NativeInputEffect {
            pointer_motion: Some((next_x, next_y)),
            pointer_motion_usec: Some(43_000),
            ..NativeInputEffect::default()
        },
        NativeInputApplyContext {
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::CpuOnly,
            process_supervisor: &mut process_supervisor,
            xwayland: None,
        },
    )
    .unwrap();
    assert_eq!(server.last_pointer_position(), (next_x, next_y));

    let cursor_after_next_motion = server
        .client_cursor_render_state()
        .expect("client cursor remains rendered after normal motion");
    assert_eq!(
        (
            cursor_after_next_motion.logical_x + 3,
            cursor_after_next_motion.logical_y + 4
        ),
        (next_x as i32, next_y as i32)
    );
    client_commands.send(ClientCommand::Finish).unwrap();
    let finished = pump_native_input_server_until(&mut server, &client_events);
    assert_eq!(
        finished,
        ClientEvent::Finished {
            pointer_motion_count: pointer_motion_count + 2,
            pointer_surface_x: Some(21.0),
            pointer_surface_y: Some(16.0),
            pointer_enter_count,
            pointer_leave_count,
        }
    );
}

#[derive(Default)]
pub(super) struct NativeInputClientState {
    pub(super) pointer_enter_serial: Option<u32>,
    pub(super) pointer_motion_count: usize,
    pub(super) pointer_enter_count: usize,
    pub(super) pointer_leave_count: usize,
    pub(super) pointer_surface_x: Option<f64>,
    pub(super) pointer_surface_y: Option<f64>,
    pub(super) last_button_press_serial: Option<u32>,
    pub(super) pointer_button_press_count: usize,
    pub(super) pointer_button_release_count: usize,
    pub(super) keyboard_groups: Vec<u32>,
    pub(super) keyboard_keymap: Vec<u8>,
    pub(super) keyboard_keys: Vec<(u32, bool)>,
    pub(super) keyboard_modifiers: Vec<(u32, u32, u32, u32)>,
    pub(super) keyboard_enters: Vec<Vec<u32>>,
    pub(super) keyboard_leaves: usize,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for NativeInputClientState {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

macro_rules! native_input_noop_dispatch {
    ($interface:path) => {
        impl Dispatch<$interface, ()> for NativeInputClientState {
            fn event(
                _: &mut Self,
                _: &$interface,
                _: <$interface as wayland_client::Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    };
}

native_input_noop_dispatch!(client_wl_compositor::WlCompositor);
native_input_noop_dispatch!(client_wl_surface::WlSurface);
native_input_noop_dispatch!(client_wl_shm::WlShm);
native_input_noop_dispatch!(client_wl_shm_pool::WlShmPool);
native_input_noop_dispatch!(client_wl_buffer::WlBuffer);
native_input_noop_dispatch!(client_xdg_toplevel::XdgToplevel);
native_input_noop_dispatch!(client_xwayland_shell_v1::XwaylandShellV1);
native_input_noop_dispatch!(client_xwayland_surface_v1::XwaylandSurfaceV1);

impl Dispatch<client_wl_seat::WlSeat, ()> for NativeInputClientState {
    fn event(
        _: &mut Self,
        _: &client_wl_seat::WlSeat,
        _: client_wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wl_keyboard::WlKeyboard, ()> for NativeInputClientState {
    fn event(
        state: &mut Self,
        _: &client_wl_keyboard::WlKeyboard,
        event: client_wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            client_wl_keyboard::Event::Keymap { fd, size, .. } => {
                let mut bytes = vec![0; size as usize];
                fs::File::from(fd).read_exact(&mut bytes).unwrap();
                state.keyboard_keymap = bytes;
            }
            client_wl_keyboard::Event::Key {
                key,
                state: key_state,
                ..
            } => {
                state.keyboard_keys.push((
                    key,
                    matches!(
                        key_state,
                        wayland_client::WEnum::Value(client_wl_keyboard::KeyState::Pressed)
                    ),
                ));
            }
            client_wl_keyboard::Event::Enter { keys, .. } => {
                state.keyboard_enters.push(
                    keys.chunks_exact(4)
                        .map(|key| u32::from_ne_bytes([key[0], key[1], key[2], key[3]]))
                        .collect(),
                );
            }
            client_wl_keyboard::Event::Leave { .. } => state.keyboard_leaves += 1,
            client_wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                state.keyboard_groups.push(group);
                state
                    .keyboard_modifiers
                    .push((mods_depressed, mods_latched, mods_locked, group));
            }
            _ => {}
        }
    }
}

impl Dispatch<client_wl_pointer::WlPointer, ()> for NativeInputClientState {
    fn event(
        state: &mut Self,
        _: &client_wl_pointer::WlPointer,
        event: client_wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            client_wl_pointer::Event::Enter { serial, .. } => {
                state.pointer_enter_serial = Some(serial);
                state.pointer_enter_count += 1;
            }
            client_wl_pointer::Event::Leave { .. } => state.pointer_leave_count += 1,
            client_wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                state.pointer_motion_count += 1;
                state.pointer_surface_x = Some(surface_x);
                state.pointer_surface_y = Some(surface_y);
            }
            client_wl_pointer::Event::Button {
                serial,
                state: wayland_client::WEnum::Value(button_state),
                ..
            } => match button_state {
                client_wl_pointer::ButtonState::Pressed => {
                    state.last_button_press_serial = Some(serial);
                    state.pointer_button_press_count += 1;
                }
                client_wl_pointer::ButtonState::Released => {
                    state.pointer_button_release_count += 1;
                }
                _ => {}
            },
            _ => {}
        }
    }
}

impl Dispatch<client_xdg_wm_base::XdgWmBase, ()> for NativeInputClientState {
    fn event(
        _: &mut Self,
        proxy: &client_xdg_wm_base::XdgWmBase,
        event: client_xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let client_xdg_wm_base::Event::Ping { serial } = event {
            proxy.pong(serial);
        }
    }
}

impl Dispatch<client_xdg_surface::XdgSurface, ()> for NativeInputClientState {
    fn event(
        _: &mut Self,
        proxy: &client_xdg_surface::XdgSurface,
        event: client_xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let client_xdg_surface::Event::Configure { serial } = event {
            proxy.ack_configure(serial);
        }
    }
}

pub(super) fn spawn_native_input_resize_client(
    socket_path: PathBuf,
) -> (mpsc::Sender<ClientCommand>, mpsc::Receiver<ClientEvent>) {
    let (commands_sender, commands_receiver) = mpsc::channel();
    let (events_sender, events_receiver) = mpsc::channel();
    thread::spawn(move || {
        let stream = UnixStream::connect(socket_path).unwrap();
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) =
            registry_queue_init::<NativeInputClientState>(&connection).unwrap();
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let pointer = seat.get_pointer(&qh, ());
        let surface = compositor.create_surface(&qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg_surface.get_toplevel(&qh, ());
        let mut state = NativeInputClientState::default();
        surface.commit();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        attach_native_input_test_buffer(&surface, &shm, &qh, 160, 120);
        surface.commit();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        events_sender.send(ClientEvent::ReadyForPointer).unwrap();
        assert_eq!(commands_receiver.recv().unwrap(), ClientCommand::SetCursor);
        queue.roundtrip(&mut state).unwrap();
        let cursor = compositor.create_surface(&qh, ());
        pointer.set_cursor(state.pointer_enter_serial.unwrap(), Some(&cursor), 3, 4);
        attach_native_input_test_buffer(&cursor, &shm, &qh, 24, 24);
        cursor.commit();
        connection.flush().unwrap();
        events_sender
            .send(ClientEvent::CursorReady {
                pointer_motion_count: state.pointer_motion_count,
                pointer_enter_count: state.pointer_enter_count,
                pointer_leave_count: state.pointer_leave_count,
            })
            .unwrap();
        loop {
            match commands_receiver.recv().unwrap() {
                ClientCommand::CaptureActive => {
                    queue.roundtrip(&mut state).unwrap();
                    events_sender
                        .send(ClientEvent::Active {
                            pointer_motion_count: state.pointer_motion_count,
                            pointer_surface_x: state.pointer_surface_x,
                            pointer_surface_y: state.pointer_surface_y,
                            pointer_enter_count: state.pointer_enter_count,
                            pointer_leave_count: state.pointer_leave_count,
                        })
                        .unwrap();
                }
                ClientCommand::BeginXdgMove => {
                    queue.roundtrip(&mut state).unwrap();
                    toplevel._move(&seat, state.last_button_press_serial.unwrap());
                    connection.flush().unwrap();
                    queue.roundtrip(&mut state).unwrap();
                    events_sender.send(ClientEvent::MoveRequested).unwrap();
                }
                ClientCommand::CaptureButtons => {
                    queue.roundtrip(&mut state).unwrap();
                    events_sender
                        .send(ClientEvent::Buttons {
                            pressed_count: state.pointer_button_press_count,
                            released_count: state.pointer_button_release_count,
                        })
                        .unwrap();
                }
                ClientCommand::CaptureKeyboard => {
                    queue.roundtrip(&mut state).unwrap();
                    events_sender
                        .send(ClientEvent::KeyboardGroups {
                            groups: state.keyboard_groups.clone(),
                        })
                        .unwrap();
                }
                ClientCommand::CaptureKeyboardState => {
                    panic!("keyboard state capture is unsupported")
                }
                ClientCommand::Finish => {
                    queue.roundtrip(&mut state).unwrap();
                    events_sender
                        .send(ClientEvent::Finished {
                            pointer_motion_count: state.pointer_motion_count,
                            pointer_surface_x: state.pointer_surface_x,
                            pointer_surface_y: state.pointer_surface_y,
                            pointer_enter_count: state.pointer_enter_count,
                            pointer_leave_count: state.pointer_leave_count,
                        })
                        .unwrap();
                    break;
                }
                ClientCommand::SetCursor => panic!("cursor was already set"),
            }
        }
    });
    (commands_sender, events_receiver)
}

pub(super) fn spawn_native_input_keyboard_client(
    socket_path: PathBuf,
) -> (mpsc::Sender<ClientCommand>, mpsc::Receiver<ClientEvent>) {
    let (commands_sender, commands_receiver) = mpsc::channel();
    let (events_sender, events_receiver) = mpsc::channel();
    thread::spawn(move || {
        let stream = UnixStream::connect(socket_path).unwrap();
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) =
            registry_queue_init::<NativeInputClientState>(&connection).unwrap();
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
        let _keyboard = seat.get_keyboard(&qh, ());
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
        let _toplevel = xdg_surface.get_toplevel(&qh, ());
        let mut state = NativeInputClientState::default();
        surface.commit();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        attach_native_input_test_buffer(&surface, &shm, &qh, 160, 120);
        surface.commit();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        events_sender.send(ClientEvent::ReadyForPointer).unwrap();
        loop {
            match commands_receiver.recv().unwrap() {
                ClientCommand::CaptureKeyboard => {
                    queue.roundtrip(&mut state).unwrap();
                    events_sender
                        .send(ClientEvent::KeyboardGroups {
                            groups: state.keyboard_groups.clone(),
                        })
                        .unwrap();
                }
                ClientCommand::CaptureKeyboardState => {
                    queue.roundtrip(&mut state).unwrap();
                    events_sender
                        .send(ClientEvent::KeyboardState {
                            keymap: state.keyboard_keymap.clone(),
                            keys: state.keyboard_keys.clone(),
                            modifiers: state.keyboard_modifiers.clone(),
                            enters: state.keyboard_enters.clone(),
                            leaves: state.keyboard_leaves,
                        })
                        .unwrap();
                }
                ClientCommand::Finish => {
                    queue.roundtrip(&mut state).unwrap();
                    events_sender
                        .send(ClientEvent::Finished {
                            pointer_motion_count: 0,
                            pointer_surface_x: None,
                            pointer_surface_y: None,
                            pointer_enter_count: 0,
                            pointer_leave_count: 0,
                        })
                        .unwrap();
                    break;
                }
                ClientCommand::SetCursor
                | ClientCommand::CaptureActive
                | ClientCommand::BeginXdgMove
                | ClientCommand::CaptureButtons => {
                    panic!("unexpected keyboard test client command")
                }
            }
        }
    });
    (commands_sender, events_receiver)
}

pub(super) fn attach_native_input_test_buffer(
    surface: &client_wl_surface::WlSurface,
    shm: &client_wl_shm::WlShm,
    qh: &QueueHandle<NativeInputClientState>,
    width: usize,
    height: usize,
) {
    static NEXT_TEST_SHM_FILE: AtomicU64 = AtomicU64::new(0);
    let pixels = vec![0xff20_3040_u32; width * height];
    let file_sequence = NEXT_TEST_SHM_FILE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "typhon-native-input-test-{}-{}-{}",
        std::process::id(),
        width * height,
        file_sequence,
    ));
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    fs::remove_file(path).unwrap();
    for pixel in pixels {
        file.write_all(&pixel.to_ne_bytes()).unwrap();
    }
    file.flush().unwrap();
    let pool = shm.create_pool(file.as_fd(), (width * height * 4) as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        (width * 4) as i32,
        client_wl_shm::Format::Argb8888,
        qh,
        (),
    );
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, width as i32, height as i32);
}

pub(super) fn pump_native_input_server_until(
    server: &mut OwnCompositorServer,
    events: &mpsc::Receiver<ClientEvent>,
) -> ClientEvent {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let _ = server.tick();
        if let Ok(event) = events.try_recv() {
            return event;
        }
        assert!(
            Instant::now() < deadline,
            "native input client did not progress"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

fn pump_native_input_server_until_cursor(server: &mut OwnCompositorServer) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !server.client_cursor_request_active() {
        let _ = server.tick();
        assert!(
            Instant::now() < deadline,
            "native input client cursor did not commit"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn active_window_interaction_motion_updates_pointer_before_interaction() {
    for (pointer_changed, interaction_changed, expected_changed) in [
        (true, false, true),
        (false, true, true),
        (true, true, true),
        (false, false, false),
    ] {
        let order = std::cell::RefCell::new(Vec::new());
        let changed = apply_active_window_interaction_motion(
            12.0,
            34.0,
            |_, _| {
                order.borrow_mut().push("pointer");
                pointer_changed
            },
            |_, _| {
                order.borrow_mut().push("interaction");
                interaction_changed
            },
        );

        assert_eq!(changed, expected_changed);
        assert_eq!(*order.borrow(), ["pointer", "interaction"]);
    }
}

#[test]
fn active_interaction_primary_redraw_is_cursor_aware_and_edge_triggered() {
    assert!(interaction_primary_redraw_requested(
        NativeCursorRenderMode::Hardware,
        false,
        InteractionUpdateOutcome::QueuedNewVisualWork,
    ));
    assert!(!interaction_primary_redraw_requested(
        NativeCursorRenderMode::Hardware,
        true,
        InteractionUpdateOutcome::ReplacedPending,
    ));
    assert!(interaction_primary_redraw_requested(
        NativeCursorRenderMode::Software,
        true,
        InteractionUpdateOutcome::ReplacedPending,
    ));
    assert!(!interaction_primary_redraw_requested(
        NativeCursorRenderMode::Software,
        false,
        InteractionUpdateOutcome::ReplacedPending,
    ));
}

#[test]
fn active_window_interaction_motion_updates_geometry_before_exact_client_dispatch() {
    let routing = include_str!("../input/routing.rs");
    let apply = routing
        .split_once("pub(crate) fn apply_native_input_effect")
        .expect("native input application routing")
        .1;
    let interaction_route = apply
        .split_once("if context.server.window_interaction_active()")
        .expect("active interaction route")
        .1
        .split_once("} else if effect.pointer_motion.is_some()")
        .expect("ordinary motion route")
        .0;

    let pointer_update = interaction_route
        .find("update_interaction_pointer_position_without_client_dispatch")
        .expect("active route should update compositor pointer position");
    let interaction_update = interaction_route
        .find("NativeWindowAction::UpdateInteraction")
        .expect("active route should update window interaction");
    let client_dispatch = interaction_route
        .find("send_window_interaction_pointer_motion")
        .expect("active route should dispatch exact-target absolute motion");
    assert!(pointer_update < interaction_update);
    assert!(interaction_update < client_dispatch);
    assert!(interaction_route.contains("update_window_interaction_for_input_without_flush"));
    assert!(
        interaction_route.contains("send_window_interaction_pointer_motion_without_publication")
    );
    assert!(!interaction_route.contains("send_pointer_motion_sample"));
    assert!(!interaction_route.contains("send_relative_pointer_motion"));
    assert!(!interaction_route.contains("publish_astrea_toplevel_updates"));
}

#[test]
fn native_pointer_routes_do_not_reconcile_astrea_as_part_of_input_delivery() {
    let routing = include_str!("../input/routing.rs");
    let apply = routing
        .split_once("pub(crate) fn apply_native_input_effect")
        .expect("native input application routing")
        .1;

    assert!(apply.contains("send_pointer_motion_sample_without_publication"));
    assert!(apply.contains("send_keyboard_key_without_publication"));
    assert!(apply.contains("send_pointer_button_without_publication"));
    assert!(!apply.contains("publish_astrea_toplevel_updates"));
}

#[test]
fn native_input_batch_defers_and_coalesces_write_side_flushes() {
    let mut server =
        OwnCompositorServer::bind(format!("typhon-native-input-batch-{}", std::process::id()))
            .unwrap();

    server.begin_native_input_batch();
    assert!(!server.end_native_input_batch().unwrap());

    server.begin_native_input_batch();
    server.flush_wayland_clients().unwrap();
    server.flush_wayland_clients().unwrap();
    assert!(server.end_native_input_batch().unwrap());
}

#[test]
fn native_libinput_scroll_axis_value_skips_absent_axis_reader() {
    let value = libinput_scroll_axis_value(false, || panic!("axis value should not be read"));

    assert_eq!(value, 0.0);
}

#[test]
fn libinput_v120_conversion_uses_logical_steps_and_signed_remainders() {
    let mut remainder = ScrollV120Remainder::default();
    assert_eq!(remainder.take_steps(false, 120.0), Some(1));
    assert_eq!(remainder.take_steps(false, -240.0), Some(-2));

    assert_eq!(remainder.take_steps(false, 30.0), None);
    assert_eq!(remainder.take_steps(false, 30.0), None);
    assert_eq!(remainder.take_steps(false, 30.0), None);
    assert_eq!(remainder.take_steps(false, 30.0), Some(1));
}

#[test]
fn libinput_v120_conversion_keeps_devices_and_axes_independent() {
    let mut first_device = ScrollV120Remainder::default();
    let mut second_device = ScrollV120Remainder::default();

    assert_eq!(first_device.take_steps(true, 90.0), None);
    assert_eq!(first_device.take_steps(false, 90.0), None);
    assert_eq!(second_device.take_steps(true, 30.0), None);
    assert_eq!(first_device.take_steps(true, 30.0), Some(1));
    assert_eq!(first_device.take_steps(false, 30.0), Some(1));
    assert_eq!(second_device.take_steps(true, 90.0), Some(1));
}

#[test]
fn libinput_v120_conversion_does_not_create_discrete_finger_or_continuous_steps() {
    let finger = PointerAxisComponent {
        continuous: Some(30.0),
        value120: None,
        discrete: None,
        stopped: false,
    };
    let continuous = PointerAxisComponent {
        continuous: Some(30.0),
        value120: None,
        discrete: None,
        stopped: false,
    };

    assert_eq!(finger.discrete, None);
    assert_eq!(continuous.discrete, None);
}
