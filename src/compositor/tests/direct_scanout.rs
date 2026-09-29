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

fn create_mapped_layer_shell_surface(
    socket_path: &PathBuf,
    layer: client_zwlr_layer_shell_v1::Layer,
    namespace: &str,
    width: usize,
    height: usize,
) -> Result<Connection, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let layer_shell: client_zwlr_layer_shell_v1::ZwlrLayerShellV1 = globals.bind(&qh, 4..=4, ())?;
    let mut client_state = RegistryTestState::default();

    let surface = compositor.create_surface(&qh, ());
    let layer_surface =
        layer_shell.get_layer_surface(&surface, None, layer, namespace.to_string(), &qh, ());
    layer_surface.set_anchor(
        client_zwlr_layer_surface_v1::Anchor::Top | client_zwlr_layer_surface_v1::Anchor::Left,
    );
    layer_surface.set_size(width as u32, height as u32);
    layer_surface.set_exclusive_zone(-1);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut client_state)?;
    commit_test_buffered_surface(&surface, &shm, &qh, width, height)?;
    connection.flush()?;
    queue.roundtrip(&mut client_state)?;
    Ok(connection)
}

fn settle_fullscreen_presentation(commands: &Sender<ServerCommand>) {
    let owner_root_surface_id = capture_fullscreen_render_plan_metrics(commands)
        .owner_root_surface_id
        .expect("fullscreen owner should be registered");
    settle_presentation_for_root(commands, owner_root_surface_id);
}

fn settle_active_root_presentation(commands: &Sender<ServerCommand>) {
    let owner_root_surface_id = capture_renderable_surface_snapshot(commands)
        .into_iter()
        .find(|surface| surface.parent_surface_id.is_none())
        .map(|surface| surface.surface_id)
        .expect("active root surface should be registered");
    settle_presentation_for_root(commands, owner_root_surface_id);
}

