use super::*;

#[test]
fn fresh_restore_freezes_ssd_until_physical_settlement() {
    let (mut state, window_id) = ssd_test_state(401);
    state.lifecycle_animation_renderer_available = Some(true);
    let root_surface_id = 401;
    let group = visual_group(rect(384.0, 60.0, 832.0, 640.0));
    let decoration_a = state
        .native_decoration_render_instances_for_scale(&state.renderable_surfaces, 1.0)
        .into_iter()
        .next()
        .expect("authoritative SSD snapshot A");

    state.begin_lifecycle_restore(
        window_id,
        root_surface_id,
        Some(group),
        ResolvedEffectScene::new(41, Vec::new()),
        vec![decoration_a.clone()],
    );
    let first_identity = active_lifecycle_identity(
        &state,
        state
            .scene_node_id_for_window_group(window_id)
            .expect("window group SceneNode"),
    );
    let first_payload = state
        .retained_lifecycle_payloads
        .get_exact(first_identity)
        .expect("fresh retained payload captures SSD");
    assert_eq!(
        first_payload
            .frozen_decoration
            .as_ref()
            .expect("fresh SSD frozen in payload")
            .scene_snapshot()
            .visual_signature(),
        decoration_a.scene_snapshot().visual_signature()
    );

    assert_eq!(
        lifecycle_decoration_signature(
            &state,
            root_surface_id,
            &state.renderable_surfaces,
            AnimationTime::monotonic_now().unwrap(),
        ),
        Some(decoration_a.scene_snapshot().visual_signature())
    );

    state.focused_window_id = Some(window_id);
    let decoration_b = state
        .native_decoration_render_instances_for_scale(&state.renderable_surfaces, 1.0)
        .into_iter()
        .next()
        .expect("live SSD snapshot B");
    assert_ne!(
        decoration_a.scene_snapshot().visual_signature(),
        decoration_b.scene_snapshot().visual_signature()
    );
    assert_eq!(
        lifecycle_decoration_signature(
            &state,
            root_surface_id,
            &state.renderable_surfaces,
            AnimationTime::monotonic_now().unwrap(),
        ),
        Some(decoration_a.scene_snapshot().visual_signature())
    );

    settle_lifecycle_transition(&mut state, window_id);
    assert!(
        state
            .retained_lifecycle_payloads
            .get_exact(first_identity)
            .is_none()
    );
}

