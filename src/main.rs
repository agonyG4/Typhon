use std::{
    error::Error,
    fs, io,
    path::PathBuf,
    process::{Command, ExitCode},
};

mod egl_renderer;
mod native_output;
mod nested_output;
mod nested_renderer;
mod prototype;

use oblivion_one::{
    DEFAULT_APP, DesktopOptions, HyprlandLaunchPlan, NestedBackend, NestedLaunchPlan,
    NestedOptions,
    compositor::{CompositorPlan, OwnCompositorServer},
    default_state_dir, discover_tools, export_lines, parse_session_env,
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
        Mode::Help => {
            print_help();
            Ok(())
        }
        Mode::Doctor => doctor(),
        Mode::Compositor(options) => own_compositor(options),
        Mode::Desktop(options) => desktop(options),
        Mode::Nested(options) => nested(options),
        Mode::Run(command) => run_app(command),
        Mode::Env => print_env(),
        Mode::Smoke => smoke(),
        Mode::Prototype { inside_de } => prototype::run_prototype(inside_de),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Mode {
    Help,
    Doctor,
    Compositor(CompositorCliOptions),
    Desktop(DesktopOptions),
    Nested(NestedOptions),
    Run(Vec<String>),
    Env,
    Smoke,
    Prototype { inside_de: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompositorCliOptions {
    socket_name: String,
    check_only: bool,
    renderer: nested_renderer::OutputRendererPreference,
    output_backend: CompositorOutputBackend,
    app: Vec<String>,
}

impl Default for CompositorCliOptions {
    fn default() -> Self {
        Self {
            socket_name: "oblivion-one-0".to_string(),
            check_only: false,
            renderer: nested_renderer::OutputRendererPreference::Gpu,
            output_backend: CompositorOutputBackend::Auto,
            app: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompositorOutputBackend {
    Auto,
    Nested,
    Native,
}

impl CompositorOutputBackend {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" | "Auto" => Some(Self::Auto),
            "nested" | "Nested" => Some(Self::Nested),
            "native" | "Native" => Some(Self::Native),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedCompositorOutputBackend {
    Nested,
    Native,
}

impl ResolvedCompositorOutputBackend {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Nested => "nested",
            Self::Native => "native",
        }
    }
}

fn resolve_compositor_output_backend(
    requested: CompositorOutputBackend,
    host_display_available: bool,
) -> ResolvedCompositorOutputBackend {
    match requested {
        CompositorOutputBackend::Auto if host_display_available => {
            ResolvedCompositorOutputBackend::Nested
        }
        CompositorOutputBackend::Auto => ResolvedCompositorOutputBackend::Native,
        CompositorOutputBackend::Nested => ResolvedCompositorOutputBackend::Nested,
        CompositorOutputBackend::Native => ResolvedCompositorOutputBackend::Native,
    }
}

fn host_display_available() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var_os("WAYLAND_SOCKET").is_some()
        || std::env::var_os("DISPLAY").is_some()
}

fn parse_args(args: Vec<String>) -> Result<Mode, io::Error> {
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(Mode::Help);
    };

    match command {
        "-h" | "--help" | "help" => Ok(Mode::Help),
        "doctor" => Ok(Mode::Doctor),
        "compositor" => parse_compositor_args(&args[1..]).map(Mode::Compositor),
        "de" | "desktop" => parse_desktop_args(&args[1..]).map(Mode::Desktop),
        "env" => Ok(Mode::Env),
        "smoke" => Ok(Mode::Smoke),
        "prototype" | "proto" => parse_prototype_args(&args[1..]),
        "run" => {
            let command = if args.len() > 1 {
                args[1..].to_vec()
            } else {
                vec![DEFAULT_APP.to_string()]
            };
            Ok(Mode::Run(command))
        }
        "nested" => parse_nested_args(&args[1..]).map(Mode::Nested),
        other => Err(invalid_input(format!("unknown command `{other}`"))),
    }
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
            "--check" => {
                options.check_only = true;
            }
            "--socket" => {
                index += 1;
                let Some(socket_name) = args.get(index) else {
                    return Err(invalid_input("--socket needs a socket name"));
                };
                options.socket_name = socket_name.clone();
            }
            "--renderer" => {
                index += 1;
                let Some(renderer) = args.get(index) else {
                    return Err(invalid_input("--renderer needs auto, gpu, or cpu"));
                };
                options.renderer = parse_renderer_arg(renderer)?;
            }
            "--output" => {
                index += 1;
                let Some(output) = args.get(index) else {
                    return Err(invalid_input("--output needs auto, nested, or native"));
                };
                options.output_backend = parse_output_backend_arg(output)?;
            }
            value if value.starts_with("--socket=") => {
                options.socket_name = value["--socket=".len()..].to_string();
            }
            value if value.starts_with("--renderer=") => {
                options.renderer = parse_renderer_arg(&value["--renderer=".len()..])?;
            }
            value if value.starts_with("--output=") => {
                options.output_backend = parse_output_backend_arg(&value["--output=".len()..])?;
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

fn parse_output_backend_arg(value: &str) -> Result<CompositorOutputBackend, io::Error> {
    CompositorOutputBackend::parse(value).ok_or_else(|| {
        invalid_input(format!(
            "--output expects `auto`, `nested`, or `native`, got `{value}`"
        ))
    })
}

fn parse_renderer_arg(value: &str) -> Result<nested_renderer::OutputRendererPreference, io::Error> {
    nested_renderer::OutputRendererPreference::parse(value).ok_or_else(|| {
        invalid_input(format!(
            "--renderer expects `auto`, `gpu`, or `cpu`, got `{value}`"
        ))
    })
}

fn parse_desktop_args(args: &[String]) -> Result<DesktopOptions, io::Error> {
    let executable = std::env::current_exe().map_err(|error| {
        io::Error::other(format!("failed to resolve current executable: {error}"))
    })?;
    let mut options = DesktopOptions::with_defaults(default_state_dir(), executable);
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--width" => {
                index += 1;
                options.width = parse_u32_arg("--width", args.get(index))?;
            }
            "--height" => {
                index += 1;
                options.height = parse_u32_arg("--height", args.get(index))?;
            }
            "--refresh" => {
                index += 1;
                options.refresh = parse_u32_arg("--refresh", args.get(index))?;
            }
            "--state-dir" => {
                index += 1;
                let Some(path) = args.get(index) else {
                    return Err(invalid_input("--state-dir needs a path"));
                };
                options.state_dir = PathBuf::from(path);
            }
            "--backend" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(invalid_input(
                        "--backend needs oblivion, hyprland, or gamescope",
                    ));
                };
                options.backend = parse_backend_arg(value)?;
            }
            value if value.starts_with("--width=") => {
                options.width = parse_u32_value("--width", &value["--width=".len()..])?;
            }
            value if value.starts_with("--height=") => {
                options.height = parse_u32_value("--height", &value["--height=".len()..])?;
            }
            value if value.starts_with("--refresh=") => {
                options.refresh = parse_u32_value("--refresh", &value["--refresh=".len()..])?;
            }
            value if value.starts_with("--state-dir=") => {
                options.state_dir = PathBuf::from(&value["--state-dir=".len()..]);
            }
            value if value.starts_with("--backend=") => {
                options.backend = parse_backend_arg(&value["--backend=".len()..])?;
            }
            other => return Err(invalid_input(format!("unknown de option `{other}`"))),
        }

        index += 1;
    }

    Ok(options)
}

