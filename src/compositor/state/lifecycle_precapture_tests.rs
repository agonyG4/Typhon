use super::*;

#[test]
fn lifecycle_precapture_candidates_clone_only_the_captured_surface_tree() {
    let generation = crate::xwayland::XwaylandGeneration::new(
        std::num::NonZeroU64::new(388).expect("XWayland generation"),
    );
    let root_surface_id = 3_881;
    let mut state = CompositorState::new(None);
    let window_id = super::desktop_window_tests::insert_x11(
        &mut state,
        super::desktop_window_tests::x11_snapshot(generation, 3_880, root_surface_id),
    );
    let mut child = ssd_test_surface(root_surface_id + 1);
    child.placement = SurfacePlacement::subsurface(root_surface_id, 8, 12);
    state.append_renderable_surface(ssd_test_surface(root_surface_id));
    state.append_renderable_surface(child);
    state.append_renderable_surface(ssd_test_surface(root_surface_id + 2));

    let candidates = state.lifecycle_surface_candidates(window_id, root_surface_id);

    assert_eq!(
        candidates
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>(),
        vec![root_surface_id, root_surface_id + 1],
        "unrelated globally renderable surfaces are not cloned into the lease candidate set"
    );
}

#[test]
fn xwayland_withdrawal_keeps_the_admission_source_for_first_capture() {
    let generation = crate::xwayland::XwaylandGeneration::new(
        std::num::NonZeroU64::new(390).expect("XWayland generation"),
    );
    let root_surface_id = 3_901;
    let snapshot = super::desktop_window_tests::x11_snapshot(generation, 3_900, root_surface_id);
    let mut state = CompositorState::new(None);
    state.lifecycle_animation_renderer_available = Some(true);
    super::window_open_animation_tests::set_window_open_preset(
        &mut state,
        crate::animation_control::AnimationPreset::Astrea,
    );
    let window_id = super::desktop_window_tests::insert_x11(&mut state, snapshot);
    let source = ssd_test_surface(root_surface_id);
    let admitted_buffer_id = source.buffer_id();
    state.append_renderable_surface(source);
    state
        .surface_presentation_generations
        .insert(root_surface_id, 1);
    state.rebuild_active_scene_view();

    assert!(state.minimize_desktop_window(window_id));
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("minimized X11 window scene node");
    let identity = active_lifecycle_identity(&state, scene_node_id);
    assert!(
        state
            .retained_lifecycle_payloads
            .get_exact(identity)
            .is_some()
    );
    assert!(state.renderable_surfaces.is_empty());
    assert_eq!(
        state
            .window(window_id)
            .expect("minimized X11 window")
            .state
            .minimized_surface(root_surface_id)
            .expect("source moved into minimized state")
            .buffer_id(),
        admitted_buffer_id
    );

    state.retire_xwayland_attachment(root_surface_id);
    assert!(
        !state
            .surface_presentation_generations
            .contains_key(&root_surface_id),
        "withdrawal must retire the live presentation generation"
    );
    assert!(
        state
            .window(window_id)
            .expect("X11 window remains semantically minimized")
            .state
            .minimized_surface(root_surface_id)
            .is_none(),
        "withdrawal must remove the dead source from live minimized state"
    );

    let sample = state
        .lifecycle_scene_sample_at(AnimationTime::monotonic_now().expect("lifecycle sample time"));
    let frozen = state.lifecycle_renderable_surfaces(&sample);
    assert_eq!(
        frozen.len(),
        1,
        "first capture still receives one exact source"
    );
    assert_eq!(frozen[0].surface_id, root_surface_id);
    assert_eq!(frozen[0].buffer_id(), admitted_buffer_id);
    assert!(
        state
            .renderable_surfaces
            .iter()
            .all(|surface| surface.surface_id != root_surface_id),
        "the frozen source must not re-enter ordinary scene rendering"
    );

    let admitted_payload_id = sample.samples[0].payload_id;
    assert!(state.restore_minimized_desktop_window(window_id));
    let reversed =
        state.lifecycle_scene_sample_at(AnimationTime::monotonic_now().expect("restore sample"));
    let reverse_sample = reversed.samples.first().expect("reversed lifecycle sample");
    assert_eq!(reverse_sample.payload_id, admitted_payload_id);
    assert_ne!(reverse_sample.presentation_identity, identity);
    assert_eq!(
        reverse_sample.direction,
        crate::window_lifecycle_animation::LifecycleDirection::Restore
    );
    assert!(
        state
            .retained_lifecycle_payloads
            .get_entry_exact(reverse_sample.presentation_identity)
            .is_some_and(|entry| entry.pre_capture.is_some())
    );
    let reversed_source = state.lifecycle_renderable_surfaces(&reversed);
    assert_eq!(reversed_source.len(), 1);
    assert_eq!(reversed_source[0].buffer_id(), admitted_buffer_id);
    assert!(state.pending_xwayland_restores.is_empty());
    assert!(
        state
            .renderable_surfaces
            .iter()
            .all(|surface| surface.surface_id != root_surface_id)
    );
}

