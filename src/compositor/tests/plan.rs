use super::*;

#[test]
fn compositor_plan_advertises_minimum_real_client_protocols() {
    let plan = CompositorPlan::new("oblivion-one-test");
    let expected = [
        "wl_compositor",
        "wl_subcompositor",
        "wl_data_device_manager",
        "wl_shm",
        "wp_viewporter",
        "wp_fractional_scale_manager_v1",
        "wp_presentation",
        "wp_color_manager_v1",
        "zwp_primary_selection_device_manager_v1",
        "ext_data_control_manager_v1",
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
fn unsupported_gaming_protocol_stubs_are_hidden_from_public_plan() {
    let plan = CompositorPlan::new("oblivion-one-test");
    let protocols = plan.protocol_names();

    assert!(!protocols.contains(&"zwp_relative_pointer_manager_v1"));
    assert!(!protocols.contains(&"zwp_pointer_constraints_v1"));
    assert!(!protocols.contains(&"zwp_idle_inhibit_manager_v1"));
}

#[test]
fn staging_frame_pacing_protocols_are_hidden_until_barriers_are_implemented() {
    let plan = CompositorPlan::new("oblivion-one-test");
    let protocols = plan.protocol_names();

    assert!(!protocols.contains(&"wp_fifo_manager_v1"));
    assert!(!protocols.contains(&"wp_commit_timing_manager_v1"));
}

#[test]
fn compositor_state_tracks_created_xdg_toplevels() {
    let mut state = CompositorState::default();

    state.note_xdg_toplevel_created("kitty");

    assert_eq!(state.accepted_clients, 0);
    assert_eq!(state.xdg_toplevels, 1);
    assert_eq!(state.last_app_id.as_deref(), Some("kitty"));
}