fn parse_prototype_args(args: &[String]) -> Result<Mode, io::Error> {
    let mut inside_de = false;

    for arg in args {
        match arg.as_str() {
            "--inside-de" => inside_de = true,
            other => return Err(invalid_input(format!("unknown prototype option `{other}`"))),
        }
    }

    Ok(Mode::Prototype { inside_de })
}

fn parse_backend_arg(value: &str) -> Result<NestedBackend, io::Error> {
    NestedBackend::parse(value).ok_or_else(|| {
        invalid_input(format!(
            "--backend expects `oblivion`, `hyprland`, or `gamescope`, got `{value}`"
        ))
    })
}

fn parse_nested_args(args: &[String]) -> Result<NestedOptions, io::Error> {
    let mut options = NestedOptions::with_defaults(default_state_dir());
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            options.app = args[index + 1..].to_vec();
            break;
        }

        match arg.as_str() {
            "--width" => {
                index += 1;
                options.width = parse_u32_arg("--width", args.get(index))?;
            }
            "--height" => {
                index += 1;
                options.height = parse_u32_arg("--height", args.get(index))?;
            }
            "--refresh" => {
                index += 1;
                options.refresh = parse_u32_arg("--refresh", args.get(index))?;
            }
            "--state-dir" => {
                index += 1;
                let Some(path) = args.get(index) else {
                    return Err(invalid_input("--state-dir needs a path"));
                };
                options.state_dir = PathBuf::from(path);
            }
            value if value.starts_with("--width=") => {
                options.width = parse_u32_value("--width", &value["--width=".len()..])?;
            }
            value if value.starts_with("--height=") => {
                options.height = parse_u32_value("--height", &value["--height=".len()..])?;
            }
            value if value.starts_with("--refresh=") => {
                options.refresh = parse_u32_value("--refresh", &value["--refresh=".len()..])?;
            }
            value if value.starts_with("--state-dir=") => {
                options.state_dir = PathBuf::from(&value["--state-dir=".len()..]);
            }
            other => return Err(invalid_input(format!("unknown nested option `{other}`"))),
        }

        index += 1;
    }

    Ok(options)
}