#[test]
fn xwayland_generation_end_retires_the_precapture_entry_before_capture() {
    let generation = crate::xwayland::XwaylandGeneration::new(
        std::num::NonZeroU64::new(389).expect("XWayland generation"),
    );
    let root_surface_id = 3_891;
    let snapshot = super::desktop_window_tests::x11_snapshot(generation, 3_890, root_surface_id);
    let mut state = CompositorState::new(None);
    state.lifecycle_animation_renderer_available = Some(true);
    super::window_open_animation_tests::set_window_open_preset(
        &mut state,
        crate::animation_control::AnimationPreset::Astrea,
    );
    let window_id = super::desktop_window_tests::insert_x11(&mut state, snapshot);
    state.append_renderable_surface(ssd_test_surface(root_surface_id));
    state
        .surface_presentation_generations
        .insert(root_surface_id, 1);
    state.rebuild_active_scene_view();

    assert!(state.minimize_desktop_window(window_id));
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("minimized X11 window scene node");
    let identity = active_lifecycle_identity(&state, scene_node_id);
    assert!(
        state
            .retained_lifecycle_payloads
            .get_entry_exact(identity)
            .is_some_and(|entry| entry.pre_capture.is_some())
    );
    let stale_sample =
        state.lifecycle_scene_sample_at(AnimationTime::monotonic_now().expect("sample time"));

    state.clear_xwayland_generation(generation);

    assert!(
        state
            .retained_lifecycle_payloads
            .get_entry_exact(identity)
            .is_none(),
        "generation teardown retires the lifecycle entry and frozen source"
    );
    assert!(state.pending_xwayland_restores.is_empty());
    assert!(
        state
            .lifecycle_scene_sample_at(AnimationTime::monotonic_now().expect("sample time"))
            .samples
            .is_empty()
    );
    assert!(
        state
            .lifecycle_renderable_surfaces(&stale_sample)
            .is_empty()
    );
}

