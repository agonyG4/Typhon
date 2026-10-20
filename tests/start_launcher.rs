use std::{
    fs,
    os::unix::fs as unix_fs,
    path::PathBuf,
    process::{Command, Stdio},
};

fn dry_run_launcher(path: PathBuf) -> std::process::Output {
    Command::new(path)
        .env("OBLIVION_ONE_DRY_RUN", "1")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("start launcher should run")
}

#[test]
fn start_launcher_uses_native_output_without_host_display() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = dry_run_launcher(repo_dir.join("bin/start-oblivion-one"));

    assert!(
        output.status.success(),
        "launcher failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("compositor"));
    assert!(!stdout.contains("--output"));
    assert!(stdout.contains("target/release/oblivion-one"));
    assert!(!stdout.contains("cargo run"));
}

#[test]
fn start_launcher_documents_safe_nvidia_egl_wayland2_default() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(repo_dir.join("bin/start-oblivion-one"))
        .arg("--help")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("start launcher help should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let compatibility_line = stdout
        .lines()
        .find(|line| line.contains("OBLIVION_ONE_NVIDIA_EGL_WAYLAND2_COMPAT"))
        .expect("launcher help should document NVIDIA EGL Wayland2 compatibility");
    assert!(compatibility_line.contains("auto (default)"));
    assert!(compatibility_line.contains("off"));
    assert!(compatibility_line.contains("rollback"));
}

#[test]
fn start_launcher_documents_kms_preferred_dmabuf_escape_hatch() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(repo_dir.join("bin/start-oblivion-one"))
        .arg("--help")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("start launcher help should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let policy_line = stdout
        .lines()
        .find(|line| line.contains("OBLIVION_ONE_DMABUF_KMS_PREFERRED"))
        .expect("launcher help should document KMS-preferred DMA-BUF policy");
    assert!(policy_line.contains("auto (default)"));
    assert!(policy_line.contains("off"));
    assert!(policy_line.contains("force"));
}

#[test]
fn nvidia_egl_wayland2_qualifier_accepts_external_kitty_toplevel() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let qualifier = fs::read_to_string(repo_dir.join("bin/qualify-nvidia-egl-wayland2"))
        .expect("NVIDIA EGL Wayland2 qualifier should be readable");

    assert!(qualifier.contains(r"app\.first_toplevel"));
    assert!(qualifier.contains(r"app\.toplevel.*app_id=kitty"));
}

#[test]
fn start_launcher_ignores_inherited_host_display_variables_and_stays_native() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(repo_dir.join("bin/start-oblivion-one"))
        .env("OBLIVION_ONE_DRY_RUN", "1")
        .env("WAYLAND_DISPLAY", "wayland-test")
        .env("DISPLAY", ":0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("start launcher should run");

    assert!(
        output.status.success(),
        "launcher failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("compositor"));
    assert!(!stdout.contains("--output"));
    assert!(!stdout.contains("nested"));
}

#[test]
fn start_launcher_rejects_removed_output_selector_environment() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(repo_dir.join("bin/start-oblivion-one"))
        .env("OBLIVION_ONE_DRY_RUN", "1")
        .env("OBLIVION_ONE_OUTPUT", "nested")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("start launcher should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("OBLIVION_ONE_OUTPUT is no longer supported"));
}

#[test]
fn start_launcher_accepts_sddm_native_launch_with_inherited_display_variables() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(repo_dir.join("bin/start-oblivion-one"))
        .env("OBLIVION_ONE_DRY_RUN", "1")
        .env("OBLIVION_ONE_SDDM_SESSION", "1")
        .env("WAYLAND_DISPLAY", "wayland-from-greeter")
        .env("DISPLAY", ":0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("start launcher should run");

    assert!(
        output.status.success(),
        "launcher failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("--output"));
    assert!(stdout.contains("compositor"));
}

#[test]
fn tty_start_launcher_forces_native_release_output() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let tty_script = fs::read_to_string(repo_dir.join("bin/start-oblivion-one-tty"))
        .expect("tty launcher should be readable");
    let output = Command::new(repo_dir.join("bin/start-oblivion-one-tty"))
        .env("OBLIVION_ONE_DRY_RUN", "1")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env_remove("DISPLAY")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("tty start launcher should run");

    assert!(
        output.status.success(),
        "launcher failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--socket"));
    assert!(stdout.contains("oblivion-one-tty"));
    assert!(!stdout.contains("--output"));
    assert!(stdout.contains("target/release/oblivion-one"));
    assert!(tty_script.contains("OBLIVION_ONE_MODE=\"${OBLIVION_ONE_MODE:-1920x1080@165}\""));
}

