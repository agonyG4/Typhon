use super::*;
use crate::native_output::runtime::{
    NativePointerConstraint, NativePointerConstraintBackendAction,
};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::{
        fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_buffer as client_wl_buffer, wl_compositor as client_wl_compositor,
        wl_pointer as client_wl_pointer, wl_registry, wl_seat as client_wl_seat,
        wl_shm as client_wl_shm, wl_shm_pool as client_wl_shm_pool,
        wl_surface as client_wl_surface,
    },
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface as client_xdg_surface, xdg_toplevel as client_xdg_toplevel,
    xdg_wm_base as client_xdg_wm_base,
};

#[test]
fn raw_evdev_events_discarded_during_suspend_do_not_replay() {
    let mut pipe = [0; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) },
        0
    );
    let read = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
    let mut devices = NativeInputDevices {
        devices: vec![NativeInputDevice {
            file: fs::File::from(read),
            path: PathBuf::from("test-event"),
        }],
        suspended: false,
    };
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
    let mut backend = NativeInputBackend::RawEvdev(NativeInputDevices {
        devices: vec![NativeInputDevice {
            file: fs::File::from(read),
            path: PathBuf::from("test-event"),
        }],
        suspended: false,
    });

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
            seat_session: None,
            process_supervisor: &mut process_supervisor,
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
            seat_session: None,
            process_supervisor: &mut process_supervisor,
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
            seat_session: None,
            process_supervisor: &mut process_supervisor,
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
            seat_session: None,
            process_supervisor: &mut process_supervisor,
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
                seat_session: None,
                process_supervisor: &mut process_supervisor,
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
        PointerConstraintBackendRequest::ActivateLocked {
            id,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
        },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );
    let duplicate = backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked {
            id,
            anchor: CompositorOutputPosition { x: 30.0, y: 40.0 },
        },
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
        PointerConstraintBackendRequest::ActivateLocked {
            id: active,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
        },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );

    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id: stale,
            restore_position: Some(CompositorOutputPosition { x: 40.0, y: 50.0 }),
        },
        CompositorOutputPosition { x: 99.0, y: 99.0 },
    );

    assert_eq!(action, NativePointerConstraintBackendAction::default());
    assert!(backend.active_locked());
}

#[test]
fn native_pointer_constraint_backend_deactivation_restores_hint_or_anchor() {
    let mut backend = NativePointerConstraintBackend::new();
    let id = PointerConstraintBackendId {
        constraint_id: 10,
        generation: 1,
    };
    backend.handle_request(
        PointerConstraintBackendRequest::ActivateLocked {
            id,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
        },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );

    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: Some(CompositorOutputPosition { x: 30.0, y: 40.0 }),
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
        PointerConstraintBackendRequest::ActivateLocked {
            id,
            anchor: CompositorOutputPosition { x: 10.0, y: 20.0 },
        },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );
    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: None,
        },
        CompositorOutputPosition { x: 99.0, y: 99.0 },
    );

    assert_eq!(
        action.restore_position,
        Some(CompositorOutputPosition { x: 10.0, y: 20.0 })
    );
}

#[test]
fn native_pointer_constraint_backend_preserves_fractional_activation_anchor() {
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
        PointerConstraintBackendRequest::ActivateLocked { id, anchor },
        anchor,
    );
    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: None,
        },
        CompositorOutputPosition { x: 0.0, y: 0.0 },
    );

    assert_eq!(action.restore_position, Some(anchor));
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
        },
        CompositorOutputPosition { x: 30.0, y: 40.0 },
    );

    let action = backend.handle_request(
        PointerConstraintBackendRequest::Deactivate {
            id,
            restore_position: None,
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
fn native_input_shortcut_inhibition_forwards_window_shortcuts_to_client() {
    let mut input = NativeInputState::new(320, 200);
    input.set_keyboard_shortcuts_inhibited(true);

    let alt = input.handle_key_event(KEY_LEFTALT, 1);
    let fullscreen = input.handle_key_event(KEY_F11, 1);

    assert_eq!(
        alt.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_LEFTALT, true)]
    );
    assert!(alt.window_actions.is_empty());
    assert_eq!(
        fullscreen.keyboard_events,
        vec![NativeKeyboardEvent::new(KEY_F11, true)]
    );
    assert!(fullscreen.window_actions.is_empty());
}