#[test]
fn xwayland_precapture_transfers_the_exact_dmabuf_release_to_the_capture_frame() {
    let generation = crate::xwayland::XwaylandGeneration::new(
        std::num::NonZeroU64::new(391).expect("XWayland generation"),
    );
    let root_surface_id = 3_911;
    let snapshot = super::desktop_window_tests::x11_snapshot(generation, 3_910, root_surface_id);
    let mut state = CompositorState::new(None);
    state.lifecycle_animation_renderer_available = Some(true);
    super::window_open_animation_tests::set_window_open_preset(
        &mut state,
        crate::animation_control::AnimationPreset::Astrea,
    );
    let window_id = super::desktop_window_tests::insert_x11(&mut state, snapshot);
    let source = super::desktop_window_tests::x11_scanout_surface(
        root_surface_id,
        300,
        200,
        SurfacePlacement::absolute_root_at(30, 40),
        crate::render_backend::buffer::DrmFormat::Xrgb8888,
    );
    let child_surface_ids = [root_surface_id + 1, root_surface_id + 2];
    let children = child_surface_ids.map(|surface_id| {
        super::desktop_window_tests::x11_scanout_surface(
            surface_id,
            96,
            72,
            SurfacePlacement::subsurface(root_surface_id, 8, 12),
            crate::render_backend::buffer::DrmFormat::Xrgb8888,
        )
    });
    let mut sources = vec![source];
    sources.extend(children);
    let obligations = sources
        .iter()
        .enumerate()
        .map(
            |(index, surface)| crate::compositor::state_data::DmabufReleaseObligation {
                buffer_id: surface.buffer_id(),
                release: crate::compositor::state_data::SurfaceBufferRelease::ExplicitSync(
                    crate::compositor::ExplicitSyncPoint::for_tests_with_signal_script(
                        391 + index as u32,
                        surface.buffer_id().get(),
                        [true],
                    ),
                ),
            },
        )
        .collect::<Vec<_>>();
    for (index, surface) in sources.into_iter().enumerate() {
        let surface_id = surface.surface_id;
        state.append_renderable_surface(surface);
        state.surface_presentation_generations.insert(surface_id, 1);
        state
            .active_dmabuf_buffers
            .insert(surface_id, obligations[index].clone());
        if surface_id != root_surface_id {
            state.surface_topology.set_placement(
                surface_id,
                SurfacePlacement::subsurface(root_surface_id, 8, 12),
            );
        }
    }
    state.rebuild_active_scene_view();

    assert!(state.minimize_desktop_window(window_id));
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("minimized X11 scene node");
    let identity = active_lifecycle_identity(&state, scene_node_id);
    assert_eq!(state.active_dmabuf_buffers.len(), 0);
    assert_eq!(state.lifecycle_precapture_dmabuf_release_count(), 3);
    for obligation in &obligations {
        assert!(
            state
                .retained_lifecycle_payloads
                .holds_release_token(obligation)
        );
    }
    assert_eq!(state.pending_dmabuf_buffer_releases.len(), 0);

    state.retire_xwayland_attachment(root_surface_id);
    for obligation in &obligations {
        assert!(
            state
                .retained_lifecycle_payloads
                .holds_release_token(obligation)
        );
    }
    assert_eq!(state.pending_dmabuf_buffer_releases.len(), 0);

    let sample =
        state.lifecycle_scene_sample_at(AnimationTime::monotonic_now().expect("sample time"));
    let captured_source = state.lifecycle_renderable_surfaces(&sample);
    assert_eq!(captured_source.len(), 3);
    for obligation in &obligations {
        assert!(
            captured_source
                .iter()
                .any(|surface| { surface.buffer_id() == obligation.buffer_id })
        );
    }

    let lifecycle_sample = sample.samples.first().expect("active lifecycle sample");
    let evidence = LifecycleRenderEvidence::from_consumed([LifecycleRenderEvidenceEntry {
        window_id: lifecycle_sample.window_id,
        root_surface_id: lifecycle_sample.root_surface_id,
        presentation_identity: lifecycle_sample.presentation_identity,
        payload_id: lifecycle_sample.payload_id,
    }]);
    let rendered_snapshot = LifecycleFrameSnapshot::qualified_from_sample(&sample, &evidence);
    let batch_id = state.take_frame_batch_for_render(391);
    assert_eq!(
        state.settle_lifecycle_precapture_for_rendered_frame(batch_id, &rendered_snapshot),
        3,
        "successful renderer evidence transfers every exact source release token"
    );
    assert!(
        state
            .retained_lifecycle_payloads
            .get_entry_exact(identity)
            .is_some_and(|entry| entry.pre_capture.is_none())
    );
    let releases = &state
        .frame_batches
        .get(&batch_id)
        .expect("capture frame batch")
        .dmabuf_releases_to_complete_on_present;
    assert_eq!(releases.len(), 3);
    for obligation in &obligations {
        assert!(
            releases
                .iter()
                .any(|release| release.same_release_token(obligation))
        );
        assert!(state.buffer_release_is_owned(obligation));
    }

    let gpu_lease_id = crate::compositor::DmabufGpuReleaseLeaseId::new(
        std::num::NonZeroU64::new(391).expect("GPU release lease ID"),
    );
    assert_eq!(
        state.transfer_frame_batch_dmabuf_releases_to_gpu_lease(batch_id, gpu_lease_id),
        3
    );
    assert!(state.dmabuf_gpu_release_leases.contains_key(&gpu_lease_id));
    assert_eq!(state.complete_dmabuf_gpu_release_lease(gpu_lease_id), 3);
    for obligation in &obligations {
        assert!(!state.buffer_release_is_owned(obligation));
    }
    assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 3);

    let settled_at = AnimationTime::monotonic_now().expect("settled minimize time");
    assert!(
        state
            .window_lifecycle_animator
            .snap_to_endpoint(identity, settled_at)
    );
    let settled_sample = state.lifecycle_scene_sample_at(settled_at);
    let settled = settled_sample
        .samples
        .first()
        .expect("settled minimize sample");
    let evidence = LifecycleRenderEvidence::from_consumed([LifecycleRenderEvidenceEntry {
        window_id: settled.window_id,
        root_surface_id: settled.root_surface_id,
        presentation_identity: settled.presentation_identity,
        payload_id: settled.payload_id,
    }]);
    let presented = LifecycleFrameSnapshot::qualified_from_sample(&settled_sample, &evidence);
    state.publish_presented_lifecycle(392, &presented);
    let visual_snapshot = state
        .window(window_id)
        .and_then(|window| window.state.minimized_visual_snapshot())
        .expect("settled presentation promotes the compositor-owned visual");
    assert_eq!(
        visual_snapshot.resource_payload_id,
        lifecycle_sample.visual_source.resource_payload_id
    );
    assert_eq!(state.lifecycle_precapture_dmabuf_release_count(), 0);
    for obligation in &obligations {
        assert!(!state.buffer_release_is_owned(obligation));
    }

    assert!(state.restore_minimized_desktop_window(window_id));
    assert!(state.pending_xwayland_restores.contains_key(&window_id));
    assert!(
        state
            .window(window_id)
            .is_some_and(|window| window.state.is_minimized())
    );
    assert!(
        state
            .window(window_id)
            .and_then(|window| window.state.minimized_visual_snapshot())
            .is_some()
    );
    assert_eq!(state.lifecycle_precapture_dmabuf_release_count(), 0);
}