#[test]
fn start_launcher_resolves_repo_when_invoked_through_installed_symlink() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let link_dir = repo_dir
        .join("target")
        .join("start-launcher-tests")
        .join(std::process::id().to_string());
    let _ = fs::remove_dir_all(&link_dir);
    fs::create_dir_all(&link_dir).expect("test link dir should be created");
    let link_path = link_dir.join("start-oblivion-one");
    unix_fs::symlink(repo_dir.join("bin/start-oblivion-one"), &link_path)
        .expect("launcher symlink should be created");

    let output = Command::new(link_path)
        .env("OBLIVION_ONE_DRY_RUN", "1")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("start launcher should run");
    let _ = fs::remove_dir_all(&link_dir);

    assert!(
        output.status.success(),
        "launcher failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let normalized_stdout = stdout.replace("\\(", "(").replace("\\)", ")");
    assert!(
        normalized_stdout.contains(&format!(
            "{}/target/release/oblivion-one",
            repo_dir.display()
        )),
        "launcher used the wrong release binary path: {stdout}"
    );
    assert!(!stdout.contains("--output"));
}

#[test]
fn start_launcher_warns_when_native_input_group_is_missing() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(repo_dir.join("bin/start-oblivion-one"))
        .env("OBLIVION_ONE_DRY_RUN", "1")
        .env("OBLIVION_ONE_FORCE_INPUT_GROUP_WARNING", "1")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("start launcher should run");

    assert!(
        output.status.success(),
        "launcher failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("native input needs access to /dev/input/event*"));
    assert!(stderr.contains("install-start-oblivion-one --input-permissions"));
}

