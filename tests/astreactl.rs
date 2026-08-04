use std::{
    io::{Read, Write},
    os::unix::net::UnixListener,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn valid_result(command: &str) -> serde_json::Value {
    match command {
        "version" => serde_json::json!({
            "protocolVersion": 1,
            "compositorName": "Typhon",
            "compositorVersion": "0.1.0",
            "gitCommit": null,
            "buildProfile": "debug",
            "rustcVersion": null
        }),
        "status" => serde_json::json!({
            "instance": "test",
            "waylandDisplay": "wayland-1",
            "uptimeMs": 1,
            "sessionState": "active",
            "shutdownState": "running",
            "outputCount": 1,
            "mappedWindowCount": 0,
            "minimizedWindowCount": 0,
            "activeWindow": null,
            "xwayland": {"configured": false, "state": "disabled", "generation": null},
            "control": {"endpointActive": true, "clientCount": 0, "accepted": 1}
        }),
        "doctor" => serde_json::json!({"healthy": true, "checks": []}),
        "outputs" => serde_json::json!({"outputs": [], "total": 0, "truncated": false}),
        "windows" => serde_json::json!({"windows": [], "total": 0, "truncated": false}),
        "activewindow" | "active-window" => serde_json::json!({"window": null}),
        "cursor.get" | "cursor.set-theme" | "cursor.set-size" | "cursor.set" | "cursor.reload" => {
            serde_json::json!({
                "desiredTheme": "default",
                "desiredSizePx": 24,
                "activeTheme": "default",
                "activeSizePx": 24,
                "generation": 1,
                "backend": "software",
                "source": "default",
                "persistence": "missing"
            })
        }
        other => panic!("no typed fixture for {other}"),
    }
}

fn envelope(result: serde_json::Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&serde_json::json!({
        "protocol": "astrea.control",
        "version": 1,
        "id": 1,
        "ok": true,
        "result": result
    }))
    .unwrap();
    bytes.push(b'\n');
    bytes
}

fn server_error_envelope() -> Vec<u8> {
    envelope_with_error(serde_json::json!({
        "code": "internal",
        "message": "test server failure"
    }))
}

fn envelope_with_error(error: serde_json::Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&serde_json::json!({
        "protocol": "astrea.control",
        "version": 1,
        "id": 1,
        "ok": false,
        "error": error
    }))
    .unwrap();
    bytes.push(b'\n');
    bytes
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_astreactl"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn invalid_cli_arguments_use_usage_exit_code() {
    for args in [
        vec!["status", "version"],
        vec!["--instance"],
        vec!["--socket", "relative", "status"],
        vec!["--timeout", "0s", "status"],
        vec!["--timeout", "61s", "status"],
        vec!["--instance", "../escape", "status"],
        vec!["--instance", "one", "--instance", "two", "status"],
        vec!["--json", "--json", "status"],
        vec!["--timeout", "1s", "--timeout", "2s", "status"],
        vec!["--socket", "/tmp/one", "--socket", "/tmp/two", "status"],
        vec![
            "--instance",
            "one",
            "--socket",
            "/tmp/control.sock",
            "status",
        ],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2), "args={args:?}");
    }
}

#[test]
fn version_flag_is_distinct_from_version_command() {
    let flag = run(&["--version"]);
    assert_eq!(flag.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&flag.stdout).starts_with("astreactl "));
}