#[test]
fn reversal_preserves_existing_frozen_ssd_snapshot() {
    let (mut state, window_id) = ssd_test_state(402);
    state.lifecycle_animation_renderer_available = Some(true);
    let root_surface_id = 402;
    let group_a = visual_group(rect(384.0, 60.0, 832.0, 640.0));
    let group_b = visual_group(rect(384.0, 20.0, 832.0, 680.0));
    let decoration_a = lifecycle_decoration(window_id, root_surface_id, 0x31);
    let decoration_b = lifecycle_decoration(window_id, root_surface_id, 0x32);

    state.begin_lifecycle_minimize(
        window_id,
        root_surface_id,
        Some(rect(400.0, 100.0, 800.0, 600.0)),
        Some(rect(400.0, 100.0, 800.0, 600.0)),
        Some(group_a),
        ResolvedEffectScene::default(),
        vec![decoration_a.clone()],
    );
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group SceneNode");
    let old_identity = active_lifecycle_identity(&state, scene_node_id);
    let original_payload = std::sync::Arc::clone(
        state
            .retained_lifecycle_payloads
            .get_exact(old_identity)
            .expect("fresh immutable payload"),
    );
    state
        .renderable_surfaces
        .iter_mut()
        .find(|surface| surface.surface_id == root_surface_id)
        .expect("live root surface")
        .placement = SurfacePlacement::root_at(55, 66);
    let new_child_id = root_surface_id + 1;
    let mut new_child = ssd_test_surface(new_child_id);
    new_child.placement = SurfacePlacement::subsurface(root_surface_id, 90, 91);
    state.append_renderable_surface(new_child);
    state
        .surface_presentation_generations
        .insert(new_child_id, 1);
    let before = state
        .window_lifecycle_animator
        .sample(
            old_identity,
            AnimationTime::monotonic_now().expect("monotonic time"),
        )
        .expect("minimize sample");
    state.begin_lifecycle_restore(
        window_id,
        root_surface_id,
        Some(group_b),
        ResolvedEffectScene::new(42, Vec::new()),
        vec![decoration_b],
    );
    let new_identity = active_lifecycle_identity(&state, scene_node_id);
    let after = state
        .window_lifecycle_animator
        .sample(
            new_identity,
            AnimationTime::monotonic_now().expect("monotonic time"),
        )
        .expect("restore sample");

    assert_eq!(after.presentation_identity, new_identity);
    assert_eq!(new_identity.scene_node_id(), scene_node_id);
    assert_ne!(new_identity.transaction_id(), old_identity.transaction_id());
    assert_ne!(new_identity.revision_id(), old_identity.revision_id());
    assert!((after.progress - before.progress).abs() < 0.01);
    let reversed_payload = state
        .retained_lifecycle_payloads
        .get_exact(new_identity)
        .expect("reversed lifecycle payload");
    assert_eq!(reversed_payload.payload_id, original_payload.payload_id);
    assert!(std::sync::Arc::ptr_eq(reversed_payload, &original_payload));
    assert_eq!(reversed_payload.visual_group, group_a);
    assert_eq!(reversed_payload.root_surface_id, root_surface_id);
    assert_eq!(reversed_payload.window_id, window_id);
    assert_eq!(reversed_payload.surface_presentation.nodes.len(), 1);
    assert_eq!(
        reversed_payload.surface_presentation.nodes[0].placement,
        SurfacePlacement::root()
    );
    assert_eq!(
        reversed_payload
            .frozen_decoration
            .as_ref()
            .expect("reversal retains exact frozen SSD")
            .scene_snapshot()
            .visual_signature(),
        decoration_a.scene_snapshot().visual_signature()
    );
    assert!(std::sync::Arc::ptr_eq(
        &reversed_payload.effect_scene,
        &original_payload.effect_scene
    ));
    let projected = state.lifecycle_renderable_surfaces(
        &state.lifecycle_scene_sample_at(AnimationTime::monotonic_now().expect("sample")),
    );
    assert_eq!(
        projected
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>(),
        vec![root_surface_id]
    );
    assert_eq!(projected[0].placement, SurfacePlacement::root());
    assert!(
        state
            .presentation_animator
            .transaction_record(old_identity.transaction_id())
            .is_none()
    );
    assert!(
        state
            .presentation_animator
            .transaction_record(new_identity.transaction_id())
            .is_some()
    );

    assert_eq!(
        lifecycle_decoration_signature(
            &state,
            root_surface_id,
            &[],
            AnimationTime::monotonic_now().unwrap(),
        ),
        Some(decoration_a.scene_snapshot().visual_signature())
    );
    assert_eq!(
        state
            .lifecycle_visual_group_for_scene_node(scene_node_id)
            .expect("reversed transition")
            .canonical_visual_rect,
        group_a.canonical_visual_rect
    );
}

#[test]
fn frozen_ssd_resolution_requires_the_exact_payload_identity() {
    let mut state = CompositorState::new(None);
    state.lifecycle_animation_renderer_available = Some(true);
    let window_a = state.allocate_window_id().expect("first window ID");
    let window_b = state.allocate_window_id().expect("second window ID");
    let root_a = 421;
    let root_b = 422;
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_a, root_a))
        .expect("first XDG window");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_b, root_b))
        .expect("second XDG window");
    state.append_renderable_surface(ssd_test_surface(root_a));
    state.append_renderable_surface(ssd_test_surface(root_b));
    state.surface_presentation_generations.insert(root_a, 1);
    state.surface_presentation_generations.insert(root_b, 1);
    state.rebuild_active_scene_view();
    let group = visual_group(rect(384.0, 60.0, 832.0, 640.0));
    let decoration_a = lifecycle_decoration(window_a, root_a, 0x71);
    let decoration_b = lifecycle_decoration(window_b, root_b, 0x72);

    state.begin_lifecycle_restore(
        window_a,
        root_a,
        Some(group),
        ResolvedEffectScene::default(),
        vec![decoration_a.clone()],
    );
    state.begin_lifecycle_restore(
        window_b,
        root_b,
        Some(group),
        ResolvedEffectScene::default(),
        vec![decoration_b.clone()],
    );
    let mut sample = state.lifecycle_scene_sample_at(
        AnimationTime::monotonic_now().expect("sample both lifecycle owners"),
    );
    let index_a = sample
        .lamps
        .iter()
        .position(|lamp| lamp.root_surface_id == root_a)
        .expect("first lifecycle sample");
    let identity_b = sample
        .lamps
        .iter()
        .find(|lamp| lamp.root_surface_id == root_b)
        .expect("second lifecycle sample")
        .presentation_identity;
    // Preserve A's matching root, WindowId, and payload ID while selecting B's
    // active retained identity. Root/WindowId lookup must not return SSD A.
    sample.lamps[index_a].presentation_identity = identity_b;

    let resolved = state.lifecycle_decoration_render_instances(&sample, &[]);

    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].root_surface_id(), root_b);
    assert_eq!(
        resolved[0].scene_snapshot().visual_signature(),
        decoration_b.scene_snapshot().visual_signature()
    );
    assert_ne!(
        resolved[0].scene_snapshot().visual_signature(),
        decoration_a.scene_snapshot().visual_signature()
    );
}