#[test]
fn install_launcher_writes_sddm_session_entry_to_target_dir() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let session_dir = repo_dir
        .join("target")
        .join("start-launcher-tests")
        .join(format!("sddm-session-{}", std::process::id()));
    let _ = fs::remove_dir_all(&session_dir);

    let output = Command::new(repo_dir.join("bin/install-start-oblivion-one"))
        .arg("--sddm-session")
        .arg("--target-dir")
        .arg(&session_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("installer should run");

    assert!(
        output.status.success(),
        "installer failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let desktop_entry = fs::read_to_string(session_dir.join("oblivion-one.desktop"))
        .expect("session desktop entry should be written");
    let _ = fs::remove_dir_all(&session_dir);

    assert!(desktop_entry.contains("Name=Oblivion One"));
    assert!(desktop_entry.contains("Type=Application"));
    assert!(desktop_entry.contains("DesktopNames=OblivionOne"));
    assert!(desktop_entry.contains("OBLIVION_ONE_PROFILE=release"));
    assert!(!desktop_entry.contains("OBLIVION_ONE_OUTPUT"));
    assert!(desktop_entry.contains("OBLIVION_ONE_MODE=1920x1080@165"));
    assert!(desktop_entry.contains("OBLIVION_ONE_SDDM_SESSION=1"));
    assert!(desktop_entry.contains("start-oblivion-one"));
}

#[test]
fn install_launcher_can_enable_sddm_perf_log_entry() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let session_dir = repo_dir
        .join("target")
        .join("start-launcher-tests")
        .join(format!("sddm-session-perf-{}", std::process::id()));
    let _ = fs::remove_dir_all(&session_dir);

    let output = Command::new(repo_dir.join("bin/install-start-oblivion-one"))
        .arg("--sddm-session")
        .arg("--target-dir")
        .arg(&session_dir)
        .arg("--perf-log")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("installer should run");

    assert!(
        output.status.success(),
        "installer failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let desktop_entry = fs::read_to_string(session_dir.join("oblivion-one.desktop"))
        .expect("session desktop entry should be written");
    let _ = fs::remove_dir_all(&session_dir);

    assert!(desktop_entry.contains("OBLIVION_ONE_PERF_LOG=1"));
    assert!(desktop_entry.contains("OBLIVION_ONE_MODE=1920x1080@165"));
}

#[test]
fn install_launcher_writes_nvidia_vram_profile_to_target_dir() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let profile_dir = repo_dir
        .join("target")
        .join("start-launcher-tests")
        .join(format!("nvidia-profile-{}", std::process::id()));
    let _ = fs::remove_dir_all(&profile_dir);

    let output = Command::new(repo_dir.join("bin/install-start-oblivion-one"))
        .arg("--nvidia-profile")
        .arg("--target-dir")
        .arg(&profile_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("installer should run");

    assert!(
        output.status.success(),
        "installer failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let profile = fs::read_to_string(profile_dir.join("50-oblivion-one.json"))
        .expect("nvidia profile should be written");
    let _ = fs::remove_dir_all(&profile_dir);

    assert!(profile.contains(r#""matches": "oblivion-one""#));
    assert!(profile.contains(r#""profile": "No VidMem Reuse""#));
}

#[test]
fn install_launcher_adds_tty_start_symlink_for_user_install() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let home_dir = repo_dir
        .join("target")
        .join("start-launcher-tests")
        .join(format!("home-{}", std::process::id()));
    let _ = fs::remove_dir_all(&home_dir);
    fs::create_dir_all(&home_dir).expect("test home should be created");

    let output = Command::new(repo_dir.join("bin/install-start-oblivion-one"))
        .env("HOME", &home_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("installer should run");

    assert!(
        output.status.success(),
        "installer failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tty_link = home_dir.join(".local/bin/start-oblivion-one-tty");
    let tty_link_exists = tty_link.exists();
    let _ = fs::remove_dir_all(&home_dir);

    assert!(tty_link_exists);
    assert!(String::from_utf8_lossy(&output.stdout).contains("start-oblivion-one-tty"));
}

#[test]
fn start_launcher_log_relay_outlives_session_group_signals() {
    use std::os::unix::{fs::PermissionsExt, process::CommandExt};
    use std::time::{Duration, Instant};

    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let test_dir = repo_dir
        .join("target")
        .join("start-launcher-tests")
        .join(format!("log-relay-{}", std::process::id()));
    let _ = fs::remove_dir_all(&test_dir);
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    let fake_binary = test_dir.join("fake-oblivion-one");
    let log_file = test_dir.join("session.log");
    fs::write(
        &fake_binary,
        "#!/usr/bin/env bash\n\
         stop=0\n\
         trap 'stop=1' TERM HUP\n\
         echo 'fake compositor: running' >&2\n\
         while [[ $stop == 0 ]]; do sleep 0.05; done\n\
         sleep 0.2\n\
         echo 'fake compositor: cleanup summary' >&2\n",
    )
    .expect("fake compositor should be written");
    fs::set_permissions(&fake_binary, fs::Permissions::from_mode(0o755))
        .expect("fake compositor should be executable");

    let mut launcher = Command::new(repo_dir.join("bin/start-oblivion-one"))
        .env("OBLIVION_ONE_BINARY", &fake_binary)
        .env("OBLIVION_ONE_STATE_DIR", &test_dir)
        .env("OBLIVION_ONE_LOG_FILE", &log_file)
        .env("OBLIVION_ONE_SKIP_INPUT_GROUP_CHECK", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("start launcher should run");
    let process_group = launcher.id() as libc::pid_t;

    let deadline = Instant::now() + Duration::from_secs(10);
    while !fs::read_to_string(&log_file)
        .unwrap_or_default()
        .contains("fake compositor: running")
    {
        assert!(Instant::now() < deadline, "fake compositor did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Session scope teardown delivers SIGTERM and SIGHUP to every member.
    unsafe {
        libc::kill(-process_group, libc::SIGTERM);
        libc::kill(-process_group, libc::SIGHUP);
    }
    let status = launcher.wait().expect("launcher should exit");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut log = String::new();
    while Instant::now() < deadline {
        log = fs::read_to_string(&log_file).unwrap_or_default();
        if log.contains("fake compositor: cleanup summary") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = fs::remove_dir_all(&test_dir);

    assert!(
        status.success(),
        "launcher exited with {status}; log:\n{log}"
    );
    assert!(
        log.contains("fake compositor: cleanup summary"),
        "cleanup output after session signals was lost; log:\n{log}"
    );
}

#[test]
fn start_launcher_rotates_oversized_session_log_at_launch() {
    use std::os::unix::fs::PermissionsExt;

    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let test_dir = repo_dir
        .join("target")
        .join("start-launcher-tests")
        .join(format!("log-rotate-{}", std::process::id()));
    let _ = fs::remove_dir_all(&test_dir);
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    let fake_binary = test_dir.join("fake-oblivion-one");
    fs::write(&fake_binary, "#!/usr/bin/env bash\nexit 0\n")
        .expect("fake compositor should be written");
    fs::set_permissions(&fake_binary, fs::Permissions::from_mode(0o755))
        .expect("fake compositor should be executable");
    let log_file = test_dir.join("session.log");
    let rotated_log = test_dir.join("session.log.1");
    let old_content = "old session content that exceeds the rotation threshold\n";
    fs::write(&log_file, old_content).expect("old session log should be written");

    let output = Command::new(repo_dir.join("bin/start-oblivion-one"))
        .env("OBLIVION_ONE_BINARY", &fake_binary)
        .env("OBLIVION_ONE_STATE_DIR", &test_dir)
        .env("OBLIVION_ONE_LOG_FILE", &log_file)
        .env("OBLIVION_ONE_SKIP_INPUT_GROUP_CHECK", "1")
        .env("OBLIVION_ONE_LOG_ROTATE_BYTES", "16")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("start launcher should run");
    let rotated = fs::read_to_string(&rotated_log).unwrap_or_default();
    let current = fs::read_to_string(&log_file).unwrap_or_default();
    let _ = fs::remove_dir_all(&test_dir);

    assert!(
        output.status.success(),
        "launcher failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        rotated.contains(old_content),
        "old log content should move to session.log.1; rotated log:\n{rotated}"
    );
    assert!(
        !current.contains(old_content),
        "new session.log should not keep old content; log:\n{current}"
    );
}
