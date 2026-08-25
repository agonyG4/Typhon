use super::*;

use std::{fs, path::PathBuf, time::Duration};

#[test]
fn binding_application_launch_receives_current_xwayland_environment() {
    let output = std::env::temp_dir().join(format!(
        "typhon-binding-launch-environment-{}",
        std::process::id()
    ));
    let _ = fs::remove_file(&output);
    let mut server = OwnCompositorServer::bind(format!(
        "typhon-binding-launch-environment-{}",
        std::process::id()
    ))
    .unwrap();
    let mut process_supervisor = ChildSupervisor::new();
    let mut resize_perf = NativeResizePerfState::default();
    let xauthority = PathBuf::from("/run/user/1000/typhon authority/current auth");
    let xwayland = oblivion_one::xwayland::XwaylandAppEnvironment {
        display: ":43".to_string(),
        xauthority: xauthority.clone(),
    };

    let application = apply_native_input_effect(
        NativeInputEffect {
            launch_command: Some(vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "printf '%s\\n' \"$WAYLAND_DISPLAY\" \"$DISPLAY\" \"$XAUTHORITY\" \"$OBLIVION_ONE_XWAYLAND_DISPLAY\" > \"$1\"".to_string(),
                "sh".to_string(),
                output.to_string_lossy().into_owned(),
            ]),
            launch_source: Some(NativeLaunchSource::BindingApplication),
            ..NativeInputEffect::default()
        },
        NativeInputApplyContext {
            server: &mut server,
            perf: NativePerfLogger::from_env(),
            resize_perf: &mut resize_perf,
            cursor_mode: NativeCursorRenderMode::Software,
            app_gpu_policy: EffectiveCompositorAppGpuPolicy::Accelerated,
            seat_session: None,
            process_supervisor: &mut process_supervisor,
            xwayland: Some(&xwayland),
        },
    )
    .unwrap();

    assert_eq!(
        application.launch.expect("binding launch").source,
        NativeLaunchSource::BindingApplication
    );
    wait_for_no_active_children(&mut process_supervisor);
    let observed = fs::read_to_string(&output).expect("binding child environment");
    assert_eq!(
        observed.lines().collect::<Vec<_>>(),
        vec![
            server.socket_name(),
            ":43",
            xauthority.to_str().unwrap(),
            ":43",
        ]
    );
    fs::remove_file(output).unwrap();
}

fn wait_for_no_active_children(process_supervisor: &mut ChildSupervisor) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while process_supervisor.active_count() > 0 && std::time::Instant::now() < deadline {
        process_supervisor.reap_exited().unwrap();
        if process_supervisor.active_count() > 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