#[test]
fn missing_previous_payload_rejects_reversal_without_mutating_old_chain() {
    let (mut state, window_id) = ssd_test_state(414);
    state.lifecycle_animation_renderer_available = Some(true);
    let root_surface_id = 414;
    let group = visual_group(rect(384.0, 60.0, 832.0, 640.0));
    state.begin_lifecycle_minimize(
        window_id,
        root_surface_id,
        Some(rect(400.0, 100.0, 800.0, 600.0)),
        Some(rect(400.0, 100.0, 800.0, 600.0)),
        Some(group),
        ResolvedEffectScene::new(414, Vec::new()),
        vec![lifecycle_decoration(window_id, root_surface_id, 0x51)],
    );
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group SceneNode");
    let old_identity = active_lifecycle_identity(&state, scene_node_id);
    let old_member = state
        .presentation_animator
        .transaction_record(old_identity.transaction_id())
        .expect("old ledger member")
        .members()[0];
    let now = AnimationTime::monotonic_now().expect("monotonic test time");
    let before = state
        .window_lifecycle_animator
        .sample(old_identity, now)
        .expect("old motion state");
    let removed_payload = std::sync::Arc::clone(
        state
            .retained_lifecycle_payloads
            .get_exact(old_identity)
            .expect("old immutable payload before deliberate removal"),
    );
    let original_payload_decoration = removed_payload
        .frozen_decoration
        .as_ref()
        .expect("old frozen SSD")
        .scene_snapshot()
        .visual_signature();
    assert!(
        state
            .retained_lifecycle_payloads
            .retire_exact(old_identity)
            .is_some()
    );

    state.begin_lifecycle_restore(
        window_id,
        root_surface_id,
        Some(visual_group(rect(384.0, 20.0, 832.0, 680.0))),
        ResolvedEffectScene::new(999, Vec::new()),
        vec![lifecycle_decoration(window_id, root_surface_id, 0x52)],
    );

    assert_eq!(
        state.presentation_animator.active_retained_visual(
            scene_node_id,
            PresentationRetainedVisualKind::WindowLifecycle
        ),
        Some(old_identity)
    );
    assert_eq!(
        state.window_lifecycle_animator.sample(old_identity, now),
        Some(before)
    );
    assert!(
        state
            .presentation_animator
            .transaction_record(old_identity.transaction_id())
            .is_some_and(|record| record.members()[0] == old_member)
    );
    assert_eq!(state.presentation_animator.transaction_count(), 1);
    assert_eq!(state.window_lifecycle_animator.active_count(), 1);
    assert!(
        state
            .retained_lifecycle_payloads
            .get_exact(old_identity)
            .is_none()
    );
    assert_eq!(
        original_payload_decoration,
        removed_payload
            .frozen_decoration
            .as_ref()
            .expect("old payload remains immutable")
            .scene_snapshot()
            .visual_signature()
    );
}