fn settle_presentation_for_root(commands: &Sender<ServerCommand>, owner_root_surface_id: u32) {
    commands
        .send(ServerCommand::CancelRootPresentationProperties {
            root_surface_id: owner_root_surface_id,
        })
        .unwrap();
    commands
        .send(ServerCommand::PublishTestPresentationAt {
            frame_id: 1,
            at: AnimationTime::from_nanos(u64::MAX),
        })
        .unwrap();
    wait_for_server_commands(commands);
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
    settle_fullscreen_presentation(&commands);
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
fn fullscreen_identity_viewport_xbgr_dmabuf_is_direct_scanout_candidate() {
    let socket_name = unique_socket_name();
    let mut probe =
        crate::compositor::gpu_protocol_capabilities::GpuProtocolProbe::valid_for_tests();
    let xbgr = crate::compositor::gpu_protocol_capabilities::GpuFormat::new(
        DrmFormat::XBGR8888_FOURCC,
        crate::render_backend::buffer::DrmModifier::LINEAR.0,
    );
    probe.importer_formats.push(xbgr);
    probe.feedback_format_table.push(xbgr);
    let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    server.enable_gpu_buffer_protocols_with_capabilities(
        crate::compositor::gpu_protocol_capabilities::GpuProtocolCapabilities::from_probe(probe),
    );
    server.set_dmabuf_feedback(
        crate::render_backend::egl_gles::EglGlesDmabufFeedback::from_formats([
            crate::render_backend::egl_gles::EglGlesDmabufFormat::new(
                DrmFormat::Xbgr8888,
                crate::render_backend::buffer::DrmModifier::LINEAR,
            ),
        ]),
        None,
        None,
    );
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let state = create_fullscreen_identity_viewport_xbgr_dmabuf(&socket_path, &commands).unwrap();
    settle_fullscreen_presentation(&commands);
    assert!(state.toplevel_has_state(client_xdg_toplevel::State::Fullscreen));
    let candidate = capture_direct_scanout_candidate(&commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert_eq!(candidate.format, DrmFormat::Xbgr8888);
}

#[test]
fn fullscreen_top_layer_shell_is_in_raw_scene_but_not_direct_scanout_presentation() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let _game = create_fullscreen_identity_viewport_xrgb_dmabuf(&socket_path, &commands).unwrap();
    settle_fullscreen_presentation(&commands);
    let metrics = capture_fullscreen_render_plan_metrics(&commands);
    assert!(metrics.fullscreen_composition_active);
    assert!(!metrics.fullscreen_transition_pending);
    let source_surface_id = capture_direct_scanout_candidate(&commands)
        .expect("baseline fullscreen candidate")
        .surface_id;
    assert!(set_direct_scanout_test_blur_effect(
        &commands,
        source_surface_id
    ));
    let baseline = capture_direct_scanout_candidate(&commands).unwrap();

    let layer_connections = vec![
        create_mapped_layer_shell_surface(
            &socket_path,
            client_zwlr_layer_shell_v1::Layer::Top,
            "test-bar-reserve",
            64,
            32,
        )
        .unwrap(),
        create_mapped_layer_shell_surface(
            &socket_path,
            client_zwlr_layer_shell_v1::Layer::Top,
            "test-bar-launcher",
            64,
            32,
        )
        .unwrap(),
        create_mapped_layer_shell_surface(
            &socket_path,
            client_zwlr_layer_shell_v1::Layer::Top,
            "test-bar-status",
            64,
            32,
        )
        .unwrap(),
        create_mapped_layer_shell_surface(
            &socket_path,
            client_zwlr_layer_shell_v1::Layer::Top,
            "test-dock",
            64,
            64,
        )
        .unwrap(),
    ];

    let raw_scene = capture_renderable_surface_snapshot(&commands);
    let top_ids = raw_scene
        .iter()
        .filter(|surface| surface.width == 64 && matches!(surface.height, 32 | 64))
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    assert_eq!(top_ids.len(), 4);
    assert!(set_direct_scanout_test_blur_effect(&commands, top_ids[0]));
    let presented_ids = capture_native_frame_surface_ids(&commands);
    let analysis = capture_direct_scanout_scene_analysis(&commands);
    let candidate = capture_direct_scanout_candidate(&commands).unwrap();
    drop(layer_connections);
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(
        top_ids
            .iter()
            .all(|surface_id| !presented_ids.contains(surface_id))
    );
    assert!(analysis.coverage.visible_content_above.is_empty());
    assert!(
        !analysis
            .blockers
            .reasons()
            .contains(&DirectScanoutSceneRejection::OverlayVisible)
    );
    assert_eq!(candidate.surface_id, baseline.surface_id);
    assert_eq!(candidate.root_surface_id, baseline.root_surface_id);
    assert_eq!(
        candidate.surface_scene_node_id,
        baseline.surface_scene_node_id
    );
    assert_eq!(
        candidate.window_scene_node_id,
        baseline.window_scene_node_id
    );
    assert_eq!(candidate.generation, baseline.generation);
    assert_eq!(
        candidate.surface_presentation_generation,
        baseline.surface_presentation_generation
    );
    assert!(analysis.effects.instances.iter().any(|effect| {
        effect.anchor == EffectAnchor::BeforeSurface(top_ids[0])
            && effect.disposition == DirectScanoutEffectDisposition::PresentationCulled
    }));
    assert!(analysis.effects.instances.iter().any(|effect| {
        effect.anchor == EffectAnchor::BeforeSurface(source_surface_id)
            && effect.disposition == DirectScanoutEffectDisposition::OccludedByOpaqueScanoutSource
    }));
    assert!(!analysis.effects.requires_composition);
}

#[test]
fn fullscreen_top_layer_filter_keeps_the_subsurface_scanout_source_aligned() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (_, root_surface_id, child_surface_id) =
        create_fullscreen_dmabuf_subsurface_scanout_source(&socket_path, &commands).unwrap();
    settle_fullscreen_presentation(&commands);
    let baseline = capture_direct_scanout_candidate(&commands).unwrap();
    let _top_connection = create_mapped_layer_shell_surface(
        &socket_path,
        client_zwlr_layer_shell_v1::Layer::Top,
        "subsurface-scanout-top-layer",
        64,
        32,
    )
    .unwrap();
    let raw_scene = capture_renderable_surface_snapshot(&commands);
    let top_surface_id = raw_scene
        .iter()
        .find(|surface| surface.width == 64 && surface.height == 32)
        .expect("Top layer should remain in the raw scene")
        .surface_id;
    let presented_ids = capture_native_frame_surface_ids(&commands);
    let analysis = capture_direct_scanout_scene_analysis(&commands);
    let candidate = capture_direct_scanout_candidate(&commands).unwrap();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(
        raw_scene
            .iter()
            .any(|surface| surface.surface_id == top_surface_id)
    );
    assert!(!presented_ids.contains(&top_surface_id));
    assert!(analysis.coverage.visible_content_above.is_empty());
    let group = analysis
        .coverage
        .covering_application_group
        .expect("fullscreen owner group remains output covering");
    assert_eq!(
        group.root_surface_id,
        root_surface_id,
        "raw={:?} child={} candidate={:?} coverage_group={:?}",
        raw_scene
            .iter()
            .map(|surface| (surface.surface_id, surface.parent_surface_id))
            .collect::<Vec<_>>(),
        child_surface_id,
        candidate,
        group
    );
    assert_eq!(
        group
            .covering_surface
            .as_ref()
            .map(|surface| surface.surface_id),
        Some(child_surface_id)
    );
    assert_eq!(candidate.surface_id, child_surface_id);
    assert_eq!(candidate.root_surface_id, root_surface_id);
    assert_eq!(candidate.surface_id, baseline.surface_id);
    assert_eq!(candidate.root_surface_id, baseline.root_surface_id);
    assert_eq!(
        candidate.surface_scene_node_id,
        baseline.surface_scene_node_id
    );
    assert_eq!(
        candidate.window_scene_node_id,
        baseline.window_scene_node_id
    );
    assert_eq!(
        candidate.surface_presentation_generation,
        baseline.surface_presentation_generation
    );
}

