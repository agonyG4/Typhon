use super::CompositorState;
use super::window_exit_physical::prove_window_exit_source;
use super::window_exit_retained::{
    PreparedWindowExit, WindowExitFrozenContent, WindowExitPainterOrder, WindowExitPayload,
    WindowExitPropertyRevisions, WindowExitReleaseObligation,
};
use crate::compositor::ExplicitSyncPoint;
use crate::compositor::surface::SurfaceCommitSequence;
use crate::compositor::{
    PresentedCanonicalSceneSnapshot, PresentedFramePublication, PresentedLifecycleScene,
    PresentedSurfaceContentEvidence, SurfacePresentationKey,
};
use crate::compositor::{
    PresentedWindowGeometry, RenderableSurface, RenderableSurfaceDamage, SurfacePlacement,
    WindowExitFrameEvidence,
};
use crate::core::{OutputId, SceneNodeId, WindowId};
use crate::presentation_animation::{
    AnimationCurve, AnimationTime, EasingCurve, PresentationClip, PresentationFrameSnapshot,
    PresentationGeometryMutation, PresentationOpacity, PresentationOpacityMutation,
    PresentationRetainedVisualIdentity, PresentationRetainedVisualKind,
    PresentationSampleTimeSource, PresentationTransactionMemberKind,
    PresentationTransactionRequest, PresentationWindowTarget,
};
use crate::render_backend::buffer::{
    BufferId, BufferIdAllocator, BufferSize, CommittedSurfaceBuffer, DrmFormat,
};
use std::sync::Arc;
use std::time::Duration;

fn evidence(
    surface_id: u32,
    generation: u64,
    commit: u64,
    buffer_id: u64,
    scene_node_id: u64,
    visual_root_surface_id: u32,
    presentation_owner_root_surface_id: u32,
) -> PresentedSurfaceContentEvidence {
    PresentedSurfaceContentEvidence {
        key: SurfacePresentationKey {
            surface_id,
            generation,
        },
        commit_sequence: SurfaceCommitSequence(commit),
        buffer_id: BufferId::for_tests(buffer_id),
        scene_node_id: SceneNodeId::from_raw(scene_node_id).expect("test scene node"),
        visual_root_surface_id,
        presentation_owner_root_surface_id,
    }
}

fn fixture() -> (
    PresentedCanonicalSceneSnapshot,
    Vec<PresentedSurfaceContentEvidence>,
) {
    let output_id = OutputId::from_raw(1).expect("test output");
    let root = evidence(10, 7, 31, 400, 80, 10, 10);
    let child = evidence(11, 3, 15, 401, 80, 10, 10);
    let mut surfaces = vec![root, child];
    surfaces.sort_unstable_by_key(|surface| surface.key.surface_id);
    (
        PresentedCanonicalSceneSnapshot {
            output_id,
            render_generation: 50,
            effect_identity_signature: 900,
            surfaces: surfaces.clone(),
        },
        surfaces,
    )
}

fn shm_surface(surface_id: u32) -> RenderableSurface {
    let buffer_id = BufferIdAllocator::default()
        .allocate()
        .expect("test buffer id");
    RenderableSurface {
        surface_id,
        x: 0,
        y: 0,
        width: 4,
        height: 4,
        placement: SurfacePlacement::root_at(0, 0),
        render_backend: crate::compositor::SurfaceRenderBackend::NativeWayland,
        render_placement: None,
        visual_clip: None,
        render_target_size: None,
        generation: 1,
        commit_sequence: SurfaceCommitSequence::initial(),
        buffer: CommittedSurfaceBuffer::shm_snapshot(
            buffer_id,
            BufferSize::new(4, 4).expect("test buffer size"),
            vec![0; 16],
        ),
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        damage: RenderableSurfaceDamage::Full,
    }
}

fn admission_window(
    mode: super::super::window_state::ToplevelMode,
    size: (u32, u32),
) -> (CompositorState, u32, SceneNodeId, WindowId) {
    use std::os::unix::net::UnixStream;

    let mut state = CompositorState::new(None);
    let size = if size == (0, 0) {
        (state.output_size.width, state.output_size.height)
    } else {
        size
    };
    let display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (server_end, _peer) = UnixStream::pair().expect("test client socket");
    let client = display_handle
        .insert_client(server_end, Arc::new(()))
        .expect("test Wayland client");
    let surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let root_surface_id = crate::compositor::compositor_surface_id(&surface);
    state
        .assign_surface_role(root_surface_id, crate::compositor::SurfaceRole::XdgToplevel)
        .expect("XDG toplevel role");
    let window_id = state.allocate_window_id().expect("test window id");
    state
        .insert_desktop_window(crate::compositor::DesktopWindow::new_xdg(
            window_id,
            root_surface_id,
        ))
        .expect("managed XDG window");
    let placement = state.surface_placement(root_surface_id);
    let mut surface = shm_surface(root_surface_id);
    surface.width = size.0;
    surface.height = size.1;
    let buffer_identity = surface.buffer_identity().clone();
    surface.buffer = CommittedSurfaceBuffer::shm_snapshot(
        buffer_identity,
        BufferSize::new(size.0, size.1).expect("test buffer size"),
        vec![0; (size.0 * size.1) as usize],
    );
    surface.placement = placement;
    state.append_renderable_surface(surface);
    state
        .surface_presentation_generations
        .insert(root_surface_id, 1);
    {
        let window = state.window_mut(window_id).expect("managed test window");
        window.state.set_mode(mode);
    }
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("WindowGroup scene node");
    state.rebuild_active_scene_view();

    let mut configuration = state.animation_control.configuration().clone();
    configuration.overrides.insert(
        crate::animation_control::AnimationSlot::WindowClose,
        crate::animation_control::AnimationEffect::WindowScale,
    );
    state
        .set_animation_configuration(configuration)
        .expect("enable Window Close for admission test");
    (state, root_surface_id, scene_node_id, window_id)
}

