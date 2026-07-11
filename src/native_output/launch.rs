use super::*;
use oblivion_one::astrea_shell_control::server::astrea_launch_request_v1;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Debug, Default)]
pub(crate) struct AstreaLaunchLifecycleTracker {
    observers: HashMap<u32, astrea_launch_request_v1::AstreaLaunchRequestV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AstreaShortcutFallbackKind {
    Spotlight,
    AltTab,
}

impl AstreaShortcutFallbackKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Spotlight => "spotlight",
            Self::AltTab => "alt_tab",
        }
    }

    pub(crate) fn command(self) -> Option<Vec<String>> {
        match self {
            Self::Spotlight => external_spotlight_command(),
            Self::AltTab => external_alt_tab_command(),
        }
    }

    pub(crate) const fn source(self) -> NativeLaunchSource {
        match self {
            Self::Spotlight => NativeLaunchSource::Spotlight,
            Self::AltTab => NativeLaunchSource::AltTab,
        }
    }
}

pub(crate) fn astrea_shortcut_fallback_kind(
    shortcut: &AstreaShortcutEvent,
    protocol_clients: usize,
) -> Option<AstreaShortcutFallbackKind> {
    if protocol_clients > 0 || shortcut.phase != AstreaShortcutPhase::Pressed {
        return None;
    }
    match (shortcut.namespace.as_str(), shortcut.name.as_str()) {
        ("astrea-shell", "spotlight_toggle") => Some(AstreaShortcutFallbackKind::Spotlight),
        ("astrea-shell", "alt_tab_next") => Some(AstreaShortcutFallbackKind::AltTab),
        _ => None,
    }
}

impl AstreaLaunchLifecycleTracker {
    pub(crate) fn track(
        &mut self,
        pid: u32,
        request: astrea_launch_request_v1::AstreaLaunchRequestV1,
    ) {
        self.observers.insert(pid, request);
    }

    pub(crate) fn complete(&mut self, pid: u32, status: std::process::ExitStatus) -> bool {
        let Some(request) = self.observers.remove(&pid) else {
            return false;
        };
        if request.is_alive() {
            request.finished(astrea_launch_finished_status(status));
        }
        true
    }

    pub(crate) fn prune_dead(&mut self) {
        self.observers.retain(|_, request| request.is_alive());
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.observers.len()
    }
}

pub(crate) fn astrea_launch_finished_status(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return -signal;
    }
    -255
}

pub(crate) fn launch_native_shell_command(
    server: &OwnCompositorServer,
    supervisor: &mut ChildSupervisor,
    command: Vec<String>,
    app_gpu_policy: EffectiveCompositorAppGpuPolicy,
    source: NativeLaunchSource,
) -> NativeResult<Option<NativeAppLaunchPerf>> {
    let Some(request) = native_launch_request(command, app_gpu_policy, source) else {
        return Ok(None);
    };
    let socket_name = server.socket_name().to_string();
    let spawn_start = Instant::now();
    let process_options = native_process_options(&request);
    let spawn_result = match request.source {
        NativeLaunchSource::ExternalShell => {
            let socket_name = socket_name.clone();
            let argv = request.argv.clone();
            let gpu_policy = request.gpu_policy;
            supervisor.spawn_restartable(
                move || {
                    compositor_app_command_with_policy(&socket_name, &argv, gpu_policy)?
                        .ok_or_else(|| io::Error::other("native shell command is empty"))
                },
                process_options,
            )
        }
        _ => match compositor_app_command_with_policy(
            &socket_name,
            &request.argv,
            request.gpu_policy,
        )? {
            Some(command) => supervisor.spawn(command, process_options),
            None => return Ok(None),
        },
    };
    match spawn_result {
        Ok(pid) => {
            let spawn_us = elapsed_micros(spawn_start);
            println!(
                "spawned `{}` from native {} on Oblivion Wayland socket `{socket_name}` as pid {pid} (gpu policy {})",
                request.program,
                request.source.as_str(),
                request.gpu_policy.as_str()
            );
            Ok(Some(NativeAppLaunchPerf {
                program: request.program,
                command: request.command,
                pid,
                spawn_us,
                started_at: spawn_start,
                gpu_policy: request.gpu_policy,
                source: request.source,
            }))
        }
        Err(error) => Err(io::Error::other(format!(
            "failed to spawn `{}` from native {} with app policy {}: {error}",
            request.program,
            request.source.as_str(),
            request.gpu_policy.as_str()
        ))
        .into()),
    }
}