#[test]
fn typed_results_are_validated_for_every_read_only_command() {
    let commands = [
        ("version", "protocolVersion"),
        ("status", "instance"),
        ("doctor", "healthy"),
        ("outputs", "total"),
        ("windows", "total"),
        ("activewindow", "window"),
    ];
    for (command, required) in commands {
        let valid = run_socket_once(command, envelope(valid_result(command)));
        assert_eq!(valid.status.code(), Some(0), "command={command}");

        for result in [serde_json::json!({}), serde_json::Value::Null] {
            let malformed = run_socket_once(command, envelope(result));
            assert_eq!(
                malformed.status.code(),
                Some(6),
                "command={command} stdout={:?} stderr={:?}",
                malformed.stdout,
                malformed.stderr
            );
        }

        let mut missing = valid_result(command);
        missing.as_object_mut().unwrap().remove(required);
        assert_eq!(
            run_socket_once(command, envelope(missing)).status.code(),
            Some(6),
            "missing field command={command}"
        );

        let mut wrong_type = valid_result(command);
        wrong_type[required] = match command {
            "status" | "activewindow" => serde_json::json!(7),
            _ => serde_json::json!("wrong"),
        };
        assert_eq!(
            run_socket_once(command, envelope(wrong_type)).status.code(),
            Some(6),
            "wrong field type command={command}"
        );

        let other = if command == "version" {
            "status"
        } else {
            "version"
        };
        assert_eq!(
            run_socket_once(command, envelope(valid_result(other)))
                .status
                .code(),
            Some(6),
            "wrong result type command={command}"
        );

        let mut extra = valid_result(command);
        extra["unknownField"] = serde_json::json!(true);
        assert_eq!(
            run_socket_once(command, envelope(extra)).status.code(),
            Some(6),
            "unknown field command={command}"
        );
    }

    let mut bad_output_mode = serde_json::json!({
        "outputs": [{
            "id": "output-1",
            "name": "Output",
            "make": null,
            "model": null,
            "serial": null,
            "enabled": true,
            "currentMode": {"width": 1920, "height": 1080, "refreshMillihz": 60000},
            "physicalSizeMm": null,
            "scaleMilli": 1000,
            "transform": "normal",
            "position": {"x": 0, "y": 0},
            "focused": true,
            "backend": "atomic",
            "vrr": {"state": "available"},
            "directScanout": {"state": "unavailable"}
        }],
        "total": 1,
        "truncated": false
    });
    bad_output_mode["outputs"][0]["currentMode"]["width"] = serde_json::json!("wide");
    assert_eq!(
        run_socket_once("outputs", envelope(bad_output_mode))
            .status
            .code(),
        Some(6)
    );

    let bad_window_id = serde_json::json!({
        "windows": [{
            "id": "not-a-window-id",
            "appId": null,
            "title": "Title",
            "pid": null,
            "kind": "xdg_toplevel",
            "mapped": true,
            "active": false,
            "minimized": false,
            "maximized": false,
            "fullscreen": false,
            "urgent": null,
            "skipTaskbar": false,
            "workspace": null,
            "output": null,
            "geometry": null,
            "focusSerial": null
        }],
        "total": 1,
        "truncated": false
    });
    assert_eq!(
        run_socket_once("windows", envelope(bad_window_id))
            .status
            .code(),
        Some(6)
    );

    let bad_doctor_severity = serde_json::json!({
        "healthy": true,
        "checks": [{"id": "check", "severity": "not-a-severity", "summary": "bad", "detail": null}]
    });
    assert_eq!(
        run_socket_once("doctor", envelope(bad_doctor_severity))
            .status
            .code(),
        Some(6)
    );
}

