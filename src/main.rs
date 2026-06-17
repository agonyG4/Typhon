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
    CompositorAppGpuPreference, DEFAULT_APP, DesktopOptions, HyprlandLaunchPlan, NestedBackend,
    NestedLaunchPlan, NestedOptions,
    compositor::{
        CompositorPlan, InputProtocolCapabilities, OwnCompositorServer,
        RendererProtocolCapabilities, SelectionProtocolCapabilities,
        client_protocols_for_capabilities,
    },
    default_state_dir, discover_tools, export_lines, parse_session_env,
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
        Mode::Portal(options) => portal(options),
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
    Portal(PortalCliOptions),
    Prototype { inside_de: bool },
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
    renderer: nested_renderer::OutputRendererPreference,
    output_backend: CompositorOutputBackend,
    nested_output: nested_output::NestedOutputConfig,
    nested_output_explicit: bool,
    app: Vec<String>,
}

impl Default for CompositorCliOptions {
    fn default() -> Self {
        Self {
            socket_name: "oblivion-one-0".to_string(),
            check_only: false,
            renderer: nested_renderer::OutputRendererPreference::Gpu,
            output_backend: CompositorOutputBackend::Auto,
            nested_output: nested_output::NestedOutputConfig::default(),
            nested_output_explicit: false,
            app: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompositorRuntimeOptions {
    socket_name: String,
    check_only: bool,
    renderer: nested_renderer::OutputRendererPreference,
    output_backend: ResolvedCompositorOutputBackend,
    nested_output: nested_output::NestedOutputConfig,
    app: Vec<String>,
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

fn resolve_compositor_runtime_options(
    options: CompositorCliOptions,
    host_display_available: bool,
) -> Result<CompositorRuntimeOptions, io::Error> {
    let output_backend =
        resolve_compositor_output_backend(options.output_backend, host_display_available);
    if output_backend == ResolvedCompositorOutputBackend::Native && options.nested_output_explicit {
        return Err(invalid_input(
            "--width, --height, and --refresh configure the nested host output. For native KMS mode selection, use OBLIVION_ONE_MODE.",
        ));
    }

    Ok(CompositorRuntimeOptions {
        socket_name: options.socket_name,
        check_only: options.check_only,
        renderer: options.renderer,
        output_backend,
        nested_output: options.nested_output,
        app: options.app,
    })
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
        "portal" => parse_portal_args(&args[1..]).map(Mode::Portal),
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
            "--width" => {
                index += 1;
                options.nested_output.width = parse_nested_width_arg(args.get(index))?;
                options.nested_output_explicit = true;
            }
            "--height" => {
                index += 1;
                options.nested_output.height = parse_nested_height_arg(args.get(index))?;
                options.nested_output_explicit = true;
            }
            "--refresh" => {
                index += 1;
                options.nested_output.refresh_hz = parse_nested_refresh_arg(args.get(index))?;
                options.nested_output_explicit = true;
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
            value if value.starts_with("--width=") => {
                options.nested_output.width = parse_nested_width_value(&value["--width=".len()..])?;
                options.nested_output_explicit = true;
            }
            value if value.starts_with("--height=") => {
                options.nested_output.height =
                    parse_nested_height_value(&value["--height=".len()..])?;
                options.nested_output_explicit = true;
            }
            value if value.starts_with("--refresh=") => {
                options.nested_output.refresh_hz =
                    parse_nested_refresh_value(&value["--refresh=".len()..])?;
                options.nested_output_explicit = true;
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

fn parse_nested_width_arg(value: Option<&String>) -> Result<u32, io::Error> {
    let Some(value) = value else {
        return Err(invalid_input("--width needs a value"));
    };
    parse_nested_width_value(value)
}

fn parse_nested_height_arg(value: Option<&String>) -> Result<u32, io::Error> {
    let Some(value) = value else {
        return Err(invalid_input("--height needs a value"));
    };
    parse_nested_height_value(value)
}

fn parse_nested_refresh_arg(value: Option<&String>) -> Result<u32, io::Error> {
    let Some(value) = value else {
        return Err(invalid_input("--refresh needs a value"));
    };
    parse_nested_refresh_value(value)
}

fn parse_nested_width_value(value: &str) -> Result<u32, io::Error> {
    parse_u32_value_in_range("--width", value, 320, 16_384, "")
}

fn parse_nested_height_value(value: &str) -> Result<u32, io::Error> {
    parse_u32_value_in_range("--height", value, 240, 16_384, "")
}

fn parse_nested_refresh_value(value: &str) -> Result<u32, io::Error> {
    parse_u32_value_in_range("--refresh", value, 24, 1_000, " Hz")
}

fn parse_u32_value_in_range(
    name: &str,
    value: &str,
    min: u32,
    max: u32,
    unit: &str,
) -> Result<u32, io::Error> {
    let parsed = parse_u32_value(name, value)?;
    if !(min..=max).contains(&parsed) {
        return Err(invalid_input(format!(
            "{name} must be between {min} and {max}{unit}, got {parsed}"
        )));
    }
    Ok(parsed)
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
    let options = resolve_compositor_runtime_options(options, host_display_available())?;
    let plan = CompositorPlan::new(&options.socket_name);
    let output_backend = options.output_backend;
    println!("Oblivion One compositor");
    println!("socket: {}", plan.socket_name);
    println!("external compositor: {}", plan.uses_external_compositor());
    println!("renderer preference: {}", options.renderer.as_str());
    println!("output backend: {}", output_backend.as_str());
    let native_app_gpu_preference = CompositorAppGpuPreference::from_native_env();
    let protocol_names = compositor_protocol_names_for_output_backend(&plan, output_backend);
    println!("protocols: {}", protocol_names.join(", "));
    println!("command: {}", plan.command_preview());

    let server = match output_backend {
        ResolvedCompositorOutputBackend::Nested => OwnCompositorServer::bind_with_capabilities(
            &options.socket_name,
            true,
            InputProtocolCapabilities::nested_winit(),
            SelectionProtocolCapabilities::core_clipboard(),
            renderer_protocol_capabilities_for_output_backend(output_backend),
        )?,
        ResolvedCompositorOutputBackend::Native => {
            println!("gpu buffer protocols: deferred until the native scanout backend is known");
            OwnCompositorServer::bind_with_capabilities(
                &options.socket_name,
                false,
                InputProtocolCapabilities::native_libinput(),
                SelectionProtocolCapabilities::core_clipboard(),
                renderer_protocol_capabilities_for_output_backend(output_backend),
            )?
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
            nested_output::run(server, options.renderer, options.nested_output, options.app)
        }
        ResolvedCompositorOutputBackend::Native => {
            native_output::run(server, options.app, native_app_gpu_preference)
        }
    }
}

fn compositor_protocol_names_for_output_backend(
    plan: &CompositorPlan,
    output_backend: ResolvedCompositorOutputBackend,
) -> Vec<&'static str> {
    let mut protocols = plan.protocol_names().to_vec();
    if output_backend == ResolvedCompositorOutputBackend::Nested {
        protocols = client_protocols_for_capabilities(
            InputProtocolCapabilities::nested_winit(),
            SelectionProtocolCapabilities::core_clipboard(),
            renderer_protocol_capabilities_for_output_backend(output_backend),
        )
        .into_iter()
        .map(|protocol| protocol.name())
        .collect();
    }
    if output_backend == ResolvedCompositorOutputBackend::Native {
        protocols = client_protocols_for_capabilities(
            InputProtocolCapabilities::native_libinput(),
            SelectionProtocolCapabilities::core_clipboard(),
            renderer_protocol_capabilities_for_output_backend(output_backend),
        )
        .into_iter()
        .map(|protocol| protocol.name())
        .filter(|protocol| {
            !matches!(
                *protocol,
                "zwp_linux_dmabuf_v1" | "wp_linux_drm_syncobj_manager_v1" | "wl_drm"
            )
        })
        .collect();
    }
    protocols
}

const fn renderer_protocol_capabilities_for_output_backend(
    _output_backend: ResolvedCompositorOutputBackend,
) -> RendererProtocolCapabilities {
    RendererProtocolCapabilities::unsupported()
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
  oblivion-one compositor [--check] [--socket oblivion-one-0] [--renderer auto|gpu|cpu] [--output auto|nested|native] [--width 1280] [--height 800] [--refresh 60] [-- app args...]
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
  cargo run -- compositor --width 1920 --height 1080 --refresh 165 -- zen-browser
  cargo run -- compositor -- kitty --single-instance=no --session=none --class OblivionOneKitty
  cargo run -- compositor -- brave
  cargo run -- de
  cargo run -- prototype
  cargo run -- nested -- kitty --class OblivionOne
  cargo run -- run kitty

Compositor starts Oblivion One's own Wayland server path.
Output auto uses nested under an existing display and native without one.
Compositor --width/--height/--refresh configure the nested host output only; native KMS mode selection uses OBLIVION_ONE_MODE.
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
                nested_output: nested_output::NestedOutputConfig::default(),
                nested_output_explicit: false,
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
    fn parse_own_compositor_nested_output_options() {
        let mode = parse_args(vec![
            "compositor".to_string(),
            "--width".to_string(),
            "1920".to_string(),
            "--height".to_string(),
            "1080".to_string(),
            "--refresh".to_string(),
            "165".to_string(),
        ])
        .unwrap();

        let Mode::Compositor(options) = mode else {
            panic!("expected compositor mode");
        };
        assert_eq!(
            options.nested_output,
            nested_output::NestedOutputConfig {
                width: 1920,
                height: 1080,
                refresh_hz: 165,
            }
        );
        assert!(options.nested_output_explicit);
    }

    #[test]
    fn parse_own_compositor_nested_output_equals_options() {
        let mode = parse_args(vec![
            "compositor".to_string(),
            "--width=1920".to_string(),
            "--height=1080".to_string(),
            "--refresh=165".to_string(),
        ])
        .unwrap();

        let Mode::Compositor(options) = mode else {
            panic!("expected compositor mode");
        };
        assert_eq!(
            options.nested_output,
            nested_output::NestedOutputConfig {
                width: 1920,
                height: 1080,
                refresh_hz: 165,
            }
        );
        assert!(options.nested_output_explicit);
    }

    #[test]
    fn parse_own_compositor_nested_options_stop_at_app_delimiter() {
        let mode = parse_args(vec![
            "compositor".to_string(),
            "--width".to_string(),
            "1600".to_string(),
            "--height".to_string(),
            "900".to_string(),
            "--refresh".to_string(),
            "165".to_string(),
            "--".to_string(),
            "zen-browser".to_string(),
            "--width".to_string(),
            "800".to_string(),
        ])
        .unwrap();

        let Mode::Compositor(options) = mode else {
            panic!("expected compositor mode");
        };
        assert_eq!(
            options.nested_output,
            nested_output::NestedOutputConfig {
                width: 1600,
                height: 900,
                refresh_hz: 165,
            }
        );
        assert_eq!(
            options.app,
            vec![
                "zen-browser".to_string(),
                "--width".to_string(),
                "800".to_string(),
            ]
        );
    }

    #[test]
    fn parse_own_compositor_rejects_invalid_nested_output_options() {
        for (option, value, expected) in [
            ("--width", "0", "--width must be between 320 and 16384"),
            ("--height", "0", "--height must be between 240 and 16384"),
            ("--refresh", "0", "--refresh must be between 24 and 1000 Hz"),
            ("--width", "abc", "--width expects a positive integer"),
            (
                "--height",
                "20000",
                "--height must be between 240 and 16384",
            ),
            (
                "--refresh",
                "2000",
                "--refresh must be between 24 and 1000 Hz",
            ),
        ] {
            let error = parse_args(vec![
                "compositor".to_string(),
                option.to_string(),
                value.to_string(),
            ])
            .unwrap_err();
            assert!(
                error.to_string().contains(expected),
                "{option} {value} produced {error}"
            );
        }
    }

    #[test]
    fn parse_own_compositor_rejects_missing_nested_output_option_values() {
        for option in ["--width", "--height", "--refresh"] {
            let error = parse_args(vec!["compositor".to_string(), option.to_string()]).unwrap_err();
            assert!(
                error.to_string().contains(option),
                "{option} produced {error}"
            );
            assert!(error.to_string().contains("needs a value"));
        }
    }

    #[test]
    fn native_compositor_rejects_explicit_nested_output_options() {
        let options = parse_compositor_args(&[
            "--output".to_string(),
            "native".to_string(),
            "--width".to_string(),
            "1920".to_string(),
        ])
        .unwrap();

        let error = resolve_compositor_runtime_options(options, true).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("configure the nested host output")
        );
        assert!(error.to_string().contains("OBLIVION_ONE_MODE"));
    }

    #[test]
    fn auto_compositor_allows_nested_output_options_when_host_display_is_available() {
        let options = parse_compositor_args(&[
            "--width".to_string(),
            "1920".to_string(),
            "--height".to_string(),
            "1080".to_string(),
            "--refresh".to_string(),
            "165".to_string(),
        ])
        .unwrap();

        let runtime = resolve_compositor_runtime_options(options, true).unwrap();

        assert_eq!(
            runtime.output_backend,
            ResolvedCompositorOutputBackend::Nested
        );
        assert_eq!(runtime.nested_output.refresh_hz, 165);
    }

    #[test]
    fn auto_compositor_rejects_nested_output_options_when_it_resolves_to_native() {
        let options = parse_compositor_args(&["--width".to_string(), "1920".to_string()]).unwrap();

        let error = resolve_compositor_runtime_options(options, false).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("configure the nested host output")
        );
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
    fn native_output_defers_gpu_buffer_protocols_until_backend_is_known() {
        let protocols = compositor_protocol_names_for_output_backend(
            &CompositorPlan::new("oblivion-one-test"),
            ResolvedCompositorOutputBackend::Native,
        );

        assert!(protocols.contains(&"wl_shm"));
        assert!(!protocols.contains(&"zwp_linux_dmabuf_v1"));
        assert!(!protocols.contains(&"wp_linux_drm_syncobj_manager_v1"));
        assert!(!protocols.contains(&"wl_drm"));
    }

    #[test]
    fn native_cpu_output_omits_gpu_buffer_protocols() {
        let protocols = compositor_protocol_names_for_output_backend(
            &CompositorPlan::new("oblivion-one-test"),
            ResolvedCompositorOutputBackend::Native,
        );

        assert!(protocols.contains(&"wl_shm"));
        assert!(!protocols.contains(&"zwp_linux_dmabuf_v1"));
        assert!(!protocols.contains(&"wp_linux_drm_syncobj_manager_v1"));
        assert!(!protocols.contains(&"wl_drm"));
    }

    #[test]
    fn current_output_backends_do_not_claim_color_management() {
        for backend in [
            ResolvedCompositorOutputBackend::Nested,
            ResolvedCompositorOutputBackend::Native,
        ] {
            let protocols = compositor_protocol_names_for_output_backend(
                &CompositorPlan::new("oblivion-one-test"),
                backend,
            );

            assert!(!protocols.contains(&"wp_color_manager_v1"));
        }
    }

    #[test]
    fn parse_portal_check_mode() {
        let mode = parse_args(vec!["portal".to_string(), "--check".to_string()]).unwrap();

        assert_eq!(
            mode,
            Mode::Portal(PortalCliOptions {
                check_only: true,
                install_only: false,
            })
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
                nested_output: nested_output::NestedOutputConfig::default(),
                nested_output_explicit: false,
                app: vec![
                    "kitty".to_string(),
                    "--class".to_string(),
                    "OblivionTest".to_string(),
                ],
            })
        );
    }
}
