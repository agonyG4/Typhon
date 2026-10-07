use super::*;
use crate::system_action_protocol::{PROTOCOL_MAJOR, PROTOCOL_MINOR, SystemActionMessage};

struct RuntimeTempDir(PathBuf);

impl Drop for RuntimeTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn runtime_dir() -> RuntimeTempDir {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "typhon-system-action-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let path = root.join("runtime");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    RuntimeTempDir(root)
}

fn runtime_path(root: &RuntimeTempDir) -> PathBuf {
    root.0.join("runtime")
}

#[test]
fn final_endpoint_path_limit_is_checked_before_temporary_bind() {
    let root = runtime_dir();
    let mut event_loop = NativeEventLoop::new().unwrap();
    let error =
        NativeSystemActionTransport::bind(&mut event_loop, &runtime_path(&root), &"i".repeat(70))
            .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
}

fn connect_client(path: &Path) -> OwnedFd {
    let (address, address_len) = unix_socket_address(path).unwrap();
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0) };
    assert!(fd >= 0);
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let result = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            address_len,
        )
    };
    assert_eq!(result, 0, "connect: {}", io::Error::last_os_error());
    fd
}

fn client_send(fd: RawFd, packet: &[u8; PACKET_SIZE]) {
    let sent = unsafe { libc::send(fd, packet.as_ptr().cast(), packet.len(), libc::MSG_NOSIGNAL) };
    assert_eq!(sent, packet.len() as isize);
}

fn client_receive(fd: RawFd) -> [u8; PACKET_SIZE] {
    let mut packet = [0_u8; PACKET_SIZE];
    let received = unsafe { libc::recv(fd, packet.as_mut_ptr().cast(), packet.len(), 0) };
    assert_eq!(received, PACKET_SIZE as isize);
    packet
}

fn ready_transport(
    caps: AstreaSystemActionCapabilities,
) -> (
    NativeSystemActionTransport,
    NativeEventLoop,
    RuntimeTempDir,
    OwnedFd,
) {
    let root = runtime_dir();
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut transport =
        NativeSystemActionTransport::bind(&mut event_loop, &runtime_path(&root), "test-instance")
            .unwrap();
    let paths =
        ControlRuntimePaths::for_runtime_dir(&runtime_path(&root), "test-instance").unwrap();
    let client = connect_client(&paths.prepare_system_action_endpoint().unwrap());
    client_send(
        client.as_raw_fd(),
        &encode(SystemActionMessage::Hello {
            major: PROTOCOL_MAJOR,
            minor: PROTOCOL_MINOR,
            capabilities: caps.wire_bits(),
        }),
    );
    let listener_wakeup = event_loop.wait().unwrap();
    transport.service(&mut event_loop, &listener_wakeup.system_action_events, 100);
    let peer_wakeup = event_loop.wait().unwrap();
    transport.service(&mut event_loop, &peer_wakeup.system_action_events, 101);
    assert_eq!(transport.take_capability_update(), Some(caps));
    let welcome = decode(&client_receive(client.as_raw_fd())).unwrap();
    assert!(matches!(welcome, SystemActionMessage::Welcome { .. }));
    (transport, event_loop, root, client)
}

