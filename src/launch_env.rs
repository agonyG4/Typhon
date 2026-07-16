use std::{
    collections::HashMap,
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    default_state_dir,
    portal::{PortalRuntime, prepend_data_dir},
    xwayland::XwaylandAppEnvironment,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum X11Bridge {
    Disabled,
    IsolatedXWayland {
        display: String,
        xauthority: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositorAppEnvironment {
    pub wayland_display: String,
    pub x11_bridge: X11Bridge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositorAppGpuPreference {
    Auto,
    Accelerated,
    CpuOnly,
}

impl CompositorAppGpuPreference {
    pub fn from_native_env() -> Self {
        Self::from_native_env_value(env::var("OBLIVION_ONE_NATIVE_APP_GPU").ok().as_deref())
    }

    pub fn from_native_env_value(value: Option<&str>) -> Self {
        value.map(Self::parse).unwrap_or(Self::Auto)
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "cpu" | "software" | "sw" | "0" | "false" => Self::CpuOnly,
            "gpu" | "accelerated" | "1" | "true" => Self::Accelerated,
            "auto" => Self::Auto,
            _ => Self::Auto,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Accelerated => "accelerated",
            Self::CpuOnly => "cpu",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveCompositorAppGpuPolicy {
    Accelerated,
    CpuOnly,
}

impl EffectiveCompositorAppGpuPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accelerated => "accelerated",
            Self::CpuOnly => "cpu",
        }
    }

    pub const fn is_accelerated(self) -> bool {
        match self {
            Self::Accelerated => true,
            Self::CpuOnly => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppDbusPolicy {
    Session,
    DiagnosticPrivate,
}

impl AppDbusPolicy {
    pub fn from_env() -> Self {
        match env::var("OBLIVION_ONE_APP_DBUS_POLICY").ok().as_deref() {
            Some("diagnostic-private") => Self::DiagnosticPrivate,
            Some("session") => Self::Session,
            _ => Self::Session,
        }
    }

    pub const fn private_dbus(self, dbus_run_session_available: bool) -> bool {
        matches!(self, Self::DiagnosticPrivate) && dbus_run_session_available
    }
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
                xauthority: PathBuf::new(),
            },
        }
    }

    pub fn with_isolated_xwayland_and_auth(
        wayland_display: impl Into<String>,
        xwayland_display: impl Into<String>,
        xauthority: impl Into<PathBuf>,
    ) -> Self {
        Self {
            wayland_display: wayland_display.into(),
            x11_bridge: X11Bridge::IsolatedXWayland {
                display: xwayland_display.into(),
                xauthority: xauthority.into(),
            },
        }
    }
}

pub fn configure_compositor_app_command(command: &mut Command, socket_name: &str) {
    let environment = CompositorAppEnvironment::wayland_only(socket_name);
    configure_compositor_app_command_with_environment_and_policy(
        command,
        &environment,
        EffectiveCompositorAppGpuPolicy::Accelerated,
    );
}

pub fn configure_cpu_compositor_app_command(command: &mut Command, socket_name: &str) {
    let environment = CompositorAppEnvironment::wayland_only(socket_name);
    configure_compositor_app_command_with_environment_and_policy(
        command,
        &environment,
        EffectiveCompositorAppGpuPolicy::CpuOnly,
    );
}

pub fn configure_compositor_app_command_with_environment(
    command: &mut Command,
    environment: &CompositorAppEnvironment,
) {
    configure_compositor_app_command_with_environment_and_policy(
        command,
        environment,
        EffectiveCompositorAppGpuPolicy::Accelerated,
    );
}

/// Diagnostic-only launch routing for an explicitly armed XWayland service.
/// Normal application launch continues to use the Wayland-only helpers above.
pub fn configure_compositor_app_command_with_xwayland_environment(
    command: &mut Command,
    socket_name: &str,
    xwayland: &XwaylandAppEnvironment,
) {
    let environment = CompositorAppEnvironment::with_isolated_xwayland_and_auth(
        socket_name,
        &xwayland.display,
        &xwayland.xauthority,
    );
    configure_compositor_app_command_with_environment(command, &environment);
}

fn configure_compositor_app_command_with_environment_and_policy(
    command: &mut Command,
    environment: &CompositorAppEnvironment,
    gpu_policy: EffectiveCompositorAppGpuPolicy,
) {
    remove_host_desktop_activation_environment(command);
    apply_compositor_app_runtime_guards(command);
    command.env("WAYLAND_DISPLAY", &environment.wayland_display);
    command.env("XDG_CURRENT_DESKTOP", "Astrea");
    command.env("XDG_SESSION_DESKTOP", "Astrea");
    command.env("XDG_SESSION_TYPE", "wayland");
    command.env("DESKTOP_SESSION", "Astrea");
    command.env("ASTREA_COMPOSITOR", "TYPHON");
    command.env(
        "ASTREA_SHORTCUT_BRIDGE",
        installed_tool_path("astrea-shortcut-bridge"),
    );
    command.env(
        "ASTREA_SHELL_CONTROL_BRIDGE",
        installed_tool_path("astrea-shell-control-bridge"),
    );
    command.env("ELECTRON_OZONE_PLATFORM_HINT", "wayland");
    command.env("OBLIVION_ONE_DE", "1");
    if gpu_policy == EffectiveCompositorAppGpuPolicy::CpuOnly {
        apply_cpu_composition_app_runtime_guards(command);
    }

    match &environment.x11_bridge {
        X11Bridge::Disabled => {
            command.env_remove("DISPLAY");
            command.env_remove("XAUTHORITY");
            command.env_remove("OBLIVION_ONE_XWAYLAND_DISPLAY");
            command.env("GDK_BACKEND", "wayland");
            command.env("QT_QPA_PLATFORM", "wayland");
            command.env("SDL_VIDEODRIVER", "wayland");
            command.env("CLUTTER_BACKEND", "wayland");
        }
        X11Bridge::IsolatedXWayland {
            display,
            xauthority,
        } => {
            command.env("DISPLAY", display);
            if xauthority.as_os_str().is_empty() {
                command.env_remove("XAUTHORITY");
            } else {
                command.env("XAUTHORITY", xauthority);
            }
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
    compositor_app_spawn_argv_with_policy(
        app,
        private_dbus,
        EffectiveCompositorAppGpuPolicy::Accelerated,
    )
}

pub fn compositor_cpu_app_spawn_argv(app: &[String], private_dbus: bool) -> Option<Vec<String>> {
    compositor_app_spawn_argv_with_policy(
        app,
        private_dbus,
        EffectiveCompositorAppGpuPolicy::CpuOnly,
    )
}

fn compositor_app_spawn_argv_with_policy(
    app: &[String],
    private_dbus: bool,
    gpu_policy: EffectiveCompositorAppGpuPolicy,
) -> Option<Vec<String>> {
    let app = strip_desktop_field_code_args(app);
    if app.is_empty() {
        return None;
    }
    let _ = gpu_policy;
    let app = app.to_vec();
    Some(wrap_private_dbus_session(app, private_dbus))
}

pub fn compositor_app_command_with_policy(
    socket_name: &str,
    app: &[String],
    gpu_policy: EffectiveCompositorAppGpuPolicy,
) -> io::Result<Option<Command>> {
    let Some(argv) = compositor_app_spawn_argv_for_policy(app, gpu_policy) else {
        return Ok(None);
    };
    install_oblivion_portal_runtime()?;
    Ok(compositor_app_command_from_argv(
        socket_name,
        &argv,
        gpu_policy,
    ))
}

pub fn compositor_app_spawn_argv_for_policy(
    app: &[String],
    gpu_policy: EffectiveCompositorAppGpuPolicy,
) -> Option<Vec<String>> {
    let private_dbus =
        AppDbusPolicy::from_env().private_dbus(command_available("dbus-run-session"));
    compositor_app_spawn_argv_with_policy(app, private_dbus, gpu_policy)
}

pub fn compositor_app_command_from_argv(
    socket_name: &str,
    argv: &[String],
    gpu_policy: EffectiveCompositorAppGpuPolicy,
) -> Option<Command> {
    let (program, args) = argv.split_first()?;
    let mut command = Command::new(program);
    command.args(args);
    match gpu_policy {
        EffectiveCompositorAppGpuPolicy::Accelerated => {
            configure_compositor_app_command(&mut command, socket_name);
        }
        EffectiveCompositorAppGpuPolicy::CpuOnly => {
            configure_cpu_compositor_app_command(&mut command, socket_name);
        }
    }
    Some(command)
}

fn remove_host_desktop_activation_environment(command: &mut Command) {
    for key in [
        "WAYLAND_SOCKET",
        "DISPLAY",
        "XAUTHORITY",
        "OBLIVION_ONE_XWAYLAND_DISPLAY",
        "DESKTOP_STARTUP_ID",
        "XDG_ACTIVATION_TOKEN",
        "GIO_LAUNCHED_DESKTOP_FILE",
        "GIO_LAUNCHED_DESKTOP_FILE_PID",
        "HYPRLAND_INSTANCE_SIGNATURE",
        "SWAYSOCK",
        "I3SOCK",
        "AT_SPI_BUS_ADDRESS",
        "GTK_MODULES",
    ] {
        command.env_remove(key);
    }
}

fn apply_compositor_app_runtime_guards(command: &mut Command) {
    let runtime = current_portal_runtime();
    command.env(
        "XDG_DESKTOP_PORTAL_DIR",
        runtime.portal_dir().to_string_lossy().into_owned(),
    );
    command.env(
        "XDG_DATA_DIRS",
        prepend_data_dir(
            &runtime.data_dir(),
            env::var("XDG_DATA_DIRS").ok().as_deref(),
        ),
    );
    command.env("GTK_A11Y", "none");
    command.env("NO_AT_BRIDGE", "1");
    command.env("GIO_USE_VFS", "local");
    command.env("GVFS_DISABLE_FUSE", "1");
    command.env("DISABLE_LSFG", "1");
}

fn current_portal_runtime() -> PortalRuntime {
    PortalRuntime::for_current_process(default_state_dir())
        .unwrap_or_else(|_| PortalRuntime::new(default_state_dir(), PathBuf::from("oblivion-one")))
}

fn install_oblivion_portal_runtime() -> io::Result<()> {
    current_portal_runtime().install()
}

fn apply_cpu_composition_app_runtime_guards(command: &mut Command) {
    command.env("OBLIVION_ONE_CPU_COMPOSITION", "1");
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

fn command_available(program: &str) -> bool {
    let Some(path_var) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&path_var).any(|dir| dir.join(program).is_file())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopLaunch {
    pub argv: Vec<String>,
    pub working_dir: Option<PathBuf>,
}

pub fn desktop_launch_for_id(desktop_id: &str) -> Result<DesktopLaunch, String> {
    validate_desktop_id(desktop_id)?;
    let path = find_desktop_entry(desktop_id)
        .ok_or_else(|| format!("desktop entry not found: {desktop_id}"))?;
    parse_desktop_entry_file(&path)
}

pub fn validate_desktop_id(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("desktop id is empty".to_string());
    }
    if value.len() > 1024 {
        return Err("desktop id is too large".to_string());
    }
    if value.bytes().any(|byte| byte == 0) {
        return Err("desktop id contains NUL".to_string());
    }
    if value.contains('/') || value.contains('\\') || value.contains("..") {
        return Err("desktop id must not be a path".to_string());
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        return Err("desktop id contains unsupported characters".to_string());
    }
    Ok(())
}

fn find_desktop_entry(desktop_id: &str) -> Option<PathBuf> {
    desktop_search_dirs()
        .into_iter()
        .map(|dir| dir.join(desktop_id))
        .find(|path| path.is_file())
}

fn desktop_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(value) = env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        dirs.push(PathBuf::from(value).join("applications"));
    }
    if let Some(value) = env::var_os("XDG_DATA_DIRS").filter(|value| !value.is_empty()) {
        dirs.extend(env::split_paths(&value).map(|dir| dir.join("applications")));
    }
    if let Some(home) = env::var_os("HOME").filter(|value| !value.is_empty()) {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }
    dirs.push(PathBuf::from("/usr/local/share/applications"));
    dirs.push(PathBuf::from("/usr/share/applications"));
    dirs
}

fn parse_desktop_entry_file(path: &Path) -> Result<DesktopLaunch, String> {
    let contents = fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    parse_desktop_entry(&contents, path.parent())
}

pub fn parse_desktop_entry(
    contents: &str,
    base_dir: Option<&Path>,
) -> Result<DesktopLaunch, String> {
    let mut in_entry = false;
    let mut values = HashMap::new();
    for raw in contents.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        values.entry(key.to_string()).or_insert(value.to_string());
    }
    if values.get("Type").map(String::as_str) != Some("Application") {
        return Err("desktop entry is not Type=Application".to_string());
    }
    if values
        .get("Hidden")
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
    {
        return Err("desktop entry is Hidden=true".to_string());
    }
    if let Some(try_exec) = values.get("TryExec")
        && !program_available(try_exec)
    {
        return Err(format!("TryExec is not available: {try_exec}"));
    }
    let exec = values
        .get("Exec")
        .ok_or_else(|| "desktop entry has no Exec".to_string())?;
    let argv = sanitize_desktop_exec_argv(parse_exec_argv(exec)?);
    if argv.is_empty() {
        return Err("desktop Exec produced no argv".to_string());
    }
    if argv.len() > 256 || argv.iter().map(String::len).sum::<usize>() > 64 * 1024 {
        return Err("desktop Exec argv is too large".to_string());
    }
    let working_dir = values.get("Path").and_then(|value| {
        let path = PathBuf::from(value);
        let path = if path.is_absolute() {
            path
        } else {
            base_dir.map(|base| base.join(&path)).unwrap_or(path)
        };
        path.is_dir().then_some(path)
    });
    Ok(DesktopLaunch { argv, working_dir })
}

fn parse_exec_argv(exec: &str) -> Result<Vec<String>, String> {
    let mut argv = Vec::new();
    let mut current = String::new();
    let mut chars = exec.chars();
    let mut quote = false;
    while let Some(ch) = chars.next() {
        match ch {
            '"' => quote = !quote,
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            ch if ch.is_whitespace() && !quote => {
                if !current.is_empty() {
                    argv.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if quote {
        return Err("unterminated quote in Exec".to_string());
    }
    if !current.is_empty() {
        argv.push(current);
    }
    Ok(argv)
}

fn sanitize_desktop_exec_argv(argv: Vec<String>) -> Vec<String> {
    argv.into_iter()
        .filter_map(|arg| sanitize_desktop_exec_arg(&arg))
        .filter(|arg| !arg.is_empty())
        .collect()
}

fn sanitize_desktop_exec_arg(arg: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = arg.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('f' | 'F' | 'u' | 'U' | 'i' | 'c' | 'k' | 'v' | 'm') => {
                if arg.len() == 2 {
                    return None;
                }
            }
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    Some(out)
}

fn program_available(program: &str) -> bool {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.is_file();
    }
    command_available(program)
}

fn installed_tool_path(binary: &str) -> String {
    if let Ok(current_exe) = env::current_exe()
        && let Some(dir) = current_exe.parent()
    {
        let beside = dir.join(binary);
        if beside.is_file() {
            return beside.to_string_lossy().into_owned();
        }
        if let Some(prefix) = dir.parent() {
            let libexec = prefix.join("libexec").join("typhon").join(binary);
            if libexec.is_file() {
                return libexec.to_string_lossy().into_owned();
            }
        }
    }
    binary.to_string()
}