fn parse_u32_arg(name: &str, value: Option<&String>) -> Result<u32, io::Error> {
    let Some(value) = value else {
        return Err(invalid_input(format!("{name} needs a value")));
    };
    parse_u32_value(name, value)
}

fn parse_u32_value(name: &str, value: &str) -> Result<u32, io::Error> {
    value
        .parse()
        .map_err(|_| invalid_input(format!("{name} expects a positive integer")))
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn own_compositor(options: CompositorCliOptions) -> AppResult<()> {
    let plan = CompositorPlan::new(&options.socket_name);
    let output_backend =
        resolve_compositor_output_backend(options.output_backend, host_display_available());
    println!("Oblivion One compositor");
    println!("socket: {}", plan.socket_name);
    println!("external compositor: {}", plan.uses_external_compositor());
    println!("renderer preference: {}", options.renderer.as_str());
    println!("output backend: {}", output_backend.as_str());
    let protocol_names = compositor_protocol_names_for_output_backend(&plan, output_backend);
    println!("protocols: {}", protocol_names.join(", "));
    println!("command: {}", plan.command_preview());

    let server = match output_backend {
        ResolvedCompositorOutputBackend::Nested => OwnCompositorServer::bind(&options.socket_name)?,
        ResolvedCompositorOutputBackend::Native => {
            println!("gpu buffer protocols: disabled for native CPU scanout");
            OwnCompositorServer::bind_cpu_composition(&options.socket_name)?
        }
    };
    println!("Wayland socket bound: {}", server.socket_name());

    if options.check_only {
        return Ok(());
    }

    match output_backend {
        ResolvedCompositorOutputBackend::Nested => {
            println!("Opening nested output window. Close the window or press Ctrl+C to stop.");
            println!(
                "Spotlight is built into that nested output window: press Super+Space or Ctrl+Space."
            );
            nested_output::run(server, options.renderer, options.app)
        }
        ResolvedCompositorOutputBackend::Native => native_output::run(server, options.app),
    }
}

fn compositor_protocol_names_for_output_backend(
    plan: &CompositorPlan,
    output_backend: ResolvedCompositorOutputBackend,
) -> Vec<&'static str> {
    let mut protocols = plan.protocol_names().to_vec();
    if output_backend == ResolvedCompositorOutputBackend::Native {
        protocols.retain(|protocol| {
            !matches!(
                *protocol,
                "zwp_linux_dmabuf_v1" | "wp_linux_drm_syncobj_manager_v1" | "wl_drm"
            )
        });
    }
    protocols
}

fn desktop(options: DesktopOptions) -> AppResult<()> {
    match options.backend {
        NestedBackend::Oblivion => own_compositor(CompositorCliOptions::default()),
        NestedBackend::Hyprland => hyprland_desktop(options),
        NestedBackend::Gamescope => {
            println!("Oblivion One DE nested session");
            println!("backend: gamescope");
            println!("apps launched from the prototype will target the nested display");
            nested(options.into_nested_options())
        }
    }
}

fn hyprland_desktop(options: DesktopOptions) -> AppResult<()> {
    fs::create_dir_all(&options.state_dir)?;

    let plan = HyprlandLaunchPlan::new(options);
    if plan.env_file.exists() {
        fs::remove_file(&plan.env_file)?;
    }
    fs::write(&plan.wrapper_file, &plan.wrapper_contents)?;
    fs::write(&plan.config_file, &plan.config_contents)?;

    println!("Oblivion One DE nested session");
    println!("backend: hyprland");
    println!("env file: {}", plan.env_file.display());
    println!("config: {}", plan.config_file.display());
    println!("command: {}", plan.display_command());

    let status = Command::new(&plan.program).args(&plan.args).status()?;
    if !status.success() {
        return Err(io::Error::other(format!("Hyprland exited with {status}")).into());
    }

    Ok(())
}