fn dmabuf_xdg_surface(
    root_surface_id: u32,
    size: BufferSize,
    placement: SurfacePlacement,
    identity: crate::render_backend::buffer::BufferIdentity,
) -> RenderableSurface {
    let mut surface = super::desktop_window_tests::x11_scanout_surface(
        root_surface_id,
        size.width,
        size.height,
        placement,
        DrmFormat::Xrgb8888,
    );
    let dmabuf = surface
        .dmabuf_handle()
        .cloned()
        .expect("test DMA-BUF handle");
    surface.buffer = CommittedSurfaceBuffer::dmabuf_handle(identity, dmabuf);
    surface.render_backend = crate::compositor::SurfaceRenderBackend::NativeWayland;
    surface.commit_sequence = SurfaceCommitSequence(1);
    surface
}

fn direct_fullscreen_xdg_window() -> (
    CompositorState,
    u32,
    SceneNodeId,
    WindowId,
    crate::compositor::DirectScanoutSceneCandidate,
    BufferIdAllocator,
) {
    use super::super::window_state::ToplevelMode;

    let (mut state, root_surface_id, scene_node_id, window_id) =
        admission_window(ToplevelMode::Fullscreen, (0, 0));
    let mut lifecycle = super::xdg_lifecycle::XdgSurfaceLifecycle::default();
    lifecycle.construction = super::xdg_lifecycle::XdgConstructionState::ConstructedToplevel;
    lifecycle.map_state = super::xdg_lifecycle::XdgMapState::Mapped;
    lifecycle.initial_configure_sent = true;
    lifecycle.initial_configure_acked = true;
    lifecycle.currently_mapped = true;
    state
        .xdg_surface_lifecycles
        .insert(root_surface_id, lifecycle);
    let size = BufferSize::new(state.output_size.width, state.output_size.height)
        .expect("test output size");
    let mut identities = BufferIdAllocator::default();
    let identity = identities.allocate().expect("direct buffer identity");
    let surface = dmabuf_xdg_surface(
        root_surface_id,
        size,
        SurfacePlacement::absolute_root_at(0, 0),
        identity,
    );
    assert!(
        state
            .replace_renderable_surface(root_surface_id, surface)
            .is_some()
    );
    state.set_surface_placement(root_surface_id, SurfacePlacement::absolute_root_at(0, 0));
    state
        .surface_presentation_generations
        .insert(root_surface_id, 1);
    state.set_fullscreen_presentation_owner(root_surface_id);
    state.rebuild_active_scene_view();
    let candidate = state
        .direct_scanout_scene_candidate()
        .expect("fullscreen XDG DMA-BUF is an eligible direct candidate");
    (
        state,
        root_surface_id,
        scene_node_id,
        window_id,
        candidate,
        identities,
    )
}

fn promote_direct_candidate(
    state: &mut CompositorState,
    candidate: &crate::compositor::DirectScanoutSceneCandidate,
    frame_id: u64,
    presented_at_ns: u64,
) {
    state.publish_direct_scanout_frame(
        frame_id,
        presented_at_ns,
        state.native_output_id().expect("direct output id"),
        candidate.render_generation,
        candidate.effect_identity_signature,
        candidate.surface_id,
        candidate.surface_presentation_generation,
        candidate.commit_sequence,
        candidate.buffer_identity.id(),
        candidate.surface_scene_node_id,
        candidate.window_scene_node_id,
        candidate.root_surface_id,
        candidate.presented_window_rect,
    );
}

fn publish_admission_evidence(
    state: &mut CompositorState,
    root_surface_id: u32,
    scene_node_id: SceneNodeId,
    frame_id: u64,
) {
    use crate::presentation_animation::{AnimationTime, PresentationSampleTimeSource};

    let output_id = state.native_output_id().expect("test output id");
    let now = AnimationTime::from_nanos(frame_id.saturating_add(1));
    let surfaces = state.active_scene_surfaces();
    let targets = state.native_frame_presentation_targets(surfaces);
    let sample = state.presentation_animator.sample(
        output_id,
        now,
        PresentationSampleTimeSource::ScheduledTarget,
        targets.windows(),
    );
    publish_sampled_admission_evidence(state, root_surface_id, scene_node_id, frame_id, &sample);
}

fn publish_sampled_admission_evidence(
    state: &mut CompositorState,
    root_surface_id: u32,
    scene_node_id: SceneNodeId,
    frame_id: u64,
    sample: &crate::presentation_animation::PresentationSceneSample,
) {
    use crate::presentation_animation::PresentationFrameSnapshot;
    use crate::window_lifecycle_animation::LifecycleFrameSnapshot;

    let now = sample.sampled_at;
    let canonical_scene = canonical_scene_for_sample(state, sample);
    let surfaces = state.active_scene_surfaces();
    let targets = state.native_frame_presentation_targets(surfaces.as_ref());
    let lifecycle = state.lifecycle_scene_sample_at(now);
    let geometry = targets
        .windows()
        .iter()
        .find(|target| target.root_surface_id() == root_surface_id)
        .map(|target| target.canonical_rect())
        .expect("physical admission geometry");
    let mut presented_windows = state.presented_window_geometries_for_targets(sample, &targets);
    if !presented_windows
        .iter()
        .any(|window| window.root_surface_id() == root_surface_id)
    {
        presented_windows.push(PresentedWindowGeometry::with_scene_node(
            scene_node_id,
            root_surface_id,
            geometry,
        ));
    }
    let presentation =
        PresentationFrameSnapshot::from_sample_with_presented_windows(sample, presented_windows);
    let lifecycle_snapshot = LifecycleFrameSnapshot::from_sample(&lifecycle);
    state.publish_presented_frame(PresentedFramePublication {
        frame_id,
        presentation: &presentation,
        lifecycle: &lifecycle_snapshot,
        lifecycle_scene: PresentedLifecycleScene::Initial,
        canonical_scene: Some(&canonical_scene),
        window_exits: &[],
    });
}