#[test]
fn typed_cursor_results_are_validated_for_every_cursor_command() {
    let commands = [
        ("cursor.get", vec!["cursor", "get"]),
        ("cursor.set-theme", vec!["cursor", "set-theme", "default"]),
        ("cursor.set-size", vec!["cursor", "set-size", "24"]),
        (
            "cursor.set",
            vec!["cursor", "set", "--theme", "default", "--size", "24"],
        ),
        ("cursor.reload", vec!["cursor", "reload"]),
    ];
    for (wire_command, cli_args) in commands {
        let valid = run_socket_once_args(&cli_args, envelope(valid_result(wire_command)));
        assert_eq!(valid.status.code(), Some(0), "command={wire_command}");
        for result in [serde_json::json!({}), serde_json::Value::Null] {
            assert_eq!(
                run_socket_once_args(&cli_args, envelope(result))
                    .status
                    .code(),
                Some(6),
                "command={wire_command}"
            );
        }
        let mut missing = valid_result(wire_command);
        missing.as_object_mut().unwrap().remove("activeTheme");
        assert_eq!(
            run_socket_once_args(&cli_args, envelope(missing))
                .status
                .code(),
            Some(6)
        );
        let mut wrong_type = valid_result(wire_command);
        wrong_type["activeSizePx"] = serde_json::json!("large");
        assert_eq!(
            run_socket_once_args(&cli_args, envelope(wrong_type))
                .status
                .code(),
            Some(6)
        );
        assert_eq!(
            run_socket_once_args(&cli_args, envelope(valid_result("status")))
                .status
                .code(),
            Some(6)
        );
        let mut unknown = valid_result(wire_command);
        unknown["unknownField"] = serde_json::json!(true);
        assert_eq!(
            run_socket_once_args(&cli_args, envelope(unknown))
                .status
                .code(),
            Some(6)
        );
    }
}

#[test]
fn cursor_cli_rejects_invalid_values_and_duplicate_options_locally() {
    for args in [
        vec!["cursor", "set-theme", "bad/theme"],
        vec!["cursor", "set-theme"],
        vec!["cursor", "set-size", "7"],
        vec!["cursor", "set-size"],
        vec!["cursor", "set", "--theme", "default"],
        vec!["cursor", "set", "--size", "24"],
        vec![
            "cursor", "set", "--theme", "default", "--theme", "other", "--size", "24",
        ],
        vec![
            "cursor", "set", "--theme", "default", "--size", "24", "--size", "32",
        ],
        vec!["cursor", "unknown"],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2), "args={args:?}");
    }
}

fn run_socket_cycles(command: &str, response: Vec<u8>, expected_code: i32) {
    run_socket_cycles_args(&[command], response, expected_code);
}

fn run_socket_cycles_args(args: &[&str], response: Vec<u8>, expected_code: i32) {
    let path = std::env::temp_dir().join(format!(
        "typhon-astreactl-stress-{}-{}.sock",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..100 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            stream.read_to_end(&mut request).unwrap();
            stream.write_all(&response).unwrap();
        }
    });
    for _ in 0..100 {
        let mut cli_args = vec!["--json", "--socket", path.to_str().unwrap()];
        cli_args.extend_from_slice(args);
        let output = run(&cli_args);
        assert_eq!(output.status.code(), Some(expected_code));
    }
    server.join().unwrap();
    let _ = std::fs::remove_file(path);
}

fn run_socket_once(command: &str, response: Vec<u8>) -> std::process::Output {
    run_socket_once_args(&[command], response)
}

fn run_socket_once_args(args: &[&str], response: Vec<u8>) -> std::process::Output {
    let path = std::env::temp_dir().join(format!(
        "typhon-astreactl-once-{}-{}.sock",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        stream.read_to_end(&mut request).unwrap();
        stream.write_all(&response).unwrap();
    });
    let mut cli_args = vec!["--json", "--socket", path.to_str().unwrap()];
    cli_args.extend_from_slice(args);
    let output = Command::new(env!("CARGO_BIN_EXE_astreactl"))
        .args(cli_args)
        .output()
        .unwrap();
    server.join().unwrap();
    let _ = std::fs::remove_file(path);
    output
}

#[test]
fn astreactl_handles_one_hundred_json_cycles() {
    run_socket_cycles("version", envelope(valid_result("version")), 0);
}

#[test]
fn astreactl_handles_one_hundred_malformed_response_cycles() {
    run_socket_cycles("windows", b"not-json\n".to_vec(), 6);
}

#[test]
fn astreactl_handles_one_hundred_status_cycles() {
    run_socket_cycles("status", envelope(valid_result("status")), 0);
}

#[test]
fn astreactl_handles_one_hundred_windows_cycles() {
    run_socket_cycles("windows", envelope(valid_result("windows")), 0);
}