pub(crate) fn drain_pending_process_launches(
    server: &mut OwnCompositorServer,
    supervisor: &mut ChildSupervisor,
    launch_tracker: &mut AstreaLaunchLifecycleTracker,
    app_gpu_policy: EffectiveCompositorAppGpuPolicy,
    perf: NativePerfLogger,
    pending_launches: &mut VecDeque<NativeAppLaunchPerf>,
) {
    let socket_name = server.socket_name().to_string();
    for pending in server.take_pending_process_launches() {
        if !pending.request.is_alive() {
            continue;
        }
        let Some(request) = native_launch_request(
            pending.argv,
            app_gpu_policy,
            NativeLaunchSource::ShellControl,
        ) else {
            pending.request.failed(5, "empty command".to_string());
            continue;
        };
        let spawn_start = Instant::now();
        let process_options = native_process_options(&request);
        let command = match compositor_app_command_with_policy(
            &socket_name,
            &request.argv,
            request.gpu_policy,
        ) {
            Ok(Some(command)) => command,
            Ok(None) => {
                pending.request.failed(5, "empty command".to_string());
                continue;
            }
            Err(error) => {
                pending
                    .request
                    .failed(5, format!("spawn preparation failed: {error}"));
                continue;
            }
        };
        match supervisor.spawn(command, process_options) {
            Ok(pid) => {
                launch_tracker.track(pid, pending.request.clone());
                pending.request.accepted(pid);
                let launch = NativeAppLaunchPerf {
                    program: request.program,
                    command: request.command,
                    pid,
                    spawn_us: elapsed_micros(spawn_start),
                    started_at: spawn_start,
                    gpu_policy: request.gpu_policy,
                    source: request.source,
                };
                log_native_app_spawn(perf, &launch);
                pending_launches.push_back(launch);
            }
            Err(error) => {
                pending.request.failed(5, format!("spawn failed: {error}"));
            }
        }
    }
}

fn native_process_options(request: &NativeLaunchRequest) -> ProcessOptions {
    match request.source {
        NativeLaunchSource::ExternalShell => ProcessOptions::new(ProcessKind::ShellSessionCritical)
            .with_restart_policy(RestartPolicy::CriticalSessionComponent)
            .with_label(request.program.clone()),
        NativeLaunchSource::Startup => ProcessOptions::new(ProcessKind::Application)
            .session_owned(false)
            .with_label(request.program.clone()),
        NativeLaunchSource::BindingApplication | NativeLaunchSource::ShellControl => {
            ProcessOptions::new(ProcessKind::Application)
                .session_owned(false)
                .with_label(request.program.clone())
        }
        NativeLaunchSource::Spotlight
        | NativeLaunchSource::AltTab
        | NativeLaunchSource::BindingSessionCommand => {
            ProcessOptions::new(ProcessKind::SessionService).with_label(request.program.clone())
        }
    }
}

pub(crate) fn external_shell_command() -> Option<Vec<String>> {
    command_from_env("OBLIVION_ONE_SHELL_COMMAND")
}

pub(crate) fn external_spotlight_command() -> Option<Vec<String>> {
    resolve_astrea_utility_command(
        "OBLIVION_ONE_SPOTLIGHT_COMMAND",
        "Spotlight",
        "astrea-spotlight",
        &["--toggle"],
    )
}

pub(crate) fn external_alt_tab_command() -> Option<Vec<String>> {
    resolve_astrea_utility_command(
        "OBLIVION_ONE_ALT_TAB_COMMAND",
        "AltTab",
        "astrea-alt-tab",
        &["--next"],
    )
}

pub(crate) fn resolve_astrea_utility_command(
    explicit_env: &str,
    project: &str,
    binary: &str,
    args: &[&str],
) -> Option<Vec<String>> {
    command_from_env(explicit_env)
        .or_else(|| {
            let root = std::env::var_os("ASTREA_ECLIPSE_ROOT")?;
            (!root.is_empty()).then(|| eclipse_command(Path::new(&root), project, binary, args))?
        })
        .or_else(|| path_command(binary, args))
}

pub(crate) fn external_session_switch_command(index: u8) -> Option<Vec<String>> {
    let name = match index {
        1 => "OBLIVION_ONE_SESSION_1_COMMAND",
        2 => "OBLIVION_ONE_SESSION_2_COMMAND",
        3 => "OBLIVION_ONE_SESSION_3_COMMAND",
        _ => return None,
    };
    command_from_env(name)
}

fn command_from_env(name: &str) -> Option<Vec<String>> {
    let command = std::env::var(name).ok()?;
    let command = command.trim();
    (!command.is_empty()).then(|| vec!["sh".to_string(), "-lc".to_string(), command.to_string()])
}

