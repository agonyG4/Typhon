use std::{
    collections::HashMap,
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{default_state_dir, paths::shell_quote};

const SESSION_ENV_KEYS: &[&str] = &[
    "WAYLAND_DISPLAY",
    "GAMESCOPE_WAYLAND_DISPLAY",
    "DISPLAY",
    "XDG_RUNTIME_DIR",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "XDG_SESSION_TYPE",
    "DESKTOP_SESSION",
    "OBLIVION_ONE_SESSION_ID",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum X11Bridge {
    Disabled,
    IsolatedXWayland { display: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositorAppEnvironment {
    pub wayland_display: String,
    pub x11_bridge: X11Bridge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompositorAppGpuPolicy {
    Accelerated,
    CpuOnly,
}

impl CompositorAppEnvironment {
    pub fn wayland_only(wayland_display: impl Into<String>) -> Self {
        Self {
            wayland_display: wayland_display.into(),
            x11_bridge: X11Bridge::Disabled,
        }
    }

    pub fn with_isolated_xwayland(
        wayland_display: impl Into<String>,
        xwayland_display: impl Into<String>,
    ) -> Self {
        Self {
            wayland_display: wayland_display.into(),
            x11_bridge: X11Bridge::IsolatedXWayland {
                display: xwayland_display.into(),
            },
        }
    }
}

pub fn configure_compositor_app_command(command: &mut Command, socket_name: &str) {
    let environment = CompositorAppEnvironment::wayland_only(socket_name);
    configure_compositor_app_command_with_environment_and_policy(
        command,
        &environment,
        CompositorAppGpuPolicy::Accelerated,
    );
}

pub fn configure_cpu_compositor_app_command(command: &mut Command, socket_name: &str) {
    let environment = CompositorAppEnvironment::wayland_only(socket_name);
    configure_compositor_app_command_with_environment_and_policy(
        command,
        &environment,
        CompositorAppGpuPolicy::CpuOnly,
    );
}

pub fn configure_compositor_app_command_with_environment(
    command: &mut Command,
    environment: &CompositorAppEnvironment,
) {
    configure_compositor_app_command_with_environment_and_policy(
        command,
        environment,
        CompositorAppGpuPolicy::Accelerated,
    );
}

fn configure_compositor_app_command_with_environment_and_policy(
    command: &mut Command,
    environment: &CompositorAppEnvironment,
    gpu_policy: CompositorAppGpuPolicy,
) {
    remove_host_desktop_activation_environment(command);
    apply_nested_app_runtime_guards(command);
    command.env("WAYLAND_DISPLAY", &environment.wayland_display);
    command.env("XDG_CURRENT_DESKTOP", "OblivionOne");
    command.env("XDG_SESSION_DESKTOP", "OblivionOne");
    command.env("XDG_SESSION_TYPE", "wayland");
    command.env("ELECTRON_OZONE_PLATFORM_HINT", "wayland");
    command.env("MOZ_ENABLE_WAYLAND", "1");
    command.env("OBLIVION_ONE_DE", "1");
    if gpu_policy == CompositorAppGpuPolicy::CpuOnly {
        apply_cpu_composition_app_runtime_guards(command);
    }

    match &environment.x11_bridge {
        X11Bridge::Disabled => {
            command.env_remove("DISPLAY");
            command.env_remove("OBLIVION_ONE_XWAYLAND_DISPLAY");
            command.env("GDK_BACKEND", "wayland");
            command.env("QT_QPA_PLATFORM", "wayland");
            command.env("SDL_VIDEODRIVER", "wayland");
            command.env("CLUTTER_BACKEND", "wayland");
        }
        X11Bridge::IsolatedXWayland { display } => {
            command.env("DISPLAY", display);
            command.env("OBLIVION_ONE_XWAYLAND_DISPLAY", display);
            command.env("GDK_BACKEND", "wayland,x11");
            command.env("QT_QPA_PLATFORM", "wayland;xcb");
            command.env("SDL_VIDEODRIVER", "wayland,x11");
            command.env("CLUTTER_BACKEND", "wayland,x11");
        }
    }
}

pub fn compositor_app_args_for(_program: &str, args: &[String]) -> Vec<String> {
    args.to_vec()
}

pub fn compositor_app_spawn_argv(app: &[String], private_dbus: bool) -> Option<Vec<String>> {
    compositor_app_spawn_argv_with_policy(app, private_dbus, CompositorAppGpuPolicy::Accelerated)
}

pub fn compositor_cpu_app_spawn_argv(app: &[String], private_dbus: bool) -> Option<Vec<String>> {
    compositor_app_spawn_argv_with_policy(app, private_dbus, CompositorAppGpuPolicy::CpuOnly)
}

fn compositor_app_spawn_argv_with_policy(
    app: &[String],
    private_dbus: bool,
    gpu_policy: CompositorAppGpuPolicy,
) -> Option<Vec<String>> {
    let app = strip_desktop_field_code_args(app);
    if app.is_empty() {
        return None;
    }
    let app = isolate_single_instance_app_argv(&app, gpu_policy);
    Some(wrap_private_dbus_session(app, private_dbus))
}

pub fn spawn_compositor_app(socket_name: &str, app: &[String]) -> io::Result<Option<u32>> {
    let Some(argv) = compositor_app_spawn_argv(app, command_available("dbus-run-session")) else {
        return Ok(None);
    };
    ensure_compositor_app_profile_dirs(&argv)?;
    let Some((program, args)) = argv.split_first() else {
        return Ok(None);
    };

    let mut child = Command::new(program);
    child.args(args);
    configure_compositor_app_command(&mut child, socket_name);
    let child = child.spawn()?;
    Ok(Some(child.id()))
}

pub fn spawn_cpu_compositor_app(socket_name: &str, app: &[String]) -> io::Result<Option<u32>> {
    let Some(argv) = compositor_cpu_app_spawn_argv(app, command_available("dbus-run-session"))
    else {
        return Ok(None);
    };
    ensure_compositor_app_profile_dirs(&argv)?;
    let Some((program, args)) = argv.split_first() else {
        return Ok(None);
    };

    let mut child = Command::new(program);
    child.args(args);
    configure_cpu_compositor_app_command(&mut child, socket_name);
    let child = child.spawn()?;
    Ok(Some(child.id()))
}

pub fn parse_session_env(contents: &str) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = contents
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(key, _)| SESSION_ENV_KEYS.contains(key))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();

    if !env.contains_key("WAYLAND_DISPLAY")
        && let Some(value) = env.get("GAMESCOPE_WAYLAND_DISPLAY").cloned()
    {
        env.insert("WAYLAND_DISPLAY".to_string(), value);
    }

    env
}

pub fn export_lines(env: &HashMap<String, String>) -> Vec<String> {
    SESSION_ENV_KEYS
        .iter()
        .filter_map(|key| {
            env.get(*key)
                .map(|value| format!("export {key}={}", shell_quote(value)))
        })
        .collect()
}

pub fn app_launch_env(current_env: &HashMap<String, String>) -> HashMap<String, String> {
    app_launch_env_for(current_env, None)
}

pub fn app_launch_env_for(
    current_env: &HashMap<String, String>,
    isolated_app_id: Option<&str>,
) -> HashMap<String, String> {
    let mut env = HashMap::new();

    if let Some(display) = current_env
        .get("GAMESCOPE_WAYLAND_DISPLAY")
        .filter(|display| !display.is_empty())
        .or_else(|| {
            current_env
                .get("WAYLAND_DISPLAY")
                .filter(|display| !display.is_empty())
        })
    {
        env.insert("WAYLAND_DISPLAY".to_string(), display.clone());
    }

    for key in [
        "GAMESCOPE_WAYLAND_DISPLAY",
        "DISPLAY",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
    ] {
        if let Some(value) = current_env.get(key).filter(|value| !value.is_empty()) {
            env.insert(key.to_string(), value.clone());
        }
    }

    env.insert("GDK_BACKEND".to_string(), "wayland".to_string());
    env.insert("QT_QPA_PLATFORM".to_string(), "wayland".to_string());
    env.insert("SDL_VIDEODRIVER".to_string(), "wayland".to_string());
    env.insert("CLUTTER_BACKEND".to_string(), "wayland".to_string());
    env.insert("MOZ_ENABLE_WAYLAND".to_string(), "1".to_string());
    env.insert(
        "ELECTRON_OZONE_PLATFORM_HINT".to_string(),
        "wayland".to_string(),
    );
    env.insert("OBLIVION_ONE_DE".to_string(), "1".to_string());

    if let Some(app_id) = isolated_app_id {
        let app_dir = oblivion_app_state_dir(app_id);
        env.insert(
            "OBLIVION_ONE_APP_DIR".to_string(),
            app_dir.to_string_lossy().into_owned(),
        );
        env.insert(
            "XDG_CONFIG_HOME".to_string(),
            app_dir.join("config").to_string_lossy().into_owned(),
        );
        env.insert(
            "XDG_CACHE_HOME".to_string(),
            app_dir.join("cache").to_string_lossy().into_owned(),
        );
        env.insert(
            "XDG_DATA_HOME".to_string(),
            app_dir.join("data").to_string_lossy().into_owned(),
        );
        env.insert(
            "XDG_STATE_HOME".to_string(),
            app_dir.join("state").to_string_lossy().into_owned(),
        );
    }

    env
}

pub fn oblivion_app_state_dir(app_id: &str) -> std::path::PathBuf {
    default_state_dir().join("apps").join(app_id)
}

fn remove_host_desktop_activation_environment(command: &mut Command) {
    for key in [
        "WAYLAND_SOCKET",
        "DBUS_SESSION_BUS_ADDRESS",
        "DBUS_STARTER_ADDRESS",
        "DBUS_STARTER_BUS_TYPE",
        "DESKTOP_STARTUP_ID",
        "XDG_ACTIVATION_TOKEN",
        "GIO_LAUNCHED_DESKTOP_FILE",
        "GIO_LAUNCHED_DESKTOP_FILE_PID",
        "HYPRLAND_INSTANCE_SIGNATURE",
        "AT_SPI_BUS_ADDRESS",
        "XDG_DESKTOP_PORTAL_DIR",
        "GTK_MODULES",
    ] {
        command.env_remove(key);
    }
}

fn apply_nested_app_runtime_guards(command: &mut Command) {
    command.env("GTK_USE_PORTAL", "0");
    command.env("GIO_USE_PORTALS", "0");
    command.env("QT_NO_USE_PORTAL", "1");
    command.env("GTK_A11Y", "none");
    command.env("NO_AT_BRIDGE", "1");
    command.env("GIO_USE_VFS", "local");
    command.env("GVFS_DISABLE_FUSE", "1");
    command.env("DISABLE_LSFG", "1");
}

fn apply_cpu_composition_app_runtime_guards(command: &mut Command) {
    command.env("OBLIVION_ONE_CPU_COMPOSITION", "1");
    command.env("MOZ_WEBRENDER_SOFTWARE", "1");
    command.env("LIBGL_ALWAYS_SOFTWARE", "1");
    command.env("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    command.env("GSK_RENDERER", "cairo");
}

fn strip_desktop_field_code_args(app: &[String]) -> Vec<String> {
    app.iter()
        .filter(|arg| !is_desktop_field_code_arg(arg))
        .cloned()
        .collect()
}

fn is_desktop_field_code_arg(arg: &str) -> bool {
    matches!(
        arg,
        "%f" | "%F" | "%u" | "%U" | "%i" | "%c" | "%k" | "%v" | "%m"
    )
}

fn isolate_single_instance_app_argv(
    app: &[String],
    gpu_policy: CompositorAppGpuPolicy,
) -> Vec<String> {
    let Some(program) = app.first() else {
        return Vec::new();
    };
    let app_name = executable_name(program);
    if is_gecko_browser(&app_name) {
        return gecko_browser_argv(app, &app_name);
    }
    if is_chromium_browser(&app_name) {
        return chromium_browser_argv(app, &app_name, gpu_policy);
    }
    app.to_vec()
}

fn gecko_browser_argv(app: &[String], app_name: &str) -> Vec<String> {
    let mut argv = Vec::with_capacity(app.len() + 4);
    argv.push(app[0].clone());
    if !app
        .iter()
        .any(|arg| arg == "--no-remote" || arg == "-no-remote")
    {
        argv.push("--no-remote".to_string());
    }
    if !app
        .iter()
        .any(|arg| arg == "--profile" || arg == "-profile")
    {
        argv.push("--profile".to_string());
        argv.push(browser_profile_dir(app_name).to_string_lossy().into_owned());
    }
    argv.extend(app.iter().skip(1).cloned());
    argv
}

fn chromium_browser_argv(
    app: &[String],
    app_name: &str,
    gpu_policy: CompositorAppGpuPolicy,
) -> Vec<String> {
    let mut argv = Vec::with_capacity(app.len() + 11);
    argv.push(app[0].clone());
    if !app.iter().any(|arg| arg.starts_with("--user-data-dir=")) {
        argv.push(format!(
            "--user-data-dir={}",
            browser_profile_dir(app_name).to_string_lossy()
        ));
    }
    let accelerated_args = [
        "--use-gl=egl-angle",
        "--use-angle=opengles",
        "--disable-features=Vulkan",
        "--disable-vulkan",
    ];
    let cpu_args = [
        "--disable-gpu",
        "--disable-gpu-compositing",
        "--disable-gpu-rasterization",
        "--disable-zero-copy",
        "--disable-features=Vulkan,DefaultANGLEVulkan,VizDisplayCompositor",
        "--disable-vulkan",
    ];
    let browser_args = [
        "--ozone-platform=wayland",
        "--enable-features=UseOzonePlatform",
    ]
    .into_iter()
    .chain(match gpu_policy {
        CompositorAppGpuPolicy::Accelerated => accelerated_args.as_slice().iter().copied(),
        CompositorAppGpuPolicy::CpuOnly => cpu_args.as_slice().iter().copied(),
    });
    for arg in browser_args {
        if !app.iter().any(|existing| existing == arg) {
            argv.push(arg.to_string());
        }
    }
    argv.extend(app.iter().skip(1).cloned());
    argv
}

fn wrap_private_dbus_session(app: Vec<String>, private_dbus: bool) -> Vec<String> {
    if !private_dbus {
        return app;
    }
    let mut argv = Vec::with_capacity(app.len() + 2);
    argv.push("dbus-run-session".to_string());
    argv.push("--".to_string());
    argv.extend(app);
    argv
}

fn ensure_compositor_app_profile_dirs(argv: &[String]) -> io::Result<()> {
    let mut dirs = Vec::new();
    for (index, arg) in argv.iter().enumerate() {
        if (arg == "--profile" || arg == "-profile")
            && let Some(path) = argv.get(index + 1)
        {
            dirs.push(PathBuf::from(path));
        }
        if let Some(path) = arg.strip_prefix("--user-data-dir=") {
            dirs.push(PathBuf::from(path));
        }
    }
    for dir in dirs {
        fs::create_dir_all(dir)?;
    }
    Ok(())
}

fn browser_profile_dir(app_name: &str) -> PathBuf {
    default_state_dir()
        .join("app-profiles")
        .join(sanitize_app_id(app_name))
}

fn executable_name(program: &str) -> String {
    Path::new(program)
        .file_stem()
        .or_else(|| Path::new(program).file_name())
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| program.to_ascii_lowercase())
}

fn sanitize_app_id(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "app".to_string()
    } else {
        sanitized
    }
}

fn is_gecko_browser(app_name: &str) -> bool {
    matches!(
        app_name,
        "firefox"
            | "firefox-bin"
            | "zen"
            | "zen-bin"
            | "librewolf"
            | "librewolf-bin"
            | "floorp"
            | "waterfox"
            | "thunderbird"
    )
}

fn is_chromium_browser(app_name: &str) -> bool {
    matches!(
        app_name,
        "brave"
            | "brave-browser"
            | "brave-bin"
            | "chromium"
            | "chromium-browser"
            | "google-chrome"
            | "google-chrome-stable"
            | "chrome"
            | "vivaldi"
            | "vivaldi-stable"
            | "microsoft-edge"
            | "microsoft-edge-stable"
    )
}

fn command_available(program: &str) -> bool {
    let Some(path_var) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&path_var).any(|dir| dir.join(program).is_file())
}
