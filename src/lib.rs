pub mod astrea_shell_auth;
pub mod astrea_shell_control;
pub mod astrea_shortcuts;
pub mod astrea_toplevel_management;
pub mod astreactl;
pub mod compositor;
pub mod control;
pub mod control_snapshots;
pub mod core;
pub mod cursor_geometry;
pub mod cursor_manager;
pub mod cursor_persistence;
pub mod cursor_theme;
mod defaults;
mod launch_env;
pub mod native;
mod paths;
mod pointer_debug;
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
    use crate::xwayland::XwaylandAppEnvironment;
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
    fn canonical_window_id_rejects_zero_and_round_trips_raw_values() {
        assert!(core::WindowId::from_raw(0).is_none());
        let id = core::WindowId::from_raw(42).expect("non-zero id");
        let compositor_id: compositor::WindowId = id;

        assert_eq!(id.get(), 42);
        assert_eq!(compositor_id.get(), 42);
        assert_eq!(id, compositor_id);
    }

    #[test]
    fn workspace_ids_are_nonzero_and_extensible() {
        assert!(wm::WorkspaceId::new(0).is_none());
        assert_eq!(wm::WorkspaceId::new(1).unwrap().get(), 1);
        assert_eq!(wm::WorkspaceId::new(42).unwrap().to_string(), "42");
    }

    #[test]
    fn workspace_manager_has_deterministic_default_state() {
        let manager = wm::WorkspaceManager::default();

        assert_eq!(manager.active_workspace(), wm::WorkspaceId::new(1).unwrap());
        assert_eq!(manager.workspaces().count(), 10);
        assert!(manager.contains(wm::WorkspaceId::new(10).unwrap()));
        assert!(!manager.contains(wm::WorkspaceId::new(99).unwrap()));
        assert!(wm::WorkspaceId::new(0).is_none());
        assert!(wm::WorkspaceManager::new(0).is_none());
    }

    #[test]
    fn workspace_manager_switches_known_workspace_once() {
        let mut manager = wm::WorkspaceManager::default();
        let workspace_two = wm::WorkspaceId::new(2).unwrap();

        assert_eq!(
            manager.activate(workspace_two),
            wm::WorkspaceSwitchOutcome::Changed {
                previous: wm::WorkspaceId::new(1).unwrap(),
                current: workspace_two,
            }
        );
        assert_eq!(manager.active_workspace(), workspace_two);
        assert_eq!(
            manager.activate(workspace_two),
            wm::WorkspaceSwitchOutcome::NoChange
        );
        assert_eq!(
            manager.activate(wm::WorkspaceId::new(99).unwrap()),
            wm::WorkspaceSwitchOutcome::UnknownWorkspace
        );
        assert_eq!(manager.active_workspace(), workspace_two);
    }

    #[test]
    fn ewmh_workspace_conversion_is_extensible_beyond_the_default_policy() {
        assert_eq!(wm::WorkspaceId::from_ewmh(0), wm::WorkspaceId::new(1));
        assert_eq!(wm::WorkspaceId::from_ewmh(9), wm::WorkspaceId::new(10));
        assert_eq!(wm::WorkspaceId::from_ewmh(10), wm::WorkspaceId::new(11));
        assert_eq!(wm::WorkspaceId::from_ewmh(u32::MAX), None);
    }

    #[test]
    fn workspace_manager_reports_its_configured_workspace_count() {
        let manager = wm::WorkspaceManager::new(12).expect("workspace manager");

        assert_eq!(manager.workspace_count(), 12);
        assert!(manager.contains(wm::WorkspaceId::new(11).unwrap()));
    }

    #[test]
    fn window_management_state_defaults_to_active_floating_membership() {
        let workspace = wm::WorkspaceId::new(3).unwrap();
        let state = wm::WindowManagementState::new(wm::WorkspaceLocation::Regular(workspace));

        assert_eq!(state.regular_workspace(), Some(workspace));
        assert_eq!(state.layout(), wm::LayoutMembership::Floating);
        assert_eq!(
            state.with_layout(wm::LayoutMembership::Tiled).layout(),
            wm::LayoutMembership::Tiled
        );
    }

    #[test]
    fn moving_window_management_state_preserves_layout_membership() {
        let first = wm::WorkspaceId::new(1).unwrap();
        let second = wm::WorkspaceId::new(5).unwrap();

        let floating = wm::WindowManagementState::new(wm::WorkspaceLocation::Regular(first))
            .with_location(wm::WorkspaceLocation::Regular(second));
        assert_eq!(floating.regular_workspace(), Some(second));
        assert_eq!(floating.layout(), wm::LayoutMembership::Floating);

        let tiled = wm::WindowManagementState::new(wm::WorkspaceLocation::Regular(first))
            .with_layout(wm::LayoutMembership::Tiled)
            .with_location(wm::WorkspaceLocation::Regular(second));
        assert_eq!(tiled.regular_workspace(), Some(second));
        assert_eq!(tiled.layout(), wm::LayoutMembership::Tiled);
    }

    #[test]
    fn workspace_ids_round_trip_ewmh_indices_and_reject_reserved_values() {
        for (ewmh, workspace) in (0..10).zip(1..=10) {
            let id = wm::WorkspaceId::from_ewmh(ewmh).expect("valid EWMH workspace");
            assert_eq!(id.get(), workspace);
            assert_eq!(id.to_ewmh(), ewmh);
        }
        let extensible = wm::WorkspaceId::from_ewmh(10).expect("identity conversion is extensible");
        assert_eq!(extensible.get(), 11);
        assert_eq!(extensible.to_ewmh(), 10);
        assert_eq!(wm::WorkspaceId::from_ewmh(u32::MAX), None);
    }

    #[test]
    fn special_workspace_ids_are_typed_nonzero_and_orderable() {
        let default = wm::SpecialWorkspaceId::DEFAULT;
        let second = wm::SpecialWorkspaceId::new(2).expect("non-zero special workspace");

        assert_eq!(default.get(), 1);
        assert!(wm::SpecialWorkspaceId::new(0).is_none());
        assert!(default < second);
        assert_eq!(default, wm::SpecialWorkspaceId::DEFAULT);
    }

    #[test]
    fn window_management_location_is_orthogonal_to_layout() {
        let regular = wm::WorkspaceId::new(3).expect("regular workspace");
        let special = wm::SpecialWorkspaceId::DEFAULT;
        let state = wm::WindowManagementState::new(wm::WorkspaceLocation::Regular(regular))
            .with_layout(wm::LayoutMembership::Tiled)
            .with_location(wm::WorkspaceLocation::Special(special));

        assert_eq!(state.location(), wm::WorkspaceLocation::Special(special));
        assert_eq!(state.regular_workspace(), None);
        assert_eq!(state.special_workspace(), Some(special));
        assert_eq!(state.layout(), wm::LayoutMembership::Tiled);
    }

    #[test]
    fn window_chrome_policy_is_derived_only_from_layout_membership() {
        let regular = wm::WorkspaceId::new(1).expect("regular workspace");
        let special = wm::SpecialWorkspaceId::DEFAULT;

        let regular_floating =
            wm::WindowManagementState::new(wm::WorkspaceLocation::Regular(regular));
        let special_floating =
            wm::WindowManagementState::new(wm::WorkspaceLocation::Special(special));
        let regular_tiled = regular_floating.with_layout(wm::LayoutMembership::Tiled);
        let special_tiled = special_floating.with_layout(wm::LayoutMembership::Tiled);

        assert_eq!(
            regular_floating.chrome_policy(),
            wm::WindowChromePolicy::Full
        );
        assert_eq!(
            special_floating.chrome_policy(),
            wm::WindowChromePolicy::Full
        );
        assert_eq!(
            regular_tiled.chrome_policy(),
            wm::WindowChromePolicy::Minimal
        );
        assert_eq!(
            special_tiled.chrome_policy(),
            wm::WindowChromePolicy::Minimal
        );
        assert_eq!(regular_tiled.location(), regular_floating.location());
        assert_eq!(special_tiled.location(), special_floating.location());
    }

    #[test]
    fn special_toggle_is_separate_from_regular_workspace_selection() {
        let mut manager = wm::WorkspaceManager::default();
        let regular_two = wm::WorkspaceId::new(2).expect("regular workspace");
        let special = wm::SpecialWorkspaceId::DEFAULT;

        assert_eq!(manager.visible_special_workspace(), None);
        let unknown = wm::SpecialWorkspaceId::new(2).expect("non-zero special workspace");
        assert_eq!(
            manager.toggle_special_workspace(unknown),
            wm::SpecialWorkspaceToggleOutcome::UnknownSpecial { id: unknown }
        );
        assert_eq!(
            manager.toggle_special_workspace(special),
            wm::SpecialWorkspaceToggleOutcome::Opened { id: special }
        );
        assert_eq!(manager.visible_special_workspace(), Some(special));
        assert_eq!(manager.active_workspace(), wm::WorkspaceId::new(1).unwrap());
        assert_eq!(
            manager.activate(regular_two),
            wm::WorkspaceSwitchOutcome::Changed {
                previous: wm::WorkspaceId::new(1).unwrap(),
                current: regular_two,
            }
        );
        assert_eq!(manager.visible_special_workspace(), Some(special));
        assert_eq!(
            manager.toggle_special_workspace(special),
            wm::SpecialWorkspaceToggleOutcome::Closed { id: special }
        );
        assert_eq!(manager.visible_special_workspace(), None);
        assert_eq!(manager.workspaces().count(), 10);
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
    fn supervised_child_cursor_environment_is_command_local() {
        let configuration = cursor_theme::CursorConfiguration::new("Bibata", 32).unwrap();
        let theme_before = std::env::var_os("XCURSOR_THEME");
        let size_before = std::env::var_os("XCURSOR_SIZE");
        let child = compositor_app_command_with_policy_and_xwayland_and_cursor(
            "oblivion-one-test",
            &["true".to_string()],
            EffectiveCompositorAppGpuPolicy::Accelerated,
            None,
            Some(&configuration),
        )
        .unwrap()
        .expect("command should be created");
        let env = child
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<HashMap<_, _>>();
        assert_eq!(
            env.get("XCURSOR_THEME").and_then(Option::as_deref),
            Some("Bibata")
        );
        assert_eq!(
            env.get("XCURSOR_SIZE").and_then(Option::as_deref),
            Some("32")
        );
        assert_eq!(std::env::var_os("XCURSOR_THEME"), theme_before);
        assert_eq!(std::env::var_os("XCURSOR_SIZE"), size_before);
    }

    #[test]
    fn unspecialized_supervised_child_does_not_receive_cursor_overrides() {
        let child = compositor_app_command_with_policy_and_xwayland_and_cursor(
            "oblivion-one-test",
            &["true".to_string()],
            EffectiveCompositorAppGpuPolicy::Accelerated,
            None,
            None,
        )
        .unwrap()
        .expect("command should be created");
        let env = child
            .get_envs()
            .map(|(key, value)| (key.to_string_lossy().into_owned(), value))
            .collect::<HashMap<_, _>>();

        assert_eq!(env.get("XCURSOR_THEME"), None);
        assert_eq!(env.get("XCURSOR_SIZE"), None);
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
        let launch_env = CompositorAppEnvironment::with_isolated_xwayland_and_auth(
            "oblivion-one-test",
            ":42",
            "/run/user/1000/typhon/xwayland/.Xauthority-42",
        );
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
            env.get("XAUTHORITY").and_then(Option::as_deref),
            Some("/run/user/1000/typhon/xwayland/.Xauthority-42")
        );
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
    fn opt_in_xwayland_launch_removes_host_routing_before_applying_owned_values() {
        let mut command = Command::new("true");
        command.env("DISPLAY", "host:0");
        command.env("XAUTHORITY", "/host/.Xauthority");
        command.env("OBLIVION_ONE_XWAYLAND_DISPLAY", ":host");
        let xwayland = XwaylandAppEnvironment {
            display: ":41".to_owned(),
            xauthority: PathBuf::from("/run/user/1000/typhon/xwayland/.Xauthority-41-token"),
        };

        configure_compositor_app_command_with_xwayland_environment(
            &mut command,
            "typhon-test",
            &xwayland,
        );
        let env = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<HashMap<_, _>>();

        assert_eq!(env.get("DISPLAY").and_then(Option::as_deref), Some(":41"));
        assert_eq!(
            env.get("XAUTHORITY").and_then(Option::as_deref),
            Some("/run/user/1000/typhon/xwayland/.Xauthority-41-token")
        );
        assert_eq!(
            env.get("OBLIVION_ONE_XWAYLAND_DISPLAY")
                .and_then(Option::as_deref),
            Some(":41")
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
}

#[cfg(test)]
mod control_tests;
