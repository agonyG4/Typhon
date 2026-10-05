use super::{
    ControllerDeviceId,
    device::{
        ControllerIdAllocator, InputDeviceMetadata, classify_controller_capabilities,
        process_synchronized_events,
    },
    frame::{
        ABS_HAT0X, ABS_RX, ABS_RY, ABS_X, ABS_Y, AxisRange, ControllerFrameBuilder,
        ControllerInputEvent,
    },
    manager::{
        ControllerEventBudget, ControllerManager, MAX_DISCOVERY_CANDIDATES,
        MAX_RAW_EVENTS_PER_CYCLE, drain_admitted_batch, event_candidate_paths,
        rotate_ready_work_after_last_serviced,
    },
    policy::{ControllerPolicy, parse_controller_policy},
};
use oblivion_one::compositor::{IdleManager, IdleState};
use oblivion_one::native::event_loop::{
    ControllerDeviceReadyEvent, NativeEventLoop, NativeEventSource,
};
use std::{
    cell::Cell,
    fs,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::PathBuf,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

const BTN_GAMEPAD: u16 = 0x130;
const BTN_MOUSE_LEFT: u16 = 0x110;
const BTN_TRIGGER_HAPPY: u16 = 0x2c0;
const KEY_A: u16 = 30;

#[test]
fn controller_policy_defaults_to_off_and_accepts_only_supported_values() {
    assert_eq!(parse_controller_policy(None), Ok(ControllerPolicy::Off));
    assert_eq!(
        parse_controller_policy(Some("off")),
        Ok(ControllerPolicy::Off)
    );
    assert_eq!(
        parse_controller_policy(Some("observe")),
        Ok(ControllerPolicy::Observe)
    );
    assert!(parse_controller_policy(Some("on")).is_err());
}

#[test]
fn controller_capabilities_require_controller_buttons_and_structure() {
    let metadata = InputDeviceMetadata::default();

    assert!(classify_controller_capabilities(
        &[BTN_GAMEPAD, BTN_GAMEPAD + 1],
        &[ABS_X, ABS_Y],
        metadata,
    ));
    assert!(classify_controller_capabilities(
        &[BTN_GAMEPAD],
        &[ABS_RX, ABS_RY],
        metadata,
    ));
}

#[test]
fn ordinary_keyboard_mouse_and_touchpad_capabilities_are_rejected() {
    let metadata = InputDeviceMetadata::default();

    assert!(!classify_controller_capabilities(
        &[KEY_A],
        &[],
        InputDeviceMetadata {
            keyboard: true,
            ..metadata
        },
    ));
    assert!(!classify_controller_capabilities(
        &[BTN_MOUSE_LEFT],
        &[ABS_X, ABS_Y],
        InputDeviceMetadata {
            mouse: true,
            ..metadata
        },
    ));
    assert!(!classify_controller_capabilities(
        &[BTN_MOUSE_LEFT],
        &[ABS_X, ABS_Y],
        InputDeviceMetadata {
            touchpad: true,
            ..metadata
        },
    ));
}

#[test]
fn controller_buttons_without_a_stick_or_hat_pair_are_rejected() {
    assert!(!classify_controller_capabilities(
        &[BTN_GAMEPAD, BTN_GAMEPAD + 1],
        &[2, 5],
        InputDeviceMetadata::default(),
    ));
    assert!(!classify_controller_capabilities(
        &[BTN_TRIGGER_HAPPY],
        &[ABS_X, ABS_Y],
        InputDeviceMetadata::default(),
    ));
}

#[test]
fn multiple_absolute_updates_before_syn_report_make_one_frame() {
    let mut builder = frame_builder();

    assert!(
        builder
            .process(ControllerInputEvent::Absolute {
                code: ABS_X,
                value: 200
            })
            .is_none()
    );
    assert!(
        builder
            .process(ControllerInputEvent::Absolute {
                code: ABS_Y,
                value: -100
            })
            .is_none()
    );
    let frame = builder
        .process(ControllerInputEvent::SynReport)
        .expect("SYN_REPORT produces the logical frame");

    assert_eq!(frame.left_stick, [0.2, -0.1]);
    assert!(frame.activity_transition);
    assert!(builder.process(ControllerInputEvent::SynReport).is_none());
}

#[test]
fn button_and_dpad_transitions_are_meaningful_activity() {
    let mut builder = frame_builder();
    builder.process(ControllerInputEvent::Key {
        code: BTN_GAMEPAD,
        value: 1,
    });
    let button = builder
        .process(ControllerInputEvent::SynReport)
        .expect("button frame");
    assert!(button.button_transition);
    assert!(button.activity_transition);

    builder.process(ControllerInputEvent::Absolute {
        code: ABS_HAT0X,
        value: 1,
    });
    let dpad = builder
        .process(ControllerInputEvent::SynReport)
        .expect("d-pad frame");
    assert!(dpad.dpad_transition);
    assert!(dpad.activity_transition);
}

#[test]
fn analog_drift_does_not_enter_the_activity_deadzone() {
    let mut builder = frame_builder();
    builder.process(ControllerInputEvent::Absolute {
        code: ABS_X,
        value: 100,
    });

    let frame = builder
        .process(ControllerInputEvent::SynReport)
        .expect("axis state frame");

    assert!(!frame.activity_transition);
}

#[test]
fn stick_hysteresis_emits_only_on_enter_and_reenters_after_exit() {
    let mut builder = frame_builder();
    builder.process(ControllerInputEvent::Absolute {
        code: ABS_X,
        value: 200,
    });
    assert!(
        builder
            .process(ControllerInputEvent::SynReport)
            .expect("enter frame")
            .activity_transition
    );

    builder.process(ControllerInputEvent::Absolute {
        code: ABS_X,
        value: 400,
    });
    assert!(
        !builder
            .process(ControllerInputEvent::SynReport)
            .expect("held frame")
            .activity_transition
    );

    builder.process(ControllerInputEvent::Absolute {
        code: ABS_X,
        value: 100,
    });
    assert!(
        !builder
            .process(ControllerInputEvent::SynReport)
            .expect("exit frame")
            .activity_transition
    );

    builder.process(ControllerInputEvent::Absolute {
        code: ABS_X,
        value: 200,
    });
    assert!(
        builder
            .process(ControllerInputEvent::SynReport)
            .expect("second enter frame")
            .activity_transition
    );
}

#[test]
fn analog_normalization_uses_device_ranges() {
    let mut ranges = empty_axis_ranges();
    ranges[ABS_X as usize] = AxisRange::new(100, 900);
    ranges[ABS_Y as usize] = AxisRange::new(-100, 100);
    let mut builder = ControllerFrameBuilder::new(ranges);
    builder.process(ControllerInputEvent::Absolute {
        code: ABS_X,
        value: 680,
    });
    builder.process(ControllerInputEvent::Absolute {
        code: ABS_Y,
        value: -100,
    });

    let frame = builder
        .process(ControllerInputEvent::SynReport)
        .expect("normalized frame");

    assert_eq!(frame.left_stick, [0.45, -1.0]);
}

#[test]
fn trigger_activity_is_normalized_from_the_observed_rest_point() {
    let mut ranges = empty_axis_ranges();
    ranges[2] = AxisRange::new(-1_000, 1_000).with_initial_value(0);
    let mut builder = ControllerFrameBuilder::new(ranges);

    builder.process(ControllerInputEvent::Absolute {
        code: 2,
        value: 100,
    });
    let drift = builder
        .process(ControllerInputEvent::SynReport)
        .expect("trigger drift frame");
    assert_eq!(drift.triggers[0], 0.1);
    assert!(!drift.activity_transition);

    builder.process(ControllerInputEvent::Absolute {
        code: 2,
        value: 400,
    });
    let pressed = builder
        .process(ControllerInputEvent::SynReport)
        .expect("trigger press frame");
    assert_eq!(pressed.triggers[0], 0.4);
    assert!(pressed.activity_transition);
}

#[test]
fn session_reset_clears_controller_activity_hysteresis() {
    let mut builder = frame_builder();
    builder.process(ControllerInputEvent::Absolute {
        code: ABS_X,
        value: 300,
    });
    assert!(
        builder
            .process(ControllerInputEvent::SynReport)
            .expect("active frame")
            .activity_transition
    );

    builder.clear_session_state();
    builder.process(ControllerInputEvent::Absolute {
        code: ABS_X,
        value: 200,
    });

    assert!(
        builder
            .process(ControllerInputEvent::SynReport)
            .expect("post-resume frame")
            .activity_transition
    );
}

#[test]
fn controller_id_allocator_never_reuses_a_prior_id() {
    let mut ids = ControllerIdAllocator::default();
    let removed = ids.allocate().expect("first id");
    let replugged = ids.allocate().expect("fresh id after unplug");

    assert_ne!(removed, replugged);
    assert!(replugged.get() > removed.get());
}

#[test]
fn controller_batch_admission_budget_counts_whole_batches() {
    let mut budget = ControllerEventBudget::default();

    assert!(budget.can_admit_batch());
    assert!(!budget.account_batch(MAX_RAW_EVENTS_PER_CYCLE - 1));
    assert!(budget.can_admit_batch());
    assert!(budget.account_batch(2));
    assert!(!budget.can_admit_batch());
    assert_eq!(budget.processed(), MAX_RAW_EVENTS_PER_CYCLE + 1);
    assert_eq!(MAX_RAW_EVENTS_PER_CYCLE, 384);
}

#[test]
fn synchronized_batch_is_fully_consumed_before_budget_admission() {
    let mut budget = ControllerEventBudget::default();
    budget.account_batch(MAX_RAW_EVENTS_PER_CYCLE - 2);
    assert_eq!(budget.remaining(), 2);

    let events = [
        ControllerInputEvent::Other,
        ControllerInputEvent::Other,
        ControllerInputEvent::Key {
            code: BTN_GAMEPAD,
            value: 1,
        },
        ControllerInputEvent::SynReport,
    ];
    let consumed = Rc::new(Cell::new(0));
    let fully_consumed_on_drop = Rc::new(Cell::new(false));
    let batch = DropObservedBatch::new(
        events,
        Rc::clone(&consumed),
        Rc::clone(&fully_consumed_on_drop),
    );
    let mut builder = frame_builder();
    let mut admitted_batches = 0;

    let (stats, exhausted) = drain_admitted_batch(&mut budget, || {
        admitted_batches += 1;
        Ok(process_synchronized_events(batch, &mut builder))
    })
    .expect("admitted fetch batch drains successfully")
    .expect("remaining budget admits the first batch");

    assert!(fully_consumed_on_drop.get());
    assert_eq!(consumed.get(), events.len());
    assert_eq!(stats.raw_events, events.len());
    assert_eq!(
        stats.logical_frames, 1,
        "tail SYN_REPORT must finish the frame"
    );
    assert_eq!(
        stats.activity_transitions, 1,
        "tail button activity must survive"
    );

    assert!(exhausted);
    assert_eq!(budget.processed(), MAX_RAW_EVENTS_PER_CYCLE + 2);
    assert!(
        !budget.can_admit_batch(),
        "the over-budget batch closes admission"
    );
    let second_batch = drain_admitted_batch(&mut budget, || {
        admitted_batches += 1;
        Ok(Default::default())
    })
    .expect("budget exhaustion is not an I/O error");
    assert!(second_batch.is_none());
    assert_eq!(
        admitted_batches, 1,
        "no second batch is admitted after exhaustion"
    );
}

#[test]
fn budget_exhaustion_rotates_the_next_pending_controller_to_the_front() {
    let first = ControllerDeviceId::from_raw(1).expect("first controller ID");
    let second = ControllerDeviceId::from_raw(2).expect("second controller ID");
    let third = ControllerDeviceId::from_raw(3).expect("third controller ID");
    let mut ready = [first, second, third];

    rotate_ready_work_after_last_serviced(&mut ready, Some(first));

    assert_eq!(ready, [second, third, first]);
}

#[test]
fn discovery_cap_counts_event_paths_after_filtering_other_entries() {
    let mut paths: Vec<_> = (0..MAX_DISCOVERY_CANDIDATES * 2)
        .map(|index| PathBuf::from(format!("/dev/input/by-id/controller-{index}")))
        .collect();
    paths.push(PathBuf::from("/dev/input/event17"));
    paths.push(PathBuf::from("/dev/input/event18"));

    let candidates: Vec<_> = event_candidate_paths(paths.into_iter()).collect();

    assert_eq!(
        candidates,
        [
            PathBuf::from("/dev/input/event17"),
            PathBuf::from("/dev/input/event18")
        ]
    );
    let too_many_events = (0..MAX_DISCOVERY_CANDIDATES + 1)
        .map(|index| PathBuf::from(format!("/dev/input/event{index}")));
    assert_eq!(
        event_candidate_paths(too_many_events).count(),
        MAX_DISCOVERY_CANDIDATES
    );
}

struct DropObservedBatch {
    events: std::array::IntoIter<ControllerInputEvent, 4>,
    consumed: Rc<Cell<usize>>,
    fully_consumed_on_drop: Rc<Cell<bool>>,
    total: usize,
}

impl DropObservedBatch {
    fn new(
        events: [ControllerInputEvent; 4],
        consumed: Rc<Cell<usize>>,
        fully_consumed_on_drop: Rc<Cell<bool>>,
    ) -> Self {
        let total = events.len();
        Self {
            events: events.into_iter(),
            consumed,
            fully_consumed_on_drop,
            total,
        }
    }
}

impl Iterator for DropObservedBatch {
    type Item = ControllerInputEvent;

    fn next(&mut self) -> Option<Self::Item> {
        let event = self.events.next();
        if event.is_some() {
            self.consumed.set(self.consumed.get() + 1);
        }
        event
    }
}

impl Drop for DropObservedBatch {
    fn drop(&mut self) {
        self.fully_consumed_on_drop
            .set(self.consumed.get() == self.total);
    }
}

#[test]
fn disabled_controller_manager_has_no_monitor_or_device_fds() {
    let manager = ControllerManager::new(ControllerPolicy::Off);

    assert!(manager.monitor_fd().is_none());
    assert_eq!(manager.connected_count(), 0);
}

#[test]
fn stale_readiness_for_a_removed_controller_id_is_ignored() {
    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "typhon-controller-stale-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let udev = root.join("udev");
    fs::create_dir_all(&root).expect("temporary input directory");
    let mut manager = ControllerManager::with_paths(ControllerPolicy::Observe, root.clone(), udev);

    let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
    assert!(fd >= 0);
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut event_loop = NativeEventLoop::new().expect("native event loop");
    let id = ControllerDeviceId::from_raw(73).expect("nonzero controller id");
    let token = event_loop
        .register(
            fd.as_raw_fd(),
            NativeEventSource::ControllerDevice(id.get()),
        )
        .expect("register stale controller source");
    let readiness = [ControllerDeviceReadyEvent {
        device_id: id.get(),
        token,
        flags: libc::EPOLLIN as u32,
    }];

    let outcome = manager.drain_ready(&readiness, |candidate, registered| {
        candidate == id && registered == token
    });

    assert!(!outcome.meaningful_activity);
    assert!(!outcome.backlog_pending);
    assert_eq!(manager.telemetry().raw_events, 0);
    assert_eq!(manager.telemetry().read_failures, 0);
    drop(event_loop);
    drop(fd);
    drop(manager);
    fs::remove_dir_all(root).expect("remove temporary input directory");
}

#[test]
fn controller_monitor_suspend_resume_reopens_and_rescans_without_stale_activity() {
    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "typhon-controller-session-{}-{}",
        std::process::id(),
        NEXT_DIR.fetch_add(1, Ordering::Relaxed)
    ));
    let udev = root.join("udev");
    fs::create_dir_all(&root).expect("temporary input directory");

    let mut manager = ControllerManager::with_paths(ControllerPolicy::Observe, root.clone(), udev);
    assert!(manager.monitor_fd().is_some());
    assert_eq!(manager.connected_count(), 0);

    manager.suspend();
    assert!(manager.monitor_fd().is_none());
    let stale = manager.drain_ready(&[], |_, _| false);
    assert!(!stale.meaningful_activity);
    assert!(!stale.backlog_pending);

    fs::write(root.join("event900"), b"not an evdev device")
        .expect("synthetic event-node placeholder");
    manager.resume();
    assert!(manager.monitor_fd().is_some());
    assert_eq!(manager.connected_count(), 0);
    assert!(manager.telemetry().read_failures > 0);
    assert_eq!(manager.telemetry().activity_transitions, 0);

    fs::remove_dir_all(root).expect("remove temporary input directory");
}