fn canonical_scene_for_sample(
    state: &CompositorState,
    sample: &crate::presentation_animation::PresentationSceneSample,
) -> PresentedCanonicalSceneSnapshot {
    let output_id = state.native_output_id().expect("test output id");
    let (surfaces, scene_nodes, fullscreen_plan, _) =
        state.native_frame_renderable_surfaces_with_scene_nodes_and_composition_plan();
    let owner_roots = surfaces
        .iter()
        .map(|surface| state.presentation_owner_root_for_surface(surface.surface_id))
        .collect::<Vec<_>>();
    let visual_roots = crate::compositor::visual_stack_groups(
        surfaces.as_ref(),
        state.active_scene_view.popup_surface_ids(),
    )
    .into_iter()
    .flat_map(|group| {
        group
            .surface_indices()
            .iter()
            .map(|index| (surfaces[*index].surface_id, group.root_surface_id()))
            .collect::<Vec<_>>()
    })
    .collect::<std::collections::HashMap<_, _>>();
    let visual_roots = surfaces
        .iter()
        .map(|surface| {
            visual_roots
                .get(&surface.surface_id)
                .copied()
                .unwrap_or(surface.surface_id)
        })
        .collect::<Vec<_>>();
    let keys = surfaces
        .iter()
        .map(|surface| state.surface_presentation_key_for_surface(surface.surface_id))
        .collect::<Vec<_>>();
    let lifecycle = state.lifecycle_scene_sample_at(sample.sampled_at);
    let canonical_effects = state.resolved_effect_scene_with_presentation_and_lifecycle(
        sample,
        &fullscreen_plan,
        &lifecycle,
    );
    PresentedCanonicalSceneSnapshot::capture(
        output_id,
        state.scene_render_generation,
        canonical_effects.signature,
        surfaces.as_ref(),
        scene_nodes.as_ref(),
        &owner_roots,
        &keys,
        &visual_roots,
    )
    .expect("exact canonical scene evidence")
}

#[test]
fn default_normal_xdg_window_is_admitted_with_sparse_default_properties() {
    let (mut state, root_surface_id, scene_node_id, _) =
        admission_window(super::super::window_state::ToplevelMode::Normal, (500, 400));
    publish_admission_evidence(&mut state, root_surface_id, scene_node_id, 1);

    let physical = state
        .presented_presentation
        .as_ref()
        .expect("promoted physical presentation");
    assert!(physical.opacities.is_empty());
    assert!(physical.clips.is_empty());
    assert_eq!(
        state.surface_role(root_surface_id),
        crate::compositor::SurfaceRole::XdgToplevel
    );
    let window_id = state
        .window_id_for_surface(root_surface_id)
        .expect("managed window mapping");
    assert!(
        state
            .window(window_id)
            .expect("managed window")
            .is_workspace_managed()
    );
    assert!(state.window_is_visible_in_active_scene(window_id));
    assert!(!state.presentation_animator.has_track(scene_node_id));
    assert!(state.window_exit_payloads.can_prepare_root(root_surface_id));
    assert_eq!(
        state
            .presented_window_geometry(root_surface_id)
            .expect("exact physically promoted geometry")
            .scene_node_id(),
        scene_node_id
    );
    assert!(state.prepare_window_exit(root_surface_id));

    let (mut stale, stale_root, stale_node, _) =
        admission_window(super::super::window_state::ToplevelMode::Normal, (500, 400));
    publish_admission_evidence(&mut stale, stale_root, stale_node, 2);
    stale.presented_canonical_scene = None;
    assert!(!stale.prepare_window_exit(stale_root));
}

#[test]
fn managed_xdg_window_exit_admission_respects_minimized_but_not_toplevel_mode() {
    use super::super::window_state::ToplevelMode;

    let cases = [
        (ToplevelMode::Normal, false, (500, 400), true),
        (ToplevelMode::Maximized, false, (1280, 760), true),
        (ToplevelMode::Fullscreen, false, (0, 0), true),
        (ToplevelMode::Normal, true, (500, 400), false),
    ];
    for (index, (mode, minimized, size, expected)) in cases.into_iter().enumerate() {
        let (mut state, root_surface_id, scene_node_id, window_id) = admission_window(mode, size);
        publish_admission_evidence(
            &mut state,
            root_surface_id,
            scene_node_id,
            u64::try_from(index + 3).expect("test frame id"),
        );
        if minimized {
            state
                .window_mut(window_id)
                .expect("managed test window")
                .state
                .mark_minimized_without_surfaces();
        }
        assert_eq!(
            state.prepare_window_exit(root_surface_id),
            expected,
            "WindowExit admission for {mode:?} (minimized={minimized})",
        );
    }
}