#[test]
fn queue_coalesces_only_adjacent_identical_steps_and_is_bounded() {
    let mut queue = FixedActionQueue::default();
    for occurrence in 0..5 {
        assert_eq!(
            queue.push(AstreaSystemAction::OutputVolumeUp),
            if occurrence == 0 {
                QueuePush::Queued
            } else {
                QueuePush::Coalesced
            }
        );
    }
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.front().unwrap().occurrences, 5);

    queue.clear();
    assert_eq!(
        queue.push(AstreaSystemAction::OutputVolumeUp),
        QueuePush::Queued
    );
    assert_eq!(
        queue.push(AstreaSystemAction::ToggleOutputMute),
        QueuePush::Queued
    );
    assert_eq!(
        queue.push(AstreaSystemAction::OutputVolumeUp),
        QueuePush::Queued
    );
    assert_eq!(queue.len(), 3);
    queue.clear();
    assert_eq!(queue.push(AstreaSystemAction::MediaNext), QueuePush::Queued);
    assert_eq!(queue.push(AstreaSystemAction::MediaNext), QueuePush::Queued);
    assert_eq!(queue.len(), 2);

    queue.clear();
    for index in 0..SYSTEM_ACTION_OUTBOUND_CAPACITY {
        assert_eq!(
            queue.push(if index % 2 == 0 {
                AstreaSystemAction::ToggleOutputMute
            } else {
                AstreaSystemAction::MediaNext
            }),
            QueuePush::Queued
        );
    }
    assert_eq!(
        queue.push(AstreaSystemAction::ToggleMicrophoneMute),
        QueuePush::Full
    );

    queue.clear();
    for index in 0..SYSTEM_ACTION_OUTBOUND_CAPACITY - 1 {
        assert_eq!(
            queue.push(if index % 2 == 0 {
                AstreaSystemAction::ToggleOutputMute
            } else {
                AstreaSystemAction::MediaNext
            }),
            QueuePush::Queued
        );
    }
    assert_eq!(
        queue.push(AstreaSystemAction::OutputVolumeUp),
        QueuePush::Queued
    );
    assert_eq!(
        queue.push(AstreaSystemAction::OutputVolumeUp),
        QueuePush::Coalesced
    );
    assert_eq!(queue.len(), SYSTEM_ACTION_OUTBOUND_CAPACITY);
}

#[test]
fn capability_replacement_removes_unsupported_queued_actions_in_order() {
    let mut queue = FixedActionQueue::default();
    queue.push(AstreaSystemAction::OutputVolumeUp);
    queue.push(AstreaSystemAction::ToggleOutputMute);
    queue.push(AstreaSystemAction::MediaNext);
    let supported = AstreaSystemActionCapabilities::for_action(AstreaSystemAction::OutputVolumeUp)
        .union(AstreaSystemActionCapabilities::for_action(
            AstreaSystemAction::MediaNext,
        ));
    queue.retain_capabilities(supported);
    assert_eq!(queue.len(), 2);
    assert_eq!(
        queue.pop().unwrap().action,
        AstreaSystemAction::OutputVolumeUp
    );
    assert_eq!(queue.pop().unwrap().action, AstreaSystemAction::MediaNext);
}

#[test]
fn same_uid_handshake_activates_only_the_welcomed_capability_mask() {
    let caps = AstreaSystemActionCapabilities::for_action(AstreaSystemAction::OutputVolumeUp);
    let (transport, _event_loop, _root, _client) = ready_transport(caps);
    let snapshot = transport.snapshot();
    assert!(snapshot.peer_connected);
    assert!(snapshot.protocol_ready);
    assert_eq!(snapshot.outbound_queue_depth, 0);
}

#[test]
fn advertised_capabilities_stay_empty_until_welcome_is_sent() {
    let caps = AstreaSystemActionCapabilities::for_action(AstreaSystemAction::OutputVolumeUp);
    let root = runtime_dir();
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut transport = NativeSystemActionTransport::bind(
        &mut event_loop,
        &runtime_path(&root),
        "welcome-backpressure",
    )
    .unwrap();
    let paths =
        ControlRuntimePaths::for_runtime_dir(&runtime_path(&root), "welcome-backpressure").unwrap();
    let client = connect_client(&paths.prepare_system_action_endpoint().unwrap());
    let accepted = event_loop.wait().unwrap();
    transport.service(&mut event_loop, &accepted.system_action_events, 100);

    let send_buffer = 1_024_i32;
    let result = unsafe {
        libc::setsockopt(
            transport.peer.as_ref().unwrap().fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            (&send_buffer as *const i32).cast(),
            std::mem::size_of_val(&send_buffer) as libc::socklen_t,
        )
    };
    assert_eq!(result, 0, "setsockopt: {}", io::Error::last_os_error());
    let filler = [0_u8; PACKET_SIZE];
    let mut blocked = false;
    for _ in 0..100_000 {
        match send_packet(transport.peer.as_ref().unwrap().fd.as_raw_fd(), &filler) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                blocked = true;
                break;
            }
            Err(error) => panic!("filling test socket failed: {error}"),
        }
    }
    assert!(blocked, "the bounded socket send buffer did not fill");

    client_send(
        client.as_raw_fd(),
        &encode(SystemActionMessage::Hello {
            major: PROTOCOL_MAJOR,
            minor: PROTOCOL_MINOR,
            capabilities: caps.wire_bits(),
        }),
    );
    let hello = event_loop.wait().unwrap();
    transport.service(&mut event_loop, &hello.system_action_events, 101);
    assert_eq!(
        transport.peer.as_ref().unwrap().phase,
        PeerPhase::SendingWelcome
    );
    assert_eq!(
        transport.peer.as_ref().unwrap().pending_capabilities,
        Some(caps)
    );
    assert_eq!(transport.take_capability_update(), None);

    let mut received = [0_u8; PACKET_SIZE + 1];
    loop {
        let count = unsafe {
            libc::recv(
                client.as_raw_fd(),
                received.as_mut_ptr().cast(),
                received.len(),
                libc::MSG_DONTWAIT | libc::MSG_TRUNC,
            )
        };
        if count >= 0 {
            continue;
        }
        if io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
            break;
        }
        panic!("client drain failed: {}", io::Error::last_os_error());
    }

    let writable = event_loop.wait().unwrap();
    assert!(
        writable
            .system_action_events
            .iter()
            .any(|event| { event.flags & libc::EPOLLOUT as u32 != 0 })
    );
    transport.service(&mut event_loop, &writable.system_action_events, 102);
    assert_eq!(transport.take_capability_update(), Some(caps));
    assert!(matches!(
        decode(&client_receive(client.as_raw_fd())).unwrap(),
        SystemActionMessage::Welcome { capabilities, .. } if capabilities == caps.wire_bits()
    ));
    transport.shutdown(&mut event_loop);
}