#[test]
fn shared_xwayland_dmabuf_release_token_rejects_lifecycle_admission() {
    let generation = crate::xwayland::XwaylandGeneration::new(
        std::num::NonZeroU64::new(392).expect("XWayland generation"),
    );
    let root_surface_id = 3_921;
    let child_surface_id = root_surface_id + 1;
    let snapshot = super::desktop_window_tests::x11_snapshot(generation, 3_920, root_surface_id);
    let mut state = CompositorState::new(None);
    state.lifecycle_animation_renderer_available = Some(true);
    super::window_open_animation_tests::set_window_open_preset(
        &mut state,
        crate::animation_control::AnimationPreset::Astrea,
    );
    let window_id = super::desktop_window_tests::insert_x11(&mut state, snapshot);
    let root = super::desktop_window_tests::x11_scanout_surface(
        root_surface_id,
        300,
        200,
        SurfacePlacement::absolute_root_at(30, 40),
        crate::render_backend::buffer::DrmFormat::Xrgb8888,
    );
    let buffer_id = root.buffer_id();
    let obligation = crate::compositor::state_data::DmabufReleaseObligation {
        buffer_id,
        release: crate::compositor::state_data::SurfaceBufferRelease::ExplicitSync(
            crate::compositor::ExplicitSyncPoint::for_tests_with_signal_script(
                392,
                buffer_id.get(),
                [true],
            ),
        ),
    };
    let mut child = super::desktop_window_tests::x11_scanout_surface(
        child_surface_id,
        96,
        72,
        SurfacePlacement::subsurface(root_surface_id, 8, 12),
        crate::render_backend::buffer::DrmFormat::Xrgb8888,
    );
    child.buffer = root.buffer.clone();
    state.append_renderable_surface(root);
    state.append_renderable_surface(child);
    state.surface_topology.set_placement(
        child_surface_id,
        SurfacePlacement::subsurface(root_surface_id, 8, 12),
    );
    state
        .surface_presentation_generations
        .insert(root_surface_id, 1);
    state
        .surface_presentation_generations
        .insert(child_surface_id, 1);
    state
        .active_dmabuf_buffers
        .insert(root_surface_id, obligation.clone());
    state
        .active_dmabuf_buffers
        .insert(child_surface_id, obligation.clone());
    state.rebuild_active_scene_view();

    assert!(state.minimize_desktop_window(window_id));
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("X11 scene node");
    assert!(
        state
            .presentation_animator
            .active_retained_visual(
                scene_node_id,
                crate::presentation_animation::PresentationRetainedVisualKind::WindowLifecycle,
            )
            .is_none()
    );
    assert_eq!(state.lifecycle_precapture_dmabuf_release_count(), 0);
    assert_eq!(state.active_dmabuf_buffers.len(), 2);
    assert!(state.pending_dmabuf_buffer_releases.is_empty());
    assert!(
        !state
            .retained_lifecycle_payloads
            .holds_release_token(&obligation)
    );
}