#[test]
fn controller_semantic_activity_resets_the_shared_idle_authority_once() {
    let start = std::time::Instant::now();
    let activity_at = start + std::time::Duration::from_secs(8);
    let mut idle = IdleManager::new(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_secs(10),
        start,
    );
    let mut builder = frame_builder();

    builder.process(ControllerInputEvent::Key {
        code: BTN_GAMEPAD,
        value: 1,
    });
    let button = builder
        .process(ControllerInputEvent::SynReport)
        .expect("button frame");
    if button.activity_transition {
        idle.notify_activity_at(activity_at);
    }
    assert_eq!(
        idle.state_at(start + std::time::Duration::from_secs(12)),
        IdleState::Active
    );

    builder.process(ControllerInputEvent::Absolute {
        code: ABS_X,
        value: 100,
    });
    let drift = builder
        .process(ControllerInputEvent::SynReport)
        .expect("drift frame");
    assert!(!drift.activity_transition);
    assert_eq!(
        idle.state_at(start + std::time::Duration::from_secs(14)),
        IdleState::Idle
    );
}

fn empty_axis_ranges() -> [AxisRange; 64] {
    [AxisRange::default(); 64]
}

fn frame_builder() -> ControllerFrameBuilder {
    let mut ranges = empty_axis_ranges();
    ranges[ABS_X as usize] = AxisRange::new(-1_000, 1_000);
    ranges[ABS_Y as usize] = AxisRange::new(-1_000, 1_000);
    ranges[ABS_RX as usize] = AxisRange::new(-1_000, 1_000);
    ranges[ABS_RY as usize] = AxisRange::new(-1_000, 1_000);
    ranges[ABS_HAT0X as usize] = AxisRange::new(-1, 1);
    ControllerFrameBuilder::new(ranges)
}
