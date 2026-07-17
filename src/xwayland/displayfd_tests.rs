use std::{
    fs,
    os::fd::AsRawFd,
    thread,
    time::{Duration, Instant},
};

use crate::native::event_loop::{NativeEventLoop, NativeEventSource};

use super::{
    XwaylandMode, XwaylandReactorPurpose, XwaylandStateKind,
    service_at_root_with_delayed_displayfd_writer, service_at_root_with_displayfd_writer,
    service_at_root_with_sleeping_binary,
};

#[test]
fn displayfd_parent_read_is_nonblocking_and_child_write_is_blocking() {
    let (parent_read, child_write) =
        super::super::displayfd::pipe_pair_for_tests().expect("create displayfd pipe");
    let parent_status = unsafe { libc::fcntl(parent_read.as_raw_fd(), libc::F_GETFL) };
    let child_status = unsafe { libc::fcntl(child_write.as_raw_fd(), libc::F_GETFL) };
    assert!(parent_status >= 0);
    assert!(child_status >= 0);
    assert_ne!(parent_status & libc::O_NONBLOCK, 0);
    assert_eq!(child_status & libc::O_NONBLOCK, 0);
    let parent_descriptor = unsafe { libc::fcntl(parent_read.as_raw_fd(), libc::F_GETFD) };
    let child_descriptor = unsafe { libc::fcntl(child_write.as_raw_fd(), libc::F_GETFD) };
    assert_ne!(parent_descriptor & libc::FD_CLOEXEC, 0);
    assert_ne!(child_descriptor & libc::FD_CLOEXEC, 0);
}