#[test]
fn shutdown_drains_a_held_lifecycle_precapture_dmabuf_release_once() {
    let generation = crate::xwayland::XwaylandGeneration::new(
        std::num::NonZeroU64::new(393).expect("XWayland generation"),
    );
    let root_surface_id = 3_931;
    let snapshot = super::desktop_window_tests::x11_snapshot(generation, 3_930, root_surface_id);
    let mut state = CompositorState::new(None);
    state.lifecycle_animation_renderer_available = Some(true);
    super::window_open_animation_tests::set_window_open_preset(
        &mut state,
        crate::animation_control::AnimationPreset::Astrea,
    );
    let window_id = super::desktop_window_tests::insert_x11(&mut state, snapshot);
    let source = super::desktop_window_tests::x11_scanout_surface(
        root_surface_id,
        300,
        200,
        SurfacePlacement::absolute_root_at(30, 40),
        crate::render_backend::buffer::DrmFormat::Xrgb8888,
    );
    let buffer_id = source.buffer_id();
    let obligation = crate::compositor::state_data::DmabufReleaseObligation {
        buffer_id,
        release: crate::compositor::state_data::SurfaceBufferRelease::ExplicitSync(
            crate::compositor::ExplicitSyncPoint::for_tests_with_signal_script(
                393,
                buffer_id.get(),
                [true],
            ),
        ),
    };
    state.append_renderable_surface(source);
    state
        .surface_presentation_generations
        .insert(root_surface_id, 1);
    state
        .active_dmabuf_buffers
        .insert(root_surface_id, obligation.clone());
    state.rebuild_active_scene_view();

    assert!(state.minimize_desktop_window(window_id));
    assert!(
        state
            .retained_lifecycle_payloads
            .holds_release_token(&obligation)
    );
    assert_eq!(state.lifecycle_precapture_dmabuf_release_count(), 1);

    state.release_client_buffers_for_shutdown();

    assert_eq!(state.lifecycle_precapture_dmabuf_release_count(), 0);
    assert!(
        !state
            .retained_lifecycle_payloads
            .holds_release_token(&obligation)
    );
    assert!(!state.buffer_release_is_owned(&obligation));
    assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
    assert_eq!(state.buffer_release_metrics.buffer_releases_discarded, 0);
}

#[test]
fn window_destruction_before_capture_queues_the_lifecycle_release_once() {
    let generation = crate::xwayland::XwaylandGeneration::new(
        std::num::NonZeroU64::new(394).expect("XWayland generation"),
    );
    let root_surface_id = 3_941;
    let snapshot = super::desktop_window_tests::x11_snapshot(generation, 3_940, root_surface_id);
    let mut state = CompositorState::new(None);
    state.lifecycle_animation_renderer_available = Some(true);
    super::window_open_animation_tests::set_window_open_preset(
        &mut state,
        crate::animation_control::AnimationPreset::Astrea,
    );
    let window_id = super::desktop_window_tests::insert_x11(&mut state, snapshot);
    let source = super::desktop_window_tests::x11_scanout_surface(
        root_surface_id,
        300,
        200,
        SurfacePlacement::absolute_root_at(30, 40),
        crate::render_backend::buffer::DrmFormat::Xrgb8888,
    );
    let buffer_id = source.buffer_id();
    let obligation = crate::compositor::state_data::DmabufReleaseObligation {
        buffer_id,
        release: crate::compositor::state_data::SurfaceBufferRelease::ExplicitSync(
            crate::compositor::ExplicitSyncPoint::for_tests_with_signal_script(
                394,
                buffer_id.get(),
                [true],
            ),
        ),
    };
    state.append_renderable_surface(source);
    state
        .surface_presentation_generations
        .insert(root_surface_id, 1);
    state
        .active_dmabuf_buffers
        .insert(root_surface_id, obligation.clone());
    state.rebuild_active_scene_view();

    assert!(state.minimize_desktop_window(window_id));
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window scene node before destruction");
    let identity = active_lifecycle_identity(&state, scene_node_id);
    assert!(
        state
            .retained_lifecycle_payloads
            .holds_release_token(&obligation)
    );

    state.retire_xwayland_attachment(root_surface_id);
    assert!(state.remove_desktop_window(window_id).is_some());

    assert!(
        state
            .retained_lifecycle_payloads
            .get_exact(identity)
            .is_none()
    );
    assert_eq!(state.lifecycle_precapture_dmabuf_release_count(), 0);
    assert!(
        !state
            .retained_lifecycle_payloads
            .holds_release_token(&obligation)
    );
    assert!(state.active_dmabuf_buffers.is_empty());
    assert_eq!(state.pending_dmabuf_buffer_releases.len(), 1);
    assert!(state.buffer_release_is_owned(&obligation));

    let batch_id = state.take_frame_batch_for_render(394);
    assert_eq!(state.frame_batch_dmabuf_release_count(batch_id), 1);
    state.complete_frame_batch_after_safe_abandonment(
        batch_id,
        crate::compositor::FrameBatchDiscardReason::OutputDestroyed,
    );
    assert!(!state.buffer_release_is_owned(&obligation));
    assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
    assert_eq!(
        state
            .buffer_release_metrics
            .buffer_release_duplicate_attempts,
        0
    );
}