#[test]
fn managed_x11_window_uses_the_same_exact_physical_admission() {
    use super::desktop_window_tests::{
        install_x11_scanout_surface, x11_output_snapshot, x11_scanout_surface,
    };
    use crate::render_backend::buffer::DrmFormat;
    use crate::xwayland::XwaylandGeneration;
    use std::num::NonZeroU64;

    let mut state = CompositorState::new(None);
    let root_surface_id = 711;
    let generation = XwaylandGeneration::new(NonZeroU64::new(71).expect("generation"));
    install_x11_scanout_surface(
        &mut state,
        x11_scanout_surface(
            root_surface_id,
            500,
            400,
            SurfacePlacement::absolute_root_at(30, 40),
            DrmFormat::Xrgb8888,
        ),
        x11_output_snapshot(generation, 0x711, root_surface_id),
    );
    let window_id = state
        .window_id_for_surface(root_surface_id)
        .expect("managed X11 window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("X11 WindowGroup scene node");
    let mut configuration = state.animation_control.configuration().clone();
    configuration.overrides.insert(
        crate::animation_control::AnimationSlot::WindowClose,
        crate::animation_control::AnimationEffect::WindowScale,
    );
    state
        .set_animation_configuration(configuration)
        .expect("enable Window Close for admission test");
    let buffer_id = state
        .renderable_surface(root_surface_id)
        .expect("managed X11 buffer")
        .buffer_id();
    state.active_dmabuf_buffers.insert(
        root_surface_id,
        crate::compositor::state_data::DmabufReleaseObligation {
            buffer_id,
            release: crate::compositor::state_data::SurfaceBufferRelease::ExplicitSync(
                ExplicitSyncPoint::for_tests_with_signal_script(71, buffer_id.get(), [true]),
            ),
        },
    );
    publish_admission_evidence(&mut state, root_surface_id, scene_node_id, 9);

    assert_eq!(
        state.surface_role(root_surface_id),
        crate::compositor::SurfaceRole::Xwayland
    );
    assert!(
        state
            .window(window_id)
            .expect("managed X11 window")
            .is_workspace_managed()
    );
    assert!(state.window_is_visible_in_active_scene(window_id));
    assert!(state.window_exit_payloads.can_prepare_root(root_surface_id));
    assert_eq!(
        state
            .presented_window_geometry(root_surface_id)
            .expect("X11 physical geometry")
            .scene_node_id(),
        scene_node_id
    );

    assert!(state.prepare_window_exit(root_surface_id));
}

#[test]
fn newer_unpresented_commit_is_not_admitted_from_older_physical_evidence() {
    let (mut state, root_surface_id, scene_node_id, _) =
        admission_window(super::super::window_state::ToplevelMode::Normal, (500, 400));
    publish_admission_evidence(&mut state, root_surface_id, scene_node_id, 10);
    let current = state
        .renderable_surfaces
        .iter_mut()
        .find(|surface| surface.surface_id == root_surface_id)
        .expect("current committed XDG content");
    current.commit_sequence = SurfaceCommitSequence(current.commit_sequence.0 + 1);
    state.rebuild_active_scene_view();

    assert!(!state.prepare_window_exit(root_surface_id));
}

#[test]
fn direct_scanout_pageflip_evidence_flows_through_xdg_close_render_settlement_and_release() {
    let (mut state, root_surface_id, scene_node_id, _, candidate, _) =
        direct_fullscreen_xdg_window();
    let exact_buffer_id = candidate.buffer_identity.id();
    assert_eq!(candidate.root_surface_id, root_surface_id);
    assert_eq!(
        candidate.surface_scene_node_id,
        state.active_scene_surface_scene_nodes_in_order()[state
            .active_scene_surfaces()
            .iter()
            .position(|surface| surface.surface_id == candidate.surface_id)
            .expect("candidate is in the active direct scene")]
    );
    assert_eq!(candidate.window_scene_node_id, scene_node_id);
    assert_eq!(candidate.buffer_size, candidate.output_size);

    // This is the same physical promotion helper called only after direct
    // pageflip settlement in cycle_direct::settle_direct_pageflip.
    promote_direct_candidate(&mut state, &candidate, 12, 12_000);
    let canonical = state
        .presented_canonical_scene
        .as_ref()
        .expect("direct pageflip publishes canonical content proof");
    assert_eq!(
        canonical.output_id,
        state.native_output_id().expect("direct output id")
    );
    assert_eq!(canonical.render_generation, candidate.render_generation);
    assert_eq!(
        canonical.effect_identity_signature,
        candidate.effect_identity_signature
    );
    let physical = canonical.surfaces_for_owner(root_surface_id);
    assert_eq!(physical.len(), 1);
    assert_eq!(physical[0].key.surface_id, candidate.surface_id);
    assert_eq!(
        physical[0].key.generation,
        candidate.surface_presentation_generation
    );
    assert_eq!(physical[0].commit_sequence, candidate.commit_sequence);
    assert_eq!(physical[0].buffer_id, exact_buffer_id);
    assert_eq!(physical[0].scene_node_id, candidate.surface_scene_node_id);
    assert_eq!(physical[0].visual_root_surface_id, root_surface_id);
    assert_eq!(
        physical[0].presentation_owner_root_surface_id,
        root_surface_id
    );
    let direct_presentation = state
        .presented_presentation
        .as_ref()
        .expect("direct pageflip publishes presentation snapshot");
    assert_eq!(
        direct_presentation.opacity_for_scene_node(scene_node_id),
        PresentationOpacity::OPAQUE
    );
    assert_eq!(
        direct_presentation.clip_for_scene_node(scene_node_id),
        PresentationClip::Unbounded
    );
    assert_eq!(
        state
            .presented_window_geometry(root_surface_id)
            .expect("direct physical geometry")
            .presented_rect(),
        candidate.presented_window_rect,
    );

    let sync_point =
        ExplicitSyncPoint::for_tests_with_signal_script(91, exact_buffer_id.get(), [true]);
    let release = crate::compositor::state_data::DmabufReleaseObligation {
        buffer_id: exact_buffer_id,
        release: crate::compositor::state_data::SurfaceBufferRelease::ExplicitSync(
            sync_point.clone(),
        ),
    };
    state
        .active_dmabuf_buffers
        .insert(candidate.surface_id, release.clone());

    // The XDG null-buffer path cancels old property tracks before the normal
    // RemoveContent publication boundary asks prepare_window_exit to capture.
    state.begin_xdg_empty_or_unmap_commit(root_surface_id);
    assert!(
        !state
            .xdg_surface_lifecycle(root_surface_id)
            .expect("mapped XDG lifecycle")
            .currently_mapped
    );
    assert!(state.commit_surface_remove_content(
        root_surface_id,
        SurfaceCommitSequence(2),
        Vec::new(),
        super::surface_transactions::SurfacePublicationSource::RemoveContent,
    ));
    assert!(!state.renderable_surfaces.iter().any(|surface| {
        surface.surface_id == candidate.surface_id && surface.buffer_id() == exact_buffer_id
    }));
    assert!(
        !state
            .active_dmabuf_buffers
            .contains_key(&candidate.surface_id)
    );
    assert!(state.pending_dmabuf_buffer_releases.is_empty());
    assert!(state.buffer_release_is_owned(&release));
    let exits = state.active_window_exit_render_groups();
    assert_eq!(exits.len(), 1);
    let exit = &exits[0];
    assert_eq!(exit.content.root_surface_id, root_surface_id);
    assert_eq!(exit.content.surfaces.len(), 1);
    assert_eq!(exit.content.surfaces[0].buffer_id(), exact_buffer_id);
    assert_eq!(
        exit.content.source_presented_rect,
        candidate.presented_window_rect
    );
    assert_eq!(
        exit.content.source_presented_opacity,
        PresentationOpacity::OPAQUE
    );
    assert_eq!(
        exit.content.source_presented_clip,
        PresentationClip::Unbounded
    );
    assert!(exit.content.surfaces[0].dmabuf_handle().is_some());
    assert_eq!(
        exit.content.surfaces[0].commit_sequence,
        candidate.commit_sequence
    );
    assert!(
        state
            .direct_scanout_scene_blockers()
            .reasons()
            .contains(&crate::compositor::DirectScanoutSceneRejection::WindowExitAnimation)
    );

    let (render_surfaces, render_scene_nodes, render_owner_roots) = state
        .merge_window_exit_surfaces(
            std::borrow::Cow::Owned(Vec::new()),
            std::borrow::Cow::Owned(Vec::new()),
            std::borrow::Cow::Owned(Vec::new()),
            &exits,
        );
    assert_eq!(render_surfaces.len(), 1);
    assert_eq!(render_surfaces[0].surface_id, candidate.surface_id);
    assert_eq!(render_surfaces[0].buffer_id(), exact_buffer_id);
    assert_eq!(render_scene_nodes.as_ref(), &[scene_node_id]);
    assert_eq!(render_owner_roots.as_ref(), &[root_surface_id]);

    let output_id = state.native_output_id().expect("composited output id");
    let (_, payload) = state
        .window_exit_payloads
        .active_payloads()
        .next()
        .expect("real XDG removal activated WindowExit");
    let revisions = payload
        .property_revisions
        .expect("exact close transaction revisions");
    let target = PresentationWindowTarget::with_scene_node(
        scene_node_id,
        root_surface_id,
        payload.content.source_presented_rect,
    )
    .with_canonical_opacity(PresentationOpacity::OPAQUE)
    .with_canonical_clip(PresentationClip::Unbounded);
    let settled_sample = state.presentation_animator.sample(
        output_id,
        AnimationTime::from_nanos(
            payload
                .motion_started_at
                .as_nanos()
                .saturating_add(2_000_000_000),
        ),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target],
    );
    let settled_geometry = settled_sample
        .transform_for_root(root_surface_id)
        .expect("settled close geometry");
    let settled_presentation = PresentationFrameSnapshot::from_sample_with_presented_windows(
        &settled_sample,
        vec![PresentedWindowGeometry::with_scene_node(
            scene_node_id,
            root_surface_id,
            settled_geometry.presented_rect,
        )],
    );
    assert!(settled_presentation.transforms.iter().any(|transform| {
        transform.scene_node_id == scene_node_id
            && transform.root_surface_id == root_surface_id
            && transform.transaction_id == revisions.transaction_id
            && transform.revision_id == revisions.geometry_revision_id
            && transform.mathematically_settled
    }));
    assert!(settled_presentation.opacities.iter().any(|opacity| {
        opacity.scene_node_id == scene_node_id
            && opacity.root_surface_id == root_surface_id
            && opacity.transition.is_some_and(|transition| {
                transition.transaction_id == revisions.transaction_id
                    && transition.revision_id == revisions.opacity_revision_id
                    && transition.mathematically_settled
            })
    }));
    let identity = exit.identity;
    let window_exit = [WindowExitFrameEvidence {
        identity,
        payload_id: exit.payload_id,
        root_surface_id,
        scene_node_id,
    }];
    let canonical_scene = canonical_scene_for_sample(&state, &settled_sample);
    let lifecycle_sample = state.lifecycle_scene_sample_at(settled_sample.sampled_at);
    let lifecycle =
        crate::window_lifecycle_animation::LifecycleFrameSnapshot::from_sample(&lifecycle_sample);
    let no_canonical_roots = [];
    state.publish_presented_frame(PresentedFramePublication {
        frame_id: 13,
        presentation: &settled_presentation,
        lifecycle: &lifecycle,
        lifecycle_scene: PresentedLifecycleScene::RenderedSceneReplacement {
            canonical_root_surface_ids: &no_canonical_roots,
        },
        canonical_scene: Some(&canonical_scene),
        window_exits: &window_exit,
    });
    assert!(state.window_exit_payloads.get_exact(identity).is_none());
    assert_eq!(state.active_window_exit_render_groups().len(), 0);
    assert!(state.buffer_release_is_owned(&release));
    assert_eq!(
        state
            .pending_dmabuf_buffer_releases
            .iter()
            .filter(|pending| pending.same_release_token(&release))
            .count(),
        1,
    );
    assert_eq!(
        sync_point
            .signal_script
            .as_ref()
            .expect("explicit sync release script")
            .lock()
            .expect("release script lock")
            .len(),
        1,
    );
    state.publish_presented_frame(PresentedFramePublication {
        frame_id: 13,
        presentation: &settled_presentation,
        lifecycle: &lifecycle,
        lifecycle_scene: PresentedLifecycleScene::RenderedSceneReplacement {
            canonical_root_surface_ids: &no_canonical_roots,
        },
        canonical_scene: Some(&canonical_scene),
        window_exits: &window_exit,
    });
    assert_eq!(
        state
            .pending_dmabuf_buffer_releases
            .iter()
            .filter(|pending| pending.same_release_token(&release))
            .count(),
        1,
        "a repeated physical ACK cannot return the exact release twice",
    );
}