#[test]
fn astreactl_handles_one_hundred_doctor_cycles() {
    run_socket_cycles("doctor", envelope(valid_result("doctor")), 0);
}

#[test]
fn astreactl_handles_one_hundred_outputs_cycles() {
    run_socket_cycles("outputs", envelope(valid_result("outputs")), 0);
}

#[test]
fn astreactl_handles_one_hundred_unhealthy_doctor_cycles() {
    run_socket_cycles(
        "doctor",
        envelope(serde_json::json!({"healthy": false, "checks": [{
            "id": "session.state",
            "severity": "warning",
            "summary": "session suspended",
            "detail": null
        }]})),
        7,
    );
}

#[test]
fn astreactl_handles_one_hundred_server_error_doctor_cycles() {
    run_socket_cycles("doctor", server_error_envelope(), 1);
}

#[test]
fn astreactl_handles_one_hundred_discovery_cycles() {
    let runtime = std::env::temp_dir().join(format!(
        "typhon-astreactl-discovery-stress-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let instance = runtime.join("astrea/typhon/attempt-1");
    let astrea = runtime.join("astrea");
    let typhon = astrea.join("typhon");
    std::fs::create_dir_all(&instance).unwrap();
    for directory in [
        runtime.as_path(),
        astrea.as_path(),
        typhon.as_path(),
        instance.as_path(),
    ] {
        std::fs::set_permissions(
            directory,
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
    }
    let socket = instance.join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..100 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            stream.read_to_end(&mut request).unwrap();
            stream.write_all(&envelope(valid_result("status"))).unwrap();
        }
    });
    for _ in 0..100 {
        let output = Command::new(env!("CARGO_BIN_EXE_astreactl"))
            .args(["--json", "status"])
            .env("XDG_RUNTIME_DIR", &runtime)
            .env_remove("WAYLAND_DISPLAY")
            .output()
            .unwrap();
        assert!(output.status.success(), "stderr={:?}", output.stderr);
    }
    server.join().unwrap();
    let _ = std::fs::remove_dir_all(runtime);
}

#[test]
fn astreactl_handles_one_hundred_write_half_close_cycles() {
    run_socket_cycles("activewindow", envelope(valid_result("activewindow")), 0);
}

#[test]
fn astreactl_handles_one_hundred_cursor_get_cycles() {
    run_socket_cycles_args(&["cursor", "get"], envelope(valid_result("cursor.get")), 0);
}

#[test]
fn astreactl_handles_one_hundred_cursor_write_half_close_cycles() {
    run_socket_cycles_args(&["cursor", "get"], envelope(valid_result("cursor.get")), 0);
}

#[test]
fn astreactl_handles_one_hundred_cursor_reload_cycles() {
    run_socket_cycles_args(
        &["cursor", "reload"],
        envelope(valid_result("cursor.reload")),
        0,
    );
}

#[test]
fn astreactl_handles_one_hundred_cursor_server_error_cycles() {
    run_socket_cycles_args(
        &["cursor", "set-theme", "missing"],
        server_error_envelope(),
        1,
    );
}

#[test]
fn astreactl_sends_one_request_and_prints_json_result() {
    let path = std::env::temp_dir().join(format!(
        "typhon-astreactl-test-{}-{}.sock",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    let mut expected = valid_result("status");
    expected["instance"] = serde_json::json!("test");
    let response = envelope(expected.clone());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        stream.read_to_end(&mut request).unwrap();
        assert_eq!(request.last(), Some(&b'\n'));
        assert!(request.windows(8).any(|part| part == b"\"status\""));
        stream.write_all(&response).unwrap();
    });
    let output = Command::new(env!("CARGO_BIN_EXE_astreactl"))
        .args(["--json", "--socket"])
        .arg(&path)
        .arg("status")
        .output()
        .unwrap();
    server.join().unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(output.status.success(), "stderr={:?}", output.stderr);
    let actual: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(actual, expected);
}
