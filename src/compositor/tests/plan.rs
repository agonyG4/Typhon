use super::*;

#[test]
fn compositor_plan_advertises_minimum_real_client_protocols() {
    let plan = CompositorPlan::new("oblivion-one-test");
    let expected = [
        "wl_compositor",
        "wl_subcompositor",
        "wl_data_device_manager",
        "zwp_primary_selection_device_manager_v1",
        "ext_data_control_manager_v1",
        "wl_shm",
        "wp_viewporter",
        "wp_fractional_scale_manager_v1",
        "wp_presentation",
        "wp_cursor_shape_manager_v1",
        "zxdg_decoration_manager_v1",
        "zwp_linux_dmabuf_v1",
        "wp_linux_drm_syncobj_manager_v1",
        "wl_drm",
        "xdg_wm_base",
        "wl_output",
        "wl_seat",
    ];

    assert_eq!(plan.protocol_names().as_slice(), expected.as_slice());
}

#[test]
fn default_plan_hides_color_management_until_renderer_supports_transforms() {
    let protocols = CompositorPlan::new("oblivion-one-test").protocol_names();

    assert!(!protocols.contains(&"wp_color_manager_v1"));
}

#[test]
fn renderer_color_capability_adds_color_management_once() {
    let protocols = client_protocols_for_capabilities(
        InputProtocolCapabilities::desktop_baseline(),
        SelectionProtocolCapabilities::core_clipboard(),
        RendererProtocolCapabilities {
            color_management: true,
        },
    );
    let names: Vec<_> = protocols.into_iter().map(ProtocolGlobal::name).collect();

    assert!(names.contains(&"wp_color_manager_v1"));
    assert_eq!(
        names
            .iter()
            .filter(|name| **name == "wp_color_manager_v1")
            .count(),
        1
    );
}

#[test]
fn safe_selection_profile_hides_unimplemented_selection_protocols() {
    let protocols = client_protocols_for_capabilities(
        InputProtocolCapabilities::desktop_baseline(),
        SelectionProtocolCapabilities::safe_baseline(),
        RendererProtocolCapabilities::unsupported(),
    );
    let names: Vec<_> = protocols.into_iter().map(ProtocolGlobal::name).collect();

    assert!(!names.contains(&"wl_data_device_manager"));
    assert!(!names.contains(&"zwp_primary_selection_device_manager_v1"));
    assert!(!names.contains(&"ext_data_control_manager_v1"));
}

#[test]
fn clipboard_only_profile_advertises_only_clipboard_selection() {
    let protocols = client_protocols_for_capabilities(
        InputProtocolCapabilities::desktop_baseline(),
        SelectionProtocolCapabilities {
            clipboard: true,
            primary_selection: false,
            data_control: false,
        },
        RendererProtocolCapabilities::unsupported(),
    );
    let names: Vec<_> = protocols.into_iter().map(ProtocolGlobal::name).collect();

    assert!(names.contains(&"wl_data_device_manager"));
    assert!(!names.contains(&"zwp_primary_selection_device_manager_v1"));
    assert!(!names.contains(&"ext_data_control_manager_v1"));
}

#[test]
fn default_plan_advertises_core_selection_protocols() {
    let protocols = CompositorPlan::new("oblivion-one-test").protocol_names();

    assert!(protocols.contains(&"wl_data_device_manager"));
    assert!(protocols.contains(&"zwp_primary_selection_device_manager_v1"));
    assert!(protocols.contains(&"ext_data_control_manager_v1"));
}

#[test]
fn protocol_capability_policy_does_not_duplicate_globals() {
    let protocols = client_protocols_for_capabilities(
        InputProtocolCapabilities {
            relative_pointer: true,
            pointer_constraints: true,
            pointer_warp: true,
            cursor_shape: false,
            keyboard_shortcuts_inhibit: false,
            idle_inhibit: true,
        },
        SelectionProtocolCapabilities {
            clipboard: true,
            primary_selection: true,
            data_control: true,
        },
        RendererProtocolCapabilities {
            color_management: true,
        },
    );
    let names: Vec<_> = protocols.into_iter().map(ProtocolGlobal::name).collect();

    for name in &names {
        assert_eq!(
            names.iter().filter(|candidate| *candidate == name).count(),
            1,
            "duplicated global {name}"
        );
    }
    assert!(!names.contains(&"wp_cursor_shape_manager_v1"));
}

#[test]
fn unsupported_gaming_protocol_stubs_are_hidden_from_public_plan() {
    let plan = CompositorPlan::new("oblivion-one-test");
    let protocols = plan.protocol_names();

    assert!(!protocols.contains(&"zwp_relative_pointer_manager_v1"));
    assert!(!protocols.contains(&"zwp_pointer_constraints_v1"));
    assert!(!protocols.contains(&"wp_pointer_warp_v1"));
    assert!(!protocols.contains(&"zwp_idle_inhibit_manager_v1"));
}

#[test]
fn native_libinput_profile_advertises_serviced_input_protocols() {
    let protocols = client_protocols_for_capabilities(
        InputProtocolCapabilities::native_libinput(),
        SelectionProtocolCapabilities::core_clipboard(),
        RendererProtocolCapabilities::unsupported(),
    );
    let names: Vec<_> = protocols.into_iter().map(ProtocolGlobal::name).collect();

    assert!(names.contains(&"zwp_relative_pointer_manager_v1"));
    assert!(names.contains(&"zwp_pointer_constraints_v1"));
    assert!(names.contains(&"wp_pointer_warp_v1"));
    assert!(names.contains(&"zwp_idle_inhibit_manager_v1"));
    assert!(names.contains(&"zwp_keyboard_shortcuts_inhibit_manager_v1"));
}

#[test]
fn safe_baseline_hides_frame_pacing_protocols() {
    let plan = CompositorPlan::new("oblivion-one-test");
    let protocols = plan.protocol_names();

    assert!(!protocols.contains(&"wp_fifo_manager_v1"));
    assert!(!protocols.contains(&"wp_commit_timing_manager_v1"));
}

#[test]
fn qualified_native_frame_pacing_profile_advertises_each_v1_global_once() {
    let plan = CompositorPlan::new("oblivion-one-test")
        .with_frame_pacing_capabilities(FramePacingProtocolCapabilities::qualified_native());
    let protocols = plan.protocol_names();

    for name in ["wp_fifo_manager_v1", "wp_commit_timing_manager_v1"] {
        assert_eq!(
            protocols
                .iter()
                .filter(|candidate| **candidate == name)
                .count(),
            1
        );
    }
    assert_eq!(
        crate::compositor::protocols::versions::WP_FIFO_MANAGER_V1,
        1
    );
    assert_eq!(
        crate::compositor::protocols::versions::WP_COMMIT_TIMING_MANAGER_V1,
        1
    );
}

#[test]
fn compositor_state_tracks_created_xdg_toplevels() {
    let mut state = CompositorState::default();

    state.note_xdg_toplevel_created("kitty");

    assert_eq!(state.accepted_clients, 0);
    assert_eq!(state.xdg_toplevels, 1);
    assert_eq!(state.last_app_id.as_deref(), Some("kitty"));
}
