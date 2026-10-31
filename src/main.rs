use std::{
    error::Error,
    fmt, io,
    path::PathBuf,
    process::{Command, ExitCode},
};

#[cfg(test)]
use std::sync::Mutex;

mod egl_renderer;
mod native_output;
mod pointer_debug;
mod system_action;
mod system_action_protocol;
#[path = "native/system_action_transport.rs"]
mod system_action_transport;

use oblivion_one::{
    CompositorAppGpuPreference,
    compositor::{
        CompositorPlan, FramePacingProtocolCapabilities, InputProtocolCapabilities,
        OwnCompositorServer, RendererProtocolCapabilities, SelectionProtocolCapabilities,
        client_protocols_for_capabilities_with_frame_pacing,
    },
    default_state_dir, discover_tools, gpu_residency,
    portal::{DesktopIntegration, PortalRuntime},
    session::NativeSessionProbe,
};

type AppResult<T> = Result<T, Box<dyn Error>>;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let error_chain = bounded_error_chain(error.as_ref());
            let runtime_phase = native_startup_phase(error.as_ref()).unwrap_or("runtime");
            eprintln!(
                "{}",
                serde_json::json!({
                    "event": "termination",
                    "exit_category": "error",
                    "runtime_phase": runtime_phase,
                    "error_chain": error_chain,
                })
            );
            ExitCode::from(1)
        }
    }
}

fn run() -> AppResult<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args
        .first()
        .is_some_and(|arg| arg == oblivion_one::application_scope::INTERNAL_SCOPE_EXEC_COMMAND)
    {
        match oblivion_one::application_scope::run_internal_scope_exec(&args[1..]) {
            Ok(never) => match never {},
            Err(error) => return Err(Box::new(error)),
        }
    }

    match parse_args(args)? {
        CliCommand::Help => {
            print_help();
            Ok(())
        }
        CliCommand::Doctor => doctor(),
        CliCommand::Compositor(options) => own_compositor(options),
        CliCommand::Portal(options) => portal(options),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliCommand {
    Help,
    Doctor,
    Compositor(CompositorCliOptions),
    Portal(PortalCliOptions),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PortalCliOptions {
    check_only: bool,
    install_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompositorCliOptions {
    socket_name: String,
    check_only: bool,
    app: Vec<String>,
}

impl Default for CompositorCliOptions {
    fn default() -> Self {
        Self {
            socket_name: "oblivion-one-0".to_string(),
            check_only: false,
            app: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct NativeStartupError {
    phase: &'static str,
    source: Box<dyn Error>,
}

impl fmt::Display for NativeStartupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.phase, self.source)
    }
}

impl Error for NativeStartupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

fn with_native_startup_context<T>(
    phase: &'static str,
    result: Result<T, Box<dyn Error>>,
) -> Result<T, Box<dyn Error>> {
    result.map_err(|source| Box::new(NativeStartupError { phase, source }) as Box<dyn Error>)
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn bounded_error_chain(error: &(dyn Error + 'static)) -> String {
    native_output::format_error_chain(error)
}

fn native_startup_phase(error: &(dyn Error + 'static)) -> Option<&'static str> {
    let mut current = Some(error);
    while let Some(cause) = current {
        if let Some(startup) = cause.downcast_ref::<NativeStartupError>() {
            return Some(startup.phase);
        }
        current = cause.source();
    }
    None
}

fn activation_environment_args(
    socket_name: &str,
    current_desktop: &str,
    session_type: &str,
    runtime_dir: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "--systemd".to_string(),
        format!("WAYLAND_DISPLAY={socket_name}"),
        format!("XDG_CURRENT_DESKTOP={current_desktop}"),
        format!("XDG_SESSION_TYPE={session_type}"),
    ];
    if let Some(runtime_dir) = runtime_dir.filter(|value| !value.trim().is_empty()) {
        args.push(format!("XDG_RUNTIME_DIR={runtime_dir}"));
    }
    args
}

fn import_activation_environment(socket_name: &str) {
    let current_desktop =
        std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "OblivionOne".to_string());
    let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "wayland".to_string());
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok();
    // The user manager is shared across graphical sessions. The launcher
    // clears inherited DISPLAY, and this compositor has not started its own
    // XWayland yet, so importing DISPLAY here could publish another session's
    // stale X server to newly activated services. Leave any existing shared
    // DISPLAY untouched rather than unsetting it for other sessions.
    let args = activation_environment_args(
        socket_name,
        &current_desktop,
        &session_type,
        runtime_dir.as_deref(),
    );

    let mut command = Command::new("dbus-update-activation-environment");
    command.args(&args);
    oblivion_one::process::prepare_child_signal_mask(&mut command);
    match command.status() {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!(
            "oblivion-one: D-Bus activation environment update failed with status {status}"
        ),
        Err(error) => {
            eprintln!("oblivion-one: could not update D-Bus activation environment: {error}")
        }
    }
    restart_screencast_portal();
}

/// XDPH stays bound to the Wayland socket it started on and gives up after a
/// few reconnect attempts while the compositor restarts. Clear that failure
/// and point a running instance at this compositor.
fn restart_screencast_portal() {
    for action in ["reset-failed", "try-restart"] {
        let mut command = Command::new("systemctl");
        command.args(["--user", action, "xdg-desktop-portal-hyprland.service"]);
        oblivion_one::process::prepare_child_signal_mask(&mut command);
        if let Err(error) = command.status() {
            eprintln!("oblivion-one: could not {action} xdg-desktop-portal-hyprland: {error}");
            return;
        }
    }
}

fn parse_args(args: Vec<String>) -> Result<CliCommand, io::Error> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(CliCommand::Help);
    };

    match command {
        "-h" | "--help" | "help" => Ok(CliCommand::Help),
        "doctor" => Ok(CliCommand::Doctor),
        "compositor" => parse_compositor_args(&args[1..]).map(CliCommand::Compositor),
        "portal" => parse_portal_args(&args[1..]).map(CliCommand::Portal),
        other => Err(invalid_input(format!("unknown command `{other}`"))),
    }
}

fn parse_portal_args(args: &[String]) -> Result<PortalCliOptions, io::Error> {
    let mut options = PortalCliOptions::default();
    for arg in args {
        match arg.as_str() {
            "--check" => options.check_only = true,
            "--install" => options.install_only = true,
            other => return Err(invalid_input(format!("unknown portal option `{other}`"))),
        }
    }
    Ok(options)
}

fn parse_compositor_args(args: &[String]) -> Result<CompositorCliOptions, io::Error> {
    let mut options = CompositorCliOptions::default();
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            options.app = args[index + 1..].to_vec();
            break;
        }

        match arg.as_str() {
            "--check" => options.check_only = true,
            "--socket" => {
                index += 1;
                let Some(socket_name) = args.get(index) else {
                    return Err(invalid_input("--socket needs a socket name"));
                };
                options.socket_name = socket_name.clone();
            }
            value if value.starts_with("--socket=") => {
                options.socket_name = value["--socket=".len()..].to_string();
            }
            other => {
                return Err(invalid_input(format!(
                    "unknown compositor option `{other}`"
                )));
            }
        }

        index += 1;
    }

    Ok(options)
}