#[test]
fn direct_scanout_buffer_a_does_not_admit_newer_unpresented_buffer_b_on_unmap() {
    let (mut state, root_surface_id, _, _, candidate, mut identities) =
        direct_fullscreen_xdg_window();
    let buffer_a_id = candidate.buffer_identity.id();
    promote_direct_candidate(&mut state, &candidate, 20, 20_000);

    let buffer_b_identity = identities.allocate().expect("new client buffer identity");
    let replacement = dmabuf_xdg_surface(
        root_surface_id,
        candidate.buffer_size,
        SurfacePlacement::absolute_root_at(0, 0),
        buffer_b_identity,
    );
    assert!(
        state
            .replace_renderable_surface(root_surface_id, replacement)
            .is_some()
    );
    state.surface_presentation_generations.insert(
        root_surface_id,
        candidate.surface_presentation_generation + 1,
    );
    state.rebuild_active_scene_view();
    let buffer_b_id = state
        .renderable_surface(root_surface_id)
        .expect("unpresented B content")
        .buffer_id();
    assert_ne!(buffer_a_id, buffer_b_id);
    assert_eq!(
        state
            .presented_canonical_scene
            .as_ref()
            .expect("A physical proof remains promoted")
            .surfaces_for_owner(root_surface_id)[0]
            .buffer_id,
        buffer_a_id,
    );

    let sync_point = ExplicitSyncPoint::for_tests_with_signal_script(92, buffer_b_id.get(), [true]);
    let release_b = crate::compositor::state_data::DmabufReleaseObligation {
        buffer_id: buffer_b_id,
        release: crate::compositor::state_data::SurfaceBufferRelease::ExplicitSync(sync_point),
    };
    state
        .active_dmabuf_buffers
        .insert(root_surface_id, release_b.clone());
    state.begin_xdg_empty_or_unmap_commit(root_surface_id);
    assert!(state.commit_surface_remove_content(
        root_surface_id,
        SurfaceCommitSequence(3),
        Vec::new(),
        super::surface_transactions::SurfacePublicationSource::RemoveContent,
    ));

    assert!(
        state
            .window_exit_payloads
            .active_payloads()
            .next()
            .is_none()
    );
    assert!(state.active_window_exit_render_groups().is_empty());
    assert!(
        !state
            .renderable_surfaces
            .iter()
            .any(|surface| surface.buffer_id() == buffer_b_id)
    );
    assert!(
        !state
            .renderable_surfaces
            .iter()
            .any(|surface| surface.buffer_id() == buffer_a_id)
    );
    assert!(
        !state.buffer_release_is_owned(&release_b)
            || state
                .pending_dmabuf_buffer_releases
                .iter()
                .any(|pending| { pending.same_release_token(&release_b) })
    );
    assert_eq!(
        state
            .pending_dmabuf_buffer_releases
            .iter()
            .filter(|pending| pending.same_release_token(&release_b))
            .count(),
        1,
    );
}