#[test]
fn fullscreen_overlay_layer_shell_remains_visible_and_blocks_direct_scanout() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let _game = create_fullscreen_identity_viewport_xrgb_dmabuf(&socket_path, &commands).unwrap();
    settle_fullscreen_presentation(&commands);
    let _layer_connection = create_mapped_layer_shell_surface(
        &socket_path,
        client_zwlr_layer_shell_v1::Layer::Overlay,
        "critical-overlay",
        100,
        100,
    )
    .unwrap();
    let analysis = capture_direct_scanout_scene_analysis(&commands);
    let raw_scene = capture_renderable_surface_snapshot(&commands);
    let overlay_id = raw_scene
        .iter()
        .find(|surface| surface.width == 100 && surface.height == 100)
        .expect("overlay should be in the raw scene")
        .surface_id;
    let presented_ids = capture_native_frame_surface_ids(&commands);
    let rejection = capture_direct_scanout_candidate(&commands).unwrap_err();
    let _server = stop_controllable_test_server(commands, server_thread);

    assert!(
        raw_scene
            .iter()
            .any(|surface| surface.surface_id == overlay_id)
    );
    assert!(presented_ids.contains(&overlay_id));
    assert!(
        analysis
            .coverage
            .visible_content_above
            .iter()
            .any(|content| {
                content.root_surface_id == overlay_id
                    && content.kind == PresentationCoverageContentKind::LayerShell
            })
    );
    assert_eq!(rejection, DirectScanoutSceneRejection::OverlayVisible);
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
    settle_active_root_presentation(&commands);
    let eligibility = capture_fullscreen_presentation_eligibility(&commands);
    assert_eq!(
        eligibility.rejection,
        Some(FullscreenPresentationRejection::NoFullscreenOwner)
    );
    let baseline = capture_direct_scanout_candidate(&commands).unwrap();

    let _layer_connection = create_mapped_layer_shell_surface(
        &socket_path,
        client_zwlr_layer_shell_v1::Layer::Top,
        "normal-scene-top-layer",
        64,
        32,
    )
    .unwrap();
    let analysis = capture_direct_scanout_scene_analysis(&commands);
    let raw_scene = capture_renderable_surface_snapshot(&commands);
    let top_id = raw_scene
        .iter()
        .find(|surface| surface.width == 64 && surface.height == 32)
        .expect("Top layer should be in the raw scene")
        .surface_id;
    let rejection = capture_direct_scanout_candidate(&commands).unwrap_err();

    let _server = stop_controllable_test_server(commands, server_thread);

    assert_ne!(baseline.surface_id, 0);
    assert!(raw_scene.iter().any(|surface| surface.surface_id == top_id));
    assert!(
        analysis
            .coverage
            .visible_content_above
            .iter()
            .any(|content| {
                content.root_surface_id == top_id
                    && content.kind == PresentationCoverageContentKind::LayerShell
            })
    );
    assert_eq!(rejection, DirectScanoutSceneRejection::OverlayVisible);
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
    settle_active_root_presentation(&commands);
    assert_eq!(
        capture_direct_scanout_candidate(&commands),
        Err(DirectScanoutSceneRejection::FormatNotProvenOpaque)
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
    settle_fullscreen_presentation(&commands);
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
    set_focused_root_renderable_size(&commands, 1280, 800);
    settle_fullscreen_presentation(&commands);
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
    settle_fullscreen_presentation(&commands);
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
    assert!(capture_direct_scanout_candidate(&commands).is_ok());

    replace_blur_policy_config(&commands, crate::blur_policy::BlurPolicyConfig::default());
    assert_eq!(
        background_effect_count(&capture_resolved_effect_scene(&commands)),
        0
    );
    assert!(capture_direct_scanout_candidate(&commands).is_ok());

    let _server = stop_controllable_test_server(commands, server_thread);
}
