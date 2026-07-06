use super::*;

const ECLIPSE_ROOT: &str = "/home/agony/GitHub/Eclipse";

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

fn native_process_options(request: &NativeLaunchRequest) -> ProcessOptions {
    match request.source {
        NativeLaunchSource::ExternalShell => ProcessOptions::new(ProcessKind::ShellSessionCritical)
            .with_restart_policy(RestartPolicy::CriticalSessionComponent)
            .with_label(request.program.clone()),
        NativeLaunchSource::Startup => ProcessOptions::new(ProcessKind::Application)
            .session_owned(false)
            .with_label(request.program.clone()),
        NativeLaunchSource::Spotlight
        | NativeLaunchSource::AltTab
        | NativeLaunchSource::Binding => {
            ProcessOptions::new(ProcessKind::SessionService).with_label(request.program.clone())
        }
    }
}

pub(crate) fn external_shell_command() -> Option<Vec<String>> {
    command_from_env("OBLIVION_ONE_SHELL_COMMAND")
}

pub(crate) fn external_spotlight_command() -> Option<Vec<String>> {
    command_from_env("OBLIVION_ONE_SPOTLIGHT_COMMAND")
        .or_else(|| eclipse_command("Spotlight", "astrea-spotlight", &["--toggle"]))
        .or_else(|| path_command("astrea-spotlight", &["--toggle"]))
}

pub(crate) fn external_alt_tab_command() -> Option<Vec<String>> {
    command_from_env("OBLIVION_ONE_ALT_TAB_COMMAND")
        .or_else(|| eclipse_command("AltTab", "astrea-alt-tab", &["--next"]))
        .or_else(|| path_command("astrea-alt-tab", &["--next"]))
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

fn eclipse_command(project: &str, binary: &str, args: &[&str]) -> Option<Vec<String>> {
    let candidates = [
        Path::new(ECLIPSE_ROOT)
            .join(project)
            .join("build")
            .join(binary),
        Path::new(ECLIPSE_ROOT).join("build").join(binary),
        Path::new(ECLIPSE_ROOT)
            .join("build")
            .join(project)
            .join(binary),
    ];
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
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
    std::env::split_paths(&path_var).any(|dir| dir.join(program).is_file())
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