#[test]
fn physically_presented_in_flight_property_tracks_can_be_taken_over_by_window_exit() {
    let (mut state, root_surface_id, scene_node_id, window_id) =
        admission_window(super::super::window_state::ToplevelMode::Normal, (500, 400));
    let output_id = state.native_output_id().expect("test output id");
    let targets = state.native_frame_presentation_targets(state.active_scene_surfaces());
    let target = targets
        .windows()
        .iter()
        .find(|target| target.root_surface_id() == root_surface_id)
        .copied()
        .expect("mapped root presentation target");
    let canonical = target.canonical_rect();
    let transition_rect = crate::presentation_animation::PresentationRect::new(
        canonical.x() + 48.0,
        canonical.y() + 26.0,
        canonical.width() - 12.0,
        canonical.height() - 8.0,
    )
    .expect("in-flight geometry");
    let transition_opacity = PresentationOpacity::new(0.45).expect("in-flight opacity");
    let transition_clip = PresentationClip::Rect(
        crate::presentation_animation::PresentationClipRect::new(4.0, 5.0, 300.0, 240.0)
            .expect("in-flight clip"),
    );
    let curve = AnimationCurve::easing(Duration::from_secs(1), EasingCurve::Linear);
    let started_at = AnimationTime::from_nanos(1_000_000_000);
    state
        .presentation_animator
        .commit(PresentationTransactionRequest::mixed_all(
            started_at,
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                canonical,
                transition_rect,
                curve,
            )],
            vec![PresentationOpacityMutation::new(
                scene_node_id,
                PresentationOpacity::OPAQUE,
                transition_opacity,
                curve,
            )],
            vec![
                crate::presentation_animation::PresentationClipMutation::new(
                    scene_node_id,
                    PresentationClip::Unbounded,
                    transition_clip,
                    None,
                    curve,
                ),
            ],
        ))
        .expect("in-flight Geometry+Opacity+Clip transaction");
    let physical_sample = state.presentation_animator.sample(
        output_id,
        AnimationTime::from_nanos(started_at.as_nanos() + 400_000_000),
        PresentationSampleTimeSource::ScheduledTarget,
        targets.windows(),
    );
    assert_ne!(
        physical_sample
            .transform_for_root(root_surface_id)
            .unwrap()
            .presented_rect,
        canonical
    );
    assert_eq!(
        physical_sample.opacity_for_scene_node(scene_node_id),
        transition_opacity
    );
    assert_eq!(
        physical_sample.clip_for_scene_node(scene_node_id),
        transition_clip
    );
    publish_sampled_admission_evidence(
        &mut state,
        root_surface_id,
        scene_node_id,
        11,
        &physical_sample,
    );

    assert!(state.prepare_window_exit(root_surface_id));
    assert!(state.remove_desktop_window(window_id).is_some());
    assert!(state.activate_prepared_window_exit(root_surface_id));
    let (_, payload) = state
        .window_exit_payloads
        .active_payloads()
        .next()
        .expect("admitted WindowExit payload");
    assert_eq!(
        payload.content.source_presented_rect,
        physical_sample
            .transform_for_root(root_surface_id)
            .expect("physically sampled geometry")
            .presented_rect
    );
    assert_eq!(payload.content.source_presented_opacity, transition_opacity);
    assert_eq!(payload.content.source_presented_clip, transition_clip);
}

