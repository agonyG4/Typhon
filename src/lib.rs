pub mod astrea_shell_control;
pub mod astrea_shortcuts;
pub mod compositor;
pub mod core;
pub mod cursor_theme;
mod defaults;
mod launch_env;
pub mod native;
mod paths;
pub mod portal;
pub mod process;
pub mod render_backend;
pub mod session;
pub mod shell;
pub mod syncobj;
pub mod wayland_drm;
pub mod wm;
pub mod xwayland;

pub use core::Rect;
pub use defaults::*;
pub use launch_env::*;
pub use paths::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, path::PathBuf, process::Command};

    #[test]
    fn oblivion_compositor_plan_has_no_external_compositor_dependency() {
        let plan = compositor::CompositorPlan::new("oblivion-one-0");

        assert_eq!(plan.socket_name, "oblivion-one-0");
        assert!(!plan.uses_external_compositor());
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
    fn default_state_dir_uses_home_local_state() {
        let state_dir = default_state_dir_from_home("/home/agony");

        assert_eq!(
            state_dir,
            PathBuf::from("/home/agony/.local/state/oblivion-one")
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
        assert!(!env.contains_key("MOZ_ENABLE_WAYLAND"));
        for key in [
            "WAYLAND_SOCKET",
            "DESKTOP_STARTUP_ID",
            "GIO_LAUNCHED_DESKTOP_FILE",
            "GIO_LAUNCHED_DESKTOP_FILE_PID",
            "HYPRLAND_INSTANCE_SIGNATURE",
            "AT_SPI_BUS_ADDRESS",
            "GTK_MODULES",
        ] {
            assert_eq!(env.get(key), Some(&None), "{key} should be removed");
        }
        assert_eq!(
            env.get("DBUS_SESSION_BUS_ADDRESS")
                .and_then(Option::as_deref),
            Some("unix:path=/run/user/1000/bus")
        );
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
    fn compositor_app_env_preserves_portals_and_disables_accessibility_gvfs_and_lsfg_noise() {
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
            Some("1")
        );
        assert_eq!(env.get("QT_NO_USE_PORTAL").and_then(Option::as_deref), None);
        assert_eq!(env.get("GIO_USE_PORTALS").and_then(Option::as_deref), None);
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
    fn compositor_app_spawn_private_dbus_is_diagnostic_only() {
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
    fn desktop_entry_exec_removes_field_codes_without_browser_mutation() {
        let entry = "[Desktop Entry]\nType=Application\nName=Zen\nExec=zen-browser %U\n";

        let launch = parse_desktop_entry(entry, None).unwrap();

        assert_eq!(launch.argv, vec!["zen-browser"]);
    }

    #[test]
    fn desktop_entry_exec_preserves_explicit_user_arguments() {
        let entry = "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox --new-window \"about:blank\" %%\n";

        let launch = parse_desktop_entry(entry, None).unwrap();

        assert_eq!(
            launch.argv,
            vec!["firefox", "--new-window", "about:blank", "%"]
        );
    }

    #[test]
    fn compositor_app_spawn_isolates_firefox_profiles() {
        let app = vec!["firefox".to_string()];

        let argv = compositor_app_spawn_argv(&app, false).unwrap();

        assert_eq!(argv, vec!["firefox"]);
    }

    #[test]
    fn cpu_compositor_app_spawn_preserves_explicit_user_zen_args() {
        let app = vec![
            "/opt/zen-browser-bin/zen-bin".to_string(),
            "--profile".to_string(),
            "/tmp/zen".to_string(),
        ];

        let argv = compositor_cpu_app_spawn_argv(&app, false).unwrap();
        let joined = argv.join(" ");

        assert_eq!(
            argv.first().map(String::as_str),
            Some("/opt/zen-browser-bin/zen-bin")
        );
        assert!(joined.contains("--profile /tmp/zen"));
        assert!(!joined.contains("oblivion-one/app-profiles/zen-bin"));
    }

    #[test]
    fn compositor_app_spawn_preserves_chromium_argv_too() {
        let app = vec!["brave".to_string(), "%U".to_string()];

        let argv = compositor_app_spawn_argv(&app, false).unwrap();

        assert_eq!(argv, vec!["brave"]);
    }

    #[test]
    fn cpu_compositor_spawn_does_not_rewrite_browser_argv() {
        let app = vec!["brave".to_string(), "%U".to_string()];

        let argv = compositor_cpu_app_spawn_argv(&app, false).unwrap();

        assert_eq!(argv, vec!["brave"]);
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
        assert!(!env.contains_key("MOZ_WEBRENDER_SOFTWARE"));
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