fn native_protocol_names() -> Vec<&'static str> {
    client_protocols_for_capabilities_with_frame_pacing(
        InputProtocolCapabilities::native_libinput(),
        SelectionProtocolCapabilities::core_clipboard(),
        RendererProtocolCapabilities::qualified_native(),
        FramePacingProtocolCapabilities::qualified_native(),
    )
    .into_iter()
    .map(|protocol| protocol.name())
    .filter(|protocol| {
        !matches!(
            *protocol,
            "zwp_linux_dmabuf_v1" | "wp_linux_drm_syncobj_manager_v1" | "wl_drm"
        )
    })
    .collect()
}

const LOG_PIPE_CAPACITY: libc::c_int = 1 << 20;

// The reactor thread runs SCHED_RR and must not block in `eprintln!` behind a
// starved nice-0 log relay, so give a FIFO stdio pipe room to absorb bursts.
// Best effort: unprivileged callers may be capped by pipe-max-size.
fn enlarge_stdio_pipe(fd: std::os::fd::RawFd) {
    // SAFETY: `stat` is a plain C struct that is valid when zeroed, and fstat
    // only writes into it.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 || stat.st_mode & libc::S_IFMT != libc::S_IFIFO {
        return;
    }
    // SAFETY: F_SETPIPE_SZ takes an integer argument and has no memory effects.
    let _ = unsafe { libc::fcntl(fd, libc::F_SETPIPE_SZ, LOG_PIPE_CAPACITY) };
}