#[test]
fn later_independent_restore_replaces_settled_ssd_snapshot() {
    let (mut state, window_id) = ssd_test_state(403);
    state.lifecycle_animation_renderer_available = Some(true);
    let root_surface_id = 403;
    let group = visual_group(rect(384.0, 60.0, 832.0, 640.0));
    let decoration_a = lifecycle_decoration(window_id, root_surface_id, 0x41);
    let decoration_b = lifecycle_decoration(window_id, root_surface_id, 0x42);

    state.begin_lifecycle_restore(
        window_id,
        root_surface_id,
        Some(group),
        ResolvedEffectScene::default(),
        vec![decoration_a.clone()],
    );
    let old_identity = active_lifecycle_identity(
        &state,
        state
            .scene_node_id_for_window_group(window_id)
            .expect("window group SceneNode"),
    );
    let old_payload_id = state
        .retained_lifecycle_payloads
        .get_exact(old_identity)
        .expect("first independent payload")
        .payload_id;
    let old_payload = std::sync::Arc::clone(
        state
            .retained_lifecycle_payloads
            .get_exact(old_identity)
            .expect("first independent payload"),
    );
    settle_lifecycle_transition(&mut state, window_id);
    assert!(
        state
            .retained_lifecycle_payloads
            .get_exact(old_identity)
            .is_none()
    );
    state
        .renderable_surfaces
        .iter_mut()
        .find(|surface| surface.surface_id == root_surface_id)
        .expect("current root surface")
        .width = 420;
    let new_child_id = root_surface_id + 1;
    let mut new_child = ssd_test_surface(new_child_id);
    new_child.width = 88;
    new_child.height = 77;
    new_child.placement = SurfacePlacement::subsurface(root_surface_id, -48, 72);
    state.append_renderable_surface(new_child);
    state
        .surface_presentation_generations
        .insert(new_child_id, 2);

    state.begin_lifecycle_restore(
        window_id,
        root_surface_id,
        Some(group),
        ResolvedEffectScene::default(),
        vec![decoration_b.clone()],
    );
    let new_identity = active_lifecycle_identity(
        &state,
        state
            .scene_node_id_for_window_group(window_id)
            .expect("window group SceneNode"),
    );
    let new_payload_id = state
        .retained_lifecycle_payloads
        .get_exact(new_identity)
        .expect("fresh independent payload")
        .payload_id;
    assert_ne!(old_payload_id, new_payload_id);
    let fresh_payload = state
        .retained_lifecycle_payloads
        .get_exact(new_identity)
        .expect("fresh independent payload captures current topology");
    assert_eq!(fresh_payload.surface_presentation.nodes.len(), 2);
    assert_eq!(fresh_payload.surface_presentation.nodes[0].width, 420);
    assert_eq!(
        fresh_payload.surface_presentation.nodes[1].placement,
        SurfacePlacement::subsurface(root_surface_id, -48, 72)
    );
    assert_eq!(old_payload.surface_presentation.nodes.len(), 1);
    assert_eq!(
        state
            .retained_lifecycle_payloads
            .get_exact(new_identity)
            .expect("fresh independent payload")
            .frozen_decoration
            .as_ref()
            .expect("new chain freezes SSD B")
            .scene_snapshot()
            .visual_signature(),
        decoration_b.scene_snapshot().visual_signature()
    );
    assert_eq!(
        lifecycle_decoration_signature(
            &state,
            root_surface_id,
            &[],
            AnimationTime::monotonic_now().unwrap(),
        ),
        Some(decoration_b.scene_snapshot().visual_signature())
    );
}

#[test]
fn fresh_csd_restore_does_not_synthesize_frozen_ssd() {
    let (mut state, window_id) = ssd_test_state(404);
    state.lifecycle_animation_renderer_available = Some(true);
    let root_surface_id = 404;
    let group = visual_group(rect(400.0, 100.0, 800.0, 600.0));
    state.begin_lifecycle_restore(
        window_id,
        root_surface_id,
        Some(group),
        ResolvedEffectScene::default(),
        vec![lifecycle_decoration(window_id, root_surface_id, 0x51)],
    );
    let previous_identity = active_lifecycle_identity(
        &state,
        state
            .scene_node_id_for_window_group(window_id)
            .expect("window group SceneNode"),
    );
    settle_lifecycle_transition(&mut state, window_id);
    assert!(
        state
            .retained_lifecycle_payloads
            .get_exact(previous_identity)
            .is_none()
    );

    state.begin_lifecycle_restore(
        window_id,
        root_surface_id,
        Some(group),
        ResolvedEffectScene::default(),
        Vec::new(),
    );

    let current_identity = active_lifecycle_identity(
        &state,
        state
            .scene_node_id_for_window_group(window_id)
            .expect("window group SceneNode"),
    );
    assert_ne!(previous_identity, current_identity);
    assert!(
        state
            .retained_lifecycle_payloads
            .get_exact(current_identity)
            .expect("fresh CSD payload")
            .frozen_decoration
            .is_none()
    );
    let sample = state.lifecycle_scene_sample_at(AnimationTime::monotonic_now().unwrap());
    assert!(
        state
            .lifecycle_decoration_render_instances(&sample, &[])
            .is_empty()
    );
}
