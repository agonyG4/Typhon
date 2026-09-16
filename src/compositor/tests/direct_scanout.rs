use super::*;

use crate::blur_policy::{BlurBackend, BlurRuleAction, BlurWindowMatch, BlurWindowRule};

fn fullscreen_enable_rule_config() -> crate::blur_policy::BlurPolicyConfig {
    let mut config = crate::blur_policy::BlurPolicyConfig::default();
    config.window_rules.push(BlurWindowRule {
        name: "fullscreen-identity-blur".to_string(),
        matcher: BlurWindowMatch {
            app_id: Some("^oblivion\\.identity-viewport-test$".to_string()),
            title: None,
            backend: Some(BlurBackend::Wayland),
        },
        action: BlurRuleAction::Enable,
    });
    config
}

fn background_effect_count(scene: &ResolvedEffectScene) -> usize {
    scene
        .instances
        .iter()
        .filter(|instance| instance.program == crate::effects::builtin_background_blur_program_id())
        .count()
}

#[test]
fn fullscreen_identity_viewport_xrgb_dmabuf_is_direct_scanout_candidate() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_fullscreen_identity_viewport_xrgb_dmabuf(&socket_path, &commands).unwrap();
    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));
    let eligibility = capture_fullscreen_presentation_eligibility(&commands);
    assert!(eligibility.eligible);
    let candidate = capture_direct_scanout_candidate(&commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(candidate.buffer_size, BufferSize::new(1280, 800).unwrap());
    assert_eq!(candidate.output_size, BufferSize::new(1280, 800).unwrap());
    assert_eq!(candidate.buffer_size, candidate.output_size);
    assert_ne!(candidate.surface_id, 0);
    assert_eq!(candidate.surface_id, candidate.root_surface_id);
    assert!(candidate.generation > 0);
    assert!(candidate.commit_sequence.get() > 0);
    assert!(candidate.viewport_identity_metadata_present);
}

#[test]
fn output_sized_normal_window_is_not_fullscreen_but_is_scene_candidate() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_normal_identity_viewport_xrgb_dmabuf(&socket_path, &commands).unwrap();
    assert!(!state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(0, 0),
        1280,
        800,
    );
    let eligibility = capture_fullscreen_presentation_eligibility(&commands);
    assert_eq!(
        eligibility.rejection,
        Some(FullscreenPresentationRejection::NoFullscreenOwner)
    );
    assert!(capture_direct_scanout_candidate(&commands).is_ok());

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn output_sized_argb_dmabuf_is_not_an_opaque_scanout_candidate() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let _state = create_normal_identity_viewport_argb_dmabuf(&socket_path, &commands).unwrap();
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(0, 0),
        1280,
        800,
    );
    assert_eq!(
        capture_direct_scanout_candidate(&commands),
        Err(DirectScanoutSceneRejection::FormatNotOpaqueXrgb8888)
    );

    let _server = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn fullscreen_cropped_viewport_is_rejected_before_direct_scanout_import() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let _state = create_fullscreen_viewport_xrgb_dmabuf(
        &socket_path,
        &commands,
        Some((1.0 / 256.0, 0.0, 1279.0, 800.0)),
        Some((1280, 800)),
    )
    .unwrap();
    let rejection = capture_direct_scanout_candidate(&commands).unwrap_err();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(
        rejection,
        DirectScanoutSceneRejection::ViewportSourceNonIdentity
    );
}

#[test]
fn fullscreen_scaled_viewport_is_rejected_before_direct_scanout_import() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let _state =
        create_fullscreen_viewport_xrgb_dmabuf(&socket_path, &commands, None, Some((1279, 800)))
            .unwrap();
    set_focused_root_visual_geometry(
        &commands,
        SurfacePlacement::absolute_root_at(0, 0),
        1280,
        800,
    );
    let rejection = capture_direct_scanout_candidate(&commands).unwrap_err();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(
        rejection,
        DirectScanoutSceneRejection::ViewportDestinationNonIdentity
    );
}

#[test]
fn resolved_blur_scene_controls_direct_scanout_transition() {
    let socket_name = unique_socket_name();
    let mut server = OwnCompositorServer::bind_with_capabilities(
        &socket_name,
        true,
        InputProtocolCapabilities::desktop_baseline(),
        SelectionProtocolCapabilities::core_clipboard(),
        RendererProtocolCapabilities::qualified_native(),
    )
    .unwrap();
    server
        .state
        .blur_assignment
        .replace_config(crate::blur_policy::BlurPolicyConfig::default())
        .unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let _state = create_fullscreen_identity_viewport_xrgb_dmabuf(&socket_path, &commands).unwrap();
    assert_eq!(
        background_effect_count(&capture_resolved_effect_scene(&commands)),
        0
    );
    assert!(capture_direct_scanout_candidate(&commands).is_ok());

    replace_blur_policy_config(&commands, fullscreen_enable_rule_config());
    assert_eq!(
        background_effect_count(&capture_resolved_effect_scene(&commands)),
        1
    );
    assert_eq!(
        capture_direct_scanout_candidate(&commands),
        Err(DirectScanoutSceneRejection::EffectRequiresComposition)
    );

    replace_blur_policy_config(&commands, crate::blur_policy::BlurPolicyConfig::default());
    assert_eq!(
        background_effect_count(&capture_resolved_effect_scene(&commands)),
        0
    );
    assert!(capture_direct_scanout_candidate(&commands).is_ok());

    let _server = stop_controllable_test_server(commands, server_thread);
}