#[test]
fn ready_peer_receives_typed_action_packet() {
    let volume = AstreaSystemActionCapabilities::for_action(AstreaSystemAction::OutputVolumeUp);
    let (mut transport, mut event_loop, _root, client) = ready_transport(volume);
    assert_eq!(
        transport.submit(&mut event_loop, AstreaSystemAction::OutputVolumeUp),
        SystemActionSubmitResult::Sent
    );
    assert_eq!(
        decode(&client_receive(client.as_raw_fd())).unwrap(),
        SystemActionMessage::Action {
            sequence: 1,
            action: AstreaSystemAction::OutputVolumeUp,
            occurrences: 1,
        }
    );
}

#[test]
fn second_peer_is_rejected_while_one_executor_owns_the_slot() {
    let root = runtime_dir();
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut transport =
        NativeSystemActionTransport::bind(&mut event_loop, &runtime_path(&root), "busy-peer")
            .unwrap();
    let paths = ControlRuntimePaths::for_runtime_dir(&runtime_path(&root), "busy-peer").unwrap();
    let first = connect_client(&paths.prepare_system_action_endpoint().unwrap());
    let wakeup = event_loop.wait().unwrap();
    transport.service(&mut event_loop, &wakeup.system_action_events, 600);
    let second = connect_client(&paths.prepare_system_action_endpoint().unwrap());
    let wakeup = event_loop.wait().unwrap();
    transport.service(&mut event_loop, &wakeup.system_action_events, 601);
    assert!(transport.snapshot().peer_connected);
    assert_eq!(
        decode(&client_receive(second.as_raw_fd())).unwrap(),
        SystemActionMessage::Reject {
            reason: SYSTEM_ACTION_REJECT_BUSY,
        }
    );
    drop(first);
    drop(second);
    transport.shutdown(&mut event_loop);
}

#[test]
fn peer_hup_clears_capabilities_and_unregisters_its_reactor_token() {
    let volume = AstreaSystemActionCapabilities::for_action(AstreaSystemAction::OutputVolumeUp);
    let (mut transport, mut event_loop, _root, client) = ready_transport(volume);
    let token = transport.peer.as_ref().unwrap().token;
    assert_eq!(transport.take_capability_update(), None);
    drop(client);
    let wakeup = event_loop.wait().unwrap();
    transport.service(&mut event_loop, &wakeup.system_action_events, 700);
    assert_eq!(
        transport.take_capability_update(),
        Some(AstreaSystemActionCapabilities::EMPTY)
    );
    assert!(!transport.snapshot().peer_connected);
    assert_eq!(event_loop.source_for_token(token), None);
    transport.shutdown(&mut event_loop);
}