fn eclipse_command(root: &Path, project: &str, binary: &str, args: &[&str]) -> Option<Vec<String>> {
    let candidates = [
        root.join(project).join("build").join(binary),
        root.join("build").join(binary),
        root.join("build").join(project).join(binary),
    ];
    candidates
        .into_iter()
        .find(|candidate| executable_file(candidate))
        .map(|path| command_with_args(path.display().to_string(), args))
}

fn path_command(binary: &str, args: &[&str]) -> Option<Vec<String>> {
    command_available(binary).then(|| command_with_args(binary.to_string(), args))
}

fn command_with_args(program: String, args: &[&str]) -> Vec<String> {
    std::iter::once(program)
        .chain(args.iter().map(|arg| (*arg).to_string()))
        .collect()
}

fn command_available(program: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| executable_file(&dir.join(program)))
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub(crate) fn native_launch_request(
    command: Vec<String>,
    gpu_policy: EffectiveCompositorAppGpuPolicy,
    source: NativeLaunchSource,
) -> Option<NativeLaunchRequest> {
    let program = command.first()?.clone();
    let command_label = command
        .iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    Some(NativeLaunchRequest {
        argv: command,
        program,
        command: command_label,
        gpu_policy,
        source,
    })
}

pub(crate) fn log_native_app_spawn(perf: NativePerfLogger, launch: &NativeAppLaunchPerf) {
    perf.log("app.spawn", || {
        vec![
            NativePerfField::str("program", launch.program.clone()),
            NativePerfField::str("command", launch.command.clone()),
            NativePerfField::str("source", launch.source.as_str()),
            NativePerfField::u64("pid", u64::from(launch.pid)),
            NativePerfField::u64("spawn_us", launch.spawn_us),
            NativePerfField::str("app_policy", launch.gpu_policy.as_str()),
        ]
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::ExitStatus};

    fn request_for_source(source: NativeLaunchSource) -> NativeLaunchRequest {
        native_launch_request(
            vec!["app".to_string()],
            EffectiveCompositorAppGpuPolicy::CpuOnly,
            source,
        )
        .expect("launch request")
    }

    #[test]
    fn binding_application_launches_are_not_session_owned() {
        let options =
            native_process_options(&request_for_source(NativeLaunchSource::BindingApplication));

        assert_eq!(options.kind, ProcessKind::Application);
        assert!(!options.session_owned);
    }

    #[test]
    fn binding_session_commands_remain_session_owned() {
        let options = native_process_options(&request_for_source(
            NativeLaunchSource::BindingSessionCommand,
        ));

        assert_eq!(options.kind, ProcessKind::SessionService);
        assert!(options.session_owned);
    }

    #[test]
    fn shell_control_launches_are_supervised_applications() {
        let options = native_process_options(&request_for_source(NativeLaunchSource::ShellControl));

        assert_eq!(options.kind, ProcessKind::Application);
        assert!(!options.session_owned);
    }

    #[test]
    fn astrea_fallback_is_protocol_first_and_pressed_only() {
        let pressed = AstreaShortcutEvent {
            namespace: "astrea-shell".to_string(),
            name: "spotlight_toggle".to_string(),
            phase: AstreaShortcutPhase::Pressed,
        };
        assert_eq!(
            astrea_shortcut_fallback_kind(&pressed, 0),
            Some(AstreaShortcutFallbackKind::Spotlight)
        );
        assert_eq!(astrea_shortcut_fallback_kind(&pressed, 1), None);

        for phase in [AstreaShortcutPhase::Repeated, AstreaShortcutPhase::Released] {
            let event = AstreaShortcutEvent {
                phase,
                ..pressed.clone()
            };
            assert_eq!(astrea_shortcut_fallback_kind(&event, 0), None);
        }

        for name in ["alt_tab_previous", "alt_tab_commit", "unknown"] {
            let event = AstreaShortcutEvent {
                namespace: "astrea-shell".to_string(),
                name: name.to_string(),
                phase: AstreaShortcutPhase::Pressed,
            };
            assert_eq!(astrea_shortcut_fallback_kind(&event, 0), None);
        }
    }

    #[test]
    fn astrea_finished_status_encodes_exit_code_and_signal() {
        use std::os::unix::process::ExitStatusExt;

        assert_eq!(
            astrea_launch_finished_status(ExitStatus::from_raw(7 << 8)),
            7
        );
        assert_eq!(
            astrea_launch_finished_status(ExitStatus::from_raw(libc::SIGTERM)),
            -libc::SIGTERM
        );
    }

    #[test]
    fn astrea_utility_explicit_override_wins_over_development_root() {
        let _guard = ASTREA_ENV_LOCK.lock().unwrap();
        let root = temporary_utility_root("override");
        let candidate = root
            .join("Spotlight")
            .join("build")
            .join("astrea-spotlight");
        fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        write_executable(&candidate);
        unsafe {
            std::env::set_var("OBLIVION_ONE_SPOTLIGHT_COMMAND", "custom-spotlight --dev");
            std::env::set_var("ASTREA_ECLIPSE_ROOT", &root);
        }

        let command = resolve_astrea_utility_command(
            "OBLIVION_ONE_SPOTLIGHT_COMMAND",
            "Spotlight",
            "astrea-spotlight",
            &["--toggle"],
        );

        unsafe {
            std::env::remove_var("OBLIVION_ONE_SPOTLIGHT_COMMAND");
            std::env::remove_var("ASTREA_ECLIPSE_ROOT");
        }
        fs::remove_dir_all(root).unwrap();
        assert_eq!(
            command,
            Some(vec![
                "sh".to_string(),
                "-lc".to_string(),
                "custom-spotlight --dev".to_string()
            ])
        );
    }

    #[test]
    fn astrea_utility_development_root_resolves_supported_build_layout() {
        let _guard = ASTREA_ENV_LOCK.lock().unwrap();
        let root = temporary_utility_root("development");
        let previous_path = std::env::var_os("PATH");
        let candidate = root.join("build").join("astrea-alt-tab");
        fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        write_executable(&candidate);
        unsafe {
            std::env::remove_var("OBLIVION_ONE_ALT_TAB_COMMAND");
            std::env::set_var("ASTREA_ECLIPSE_ROOT", &root);
            std::env::set_var("PATH", "/definitely/no/astrea-utilities");
        }

        let command = resolve_astrea_utility_command(
            "OBLIVION_ONE_ALT_TAB_COMMAND",
            "AltTab",
            "astrea-alt-tab",
            &["--next"],
        );

        unsafe {
            std::env::remove_var("ASTREA_ECLIPSE_ROOT");
            restore_env("PATH", previous_path);
        }
        fs::remove_dir_all(root).unwrap();
        assert_eq!(
            command,
            Some(vec![candidate.display().to_string(), "--next".to_string()])
        );
    }

    #[test]
    fn astrea_utility_path_resolution_requires_executable_candidate() {
        let _guard = ASTREA_ENV_LOCK.lock().unwrap();
        let root = temporary_utility_root("path");
        let previous_path = std::env::var_os("PATH");
        let candidate = root.join("astrea-spotlight");
        fs::create_dir_all(&root).unwrap();
        write_executable(&candidate);
        unsafe {
            std::env::remove_var("OBLIVION_ONE_SPOTLIGHT_COMMAND");
            std::env::remove_var("ASTREA_ECLIPSE_ROOT");
            std::env::set_var("PATH", &root);
        }

        let command = resolve_astrea_utility_command(
            "OBLIVION_ONE_SPOTLIGHT_COMMAND",
            "Spotlight",
            "astrea-spotlight",
            &["--toggle"],
        );

        restore_env("PATH", previous_path);
        fs::remove_dir_all(root).unwrap();
        assert_eq!(
            command,
            Some(vec!["astrea-spotlight".to_string(), "--toggle".to_string()])
        );
    }

    #[test]
    fn astrea_utility_resolution_returns_none_when_unavailable() {
        let _guard = ASTREA_ENV_LOCK.lock().unwrap();
        let root = temporary_utility_root("missing");
        let previous_path = std::env::var_os("PATH");
        unsafe {
            std::env::remove_var("OBLIVION_ONE_ALT_TAB_COMMAND");
            std::env::remove_var("ASTREA_ECLIPSE_ROOT");
            std::env::set_var("PATH", &root);
        }

        let command = resolve_astrea_utility_command(
            "OBLIVION_ONE_ALT_TAB_COMMAND",
            "AltTab",
            "astrea-alt-tab",
            &["--next"],
        );

        restore_env("PATH", previous_path);
        assert_eq!(command, None);
    }

    fn restore_env(name: &str, value: Option<std::ffi::OsString>) {
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }

    fn temporary_utility_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "typhon-astrea-utility-{label}-{}",
            std::process::id()
        ))
    }

    fn write_executable(path: &std::path::Path) {
        fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }
}