#[test]
fn native_input_shortcut_inhibition_keeps_emergency_exit_shortcut() {
    let mut input = NativeInputState::new(320, 200);
    input.set_keyboard_shortcuts_inhibited(true);
    input.handle_key_event(KEY_LEFTALT, 1);

    let effect = input.handle_key_event(KEY_P, 1);

    assert!(effect.exit_requested);
    assert!(effect.keyboard_events.is_empty());
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

    assert_eq!(effect.pointer_axis, Some((0.0, 120.0)));
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
    let x = 132.0;
    let y = 112.0;
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
            seat_session: None,
            process_supervisor: &mut process_supervisor,
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
    client_commands.send(ClientCommand::CaptureActive).unwrap();
    let active = pump_native_input_server_until(&mut server, &client_events);
    assert_eq!(
        active,
        ClientEvent::Active {
            pointer_motion_count: pointer_motion_count + 1,
            pointer_surface_x: Some(60.0),
            pointer_surface_y: Some(40.0),
            pointer_enter_count,
            pointer_leave_count,
        }
    );
    server.prepare_frame();
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
            seat_session: None,
            process_supervisor: &mut process_supervisor,
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
            seat_session: None,
            process_supervisor: &mut process_supervisor,
        },
    )
    .unwrap();

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
            pointer_surface_x: Some(61.0),
            pointer_surface_y: Some(42.0),
            pointer_enter_count,
            pointer_leave_count,
        }
    );
}

#[derive(Debug, PartialEq)]
enum ClientEvent {
    ReadyForPointer,
    Active {
        pointer_motion_count: usize,
        pointer_surface_x: Option<f64>,
        pointer_surface_y: Option<f64>,
        pointer_enter_count: usize,
        pointer_leave_count: usize,
    },
    CursorReady {
        pointer_motion_count: usize,
        pointer_enter_count: usize,
        pointer_leave_count: usize,
    },
    Finished {
        pointer_motion_count: usize,
        pointer_surface_x: Option<f64>,
        pointer_surface_y: Option<f64>,
        pointer_enter_count: usize,
        pointer_leave_count: usize,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum ClientCommand {
    SetCursor,
    CaptureActive,
    Finish,
}

#[derive(Default)]
struct NativeInputClientState {
    pointer_enter_serial: Option<u32>,
    pointer_motion_count: usize,
    pointer_enter_count: usize,
    pointer_leave_count: usize,
    pointer_surface_x: Option<f64>,
    pointer_surface_y: Option<f64>,
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
native_input_noop_dispatch!(client_wl_seat::WlSeat);
native_input_noop_dispatch!(client_wl_shm::WlShm);
native_input_noop_dispatch!(client_wl_shm_pool::WlShmPool);
native_input_noop_dispatch!(client_wl_buffer::WlBuffer);
native_input_noop_dispatch!(client_xdg_toplevel::XdgToplevel);

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

fn spawn_native_input_resize_client(
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
        let _toplevel = xdg_surface.get_toplevel(&qh, ());
        let mut state = NativeInputClientState::default();
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
                assert_eq!(commands_receiver.recv().unwrap(), ClientCommand::Finish);
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
            }
            ClientCommand::SetCursor => panic!("cursor was already set"),
        }
    });
    (commands_sender, events_receiver)
}

fn attach_native_input_test_buffer(
    surface: &client_wl_surface::WlSurface,
    shm: &client_wl_shm::WlShm,
    qh: &QueueHandle<NativeInputClientState>,
    width: usize,
    height: usize,
) {
    let pixels = vec![0xff20_3040_u32; width * height];
    let path = std::env::temp_dir().join(format!(
        "typhon-native-input-test-{}-{}",
        std::process::id(),
        width * height
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

fn pump_native_input_server_until(
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
        .find("update_pointer_position_without_client_dispatch")
        .expect("active route should update compositor pointer position");
    let interaction_update = interaction_route
        .find("NativeWindowAction::UpdateInteraction")
        .expect("active route should update window interaction");
    let client_dispatch = interaction_route
        .find("send_window_interaction_pointer_motion")
        .expect("active route should dispatch exact-target absolute motion");
    assert!(pointer_update < interaction_update);
    assert!(interaction_update < client_dispatch);
    assert!(!interaction_route.contains("send_pointer_motion_sample"));
    assert!(!interaction_route.contains("send_relative_pointer_motion"));
}

#[test]
fn native_libinput_scroll_axis_value_skips_absent_axis_reader() {
    let value = libinput_scroll_axis_value(false, || panic!("axis value should not be read"));

    assert_eq!(value, 0.0);
}
