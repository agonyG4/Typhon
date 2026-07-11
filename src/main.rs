use std::{error::Error, fmt, io, path::PathBuf, process::ExitCode};

#[cfg(test)]
use std::sync::Mutex;

mod egl_renderer;
mod native_output;

use oblivion_one::{
    CompositorAppGpuPreference,
    compositor::{
        CompositorPlan, InputProtocolCapabilities, OwnCompositorServer,
        RendererProtocolCapabilities, SelectionProtocolCapabilities,
        client_protocols_for_capabilities,
    },
    default_state_dir, discover_tools,
    portal::PortalRuntime,
    session::NativeSessionProbe,
};

type AppResult<T> = Result<T, Box<dyn Error>>;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("oblivion-one: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> AppResult<()> {
    match parse_args(std::env::args().skip(1).collect())? {
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
    client_protocols_for_capabilities(
        InputProtocolCapabilities::native_libinput(),
        SelectionProtocolCapabilities::core_clipboard(),
        RendererProtocolCapabilities::unsupported(),
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

fn own_compositor(options: CompositorCliOptions) -> AppResult<()> {
    let plan = CompositorPlan::new(&options.socket_name);
    println!("Typhon native compositor");
    println!("socket: {}", plan.socket_name);
    println!("runtime: Native");
    println!("external compositor: {}", plan.uses_external_compositor());
    let protocol_names = native_protocol_names();
    println!("protocols: {}", protocol_names.join(", "));
    println!("command: {}", plan.command_preview());

    let server = OwnCompositorServer::bind_with_capabilities(
        &options.socket_name,
        false,
        InputProtocolCapabilities::native_libinput(),
        SelectionProtocolCapabilities::core_clipboard(),
        RendererProtocolCapabilities::unsupported(),
    )?;
    println!("Wayland socket bound: {}", server.socket_name());

    if options.check_only {
        return Ok(());
    }

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
    if options.check_only || options.install_only {
        println!("Oblivion portal backend");
        println!("service: {}", runtime.service_path().display());
        println!("portal: {}", runtime.portal_path().display());
        println!("dbus name: {}", oblivion_one::portal::BACKEND_BUS_NAME);
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

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
