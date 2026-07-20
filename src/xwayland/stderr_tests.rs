use std::{
    fs,
    os::unix::fs::PermissionsExt,
    thread,
    time::{Duration, Instant},
};

use super::super::{
    XwaylandConfig, XwaylandMode, XwaylandReactorPurpose, XwaylandService, XwaylandStateKind,
    diagnostics::XwaylandFailureStage,
};
use crate::process::ChildSupervisor;

fn service_at_root_with_stderr_binary() -> (std::path::PathBuf, XwaylandService, ChildSupervisor) {
    service_at_root_with_stderr_binary_and_logging("stderr", false)
}

fn service_at_root_with_stderr_binary_and_logging(
    label: &str,
    log_stderr: bool,
) -> (std::path::PathBuf, XwaylandService, ChildSupervisor) {
    let root = super::test_root(label);
    let binary = root.join("xwayland-test-binary");
    fs::write(
        &binary,
        "#!/bin/sh\nprintf 'xwayland diagnostic\\n' >&2\nexec /bin/sleep 30\n",
    )
    .expect("write stderr test binary");
    let mut permissions = fs::metadata(&binary)
        .expect("stat stderr test binary")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&binary, permissions).expect("make stderr test binary executable");
    let mut config =
        XwaylandConfig::for_tests_at_root(XwaylandMode::BaseLazy, binary, root.clone());
    config.display_min = 1;
    config.log_stderr = log_stderr;
    let service = XwaylandService::bootstrap_with_config(config).expect("bootstrap service");
    (root, service, ChildSupervisor::new())
}

