pub mod astrea_shortcuts;
pub mod compositor;
pub mod core;
mod defaults;
mod launch_env;
mod launch_plan;
pub mod native;
mod options;
mod paths;
pub mod portal;
mod prototype_scene;
pub mod render_backend;
pub mod session;
pub mod shell;
pub mod syncobj;
pub mod wayland_drm;
pub mod wm;
pub mod xwayland;

pub use defaults::*;
pub use launch_env::*;
pub use launch_plan::{HyprlandLaunchPlan, NestedLaunchPlan};
pub use options::*;
pub use paths::*;
pub use prototype_scene::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch_plan::ENV_WRITER_SCRIPT;
    use std::{collections::HashMap, path::PathBuf, process::Command};

    #[test]
    fn oblivion_compositor_plan_has_no_external_compositor_dependency() {
        let plan = compositor::CompositorPlan::new("oblivion-one-0");

        assert_eq!(plan.socket_name, "oblivion-one-0");
        assert!(!plan.uses_external_compositor());
        assert!(!plan.command_preview().contains("Hyprland"));
        assert!(!plan.command_preview().contains("gamescope"));
    }

    #[test]
    fn architecture_layers_keep_compositor_wm_and_shell_separate() {
        let architecture = compositor::CompositorArchitecture::default();

        assert_eq!(
            architecture.layer_names(),
            vec!["core", "compositor", "wm", "shell", "session"]
        );
        assert_eq!(architecture.layer("shell").unwrap().status, "deferred");
    }

    #[test]
    fn window_manager_moves_and_resizes_floating_windows() {
        let mut wm = wm::WindowManager::new((1280, 720));
        let id = wm.add_window("kitty", Rect::new(100, 100, 640, 420));

        wm.focus(id);
        wm.move_focused_by(32, 16);
        wm.resize_focused_by(80, -40);

        let window = wm.window(id).unwrap();
        assert_eq!(window.rect, Rect::new(132, 116, 720, 380));
    }

    #[test]
    fn nested_launch_plan_wraps_app_with_environment_writer() {
        let options = NestedOptions {
            width: 1280,
            height: 720,
            refresh: 60,
            app: vec![
                "kitty".to_string(),
                "--class".to_string(),
                "OblivionOne".to_string(),
            ],
            state_dir: PathBuf::from("/tmp/oblivion-one-test"),
        };

        let plan = NestedLaunchPlan::new(options);

        assert_eq!(plan.program, "gamescope");
        assert!(plan.args.starts_with(&[
            "-W".to_string(),
            "1280".to_string(),
            "-H".to_string(),
            "720".to_string(),
            "-r".to_string(),
            "60".to_string(),
        ]));
        assert!(plan.args.contains(&"--".to_string()));
        assert_eq!(
            plan.env_file,
            PathBuf::from("/tmp/oblivion-one-test/session.env")
        );
        assert_eq!(plan.session_id, "nested");
    }

    #[test]
    fn session_env_parser_keeps_known_wayland_keys() {
        let env = parse_session_env(
            "WAYLAND_DISPLAY=gamescope-0\nXDG_RUNTIME_DIR=/run/user/1000\nXDG_CURRENT_DESKTOP=OblivionOne\nDESKTOP_SESSION=oblivion-one\nIGNORED=value\n",
        );

        assert_eq!(
            env.get("WAYLAND_DISPLAY").map(String::as_str),
            Some("gamescope-0")
        );
        assert_eq!(
            env.get("XDG_RUNTIME_DIR").map(String::as_str),
            Some("/run/user/1000")
        );
        assert_eq!(
            env.get("XDG_CURRENT_DESKTOP").map(String::as_str),
            Some("OblivionOne")
        );
        assert_eq!(
            env.get("DESKTOP_SESSION").map(String::as_str),
            Some("oblivion-one")
        );
        assert!(!env.contains_key("IGNORED"));
    }

    #[test]
    fn session_env_parser_normalizes_gamescope_wayland_display() {
        let env = parse_session_env(
            "GAMESCOPE_WAYLAND_DISPLAY=gamescope-0\nDISPLAY=:2\nXDG_RUNTIME_DIR=/run/user/1000\n",
        );

        assert_eq!(
            env.get("WAYLAND_DISPLAY").map(String::as_str),
            Some("gamescope-0")
        );
        assert_eq!(env.get("DISPLAY").map(String::as_str), Some(":2"));
    }

    #[test]
    fn default_state_dir_uses_home_local_state() {
        let state_dir = default_state_dir_from_home("/home/agony");

        assert_eq!(
            state_dir,
            PathBuf::from("/home/agony/.local/state/oblivion-one")
        );
    }

    #[test]
    fn prototype_scene_contains_mac_style_shell_parts() {
        let scene = PrototypeScene::default();

        assert_eq!(scene.size, (1280, 800));
        assert_eq!(scene.windows.len(), 3);
        assert_eq!(scene.windows[0].title, "Explorer");
        assert_eq!(scene.dock_items[0].label, "Terminal");
        assert_eq!(scene.dock_items[1].label, "Explorer");
        assert_eq!(scene.dock_items[4].program(), "quickshell");
        assert!(scene.topbar_rect().contains(12, 12));
        assert!(scene.dock_rect().contains(640, 748));
    }

    #[test]
    fn prototype_scene_can_activate_window_by_point() {
        let mut scene = PrototypeScene::default();

        let activated = scene.activate_at(600, 210);

        assert_eq!(activated, Some("settings"));
        assert_eq!(scene.active_window, 1);
    }

    #[test]
    fn prototype_scene_cycles_active_window() {
        let mut scene = PrototypeScene::default();

        scene.cycle_active();

        assert_eq!(scene.active_window, 1);
    }

    #[test]
    fn prototype_scene_resolves_dock_item_from_click_point() {
        let scene = PrototypeScene::default();
        let terminal_rect = scene.dock_item_rect(0).unwrap();

        let item = scene
            .dock_item_at(terminal_rect.x + 8, terminal_rect.y + 8)
            .unwrap();

        assert_eq!(item.id, "terminal");
        assert_eq!(item.command, &["kitty", "--class", "OblivionOneTerminal"]);
    }

    #[test]
    fn prototype_runtime_state_records_real_launches() {
        let scene = PrototypeScene::default();
        let mut runtime = PrototypeRuntimeState::default();

        runtime.record_launch(&scene.dock_items[2], 4242);

        assert_eq!(runtime.launch_count_for("browser"), 1);
        assert_eq!(runtime.launches[0].label, "Browser");
    }

    #[test]
    fn prototype_scene_moves_active_window() {
        let mut scene = PrototypeScene::default();
        let before = scene.active_window().unwrap().rect;

        scene.move_active_by(32, 18);

        assert_eq!(scene.active_window().unwrap().rect.x, before.x + 32);
        assert_eq!(scene.active_window().unwrap().rect.y, before.y + 18);
    }

    #[test]
    fn prototype_scene_resizes_active_window_with_minimum() {
        let mut scene = PrototypeScene::default();

        scene.resize_active_by(-10_000, -10_000);

        assert_eq!(scene.active_window().unwrap().rect.width, 260);
        assert_eq!(scene.active_window().unwrap().rect.height, 160);
    }

    #[test]
    fn prototype_scene_minimizes_and_restores_window() {
        let mut scene = PrototypeScene::default();

        scene.minimize_active();
        let restored = scene.restore_next_minimized();

        assert_eq!(restored, Some("explorer"));
        assert!(!scene.active_window().unwrap().minimized);
        assert_eq!(scene.active_window, 0);
    }

    #[test]
    fn prototype_scene_toggles_maximize_and_restore() {
        let mut scene = PrototypeScene::default();
        let before = scene.active_window().unwrap().rect;

        scene.toggle_maximize_active();
        assert!(scene.active_window().unwrap().maximized);
        assert_ne!(scene.active_window().unwrap().rect, before);

        scene.toggle_maximize_active();
        assert!(!scene.active_window().unwrap().maximized);
        assert_eq!(scene.active_window().unwrap().rect, before);
    }

    #[test]
    fn prototype_scene_closes_active_window() {
        let mut scene = PrototypeScene::default();

        let closed = scene.close_active();

        assert_eq!(closed, Some("explorer"));
        assert_eq!(scene.windows.len(), 2);
        assert_eq!(scene.active_window().unwrap().id, "settings");
    }

    #[test]
    fn desktop_nested_options_runs_prototype_as_primary_child() {
        let options = DesktopOptions {
            width: 1440,
            height: 900,
            refresh: 60,
            state_dir: PathBuf::from("/tmp/oblivion-one-de"),
            executable: PathBuf::from("/tmp/oblivion-one"),
            backend: NestedBackend::Gamescope,
        };

        let nested = options.into_nested_options();

        assert_eq!(
            nested.app,
            vec![
                "/tmp/oblivion-one".to_string(),
                "prototype".to_string(),
                "--inside-de".to_string()
            ]
        );
        assert_eq!(nested.width, 1440);
        assert_eq!(nested.state_dir, PathBuf::from("/tmp/oblivion-one-de"));
    }

    #[test]
    fn desktop_options_default_to_oblivion_owned_compositor() {
        let options = DesktopOptions::with_defaults(
            PathBuf::from("/tmp/oblivion-one-de"),
            PathBuf::from("/tmp/oblivion-one"),
        );

        assert_eq!(options.backend, NestedBackend::Oblivion);
    }

    #[test]
    fn hyprland_launch_plan_writes_floating_window_session_config() {
        let options = DesktopOptions {
            width: 1440,
            height: 900,
            refresh: 60,
            state_dir: PathBuf::from("/tmp/oblivion-one-de"),
            executable: PathBuf::from("/tmp/oblivion-one"),
            backend: NestedBackend::Hyprland,
        };

        let plan = HyprlandLaunchPlan::new(options);

        assert_eq!(plan.program, "Hyprland");
        assert_eq!(
            plan.args,
            vec![
                "--config".to_string(),
                "/tmp/oblivion-one-de/hyprland.conf".to_string()
            ]
        );
        assert!(
            plan.config_contents
                .contains("bindm = $mainMod, mouse:272, movewindow")
        );
        assert!(
            plan.config_contents
                .contains("bindm = $mainMod, mouse:273, resizewindow")
        );
        assert!(plan.config_contents.contains("float = yes"));
        assert!(plan.config_contents.contains("exec-once = env"));
        assert!(
            plan.config_contents
                .contains("/tmp/oblivion-one prototype --inside-de")
        );
    }

    #[test]
    fn app_launch_env_prefers_gamescope_display_for_children() {
        let env = HashMap::from([
            ("WAYLAND_DISPLAY".to_string(), "wayland-1".to_string()),
            (
                "GAMESCOPE_WAYLAND_DISPLAY".to_string(),
                "gamescope-0".to_string(),
            ),
            ("DISPLAY".to_string(), ":2".to_string()),
            ("XDG_RUNTIME_DIR".to_string(), "/run/user/1000".to_string()),
        ]);

        let child_env = app_launch_env(&env);

        assert_eq!(
            child_env.get("WAYLAND_DISPLAY").map(String::as_str),
            Some("gamescope-0")
        );
        assert_eq!(child_env.get("DISPLAY").map(String::as_str), Some(":2"));
        assert_eq!(
            child_env.get("GDK_BACKEND").map(String::as_str),
            Some("wayland")
        );
        assert_eq!(
            child_env.get("QT_QPA_PLATFORM").map(String::as_str),
            Some("wayland")
        );
    }

    #[test]
    fn app_launch_env_preserves_regular_wayland_display_without_gamescope() {
        let env = HashMap::from([("WAYLAND_DISPLAY".to_string(), "wayland-1".to_string())]);

        let child_env = app_launch_env(&env);

        assert_eq!(
            child_env.get("WAYLAND_DISPLAY").map(String::as_str),
            Some("wayland-1")
        );
    }

    #[test]
    fn compositor_app_env_removes_host_wayland_and_desktop_activation_routes() {
        let launch_env = CompositorAppEnvironment::wayland_only("oblivion-one-test");
        let mut command = Command::new("true");
        command.env("WAYLAND_SOCKET", "9");
        command.env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus");
        command.env("DESKTOP_STARTUP_ID", "hyprland-startup");
        command.env(
            "GIO_LAUNCHED_DESKTOP_FILE",
            "/usr/share/applications/kitty.desktop",
        );
        command.env("GIO_LAUNCHED_DESKTOP_FILE_PID", "123");
        command.env("HYPRLAND_INSTANCE_SIGNATURE", "host-session");
        command.env("AT_SPI_BUS_ADDRESS", "unix:path=/run/user/1000/at-spi/bus");
        command.env(
            "XDG_DESKTOP_PORTAL_DIR",
            "/usr/share/xdg-desktop-portal/portals",
        );
        command.env("GTK_MODULES", "atk-bridge");

        configure_compositor_app_command_with_environment(&mut command, &launch_env);
        let env = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<HashMap<_, _>>();

        assert_eq!(
            env.get("WAYLAND_DISPLAY").and_then(Option::as_deref),
            Some("oblivion-one-test")
        );
        assert_eq!(
            env.get("XDG_CURRENT_DESKTOP").and_then(Option::as_deref),
            Some("Astrea")
        );
        assert_eq!(
            env.get("XDG_SESSION_DESKTOP").and_then(Option::as_deref),
            Some("Astrea")
        );
        assert_eq!(
            env.get("DESKTOP_SESSION").and_then(Option::as_deref),
            Some("Astrea")
        );
        assert_eq!(
            env.get("XDG_SESSION_TYPE").and_then(Option::as_deref),
            Some("wayland")
        );
        assert_eq!(
            env.get("MOZ_ENABLE_WAYLAND").and_then(Option::as_deref),
            Some("1")
        );
        for key in [
            "WAYLAND_SOCKET",
            "DBUS_SESSION_BUS_ADDRESS",
            "DESKTOP_STARTUP_ID",
            "GIO_LAUNCHED_DESKTOP_FILE",
            "GIO_LAUNCHED_DESKTOP_FILE_PID",
            "HYPRLAND_INSTANCE_SIGNATURE",
            "AT_SPI_BUS_ADDRESS",
            "GTK_MODULES",
        ] {
            assert_eq!(env.get(key), Some(&None), "{key} should be removed");
        }
        assert!(
            env.get("XDG_DESKTOP_PORTAL_DIR")
                .and_then(Option::as_deref)
                .is_some_and(
                    |path| path.ends_with("oblivion-one/portal-share/xdg-desktop-portal/portals")
                )
        );
        assert!(
            env.get("XDG_DATA_DIRS")
                .and_then(Option::as_deref)
                .is_some_and(|path| path.contains("oblivion-one/portal-share"))
        );
        assert_eq!(
            env.get("ASTREA_COMPOSITOR").and_then(Option::as_deref),
            Some("TYPHON")
        );
        for key in ["ASTREA_SHORTCUT_BRIDGE", "ASTREA_SHELL_CONTROL_BRIDGE"] {
            let value = env
                .get(key)
                .and_then(Option::as_deref)
                .expect("bridge path should be exported");
            assert!(!value.contains("/home/agony/GitHub/Typhon/"));
        }
    }

    #[test]
    fn compositor_app_env_disables_host_portal_accessibility_gvfs_and_lsfg_noise() {
        let launch_env = CompositorAppEnvironment::wayland_only("oblivion-one-test");
        let mut command = Command::new("true");
        command.env("GTK_USE_PORTAL", "1");

        configure_compositor_app_command_with_environment(&mut command, &launch_env);
        let env = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<HashMap<_, _>>();

        assert_eq!(
            env.get("GTK_USE_PORTAL").and_then(Option::as_deref),
            Some("0")
        );
        assert_eq!(
            env.get("QT_NO_USE_PORTAL").and_then(Option::as_deref),
            Some("1")
        );
        assert_eq!(
            env.get("GIO_USE_PORTALS").and_then(Option::as_deref),
            Some("0")
        );
        assert_eq!(env.get("GTK_A11Y").and_then(Option::as_deref), Some("none"));
        assert_eq!(
            env.get("NO_AT_BRIDGE").and_then(Option::as_deref),
            Some("1")
        );
        assert_eq!(
            env.get("GIO_USE_VFS").and_then(Option::as_deref),
            Some("local")
        );
        assert_eq!(
            env.get("GVFS_DISABLE_FUSE").and_then(Option::as_deref),
            Some("1")
        );
        assert_eq!(
            env.get("DISABLE_LSFG").and_then(Option::as_deref),
            Some("1")
        );
    }

    #[test]
    fn compositor_app_spawn_wraps_apps_in_a_private_dbus_session() {
        let app = vec!["kitty".to_string()];

        let argv = compositor_app_spawn_argv(&app, true).unwrap();

        assert_eq!(argv, vec!["dbus-run-session", "--", "kitty"]);
    }

    #[test]
    fn compositor_app_spawn_uses_normal_zen_profile_by_default() {
        let app = vec!["/opt/zen-browser-bin/zen-bin".to_string()];

        let argv = compositor_app_spawn_argv(&app, false).unwrap();
        let joined = argv.join(" ");

        assert_eq!(
            argv.first().map(String::as_str),
            Some("/opt/zen-browser-bin/zen-bin")
        );
        assert!(!joined.contains("--no-remote"));
        assert!(!joined.contains("--profile"));
    }

    #[test]
    fn compositor_app_spawn_isolates_firefox_profiles() {
        let app = vec!["firefox".to_string()];

        let argv = compositor_app_spawn_argv(&app, true).unwrap();
        let joined = argv.join(" ");

        assert_eq!(argv.first().map(String::as_str), Some("dbus-run-session"));
        assert!(joined.contains("--no-remote"));
        assert!(joined.contains("--profile"));
        assert!(joined.contains("oblivion-one/app-profiles/firefox"));
    }

    #[test]
    fn cpu_compositor_app_spawn_can_force_zen_isolated_for_diagnostics() {
        let app = vec![
            "/opt/zen-browser-bin/zen-bin".to_string(),
            "--profile".to_string(),
            "/tmp/zen".to_string(),
        ];

        let argv = compositor_cpu_app_spawn_argv(&app, true).unwrap();
        let joined = argv.join(" ");

        assert_eq!(argv.first().map(String::as_str), Some("dbus-run-session"));
        assert!(joined.contains("--profile /tmp/zen"));
        assert!(!joined.contains("oblivion-one/app-profiles/zen-bin"));
    }

    #[test]
    fn compositor_app_spawn_isolates_chromium_browser_profiles() {
        let app = vec!["brave".to_string(), "%U".to_string()];

        let argv = compositor_app_spawn_argv(&app, true).unwrap();
        let joined = argv.join(" ");

        assert_eq!(argv.first().map(String::as_str), Some("dbus-run-session"));
        assert!(joined.contains("--user-data-dir="));
        assert!(joined.contains("oblivion-one/app-profiles/brave"));
        assert!(!joined.contains("%U"));
    }

    #[test]
    fn cpu_compositor_chromium_args_disable_gpu_buffer_paths() {
        let app = vec!["brave".to_string(), "%U".to_string()];

        let argv = compositor_cpu_app_spawn_argv(&app, true).unwrap();
        let joined = argv.join(" ");

        assert!(joined.contains("--user-data-dir="));
        assert!(joined.contains("--ozone-platform=wayland"));
        assert!(joined.contains("--disable-gpu"));
        assert!(joined.contains("--disable-gpu-compositing"));
        assert!(joined.contains("--disable-zero-copy"));
        assert!(!joined.contains("--use-gl=egl-angle"));
        assert!(!joined.contains("%U"));
    }

    #[test]
    fn compositor_app_env_can_expose_only_an_oblivion_owned_xwayland_display() {
        let launch_env =
            CompositorAppEnvironment::with_isolated_xwayland("oblivion-one-test", ":42");
        let mut command = Command::new("true");

        configure_compositor_app_command_with_environment(&mut command, &launch_env);
        let env = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<HashMap<_, _>>();

        assert_eq!(
            env.get("WAYLAND_DISPLAY").and_then(Option::as_deref),
            Some("oblivion-one-test")
        );
        assert_eq!(env.get("DISPLAY").and_then(Option::as_deref), Some(":42"));
        assert_eq!(
            env.get("OBLIVION_ONE_XWAYLAND_DISPLAY")
                .and_then(Option::as_deref),
            Some(":42")
        );
        assert_eq!(
            env.get("GDK_BACKEND").and_then(Option::as_deref),
            Some("wayland,x11")
        );
        assert_eq!(
            env.get("QT_QPA_PLATFORM").and_then(Option::as_deref),
            Some("wayland;xcb")
        );
    }

    #[test]
    fn cpu_compositor_app_env_forces_software_rendering_guards() {
        let mut command = Command::new("true");

        configure_cpu_compositor_app_command(&mut command, "oblivion-one-test");
        let env = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<HashMap<_, _>>();

        assert_eq!(
            env.get("WAYLAND_DISPLAY").and_then(Option::as_deref),
            Some("oblivion-one-test")
        );
        assert_eq!(
            env.get("OBLIVION_ONE_CPU_COMPOSITION")
                .and_then(Option::as_deref),
            Some("1")
        );
        assert_eq!(
            env.get("MOZ_WEBRENDER_SOFTWARE").and_then(Option::as_deref),
            Some("1")
        );
        assert_eq!(
            env.get("WEBKIT_DISABLE_DMABUF_RENDERER")
                .and_then(Option::as_deref),
            Some("1")
        );
    }

    #[test]
    fn plain_brave_compositor_args_are_passthrough_like_real_compositors() {
        let args = compositor_app_args_for("brave", &[]);

        assert!(args.is_empty());
    }

    #[test]
    fn chromium_compositor_args_preserve_existing_switches() {
        let original = vec![
            "--enable-features=Foo".to_string(),
            "--disable-features=Bar,Vulkan".to_string(),
        ];
        let args = compositor_app_args_for("chromium", &original);

        assert_eq!(args, original);
    }

    #[test]
    fn non_chromium_compositor_args_are_not_modified() {
        let args = compositor_app_args_for("kitty", &["--class".to_string(), "Test".to_string()]);

        assert_eq!(args, ["--class".to_string(), "Test".to_string()]);
    }

    #[test]
    fn nested_env_writer_exports_nested_display_before_exec() {
        assert!(ENV_WRITER_SCRIPT.contains("export WAYLAND_DISPLAY=\"$wayland_display\""));
        assert!(
            ENV_WRITER_SCRIPT.contains("export GAMESCOPE_WAYLAND_DISPLAY=\"$gamescope_display\"")
        );
        assert!(
            ENV_WRITER_SCRIPT
                .contains("wayland_display=\"${GAMESCOPE_WAYLAND_DISPLAY:-${WAYLAND_DISPLAY:-}}\"")
        );
    }

    #[test]
    fn nested_env_writer_does_not_invent_gamescope_display_for_hyprland() {
        assert!(ENV_WRITER_SCRIPT.contains("gamescope_display=\"${GAMESCOPE_WAYLAND_DISPLAY:-}\""));
        assert!(ENV_WRITER_SCRIPT.contains("if [ -n \"$gamescope_display\" ]; then"));
        assert!(ENV_WRITER_SCRIPT.contains("unset GAMESCOPE_WAYLAND_DISPLAY"));
    }

    #[test]
    fn isolated_app_launch_env_uses_per_app_xdg_dirs() {
        let env = HashMap::from([(
            "GAMESCOPE_WAYLAND_DISPLAY".to_string(),
            "gamescope-0".to_string(),
        )]);

        let child_env = app_launch_env_for(&env, Some("browser"));

        assert_eq!(
            child_env.get("WAYLAND_DISPLAY").map(String::as_str),
            Some("gamescope-0")
        );
        assert!(
            child_env
                .get("XDG_CONFIG_HOME")
                .is_some_and(|path| path.ends_with("/oblivion-one/apps/browser/config"))
        );
        assert!(
            child_env
                .get("OBLIVION_ONE_APP_DIR")
                .is_some_and(|path| path.ends_with("/oblivion-one/apps/browser"))
        );
    }

    #[test]
    fn portal_runtime_files_describe_oblivion_backend() {
        let runtime = portal::PortalRuntime::new(
            PathBuf::from("/tmp/oblivion-one-test"),
            PathBuf::from("/opt/oblivion-one/bin/oblivion-one"),
        );

        assert_eq!(
            runtime.portal_dir(),
            PathBuf::from("/tmp/oblivion-one-test/portal-share/xdg-desktop-portal/portals")
        );
        assert!(runtime.portal_contents().contains("UseIn=OblivionOne"));
        assert!(
            runtime
                .portal_contents()
                .contains("org.freedesktop.impl.portal.Settings")
        );
        assert!(
            runtime
                .portal_contents()
                .contains("org.freedesktop.impl.portal.Notification")
        );
        assert!(
            runtime
                .portal_contents()
                .contains("org.freedesktop.impl.portal.Access")
        );
        assert!(
            runtime
                .service_contents()
                .contains("Name=org.freedesktop.impl.portal.desktop.oblivion")
        );
        assert!(
            runtime
                .service_contents()
                .contains("Exec=/opt/oblivion-one/bin/oblivion-one portal")
        );
        assert!(
            runtime
                .config_contents()
                .contains("org.freedesktop.impl.portal.Settings=oblivion")
        );
        assert!(
            runtime
                .config_contents()
                .contains("org.freedesktop.impl.portal.Access=oblivion")
        );
        assert!(runtime.config_contents().contains("default=none"));
    }

    #[test]
    fn portal_settings_filter_appearance_namespace() {
        let values = portal::settings_for_namespaces(&["org.freedesktop.appearance".to_string()]);

        assert_eq!(
            values
                .get("org.freedesktop.appearance")
                .and_then(|namespace| namespace.get("color-scheme")),
            Some(&portal::PortalSettingValue::U32(1))
        );
        assert!(!values.contains_key("org.unknown"));
    }

    #[test]
    fn browser_dock_item_uses_isolated_profile_wrapper() {
        let scene = PrototypeScene::default();
        let browser = scene
            .dock_items
            .iter()
            .find(|item| item.id == "browser")
            .unwrap();

        assert!(browser.isolated_profile);
        assert_eq!(browser.program(), "sh");
        assert!(
            browser
                .command
                .join(" ")
                .contains("--user-data-dir=\"$OBLIVION_ONE_APP_DIR/brave-profile\"")
        );
    }

    #[test]
    fn browser_dock_item_uses_wayland_egl_angle_without_vulkan() {
        let scene = PrototypeScene::default();
        let browser = scene
            .dock_items
            .iter()
            .find(|item| item.id == "browser")
            .unwrap();

        let command = browser.command.join(" ");

        assert!(command.contains("--ozone-platform=wayland"));
        assert!(command.contains("--enable-features=UseOzonePlatform"));
        assert!(command.contains("--use-gl=egl-angle"));
        assert!(command.contains("--use-angle=opengles"));
        assert!(
            !command
                .split_whitespace()
                .any(|arg| arg == "--use-gl=angle")
        );
        assert!(!command.split_whitespace().any(|arg| arg == "--use-gl=egl"));
        assert!(command.contains("--disable-features=Vulkan"));
        assert!(command.contains("--disable-vulkan"));
    }

    #[test]
    fn xwayland_launch_plan_uses_rootless_compositor_owned_fds() {
        let plan = xwayland::XWaylandLaunchPlan::new(":42", "oblivion-one-0", 11, 12, 13);

        assert_eq!(plan.program, "Xwayland");
        assert_eq!(
            plan.args,
            vec![
                ":42".to_string(),
                "-rootless".to_string(),
                "-terminate".to_string(),
                "-listenfd".to_string(),
                "11".to_string(),
                "-wm".to_string(),
                "12".to_string(),
                "-displayfd".to_string(),
                "13".to_string(),
            ]
        );
        assert_eq!(
            plan.env_pairs(),
            [("WAYLAND_DISPLAY", "oblivion-one-0".to_string())]
        );
        assert!(plan.display_command().contains("-rootless"));
        assert!(plan.display_command().contains("-wm 12"));
    }
}