#[test]
fn future_minor_is_negotiated_and_unknown_capability_bits_are_truncated() {
    let root = runtime_dir();
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut transport =
        NativeSystemActionTransport::bind(&mut event_loop, &runtime_path(&root), "future-peer")
            .unwrap();
    let paths = ControlRuntimePaths::for_runtime_dir(&runtime_path(&root), "future-peer").unwrap();
    let client = connect_client(&paths.prepare_system_action_endpoint().unwrap());
    let known = AstreaSystemActionCapabilities::for_action(AstreaSystemAction::MediaNext);
    client_send(
        client.as_raw_fd(),
        &encode(SystemActionMessage::Hello {
            major: PROTOCOL_MAJOR,
            minor: 9,
            capabilities: known.wire_bits() | (1 << 63),
        }),
    );
    let wakeup = event_loop.wait().unwrap();
    transport.service(&mut event_loop, &wakeup.system_action_events, 200);
    let wakeup = event_loop.wait().unwrap();
    transport.service(&mut event_loop, &wakeup.system_action_events, 201);
    assert_eq!(transport.take_capability_update(), Some(known));
    assert_eq!(
        decode(&client_receive(client.as_raw_fd())).unwrap(),
        SystemActionMessage::Welcome {
            major: PROTOCOL_MAJOR,
            minor: PROTOCOL_MINOR,
            capabilities: known.wire_bits(),
        }
    );
}

#[test]
fn major_mismatch_is_rejected_and_peer_slot_is_released() {
    let root = runtime_dir();
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut transport =
        NativeSystemActionTransport::bind(&mut event_loop, &runtime_path(&root), "wrong-major")
            .unwrap();
    let paths = ControlRuntimePaths::for_runtime_dir(&runtime_path(&root), "wrong-major").unwrap();
    let client = connect_client(&paths.prepare_system_action_endpoint().unwrap());
    client_send(
        client.as_raw_fd(),
        &encode(SystemActionMessage::Hello {
            major: PROTOCOL_MAJOR + 1,
            minor: 0,
            capabilities: u64::MAX,
        }),
    );
    let wakeup = event_loop.wait().unwrap();
    transport.service(&mut event_loop, &wakeup.system_action_events, 300);
    let wakeup = event_loop.wait().unwrap();
    transport.service(&mut event_loop, &wakeup.system_action_events, 301);
    assert_eq!(
        decode(&client_receive(client.as_raw_fd())).unwrap(),
        SystemActionMessage::Reject {
            reason: SYSTEM_ACTION_REJECT_MAJOR_MISMATCH,
        }
    );
    assert!(!transport.snapshot().peer_connected);
    assert_eq!(transport.take_capability_update(), None);
}

#[test]
fn handshake_timeout_and_peer_hup_unregister_the_terminal_source() {
    let root = runtime_dir();
    let mut event_loop = NativeEventLoop::new().unwrap();
    let mut transport =
        NativeSystemActionTransport::bind(&mut event_loop, &runtime_path(&root), "timeout-peer")
            .unwrap();
    let paths = ControlRuntimePaths::for_runtime_dir(&runtime_path(&root), "timeout-peer").unwrap();
    let client = connect_client(&paths.prepare_system_action_endpoint().unwrap());
    let wakeup = event_loop.wait().unwrap();
    transport.service(&mut event_loop, &wakeup.system_action_events, 400);
    let token = transport.peer.as_ref().unwrap().token;
    let deadline = transport.next_deadline_ns().unwrap();
    transport.service(&mut event_loop, &Default::default(), deadline + 1);
    assert!(!transport.snapshot().peer_connected);
    assert_eq!(event_loop.source_for_token(token), None);

    drop(client);
    transport.shutdown(&mut event_loop);
}