fn nested(options: NestedOptions) -> AppResult<()> {
    fs::create_dir_all(&options.state_dir)?;

    let plan = NestedLaunchPlan::new(options);
    if plan.env_file.exists() {
        fs::remove_file(&plan.env_file)?;
    }

    println!("Oblivion One nested lab");
    println!("env file: {}", plan.env_file.display());
    println!("command: {}", plan.display_command());

    let mut command = Command::new(&plan.program);
    command.args(&plan.args);
    for (key, value) in plan.env_pairs() {
        command.env(key, value);
    }

    let status = command.status()?;
    if !status.success() {
        return Err(io::Error::other(format!("gamescope exited with {status}")).into());
    }

    Ok(())
}

fn run_app(command: Vec<String>) -> AppResult<()> {
    let env_file = default_state_dir().join("session.env");
    let contents = fs::read_to_string(&env_file).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to read {}; start `oblivion-one nested` first ({error})",
                env_file.display()
            ),
        )
    })?;
    let env = parse_session_env(&contents);

    let Some((program, args)) = command.split_first() else {
        return Err(invalid_input("run needs a command").into());
    };

    let mut child = Command::new(program);
    child.args(args).envs(&env);
    let child = child.spawn()?;
    println!(
        "spawned `{program}` in Oblivion One nested session as pid {}",
        child.id()
    );
    Ok(())
}

fn print_env() -> AppResult<()> {
    let env_file = default_state_dir().join("session.env");
    let contents = fs::read_to_string(&env_file)?;
    for line in export_lines(&parse_session_env(&contents)) {
        println!("{line}");
    }
    Ok(())
}

fn smoke() -> AppResult<()> {
    let state_dir = default_state_dir();
    let compositor_plan = CompositorPlan::new("oblivion-one-0");
    let gamescope_plan = NestedLaunchPlan::new(NestedOptions::with_defaults(state_dir));

    println!("Oblivion One smoke check");
    for tool in discover_tools(&["gamescope", DEFAULT_APP, "dbus-run-session"]) {
        let status = tool
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "missing".to_string());
        println!("{}: {status}", tool.name);
    }
    println!(
        "compositor command preview: {}",
        compositor_plan.command_preview()
    );
    println!(
        "gamescope fallback preview: {}",
        gamescope_plan.display_command()
    );
    Ok(())
}