#[test]
fn subprocess_displayfd_write_before_registration_is_recovered_by_probe() {
    let (root, mut service, mut supervisor) = service_at_root_with_displayfd_writer(
        XwaylandMode::BaseLazy,
        "displayfd-before-registration",
        false,
    );
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("generation");
    let registration = service
        .reactor_registrations()
        .find(|registration| registration.purpose == XwaylandReactorPurpose::DisplayReady)
        .expect("displayfd registration");

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut available = 0;
    while available == 0 && Instant::now() < deadline {
        let mut pending = 0;
        let result = unsafe {
            libc::ioctl(
                registration.fd,
                libc::FIONREAD,
                &mut pending as *mut libc::c_int,
            )
        };
        assert_eq!(result, 0);
        available = pending;
        if available == 0 {
            thread::sleep(Duration::from_millis(5));
        }
    }
    assert!(
        available > 0,
        "helper did not write displayfd before registration"
    );

    let mut event_loop = NativeEventLoop::new().expect("reactor");
    let token = event_loop
        .register(registration.fd, NativeEventSource::XwaylandDisplayReady)
        .expect("register displayfd");
    service.note_reactor_registration_with_token(registration, true, Some(token.raw()));
    service
        .probe_displayfd(generation, &mut supervisor)
        .expect("probe displayfd");
    assert!(
        service
            .readiness_snapshot()
            .expect("readiness")
            .display_number_validated
    );
    assert_eq!(service.state_kind(), XwaylandStateKind::Starting);

    assert!(event_loop.unregister(token).expect("unregister displayfd"));
    service.note_reactor_registration_with_token(registration, false, Some(token.raw()));
    service
        .finish_reactor_teardown()
        .expect("finish source teardown");
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(event_loop);
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn displayfd_payload_split_across_writes_completes_only_after_newline() {
    let (root, mut service, mut supervisor) =
        service_at_root_with_sleeping_binary(XwaylandMode::BaseLazy, "displayfd-split");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("generation");
    let display = service.display_number().expect("display");
    service
        .handle_displayfd_bytes(generation, display.to_string().as_bytes(), &mut supervisor)
        .expect("accept partial payload");
    assert!(
        !service
            .readiness_snapshot()
            .expect("readiness")
            .display_number_validated
    );
    service
        .handle_displayfd_bytes(generation, b"\n", &mut supervisor)
        .expect("accept final newline");
    assert!(
        service
            .readiness_snapshot()
            .expect("readiness")
            .display_number_validated
    );
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn subprocess_displayfd_write_after_registration_is_delivered_by_epoll() {
    let (root, mut service, mut supervisor) =
        service_at_root_with_delayed_displayfd_writer("displayfd-after-registration", true);
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("generation");
    let registration = service
        .reactor_registrations()
        .find(|registration| registration.purpose == XwaylandReactorPurpose::DisplayReady)
        .expect("displayfd registration");
    let mut event_loop = NativeEventLoop::new().expect("reactor");
    let token = event_loop
        .register(registration.fd, NativeEventSource::XwaylandDisplayReady)
        .expect("register displayfd");
    service.note_reactor_registration_with_token(registration, true, Some(token.raw()));

    let wakeup = event_loop.wait().expect("displayfd wakeup");
    let ready = wakeup
        .xwayland_events
        .iter()
        .find(|event| event.token == token)
        .expect("displayfd event");
    assert_ne!(ready.flags & libc::EPOLLIN as u32, 0);
    let mut ready_flags = ready.flags;
    service
        .handle_reactor_event_with_token(
            XwaylandReactorPurpose::DisplayReady,
            Some(generation),
            ready_flags,
            token.raw(),
            &mut supervisor,
        )
        .expect("drain displayfd event");
    if ready_flags & libc::EPOLLHUP as u32 == 0 {
        let wakeup = event_loop.wait().expect("displayfd close wakeup");
        let closed = wakeup
            .xwayland_events
            .iter()
            .find(|event| event.token == token)
            .expect("displayfd close event");
        ready_flags |= closed.flags;
        service
            .handle_reactor_event_with_token(
                XwaylandReactorPurpose::DisplayReady,
                Some(generation),
                closed.flags,
                token.raw(),
                &mut supervisor,
            )
            .expect("drain displayfd close event");
    }
    assert_ne!(ready_flags & libc::EPOLLHUP as u32, 0);
    assert!(
        service
            .readiness_snapshot()
            .expect("readiness")
            .display_number_validated
    );

    assert!(event_loop.unregister(token).expect("unregister displayfd"));
    service.note_reactor_registration_with_token(registration, false, Some(token.raw()));
    service
        .finish_reactor_teardown()
        .expect("finish source teardown");
    let exits = supervisor.reap_exited().expect("reap helper");
    for exit in exits {
        service
            .handle_process_exit(&exit)
            .expect("handle helper exit");
    }
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(event_loop);
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn timeout_probe_rescues_already_written_displayfd_payload() {
    let (root, mut service, mut supervisor) = service_at_root_with_displayfd_writer(
        XwaylandMode::BaseLazy,
        "displayfd-timeout-probe",
        false,
    );
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("generation");
    let registration = service
        .reactor_registrations()
        .find(|registration| registration.purpose == XwaylandReactorPurpose::DisplayReady)
        .expect("displayfd registration");
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let mut pending = 0;
        assert_eq!(
            unsafe {
                libc::ioctl(
                    registration.fd,
                    libc::FIONREAD,
                    &mut pending as *mut libc::c_int,
                )
            },
            0
        );
        if pending > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    service
        .handle_shell_bind(generation)
        .expect("shell readiness before final probe");
    service
        .handle_deadline(u64::MAX, &mut supervisor)
        .expect("final probe handles displayfd");
    assert!(
        service
            .readiness_snapshot()
            .expect("readiness")
            .display_number_validated
    );
    assert_eq!(service.state_kind(), XwaylandStateKind::RunningBase);
    assert_eq!(service.generation(), Some(generation));
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn repeated_startup_timeouts_preserve_backoff_and_reach_failed() {
    let (root, mut service, mut supervisor) =
        service_at_root_with_sleeping_binary(XwaylandMode::BaseLazy, "timeout-budget");
    for attempt in 0..3 {
        assert!(
            service
                .handle_listener_readiness(&mut supervisor)
                .expect("start generation")
        );
        service
            .handle_deadline(u64::MAX, &mut supervisor)
            .expect("contain startup timeout");
        if attempt < 2 {
            assert_eq!(service.state_kind(), XwaylandStateKind::Backoff);
            service
                .handle_deadline(u64::MAX, &mut supervisor)
                .expect("leave backoff");
            assert_eq!(service.state_kind(), XwaylandStateKind::Armed);
            assert_eq!(service.metrics.backoff_level, attempt + 1);
        }
    }
    assert_eq!(service.state_kind(), XwaylandStateKind::Failed);
    assert_eq!(service.metrics.backoff_level, 2);
    assert_eq!(service.reactor_registrations().count(), 0);
    service
        .finish_reactor_teardown()
        .expect("finish retired generation teardown");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}