#[test]
fn capability_change_replaces_mask_and_removes_unsupported_queued_actions() {
    let initial = AstreaSystemActionCapabilities::for_action(AstreaSystemAction::OutputVolumeUp)
        .union(AstreaSystemActionCapabilities::for_action(
            AstreaSystemAction::MediaNext,
        ));
    let (mut transport, mut event_loop, _root, client) = ready_transport(initial);
    let replacement = AstreaSystemActionCapabilities::for_action(AstreaSystemAction::MediaNext);
    transport
        .peer
        .as_mut()
        .unwrap()
        .outbound
        .push(AstreaSystemAction::OutputVolumeUp);
    transport
        .peer
        .as_mut()
        .unwrap()
        .outbound
        .push(AstreaSystemAction::MediaNext);
    client_send(
        client.as_raw_fd(),
        &encode(SystemActionMessage::CapabilitiesChanged {
            capabilities: replacement.wire_bits(),
        }),
    );
    let wakeup = event_loop.wait().unwrap();
    transport.service(&mut event_loop, &wakeup.system_action_events, 500);
    assert_eq!(transport.take_capability_update(), Some(replacement));
    assert_eq!(transport.peer.as_ref().unwrap().outbound.len(), 1);
    assert_eq!(
        transport
            .peer
            .as_ref()
            .unwrap()
            .outbound
            .front()
            .unwrap()
            .action,
        AstreaSystemAction::MediaNext
    );
}

#[test]
fn peer_credential_decision_is_same_uid_only() {
    assert!(peer_uid_matches(1000, 1000));
    assert!(!peer_uid_matches(1001, 1000));
}

#[test]
fn disconnect_is_applied_before_a_same_cycle_xf86_keyboard_press() {
    use crate::native_output::{
        AstreaBindingManager, BindingKeySym, KeyboardDeviceId, KeyboardSymbolicIdentity,
        KeyboardSymbolicSnapshot, ModifierMask, NativeHardwareInputEvent, NativeInputState,
        NativeKeyboardAction, NativeKeyboardInputEvent,
    };

    let caps = AstreaSystemActionCapabilities::for_action(AstreaSystemAction::OutputVolumeUp);
    let (mut transport, mut event_loop, _root, client) = ready_transport(caps);
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager =
        AstreaBindingManager::with_default_bindings_and_system_capabilities(caps);
    drop(client);

    // The runtime services this HUP and applies its capability update before
    // it routes keyboard records from the same NativeWakeup.
    let wakeup = event_loop.wait().unwrap();
    transport.service(&mut event_loop, &wakeup.system_action_events, 600);
    let revoked = transport.take_capability_update().unwrap();
    assert_eq!(revoked, AstreaSystemActionCapabilities::EMPTY);
    input.set_system_action_capabilities(revoked);

    let keysym = BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_XF86AudioRaiseVolume);
    let identity = KeyboardSymbolicIdentity {
        keysym,
        modifiers: ModifierMask::EMPTY,
    };
    let device = KeyboardDeviceId::from_raw(1).unwrap();
    let effect = input.handle_hardware_input_event_at_with_symbolic(
        NativeHardwareInputEvent::Keyboard(NativeKeyboardInputEvent::Key {
            device,
            code: 44,
            pressed: true,
        }),
        Some(KeyboardSymbolicSnapshot {
            raw: Some(identity),
            translated: Some(identity),
        }),
        700,
    );

    assert!(effect.binding_action_invocations.is_empty());
    assert!(effect.keyboard_actions.iter().any(|action| matches!(
        action,
        NativeKeyboardAction::PhysicalAndClient(event) if event.pressed
    )));
}

#[test]
fn capability_removal_is_applied_before_a_due_system_action_repeat() {
    use crate::native_output::{
        AstreaBindingManager, BindingKeySym, KeyboardDeviceId, KeyboardSymbolicIdentity,
        KeyboardSymbolicSnapshot, ModifierMask, NativeHardwareInputEvent, NativeInputState,
        NativeKeyboardInputEvent,
    };

    let capabilities =
        AstreaSystemActionCapabilities::for_action(AstreaSystemAction::OutputVolumeUp);
    let (mut transport, mut event_loop, _root, client) = ready_transport(capabilities);
    let mut input = NativeInputState::new(320, 200);
    input.binding_manager =
        AstreaBindingManager::with_default_bindings_and_system_capabilities(capabilities);
    let symbol = KeyboardSymbolicIdentity {
        keysym: BindingKeySym::new(xkbcommon::xkb::keysyms::KEY_XF86AudioRaiseVolume),
        modifiers: ModifierMask::EMPTY,
    };
    let symbolic = KeyboardSymbolicSnapshot {
        raw: Some(symbol),
        translated: Some(symbol),
    };
    let device = KeyboardDeviceId::from_raw(1).unwrap();
    let press = input.handle_hardware_input_event_at_with_symbolic(
        NativeHardwareInputEvent::Keyboard(NativeKeyboardInputEvent::Key {
            device,
            code: 44,
            pressed: true,
        }),
        Some(symbolic),
        0,
    );
    assert_eq!(press.binding_action_invocations.len(), 1);
    let repeat_generation = input.keyboard_repeat_generation();
    assert!(input.keyboard_repeat_due(600_000_000));

    client_send(
        client.as_raw_fd(),
        &encode(SystemActionMessage::CapabilitiesChanged { capabilities: 0 }),
    );
    let wakeup = event_loop.wait().unwrap();
    transport.service(&mut event_loop, &wakeup.system_action_events, 600_000_000);
    let revoked = transport.take_capability_update().unwrap();
    assert_eq!(revoked, AstreaSystemActionCapabilities::EMPTY);
    input.set_system_action_capabilities(revoked);

    let repeat = input.service_keyboard_repeat_if_unchanged_with_symbolic(
        600_000_000,
        repeat_generation,
        Some(symbolic),
    );
    assert!(repeat.binding_action_invocations.is_empty());
    assert_eq!(input.keyboard_repeat_deadline_ns(), None);
}

