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
    assert!(!stdout.contains("gamescope"));
    assert!(stdout.contains("compositor"));
    assert!(stdout.contains("--output"));
    assert!(stdout.contains("native"));
    assert!(stdout.contains("target/release/oblivion-one"));
    assert!(!stdout.contains("cargo run"));
}

#[test]
fn start_launcher_uses_nested_output_when_host_display_is_available() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(repo_dir.join("bin/start-oblivion-one"))
        .env("OBLIVION_ONE_DRY_RUN", "1")
        .env("WAYLAND_DISPLAY", "wayland-test")
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
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("gamescope"));
    assert!(stdout.contains("compositor"));
    assert!(stdout.contains("--output"));
    assert!(stdout.contains("nested"));
    assert!(!stdout.contains("--release"));
}

#[test]
fn start_launcher_forwards_nested_output_size_and_refresh_before_app_args() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(repo_dir.join("bin/start-oblivion-one"))
        .env("OBLIVION_ONE_DRY_RUN", "1")
        .env("WAYLAND_DISPLAY", "wayland-test")
        .env_remove("DISPLAY")
        .args([
            "--width",
            "1920",
            "--height",
            "1080",
            "--refresh",
            "165",
            "--",
            "zen-browser",
        ])
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
    assert!(stdout.contains("--output nested"));
    assert!(stdout.contains("--width 1920"));
    assert!(stdout.contains("--height 1080"));
    assert!(stdout.contains("--refresh 165"));
    assert!(stdout.contains("-- zen-browser"));
    assert!(stdout.find("--refresh 165").unwrap() < stdout.find("-- zen-browser").unwrap());
}

#[test]
fn start_launcher_blocks_manual_native_output_inside_host_display() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(repo_dir.join("bin/start-oblivion-one"))
        .env("OBLIVION_ONE_DRY_RUN", "1")
        .env("OBLIVION_ONE_OUTPUT", "native")
        .env("WAYLAND_DISPLAY", "wayland-host")
        .env_remove("WAYLAND_SOCKET")
        .env_remove("DISPLAY")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("start launcher should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("refusing native scanout because host display variables are set"));
}

#[test]
fn start_launcher_allows_sddm_native_output_with_inherited_display_variables() {
    let repo_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(repo_dir.join("bin/start-oblivion-one"))
        .env("OBLIVION_ONE_DRY_RUN", "1")
        .env("OBLIVION_ONE_OUTPUT", "native")
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
    assert!(stdout.contains("--output"));
    assert!(stdout.contains("native"));
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
    assert!(stdout.contains("--output"));
    assert!(stdout.contains("native"));
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
    assert!(
        stdout.contains(&format!(
            "{}/target/release/oblivion-one",
            repo_dir.display()
        )),
        "launcher used the wrong release binary path: {stdout}"
    );
    assert!(stdout.contains("--output"));
    assert!(stdout.contains("native"));
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
    assert!(desktop_entry.contains("OBLIVION_ONE_OUTPUT=native"));
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