#[test]
fn failed_generation_drains_final_stderr_with_live_logging_disabled() {
    let (root, mut service, mut supervisor) = service_at_root_with_stderr_binary();
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("generation");
    let deadline = Instant::now() + Duration::from_secs(2);
    while service.metrics.stderr_lines == 0 && Instant::now() < deadline {
        service
            .handle_stderr_ready(generation)
            .expect("drain stderr");
        thread::sleep(Duration::from_millis(10));
    }
    assert!(service.metrics.stderr_lines >= 1);
    service.inject_xwm_failure_for_tests(
        &mut supervisor,
        XwaylandFailureStage::Startup,
        "preserve stderr after startup failure",
    );
    service
        .handle_deadline(u64::MAX, &mut supervisor)
        .expect("escalate failed generation cleanup");
    let reap_deadline = Instant::now() + Duration::from_secs(2);
    while supervisor.active_count() != 0 && Instant::now() < reap_deadline {
        let _ = supervisor.reap_exited().expect("reap failed generation");
        thread::sleep(Duration::from_millis(10));
    }
    service
        .finish_reactor_teardown()
        .expect("finish bounded stderr drain");
    assert!(
        service
            .recent_failure_diagnostics()
            .iter()
            .any(|line| line.contains("xwayland diagnostic"))
    );
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn stderr_forwarding_is_controlled_independently_of_capture() {
    let (root, mut service, mut supervisor) =
        service_at_root_with_stderr_binary_and_logging("stderr-forward", true);
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("generation");
    assert_eq!(service.stderr_forwarding_for_tests(generation), Some(true));
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn crash_preserves_final_stderr_ring() {
    let (root, mut service, mut supervisor) = service_at_root_with_stderr_binary();
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("generation");
    let stderr_deadline = Instant::now() + Duration::from_secs(2);
    while service.metrics.stderr_lines == 0 && Instant::now() < stderr_deadline {
        service
            .handle_stderr_ready(generation)
            .expect("drain crash diagnostics");
        thread::sleep(Duration::from_millis(10));
    }
    let process_id = service
        .readiness_snapshot()
        .expect("readiness snapshot")
        .process_id;
    supervisor
        .kill_managed_now(process_id)
        .expect("kill Xwayland test process");
    let exit = super::reap_one(&mut supervisor);
    service
        .handle_process_exit(&exit)
        .expect("contain crashed generation");
    service
        .finish_reactor_teardown()
        .expect("finish crash stderr drain");
    assert!(
        service
            .recent_failure_diagnostics()
            .iter()
            .any(|line| line.contains("xwayland diagnostic"))
    );
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn stderr_generation_replacement_keeps_latest_failed_ring() {
    let (root, mut service, mut supervisor) = service_at_root_with_stderr_binary();
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start first generation");
    let first_generation = service.generation().expect("first generation");
    let stderr_deadline = Instant::now() + Duration::from_secs(2);
    while service.metrics.stderr_lines == 0 && Instant::now() < stderr_deadline {
        service
            .handle_stderr_ready(first_generation)
            .expect("drain first generation stderr");
        thread::sleep(Duration::from_millis(10));
    }
    service.inject_xwm_failure_for_tests(
        &mut supervisor,
        XwaylandFailureStage::Startup,
        "first generation failed",
    );
    service
        .handle_deadline(u64::MAX, &mut supervisor)
        .expect("terminate first generation");
    let reap_deadline = Instant::now() + Duration::from_secs(2);
    while supervisor.active_count() != 0 && Instant::now() < reap_deadline {
        let _ = supervisor.reap_exited().expect("reap first generation");
        thread::sleep(Duration::from_millis(10));
    }
    service
        .finish_reactor_teardown()
        .expect("publish first generation stderr");
    let first_diagnostics = service.recent_failure_diagnostics();
    assert!(
        first_diagnostics
            .iter()
            .any(|line| line.contains("xwayland diagnostic"))
    );

    service
        .handle_deadline(u64::MAX, &mut supervisor)
        .expect("rearm after first failure");
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start replacement generation");
    let second_generation = service.generation().expect("replacement generation");
    assert_ne!(first_generation, second_generation);
    assert_eq!(service.recent_failure_diagnostics(), first_diagnostics);
    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn stderr_pipe_is_nonblocking_and_closes_without_failing_generation() {
    let (root, mut service, mut supervisor) = service_at_root_with_stderr_binary();
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start generation");
    let generation = service.generation().expect("generation");
    assert_eq!(
        service
            .reactor_registrations()
            .filter(|registration| registration.purpose == XwaylandReactorPurpose::Stderr)
            .count(),
        1
    );
    service
        .handle_stderr_ready(generation)
        .expect("drain stderr");
    let deadline = Instant::now() + Duration::from_secs(2);
    while service.metrics.stderr_lines == 0 && Instant::now() < deadline {
        service
            .handle_stderr_ready(generation)
            .expect("poll stderr");
        thread::sleep(Duration::from_millis(10));
    }
    assert!(service.metrics.stderr_lines >= 1);

    let process_id = service
        .readiness_snapshot()
        .expect("readiness snapshot")
        .process_id;
    supervisor
        .kill_managed_now(process_id)
        .expect("kill process");
    let deadline = Instant::now() + Duration::from_secs(2);
    while service
        .reactor_registrations()
        .any(|registration| registration.purpose == XwaylandReactorPurpose::Stderr)
        && Instant::now() < deadline
    {
        service
            .handle_stderr_ready(generation)
            .expect("handle stderr EOF");
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        service
            .reactor_registrations()
            .filter(|registration| registration.purpose == XwaylandReactorPurpose::Stderr)
            .count(),
        0
    );
    assert_eq!(service.state_kind(), XwaylandStateKind::Starting);

    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}

#[test]
fn stderr_registration_survives_entire_managed_startup() {
    let (root, mut service, mut supervisor) = super::service_at_root_with_sleeping_binary(
        super::XwaylandMode::ManagedLazy,
        "stderr-managed-startup",
    );
    service
        .handle_listener_readiness(&mut supervisor)
        .expect("start managed generation");
    let generation = service.generation().expect("managed generation");

    let stderr_count = |service: &super::XwaylandService| {
        service
            .reactor_registrations()
            .filter(|registration| registration.purpose == XwaylandReactorPurpose::Stderr)
            .count()
    };
    let display_count = |service: &super::XwaylandService| {
        service
            .reactor_registrations()
            .filter(|registration| registration.purpose == XwaylandReactorPurpose::DisplayReady)
            .count()
    };
    let xwm_count = |service: &super::XwaylandService| {
        service
            .reactor_registrations()
            .filter(|registration| registration.purpose == XwaylandReactorPurpose::Xwm)
            .count()
    };

    assert_eq!(stderr_count(&service), 1);
    assert_eq!(display_count(&service), 1);

    service.mark_display_ready_for_tests(generation);
    assert_eq!(stderr_count(&service), 1);
    assert_eq!(display_count(&service), 0);

    service
        .handle_shell_bind(generation)
        .expect("mark shell ready");
    service.install_xwm_startup_for_tests(generation);
    assert_eq!(stderr_count(&service), 1);
    assert_eq!(xwm_count(&service), 1);

    service.emergency_cleanup(&mut supervisor).expect("cleanup");
    drop(service);
    drop(supervisor);
    fs::remove_dir_all(root).expect("remove test root");
}