fn own_compositor(options: CompositorCliOptions) -> AppResult<()> {
    enlarge_stdio_pipe(libc::STDOUT_FILENO);
    enlarge_stdio_pipe(libc::STDERR_FILENO);
    if !options.check_only {
        // Block process-directed child and termination signals before
        // constructing the Wayland server or entering native initialization.
        // The runtime signalfd later routes them through the native reactor.
        oblivion_one::process::prepare_runtime_signal_handling()?;
    }
    let plan = CompositorPlan::new(&options.socket_name)
        .with_frame_pacing_capabilities(FramePacingProtocolCapabilities::qualified_native());
    println!("Typhon native compositor");
    println!("socket: {}", plan.socket_name);
    println!("runtime: Native");
    println!("external compositor: {}", plan.uses_external_compositor());
    let protocol_names = native_protocol_names();
    println!("protocols: {}", protocol_names.join(", "));
    println!("command: {}", plan.command_preview());

    let server = OwnCompositorServer::bind_with_capabilities_and_frame_pacing(
        &options.socket_name,
        false,
        InputProtocolCapabilities::native_libinput(),
        SelectionProtocolCapabilities::core_clipboard(),
        RendererProtocolCapabilities::qualified_native(),
        FramePacingProtocolCapabilities::qualified_native(),
    )?;
    println!("Wayland socket bound: {}", server.socket_name());

    if options.check_only {
        return Ok(());
    }

    // The Wayland endpoint now exists, so D-Bus and systemd-activated clients
    // can safely inherit its display name instead of a stale parent value.
    import_activation_environment(server.socket_name());

    println!("Starting native bootstrap; Typhon requires a TTY/SDDM seat and usable DRM output.");
    with_native_startup_context(
        "native bootstrap",
        native_output::run(
            server,
            options.app,
            CompositorAppGpuPreference::from_native_env(),
        ),
    )
}

fn portal(options: PortalCliOptions) -> AppResult<()> {
    let runtime = PortalRuntime::for_current_process(default_state_dir())?;
    runtime.install()?;
    let desktop = match DesktopIntegration::from_environment(default_state_dir())
        .and_then(|desktop| desktop.install().map(|()| desktop))
    {
        Ok(desktop) => Some(desktop),
        Err(error) => {
            eprintln!("oblivion-one: desktop portal integration not installed: {error}");
            None
        }
    };
    if options.check_only || options.install_only {
        println!("Oblivion portal backend");
        println!("service: {}", runtime.service_path().display());
        println!("portal: {}", runtime.portal_path().display());
        println!("dbus name: {}", oblivion_one::portal::BACKEND_BUS_NAME);
        if let Some(desktop) = &desktop {
            for path in desktop.portal_config_paths() {
                println!("desktop portal config: {}", path.display());
            }
            println!("share picker: {}", desktop.picker_path().display());
            println!("xdph config: {}", desktop.xdph_config_path().display());
        }
        return Ok(());
    }
    oblivion_one::portal::run_backend().map_err(Into::into)
}

fn doctor() -> AppResult<()> {
    println!("Typhon doctor");
    for tool in discover_tools(&[
        "quickshell",
        "brave",
        "spotify",
        "Xwayland",
        "cargo",
        "rustc",
        "rustup",
        "dbus-run-session",
    ]) {
        let marker = if tool.is_available() { "ok" } else { "missing" };
        let detail = tool
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "not found in PATH".to_string());
        println!("{marker:7} {:18} {detail}", tool.name);
    }
    println!();
    println!(
        "SDDM session entry: {}",
        sddm_session_entry_status().unwrap_or_else(|| "missing".to_string())
    );
    println!(
        "Native session log: {}",
        default_state_dir().join("session.log").display()
    );
    println!();
    let native_session = NativeSessionProbe::detect();
    println!(
        "Native runtime dir: {}",
        display_probe_path(native_session.runtime_dir.as_deref())
    );
    println!(
        "Native KMS device: {}",
        display_probe_path(native_session.kms_device.as_deref())
    );
    println!(
        "Native render device: {}",
        display_probe_path(native_session.render_device.as_deref())
    );
    println!(
        "Native connected output: {}",
        display_probe_path(native_session.connected_output.as_deref())
    );
    println!(
        "Native input target: {}",
        native_session.plan.input_strategy.as_str()
    );
    println!(
        "Native output target: {}",
        native_session.plan.output_strategy.as_str()
    );
    println!(
        "Native raw input fallback: {}",
        display_probe_path(native_session.raw_input_device.as_deref())
    );
    println!(
        "Native session readiness: {}",
        if native_session.plan.is_production_ready() {
            "native prerequisites available"
        } else if native_session.plan.can_attempt_native_session() {
            "native startup can be attempted with fallback components"
        } else {
            "native startup blocked by missing seat/input/output prerequisites"
        }
    );
    println!(
        "Native backend status: libseat-managed input/DRM with GBM pageflip and native fallback backends."
    );
    for warning in native_session.plan.warnings() {
        println!("Native session warning: {warning}");
    }
    println!("Typhon SDDM integration is experimental: build release before selecting it.");
    Ok(())
}