#[test]
fn malformed_packet_disconnects_and_clears_capabilities() {
    let caps = AstreaSystemActionCapabilities::for_action(AstreaSystemAction::OutputVolumeUp);
    let (mut transport, mut event_loop, _root, client) = ready_transport(caps);
    let mut packet = encode(SystemActionMessage::CapabilitiesChanged {
        capabilities: caps.wire_bits(),
    });
    packet[0] ^= 1;
    client_send(client.as_raw_fd(), &packet);

    let wakeup = event_loop.wait().unwrap();
    transport.service(&mut event_loop, &wakeup.system_action_events, 200);

    assert_eq!(
        transport.take_capability_update(),
        Some(AstreaSystemActionCapabilities::EMPTY)
    );
    assert!(!transport.snapshot().peer_connected);
    assert_eq!(transport.telemetry.protocol_errors, 1);
}

#[test]
fn outbound_queue_overflow_disconnects_the_executor() {
    let caps = AstreaSystemActionCapabilities::for_action(AstreaSystemAction::OutputVolumeUp)
        .union(AstreaSystemActionCapabilities::for_action(
            AstreaSystemAction::ToggleOutputMute,
        ))
        .union(AstreaSystemActionCapabilities::for_action(
            AstreaSystemAction::MediaNext,
        ));
    let (mut transport, mut event_loop, _root, _client) = ready_transport(caps);
    for index in 0..SYSTEM_ACTION_OUTBOUND_CAPACITY {
        transport
            .peer
            .as_mut()
            .unwrap()
            .outbound
            .push(if index % 2 == 0 {
                AstreaSystemAction::ToggleOutputMute
            } else {
                AstreaSystemAction::MediaNext
            });
    }

    assert_eq!(
        transport.submit(&mut event_loop, AstreaSystemAction::OutputVolumeUp),
        SystemActionSubmitResult::Disconnected
    );
    assert_eq!(transport.telemetry.queue_overflows, 1);
    assert_eq!(
        transport.take_capability_update(),
        Some(AstreaSystemActionCapabilities::EMPTY)
    );
    assert!(!transport.snapshot().peer_connected);
}

#[test]
fn failed_epoll_interest_update_disconnects_the_executor() {
    let caps = AstreaSystemActionCapabilities::for_action(AstreaSystemAction::OutputVolumeUp)
        .union(AstreaSystemActionCapabilities::for_action(
            AstreaSystemAction::MediaNext,
        ));
    let (mut transport, mut event_loop, _root, _client) = ready_transport(caps);
    let token = transport.peer.as_ref().unwrap().token;
    assert!(event_loop.unregister(token).unwrap());
    transport
        .peer
        .as_mut()
        .unwrap()
        .outbound
        .push(AstreaSystemAction::OutputVolumeUp);

    assert_eq!(
        transport.submit(&mut event_loop, AstreaSystemAction::MediaNext),
        SystemActionSubmitResult::Disconnected
    );
    assert_eq!(
        transport.take_capability_update(),
        Some(AstreaSystemActionCapabilities::EMPTY)
    );
    assert!(!transport.snapshot().peer_connected);
}