fn activate_window_exit(
    state: &mut CompositorState,
    held_release_obligations: Vec<WindowExitReleaseObligation>,
) -> (PresentationRetainedVisualIdentity, WindowExitFrameEvidence) {
    let root_surface_id = 10;
    let window_id = WindowId::from_raw(10).expect("test WindowId");
    let scene_node_id = SceneNodeId::from_raw(80).expect("test WindowGroup SceneNodeId");
    let rect = crate::presentation_animation::PresentationRect::new(10.0, 20.0, 300.0, 200.0)
        .expect("test rectangle");
    let target_rect =
        crate::presentation_animation::PresentationRect::new(20.0, 20.0, 300.0, 200.0)
            .expect("test target rectangle");
    let started_at = AnimationTime::from_nanos(10);
    state.presentation_animator.set_enabled(true);
    let identity = state
        .presentation_animator
        .begin_retained_visual(
            scene_node_id,
            PresentationRetainedVisualKind::WindowExit,
            started_at,
        )
        .expect("reserve WindowExit identity");
    let prepared = PreparedWindowExit {
        content: Arc::new(WindowExitFrozenContent {
            window_id,
            root_surface_id,
            scene_node_id,
            surfaces: vec![shm_surface(root_surface_id)],
            surface_scene_node_ids: vec![scene_node_id],
            visual_root_surface_ids: vec![root_surface_id],
            presentation_owner_root_surface_ids: vec![root_surface_id],
            canonical_rect: rect,
            close_geometry_target: rect,
            close_curve: AnimationCurve::easing(Duration::from_millis(1), EasingCurve::EaseInCubic),
            source_presented_rect: rect,
            source_presented_opacity: PresentationOpacity::OPAQUE,
            source_presented_clip: PresentationClip::Unbounded,
            frozen_decoration: None,
            effect_scene: Arc::new(crate::compositor::ResolvedEffectScene::default()),
            painter_order: WindowExitPainterOrder {
                scene_band: 2,
                layer_rank: 2,
                stack_position: 0,
            },
            render_generation: 1,
            effect_identity_signature: 1,
        }),
        held_release_obligations,
    };
    let payload = WindowExitPayload::new(identity, prepared, started_at)
        .expect("valid frozen WindowExit payload");
    state
        .window_exit_payloads
        .publish_candidate_exact(identity, payload)
        .expect("publish candidate payload");
    let committed = state
        .presentation_animator
        .commit(PresentationTransactionRequest::mixed(
            started_at,
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                rect,
                target_rect,
                AnimationCurve::easing(Duration::from_millis(1), EasingCurve::EaseInCubic),
            )],
            vec![PresentationOpacityMutation::new(
                scene_node_id,
                PresentationOpacity::OPAQUE,
                PresentationOpacity::TRANSPARENT,
                AnimationCurve::easing(Duration::from_millis(1), EasingCurve::EaseInCubic),
            )],
        ))
        .expect("commit exact Geometry+Opacity pair");
    let mut geometry_revision_id = None;
    let mut opacity_revision_id = None;
    for member in committed.members() {
        match member.kind() {
            PresentationTransactionMemberKind::Property(
                crate::presentation_animation::PresentationPropertyKind::Geometry,
            ) => geometry_revision_id = Some(member.revision_id()),
            PresentationTransactionMemberKind::Property(
                crate::presentation_animation::PresentationPropertyKind::Opacity,
            ) => opacity_revision_id = Some(member.revision_id()),
            _ => {}
        }
    }
    let revisions = WindowExitPropertyRevisions {
        transaction_id: committed.id(),
        geometry_revision_id: geometry_revision_id.expect("Geometry revision"),
        opacity_revision_id: opacity_revision_id.expect("Opacity revision"),
    };
    assert!(
        state
            .window_exit_payloads
            .set_property_revisions_before_activation(identity, revisions)
    );
    assert_eq!(
        state
            .presentation_animator
            .activate_retained_visual_exact(identity)
            .expect("activate retained WindowExit owner"),
        None
    );
    let evidence = WindowExitFrameEvidence {
        identity,
        payload_id: identity.revision_id().get(),
        root_surface_id,
        scene_node_id,
    };
    (identity, evidence)
}

#[test]
fn exact_promoted_surface_group_proves_the_window_exit_source() {
    let (promoted, current) = fixture();
    assert!(prove_window_exit_source(
        Some(&promoted),
        promoted.output_id,
        10,
        50,
        900,
        &current,
        true,
    ));
}

#[test]
fn missing_or_stale_physical_evidence_rejects_the_window_exit_source() {
    let (promoted, current) = fixture();
    assert!(!prove_window_exit_source(
        None,
        promoted.output_id,
        10,
        50,
        900,
        &current,
        true,
    ));
    assert!(!prove_window_exit_source(
        Some(&promoted),
        promoted.output_id,
        10,
        51,
        900,
        &current,
        true,
    ));
    assert!(!prove_window_exit_source(
        Some(&promoted),
        promoted.output_id,
        10,
        50,
        901,
        &current,
        true,
    ));
    assert!(!prove_window_exit_source(
        Some(&promoted),
        promoted.output_id,
        10,
        50,
        900,
        &current,
        false,
    ));
}