fn sddm_session_entry_status() -> Option<String> {
    [
        "/usr/share/wayland-sessions/oblivion-one.desktop",
        "/usr/local/share/wayland-sessions/oblivion-one.desktop",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
    .map(|path| path.display().to_string())
}

fn display_probe_path(path: Option<&std::path::Path>) -> String {
    path.map(|path| path.display().to_string())
        .unwrap_or_else(|| "missing".to_string())
}

fn print_help() {
    println!(
        r#"Typhon

Usage:
  oblivion-one doctor
  oblivion-one compositor [--check] [--socket oblivion-one-0] [-- app args...]
  oblivion-one portal [--check|--install]

Examples:
  cargo run -- doctor
  cargo run -- compositor --check
  cargo run -- compositor -- kitty --single-instance=no --class TyphonKitty
  cargo run -- compositor -- brave

Typhon is a native TTY/SDDM Wayland compositor. It acquires the native seat,
DRM/KMS output, renderer, input devices, and shell from the current session.
Launching it inside another graphical session does not select another backend;
native initialization either succeeds or reports the failed native phase.
Native KMS mode selection uses OBLIVION_ONE_MODE. Native scanout, cursor, and
application GPU policies use their OBLIVION_ONE_* environment settings.
Owned compositor app launches are Wayland-only by default; X11 compatibility
must go through a Typhon-owned XWayland bridge.
External shells can be autostarted with OBLIVION_ONE_SHELL_COMMAND.
Super+Space dispatches OBLIVION_ONE_SPOTLIGHT_COMMAND or astrea-spotlight --toggle when available.
Alt+Tab dispatches OBLIVION_ONE_ALT_TAB_COMMAND or astrea-alt-tab --next when available.
SDDM integration is available through bin/install-start-oblivion-one --sddm-session.
"#
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Debug)]
    struct TestError {
        message: String,
        source: Option<Box<TestError>>,
    }

    impl fmt::Display for TestError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(&self.message)
        }
    }

    impl Error for TestError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.source
                .as_deref()
                .map(|source| source as &(dyn Error + 'static))
        }
    }

    fn error_chain(count: usize, message: impl Fn(usize) -> String) -> TestError {
        assert!(count > 0);
        let mut error = TestError {
            message: message(count - 1),
            source: None,
        };
        for index in (0..count - 1).rev() {
            error = TestError {
                message: message(index),
                source: Some(Box::new(error)),
            };
        }
        error
    }

    #[test]
    fn enlarge_stdio_pipe_grows_fifo_and_ignores_regular_files() {
        let mut fds = [0 as libc::c_int; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        enlarge_stdio_pipe(fds[1]);
        let size = unsafe { libc::fcntl(fds[1], libc::F_GETPIPE_SZ) };
        assert!(size > 65536, "pipe size {size} was not enlarged");
        unsafe {
            libc::close(fds[0]);
            libc::close(fds[1]);
        }

        let file = std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
            .expect("open manifest");
        enlarge_stdio_pipe(std::os::fd::AsRawFd::as_raw_fd(&file));
    }

    #[test]
    fn parse_native_compositor_defaults_without_product_mode_state() {
        let command = parse_args(vec!["compositor".to_string()]).unwrap();

        assert_eq!(
            command,
            CliCommand::Compositor(CompositorCliOptions::default())
        );
    }

    #[test]
    fn inherited_display_variables_do_not_change_native_compositor_planning() {
        let _guard = ENV_LOCK.lock().unwrap();
        let wayland_display = std::env::var_os("WAYLAND_DISPLAY");
        let display = std::env::var_os("DISPLAY");
        unsafe {
            std::env::set_var("WAYLAND_DISPLAY", "wayland-1");
            std::env::set_var("DISPLAY", ":0");
        }

        let command = parse_args(vec!["compositor".to_string(), "--check".to_string()]).unwrap();

        unsafe {
            match wayland_display {
                Some(value) => std::env::set_var("WAYLAND_DISPLAY", value),
                None => std::env::remove_var("WAYLAND_DISPLAY"),
            }
            match display {
                Some(value) => std::env::set_var("DISPLAY", value),
                None => std::env::remove_var("DISPLAY"),
            }
        }

        assert!(matches!(command, CliCommand::Compositor(_)));
    }

    #[test]
    fn native_bootstrap_failure_keeps_phase_and_source_context() {
        let result: Result<(), Box<dyn Error>> =
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "seat denied").into());

        let error = with_native_startup_context("native bootstrap", result).unwrap_err();

        assert!(error.to_string().contains("native bootstrap"));
        assert!(error.to_string().contains("seat denied"));
        assert_eq!(error.source().unwrap().to_string(), "seat denied");
        let chain = bounded_error_chain(error.as_ref());
        assert!(chain.contains("native bootstrap: seat denied"));
        assert_eq!(
            native_startup_phase(error.as_ref()),
            Some("native bootstrap")
        );
    }

    #[test]
    fn bounded_error_chain_handles_multibyte_truncation_without_panicking() {
        let error = error_chain(1, |_| format!("{}éétail", "a".repeat(2_044)));

        let formatted = bounded_error_chain(&error);

        assert!(formatted.len() <= 2_048);
        assert!(formatted.is_char_boundary(formatted.len()));
    }

    #[test]
    fn bounded_error_chain_keeps_oversized_messages_within_the_byte_limit() {
        let error = error_chain(1, |_| "🧪".repeat(2_000));

        let formatted = bounded_error_chain(&error);

        assert!(formatted.len() <= 2_048);
        assert!(formatted.is_char_boundary(formatted.len()));
    }

    #[test]
    fn bounded_error_chain_includes_nested_sources() {
        let error = error_chain(2, |index| format!("error-{index}"));

        let formatted = bounded_error_chain(&error);

        assert_eq!(formatted, "error-0: error-1");
    }

    #[test]
    fn bounded_error_chain_marks_only_chains_longer_than_eight_sources() {
        let exactly_eight = error_chain(8, |index| format!("error-{index}"));
        let more_than_eight = error_chain(9, |index| format!("error-{index}"));

        let exactly_eight = bounded_error_chain(&exactly_eight);
        let more_than_eight = bounded_error_chain(&more_than_eight);

        assert!(exactly_eight.contains("error-7"));
        assert!(!exactly_eight.contains("source chain truncated"));
        assert!(more_than_eight.contains("source chain truncated"));
        assert!(!more_than_eight.contains("error-8"));
    }

    #[test]
    fn bounded_error_chain_caps_marker_after_eight_long_sources() {
        let error = error_chain(9, |_| "x".repeat(253));

        let formatted = bounded_error_chain(&error);

        assert!(formatted.len() <= 2_048);
        assert!(formatted.is_char_boundary(formatted.len()));
    }

    #[test]
    fn activation_environment_uses_bound_socket_and_current_session_values() {
        let args = activation_environment_args(
            "oblivion-one-sddm",
            "OblivionOne",
            "wayland",
            Some("/run/user/1000"),
        );

        assert_eq!(
            args,
            [
                "--systemd",
                "WAYLAND_DISPLAY=oblivion-one-sddm",
                "XDG_CURRENT_DESKTOP=OblivionOne",
                "XDG_SESSION_TYPE=wayland",
                "XDG_RUNTIME_DIR=/run/user/1000",
            ]
        );
    }

    #[test]
    fn activation_environment_does_not_import_absent_display() {
        let args = activation_environment_args("wayland-7", "OblivionOne", "wayland", None);
        assert!(args.contains(&"WAYLAND_DISPLAY=wayland-7".to_string()));
        assert!(!args.iter().any(|arg| arg.starts_with("DISPLAY=")));
    }

    #[test]
    fn activation_environment_omits_inherited_stale_display_values() {
        let _guard = ENV_LOCK.lock().unwrap();
        let display = std::env::var_os("DISPLAY");
        unsafe { std::env::set_var("DISPLAY", ":stale") };

        let args = activation_environment_args("wayland-7", "OblivionOne", "wayland", None);

        unsafe {
            match display {
                Some(value) => std::env::set_var("DISPLAY", value),
                None => std::env::remove_var("DISPLAY"),
            }
        }
        assert!(!args.iter().any(|arg| arg.starts_with("DISPLAY=")));
    }

    #[test]
    fn native_compositor_startup_has_no_runtime_backend_selector() {
        let CliCommand::Compositor(options) = parse_args(vec![
            "compositor".to_string(),
            "--socket=typhon-native-test".to_string(),
            "--".to_string(),
            "kitty".to_string(),
        ])
        .unwrap() else {
            panic!("expected native compositor command");
        };

        assert_eq!(options.socket_name, "typhon-native-test");
        assert_eq!(options.app, ["kitty".to_string()]);
    }
}