#[test]
fn renderer_error_after_sampling_cancels_the_precapture_lease_once() {
    let generation = crate::xwayland::XwaylandGeneration::new(
        std::num::NonZeroU64::new(395).expect("XWayland generation"),
    );
    let root_surface_id = 3_951;
    let snapshot = super::desktop_window_tests::x11_snapshot(generation, 3_950, root_surface_id);
    let mut state = CompositorState::new(None);
    state.lifecycle_animation_renderer_available = Some(true);
    super::window_open_animation_tests::set_window_open_preset(
        &mut state,
        crate::animation_control::AnimationPreset::Astrea,
    );
    let window_id = super::desktop_window_tests::insert_x11(&mut state, snapshot);
    let source = super::desktop_window_tests::x11_scanout_surface(
        root_surface_id,
        300,
        200,
        SurfacePlacement::absolute_root_at(30, 40),
        crate::render_backend::buffer::DrmFormat::Xrgb8888,
    );
    let buffer_id = source.buffer_id();
    let obligation = crate::compositor::state_data::DmabufReleaseObligation {
        buffer_id,
        release: crate::compositor::state_data::SurfaceBufferRelease::ExplicitSync(
            crate::compositor::ExplicitSyncPoint::for_tests_with_signal_script(
                395,
                buffer_id.get(),
                [true],
            ),
        ),
    };
    state.append_renderable_surface(source);
    state
        .surface_presentation_generations
        .insert(root_surface_id, 1);
    state
        .active_dmabuf_buffers
        .insert(root_surface_id, obligation.clone());
    state.rebuild_active_scene_view();

    assert!(state.minimize_desktop_window(window_id));
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window scene node");
    let identity = active_lifecycle_identity(&state, scene_node_id);
    assert!(
        state
            .retained_lifecycle_payloads
            .holds_release_token(&obligation)
    );
    state.retire_xwayland_attachment(root_surface_id);
    assert!(
        state
            .retained_lifecycle_payloads
            .holds_release_token(&obligation)
    );

    // Native render failure calls this only after its EGL completion proof.
    state.cancel_lifecycle_precapture_after_render_failure();

    assert!(
        state
            .retained_lifecycle_payloads
            .get_exact(identity)
            .is_none()
    );
    assert_eq!(state.lifecycle_precapture_dmabuf_release_count(), 0);
    assert_eq!(state.pending_dmabuf_buffer_releases.len(), 1);
    assert!(state.buffer_release_is_owned(&obligation));
    let batch_id = state.take_frame_batch_for_render(395);
    assert_eq!(state.frame_batch_dmabuf_release_count(batch_id), 1);
    state.complete_frame_batch_after_safe_abandonment(
        batch_id,
        crate::compositor::FrameBatchDiscardReason::OutputDestroyed,
    );
    assert!(!state.buffer_release_is_owned(&obligation));
    assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
    assert_eq!(
        state
            .buffer_release_metrics
            .buffer_release_duplicate_attempts,
        0
    );
}
