use super::*;

pub(crate) fn launch_native_shell_command(
    server: &OwnCompositorServer,
    command: Vec<String>,
    app_gpu_policy: EffectiveCompositorAppGpuPolicy,
    source: NativeLaunchSource,
) -> NativeResult<Option<NativeAppLaunchPerf>> {
    let Some(request) = native_launch_request(command, app_gpu_policy, source) else {
        return Ok(None);
    };
    let socket_name = server.socket_name().to_string();
    let spawn_start = Instant::now();
    let spawn_result =
        spawn_compositor_app_with_policy(&socket_name, &request.argv, request.gpu_policy);
    match spawn_result {
        Ok(Some(pid)) => {
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
        Ok(None) => Ok(None),
        Err(error) => Err(io::Error::other(format!(
            "failed to spawn `{}` from native {} with app policy {}: {error}",
            request.program,
            request.source.as_str(),
            request.gpu_policy.as_str()
        ))
        .into()),
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