#[test]
fn any_surface_identity_or_membership_change_rejects_window_exit_capture() {
    let (promoted, current) = fixture();
    for mutation in 0..6 {
        let mut changed = current.clone();
        match mutation {
            0 => changed[0].key.generation += 1,
            1 => changed[0].commit_sequence.0 += 1,
            2 => changed[0].buffer_id = BufferId::for_tests(999),
            3 => changed[0].scene_node_id = SceneNodeId::from_raw(81).expect("test node"),
            4 => {
                changed.pop();
            }
            _ => changed.push(evidence(12, 1, 1, 402, 80, 10, 10)),
        }
        assert!(!prove_window_exit_source(
            Some(&promoted),
            promoted.output_id,
            10,
            50,
            900,
            &changed,
            true,
        ));
    }
}

#[test]
fn foreign_output_evidence_never_proves_window_exit_capture() {
    let (promoted, current) = fixture();
    assert!(!prove_window_exit_source(
        Some(&promoted),
        OutputId::from_raw(2).expect("foreign output"),
        10,
        50,
        900,
        &current,
        true,
    ));
}

#[test]
fn mathematically_settled_exit_waits_for_exact_physical_ack_before_retiring() {
    let mut state = CompositorState::new(None);
    let output_id = state.native_output_id().expect("test output id");
    let sync_point = ExplicitSyncPoint::for_tests_with_signal_script(99, 700, [true]);
    let obligation = crate::compositor::state_data::DmabufReleaseObligation {
        buffer_id: BufferId::for_tests(701),
        release: crate::compositor::state_data::SurfaceBufferRelease::ExplicitSync(
            sync_point.clone(),
        ),
    };
    let (identity, evidence) = activate_window_exit(
        &mut state,
        vec![WindowExitReleaseObligation {
            surface_id: evidence_surface_id(),
            obligation: obligation.clone(),
        }],
    );
    assert!(state.has_unowned_frame_work());
    assert!(
        state
            .direct_scanout_scene_blockers()
            .reasons()
            .contains(&crate::compositor::DirectScanoutSceneRejection::WindowExitAnimation)
    );
    assert!(state.buffer_release_is_owned(&obligation));
    let target = PresentationWindowTarget::with_scene_node(
        evidence.scene_node_id,
        evidence.root_surface_id,
        crate::presentation_animation::PresentationRect::new(10.0, 20.0, 300.0, 200.0)
            .expect("test target rect"),
    )
    .with_canonical_opacity(PresentationOpacity::OPAQUE)
    .with_canonical_clip(PresentationClip::Unbounded);
    let settled_sample = state.presentation_animator.sample(
        output_id,
        AnimationTime::from_nanos(2_000_010),
        PresentationSampleTimeSource::ScheduledTarget,
        &[target],
    );
    let settled_snapshot = PresentationFrameSnapshot::from_sample(&settled_sample);

    state.settle_window_exit_physical(1, &settled_snapshot, &[evidence]);
    assert!(state.window_exit_payloads.get_exact(identity).is_some());
    assert!(state.buffer_release_is_owned(&obligation));
    assert_eq!(
        state.presentation_animator.active_retained_visual(
            evidence.scene_node_id,
            PresentationRetainedVisualKind::WindowExit,
        ),
        Some(identity),
        "mathematical settlement alone must not retire WindowExit"
    );

    state.publish_presented_presentation(1, &settled_snapshot);
    let mut stale_evidence = evidence;
    stale_evidence.payload_id = stale_evidence.payload_id.saturating_add(1);
    state.settle_window_exit_physical(1, &settled_snapshot, &[stale_evidence]);
    assert!(state.window_exit_payloads.get_exact(identity).is_some());
    assert!(state.buffer_release_is_owned(&obligation));

    state.settle_window_exit_physical(1, &settled_snapshot, &[evidence]);
    assert!(state.window_exit_payloads.get_exact(identity).is_none());
    assert!(
        state
            .pending_dmabuf_buffer_releases
            .iter()
            .any(|pending| pending.same_release_token(&obligation))
    );
    assert!(state.buffer_release_is_owned(&obligation));
    assert_eq!(
        sync_point
            .signal_script
            .as_ref()
            .expect("test sync signal script")
            .lock()
            .expect("test signal script lock")
            .len(),
        1,
        "physical retirement queues the exact release token without directly signaling it"
    );
    assert!(
        !state
            .presentation_animator
            .has_pending_visible(&[evidence.scene_node_id])
    );
    assert!(
        !state
            .direct_scanout_scene_blockers()
            .reasons()
            .contains(&crate::compositor::DirectScanoutSceneRejection::WindowExitAnimation)
    );
    assert_eq!(
        state.presentation_animator.active_retained_visual(
            evidence.scene_node_id,
            PresentationRetainedVisualKind::WindowExit,
        ),
        None
    );
}

#[test]
fn remap_retirement_cancels_close_tracks_and_returns_held_release_to_queue() {
    let mut state = CompositorState::new(None);
    let sync_point = ExplicitSyncPoint::for_tests_with_signal_script(99, 702, [true]);
    let obligation = crate::compositor::state_data::DmabufReleaseObligation {
        buffer_id: BufferId::for_tests(703),
        release: crate::compositor::state_data::SurfaceBufferRelease::ExplicitSync(
            sync_point.clone(),
        ),
    };
    let (identity, evidence) = activate_window_exit(
        &mut state,
        vec![WindowExitReleaseObligation {
            surface_id: evidence_surface_id(),
            obligation: obligation.clone(),
        }],
    );

    assert!(state.retire_window_exit_for_root(evidence.root_surface_id));
    assert!(state.window_exit_payloads.get_exact(identity).is_none());
    assert!(
        !state
            .presentation_animator
            .has_geometry_track(evidence.scene_node_id)
    );
    assert!(
        !state
            .presentation_animator
            .has_opacity_track(evidence.scene_node_id)
    );
    assert!(
        state
            .pending_dmabuf_buffer_releases
            .iter()
            .any(|pending| pending.same_release_token(&obligation))
    );
    assert_eq!(
        sync_point
            .signal_script
            .as_ref()
            .expect("test sync signal script")
            .lock()
            .expect("test signal script lock")
            .len(),
        1
    );
}

fn evidence_surface_id() -> u32 {
    10
}