#[test]
fn outbound_backpressure_queues_actions_and_removes_write_interest_after_drain() {
    let caps = AstreaSystemActionCapabilities::for_action(AstreaSystemAction::OutputVolumeUp);
    let (mut transport, mut event_loop, _root, client) = ready_transport(caps);
    let send_buffer = 1_024_i32;
    let result = unsafe {
        libc::setsockopt(
            transport.peer.as_ref().unwrap().fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_SNDBUF,
            (&send_buffer as *const i32).cast(),
            std::mem::size_of_val(&send_buffer) as libc::socklen_t,
        )
    };
    assert_eq!(result, 0, "setsockopt: {}", io::Error::last_os_error());

    let mut backpressured = false;
    for _ in 0..100_000 {
        match transport.submit(&mut event_loop, AstreaSystemAction::OutputVolumeUp) {
            SystemActionSubmitResult::Sent => {}
            SystemActionSubmitResult::Queued => {
                backpressured = true;
                break;
            }
            result => panic!("unexpected submit result under backpressure: {result:?}"),
        }
    }
    assert!(backpressured, "the bounded socket send buffer did not fill");
    assert_eq!(transport.telemetry.send_would_block, 1);
    assert_eq!(transport.peer.as_ref().unwrap().outbound.len(), 1);
    assert_eq!(
        transport.submit(&mut event_loop, AstreaSystemAction::OutputVolumeUp),
        SystemActionSubmitResult::Queued
    );
    assert_eq!(
        transport
            .peer
            .as_ref()
            .unwrap()
            .outbound
            .front()
            .unwrap()
            .occurrences,
        2
    );

    let mut received = [0_u8; PACKET_SIZE + 1];
    loop {
        let count = unsafe {
            libc::recv(
                client.as_raw_fd(),
                received.as_mut_ptr().cast(),
                received.len(),
                libc::MSG_DONTWAIT | libc::MSG_TRUNC,
            )
        };
        if count >= 0 {
            continue;
        }
        if io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
            break;
        }
        panic!("client drain failed: {}", io::Error::last_os_error());
    }

    let writable = event_loop.wait().unwrap();
    assert!(
        writable
            .system_action_events
            .iter()
            .any(|event| { event.flags & libc::EPOLLOUT as u32 != 0 })
    );
    transport.service(&mut event_loop, &writable.system_action_events, 300);
    assert_eq!(transport.peer.as_ref().unwrap().outbound.len(), 0);
    assert_eq!(
        decode(&client_receive(client.as_raw_fd())).unwrap(),
        SystemActionMessage::Action {
            sequence: transport.next_sequence - 1,
            action: AstreaSystemAction::OutputVolumeUp,
            occurrences: 2,
        }
    );

    let mut write_interest_removed = false;
    for _ in 0..4 {
        let deadline = oblivion_one::native::event_loop::monotonic_now_ns().unwrap() + 5_000_000;
        event_loop.arm_deadline(Some(deadline)).unwrap();
        let wakeup = event_loop.wait().unwrap();
        if wakeup.system_action_events.is_empty() {
            write_interest_removed = true;
            break;
        }
        transport.service(&mut event_loop, &wakeup.system_action_events, deadline);
    }
    assert!(
        write_interest_removed,
        "idle writable peer remained in epoll"
    );
}

#[test]
fn shutdown_clears_capabilities_closes_sources_and_removes_socket() {
    let caps = AstreaSystemActionCapabilities::for_action(AstreaSystemAction::OutputVolumeUp);
    let (mut transport, mut event_loop, root, client) = ready_transport(caps);
    let paths =
        ControlRuntimePaths::for_runtime_dir(&runtime_path(&root), "test-instance").unwrap();
    let socket_path = paths.prepare_system_action_endpoint().unwrap();
    let listener_token = transport.listener_token.unwrap();
    let peer_token = transport.peer.as_ref().unwrap().token;

    transport.shutdown(&mut event_loop);

    assert_eq!(
        transport.take_capability_update(),
        Some(AstreaSystemActionCapabilities::EMPTY)
    );
    assert_eq!(event_loop.source_for_token(listener_token), None);
    assert_eq!(event_loop.source_for_token(peer_token), None);
    assert!(!socket_path.exists());
    assert!(!transport.snapshot().peer_connected);
    drop(client);
}