fn doctor() -> AppResult<()> {
    println!("Oblivion One doctor");
    for tool in discover_tools(&[
        "gamescope",
        DEFAULT_APP,
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
            "host prerequisites available; backend still experimental"
        } else if native_session.plan.can_attempt_native_session() {
            "experimental"
        } else {
            "blocked"
        }
    );
    println!(
        "Native backend status: libseat-managed input/DRM with GBM pageflip attempt and direct/raw/dumb fallbacks."
    );
    for warning in native_session.plan.warnings() {
        println!("Native session warning: {warning}");
    }
    println!("Native SDDM is experimental: build release before selecting it.");
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
        r#"Oblivion One

Usage:
  oblivion-one doctor
  oblivion-one compositor [--check] [--socket oblivion-one-0] [--renderer auto|gpu|cpu] [--output auto|nested|native] [-- app args...]
  oblivion-one de [--backend oblivion|hyprland|gamescope] [--width 1280] [--height 720] [--refresh 60]
  oblivion-one smoke
  oblivion-one prototype
  oblivion-one nested [--width 1280] [--height 720] [--refresh 60] [--state-dir PATH] [-- app args...]
  oblivion-one run [app args...]
  oblivion-one env

Examples:
  cargo run -- doctor
  cargo run -- compositor --check
  cargo run -- compositor -- wayland-info
  cargo run -- compositor -- kitty --single-instance=no --session=none --class OblivionOneKitty
  cargo run -- compositor -- brave
  cargo run -- de
  cargo run -- prototype
  cargo run -- nested -- kitty --class OblivionOne
  cargo run -- run kitty

Compositor starts Oblivion One's own Wayland server path.
Output auto uses nested under an existing display and native without one.
Its production renderer is GPU/EGL/GLES. CPU remains a fallback/debug renderer.
Owned compositor app launches are Wayland-only by default; X11 compatibility must go through an Oblivion-owned XWayland bridge.
DE is currently legacy lab glue while the owned compositor is built.
Prototype opens the visual shell mockup only. Nested uses gamescope as the fallback low-level lab backend.
SDDM is available as an experimental native session through bin/install-start-oblivion-one --sddm-session.
"#
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_prototype_inside_de_mode() {
        let mode = parse_args(vec!["prototype".to_string(), "--inside-de".to_string()]).unwrap();

        assert_eq!(mode, Mode::Prototype { inside_de: true });
    }

    #[test]
    fn parse_desktop_backend_option() {
        let mode = parse_args(vec![
            "de".to_string(),
            "--backend".to_string(),
            "gamescope".to_string(),
        ])
        .unwrap();

        let Mode::Desktop(options) = mode else {
            panic!("expected desktop mode");
        };
        assert_eq!(options.backend, NestedBackend::Gamescope);
    }

    #[test]
    fn parse_own_compositor_check_mode() {
        let mode = parse_args(vec![
            "compositor".to_string(),
            "--check".to_string(),
            "--socket".to_string(),
            "oblivion-one-test".to_string(),
        ])
        .unwrap();

        assert_eq!(
            mode,
            Mode::Compositor(CompositorCliOptions {
                socket_name: "oblivion-one-test".to_string(),
                check_only: true,
                renderer: nested_renderer::OutputRendererPreference::Gpu,
                output_backend: CompositorOutputBackend::Auto,
                app: Vec::new(),
            })
        );
    }

    #[test]
    fn parse_own_compositor_rejects_removed_debug_resize_option() {
        let error =
            parse_args(vec!["compositor".to_string(), "--debug-resize".to_string()]).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("unknown compositor option"));
    }

    #[test]
    fn parse_own_compositor_defaults_to_gpu_gles_renderer() {
        let mode = parse_args(vec!["compositor".to_string()]).unwrap();

        let Mode::Compositor(options) = mode else {
            panic!("expected compositor mode");
        };
        assert_eq!(
            options.renderer,
            nested_renderer::OutputRendererPreference::Gpu
        );
        assert_eq!(options.output_backend, CompositorOutputBackend::Auto);
    }

    #[test]
    fn parse_own_compositor_renderer_option() {
        let mode =
            parse_args(vec!["compositor".to_string(), "--renderer=gpu".to_string()]).unwrap();

        let Mode::Compositor(options) = mode else {
            panic!("expected compositor mode");
        };
        assert_eq!(
            options.renderer,
            nested_renderer::OutputRendererPreference::Gpu
        );
    }

    #[test]
    fn parse_own_compositor_output_backend_option() {
        let mode = parse_args(vec![
            "compositor".to_string(),
            "--output=native".to_string(),
        ])
        .unwrap();

        let Mode::Compositor(options) = mode else {
            panic!("expected compositor mode");
        };
        assert_eq!(options.output_backend, CompositorOutputBackend::Native);
    }

    #[test]
    fn auto_output_backend_uses_native_without_host_display() {
        assert_eq!(
            resolve_compositor_output_backend(CompositorOutputBackend::Auto, false),
            ResolvedCompositorOutputBackend::Native
        );
    }

    #[test]
    fn auto_output_backend_uses_nested_with_host_display() {
        assert_eq!(
            resolve_compositor_output_backend(CompositorOutputBackend::Auto, true),
            ResolvedCompositorOutputBackend::Nested
        );
    }

    #[test]
    fn parse_own_compositor_rejects_removed_gpu_api_option() {
        let error = parse_args(vec![
            "compositor".to_string(),
            "--gpu-api=vulkan".to_string(),
        ])
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("unknown compositor option"));
    }

    #[test]
    fn compositor_app_env_removes_host_x11_display() {
        use std::ffi::OsStr;

        let mut command = Command::new("true");
        oblivion_one::configure_compositor_app_command(&mut command, "oblivion-one-test");
        let envs = command.get_envs().collect::<Vec<_>>();

        assert_eq!(
            envs.iter()
                .find(|(key, _)| *key == OsStr::new("WAYLAND_DISPLAY"))
                .and_then(|(_, value)| *value),
            Some(OsStr::new("oblivion-one-test"))
        );
        assert_eq!(
            envs.iter()
                .find(|(key, _)| *key == OsStr::new("DISPLAY"))
                .map(|(_, value)| value.is_none()),
            Some(true)
        );
    }

    #[test]
    fn parse_own_compositor_app_after_delimiter() {
        let mode = parse_args(vec![
            "compositor".to_string(),
            "--socket".to_string(),
            "oblivion-one-test".to_string(),
            "--".to_string(),
            "kitty".to_string(),
            "--class".to_string(),
            "OblivionTest".to_string(),
        ])
        .unwrap();

        assert_eq!(
            mode,
            Mode::Compositor(CompositorCliOptions {
                socket_name: "oblivion-one-test".to_string(),
                check_only: false,
                renderer: nested_renderer::OutputRendererPreference::Gpu,
                output_backend: CompositorOutputBackend::Auto,
                app: vec![
                    "kitty".to_string(),
                    "--class".to_string(),
                    "OblivionTest".to_string(),
                ],
            })
        );
    }
}
