use super::*;
use crate::core::SceneNodeId;
use crate::presentation_animation::{
    PresentationRetainedVisualIdentity, PresentationRetainedVisualKind, PresentationRevisionId,
    PresentationTransactionId,
};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEST_PRESENTATION_ID: AtomicU64 = AtomicU64::new(1_000_000);

fn test_identity(window_id: WindowId) -> PresentationRetainedVisualIdentity {
    let value = NEXT_TEST_PRESENTATION_ID.fetch_add(1, Ordering::Relaxed);
    retained_identity(window_id.get(), value, value)
}

fn retained_identity(
    scene_node_id: u64,
    transaction_id: u64,
    revision_id: u64,
) -> PresentationRetainedVisualIdentity {
    PresentationRetainedVisualIdentity::new(
        SceneNodeId::from_raw(scene_node_id).expect("scene node"),
        PresentationRetainedVisualKind::WindowLifecycle,
        PresentationTransactionId::from_raw(transaction_id).expect("transaction id"),
        PresentationRevisionId::from_raw(revision_id).expect("revision id"),
    )
}

fn request(
    window_id: WindowId,
    root_surface_id: u32,
    source_rect: PresentationRect,
    anchor_rect: PresentationRect,
    direction: LifecycleDirection,
) -> LifecycleTransitionRequest {
    request_with_group(
        window_id,
        root_surface_id,
        LifecycleVisualGroup::from_bounds(
            source_rect,
            source_rect,
            source_rect,
            anchor_rect,
            1920,
            1080,
        )
        .expect("valid visual group"),
        direction,
    )
}

fn request_with_group(
    window_id: WindowId,
    root_surface_id: u32,
    visual_group: LifecycleVisualGroup,
    direction: LifecycleDirection,
) -> LifecycleTransitionRequest {
    LifecycleTransitionRequest {
        presentation_identity: test_identity(window_id),
        window_id,
        root_surface_id,
        visual_group,
        direction,
        resolved_effect_scene: ResolvedEffectScene::default(),
    }
}

#[test]
fn reversal_keeps_scene_owner_and_visual_sample_but_replaces_presentation_identity() {
    let window_id = WindowId::from_raw(601).expect("window");
    let scene_node_id = SceneNodeId::from_raw(701).expect("scene node");
    let root_surface_id = 801;
    let visual_group = LifecycleVisualGroup::from_bounds(
        rect(20.0, 30.0, 400.0, 300.0),
        rect(20.0, 30.0, 400.0, 300.0),
        rect(20.0, 30.0, 400.0, 300.0),
        rect(700.0, 500.0, 40.0, 40.0),
        1920,
        1080,
    )
    .expect("visual group");
    let first_identity = retained_identity(701, 901, 1001);
    let second_identity = retained_identity(701, 902, 1002);
    let make_request =
        |presentation_identity, direction, visual_group| LifecycleTransitionRequest {
            presentation_identity,
            window_id,
            root_surface_id,
            visual_group,
            direction,
            resolved_effect_scene: ResolvedEffectScene::default(),
        };
    let mut animator = WindowLifecycleAnimator::new(true);
    assert_eq!(
        animator.start_or_reverse(
            make_request(first_identity, LifecycleDirection::Minimize, visual_group,),
            None,
            AnimationTime::from_nanos(0),
            1.0,
        ),
        Some(first_identity)
    );
    let reverse_at = AnimationTime::from_nanos(120_400_000);
    let before = animator
        .sample(first_identity, reverse_at)
        .expect("sample before");
    let second = animator
        .start_or_reverse(
            make_request(
                second_identity,
                LifecycleDirection::Restore,
                LifecycleVisualGroup::from_bounds(
                    rect(1000.0, 1000.0, 100.0, 100.0),
                    rect(1000.0, 1000.0, 100.0, 100.0),
                    rect(1000.0, 1000.0, 100.0, 100.0),
                    rect(1200.0, 1200.0, 20.0, 20.0),
                    1920,
                    1080,
                )
                .expect("replacement request visual"),
            ),
            Some(first_identity),
            reverse_at,
            1.0,
        )
        .expect("restore reverse");
    let after = animator.sample(second, reverse_at).expect("sample after");

    assert_eq!(before.progress, after.progress);
    assert!((after.progress - 0.43).abs() < 1e-9);
    assert_eq!(after.visual_group, visual_group);
    assert_eq!(second.scene_node_id(), scene_node_id);
    assert_ne!(first_identity.transaction_id(), second.transaction_id());
    assert_ne!(first_identity.revision_id(), second.revision_id());
    assert_eq!(after.presentation_identity, second);
    assert!(animator.acknowledge(first_identity, true).is_none());
    assert_eq!(animator.active_count(), 1);

    let old_identity_sample = LampWindowSample {
        presentation_identity: first_identity,
        ..after
    };
    let first_frame = LifecycleFrameSnapshot::from_sample(&LifecycleSceneSample {
        sampled_at: reverse_at,
        lamps: vec![old_identity_sample],
        visual_sources: Vec::new(),
    });
    let second_frame = LifecycleFrameSnapshot::from_sample(&LifecycleSceneSample {
        sampled_at: reverse_at,
        lamps: vec![after],
        visual_sources: Vec::new(),
    });
    assert_ne!(first_frame.signature, second_frame.signature);
}

#[test]
fn sampling_uses_only_explicit_exact_owner_identities() {
    let window = WindowId::from_raw(701).expect("window");
    let source = rect(20.0, 30.0, 400.0, 300.0);
    let anchor = rect(700.0, 500.0, 40.0, 40.0);
    let mut animator = WindowLifecycleAnimator::new(true);
    let first_request = request(window, 801, source, anchor, LifecycleDirection::Minimize);
    let orphan = first_request.presentation_identity;
    animator
        .start_or_reverse(first_request, None, AnimationTime::from_nanos(0), 1.0)
        .expect("first execution installed");

    let current_request = request(window, 801, source, anchor, LifecycleDirection::Restore);
    let current = current_request.presentation_identity;
    animator
        .start_or_reverse(current_request, None, AnimationTime::from_nanos(1), 1.0)
        .expect("second exact execution installed");

    assert!(
        animator
            .sample(orphan, AnimationTime::from_nanos(10))
            .is_some()
    );
    let empty = animator.sample_scene(&[], AnimationTime::from_nanos(10));
    assert!(empty.lamps.is_empty());
    assert!(empty.visual_sources.is_empty());
    let active = animator.sample_scene(&[current], AnimationTime::from_nanos(10));
    assert_eq!(active.lamps.len(), 1);
    assert_eq!(active.lamps[0].presentation_identity, current);
    assert_eq!(active.visual_sources.len(), 1);
    assert_eq!(active.visual_sources[0].presentation_identity, current);
    assert!(
        animator
            .sample(current, AnimationTime::from_nanos(10))
            .is_some()
    );
}

#[test]
fn failed_exact_reversal_leaves_old_execution_unchanged() {
    let window = WindowId::from_raw(702).expect("window");
    let source = rect(20.0, 30.0, 400.0, 300.0);
    let anchor = rect(700.0, 500.0, 40.0, 40.0);
    let mut animator = WindowLifecycleAnimator::new(true);
    let old_request = request(window, 802, source, anchor, LifecycleDirection::Minimize);
    let old = old_request.presentation_identity;
    animator
        .start_or_reverse(old_request, None, AnimationTime::from_nanos(0), 1.0)
        .expect("old execution installed");
    let before = animator
        .sample(old, AnimationTime::from_nanos(100_000_000))
        .expect("old execution sample");

    let new_request = request(window, 802, source, anchor, LifecycleDirection::Restore);
    let new = new_request.presentation_identity;
    let missing_previous = retained_identity(window.get(), 990_001, 990_002);
    assert!(
        animator
            .start_or_reverse(
                new_request,
                Some(missing_previous),
                AnimationTime::from_nanos(100_000_000),
                1.0,
            )
            .is_none()
    );

    assert_eq!(
        animator.sample(old, AnimationTime::from_nanos(100_000_000)),
        Some(before)
    );
    assert!(
        animator
            .sample(new, AnimationTime::from_nanos(100_000_000))
            .is_none()
    );
    assert_eq!(animator.active_count(), 1);
}
use crate::presentation_animation::PresentationRect;

fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
    PresentationRect::new(x, y, width, height).expect("valid rectangle")
}

#[test]
fn portal_rect_is_aspect_fit_centered_and_direction_aligned() {
    let fixtures = [
        rect(-960.0, 120.0, 1600.0, 900.0), // 16:9, negative global x
        rect(80.0, -400.0, 400.0, 900.0),   // portrait, negative global y
        rect(120.0, 120.0, 600.0, 600.0),   // square
        rect(-0.01, -0.02, 0.01, 0.02),     // extremely small valid source
    ];
    for anchor in [
        rect(-32.0, 900.0, 64.0, 64.0),
        rect(0.001, 0.002, 0.003, 0.004), // extremely small valid anchor
    ] {
        for source in fixtures {
            for direction in [
                LampDirection::Bottom,
                LampDirection::Top,
                LampDirection::Left,
                LampDirection::Right,
            ] {
                let portal = lamp_portal_rect(source, anchor, direction).expect("valid portal");
                assert!(valid_rect(portal));
                assert!(rect_contains_rect(anchor, portal));
                assert!(
                    (portal.width() / portal.height() - source.width() / source.height()).abs()
                        < 1.0e-12
                );
                match direction {
                    LampDirection::Bottom => {
                        assert_eq!(portal.y(), anchor.y());
                        assert_eq!(
                            portal.x() + portal.width() * 0.5,
                            anchor.x() + anchor.width() * 0.5
                        );
                    }
                    LampDirection::Top => {
                        assert_eq!(portal.y() + portal.height(), anchor.y() + anchor.height());
                        assert_eq!(
                            portal.x() + portal.width() * 0.5,
                            anchor.x() + anchor.width() * 0.5
                        );
                    }
                    LampDirection::Left => {
                        assert_eq!(portal.x() + portal.width(), anchor.x() + anchor.width());
                        assert_eq!(
                            portal.y() + portal.height() * 0.5,
                            anchor.y() + anchor.height() * 0.5
                        );
                    }
                    LampDirection::Right => {
                        assert_eq!(portal.x(), anchor.x());
                        assert_eq!(
                            portal.y() + portal.height() * 0.5,
                            anchor.y() + anchor.height() * 0.5
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn sink_terminal_depth_is_source_aspect_independent() {
    let anchor = rect(900.0, 900.0, 64.0, 64.0);
    let sources = [
        rect(100.0, 100.0, 1600.0, 900.0), // wide
        rect(600.0, 100.0, 900.0, 1600.0), // portrait
        rect(600.0, 100.0, 800.0, 800.0),  // square
    ];
    let terminal_depths = |source: PresentationRect| {
        let group = LifecycleVisualGroup::from_bounds(source, source, source, anchor, 1920, 1080)
            .expect("valid visual group");
        let center_x = source.x() + source.width() * 0.5;
        let leading = lamp_warp_visual_point(group, [center_x, source.y()], 1.0);
        let trailing = lamp_warp_visual_point(group, [center_x, source.y() + source.height()], 1.0);
        ((trailing[1] - leading[1]).abs(), group.portal_rect.height())
    };

    let depths = sources.map(terminal_depths);
    let sink_depths = depths.map(|(sink_depth, _)| sink_depth);
    let v23_portal_depths = depths.map(|(_, portal_depth)| portal_depth);
    assert_eq!(v23_portal_depths, [36.0, 64.0, 64.0]);
    assert!((v23_portal_depths[0] - v23_portal_depths[1]).abs() > 1.0e-12);
    assert!(
        sink_depths
            .windows(2)
            .all(|pair| (pair[0] - pair[1]).abs() < 1.0e-12),
        "sink depth must be source-aspect independent; measured depths were {sink_depths:?}"
    );
    assert_eq!(sink_depths, [ASTREA_LAMP_SINK_THICKNESS; 3]);
}

#[test]
fn sink_rect_is_aspect_independent_and_directionally_symmetric() {
    let anchor = rect(-32.0, 900.0, 64.0, 64.0);
    let sources = [
        rect(-960.0, 120.0, 1600.0, 900.0),
        rect(80.0, -400.0, 400.0, 900.0),
        rect(120.0, 120.0, 600.0, 600.0),
    ];
    let mut bottom_depths = Vec::new();
    for source in sources {
        let portal = lamp_portal_rect(source, anchor, LampDirection::Bottom)
            .expect("valid aspect-fit portal");
        let sink = lamp_sink_rect(
            anchor,
            portal,
            LampDirection::Bottom,
            ASTREA_LAMP_ABSORB_DEPTH,
            ASTREA_LAMP_SINK_THICKNESS,
        )
        .expect("valid sink");
        assert!(rect_contains_rect(anchor, sink));
        assert!(sink.x().is_finite() && sink.y().is_finite());
        assert!(
            (sink.y() + sink.height() * 0.5 - (anchor.y() + 0.6 * anchor.height())).abs() < 1.0e-12
        );
        assert_eq!(sink.x(), portal.x());
        assert_eq!(sink.width(), portal.width());
        assert_eq!(sink.height(), ASTREA_LAMP_SINK_THICKNESS);
        bottom_depths.push(sink.height());
    }
    assert!(bottom_depths.windows(2).all(|pair| pair[0] == pair[1]));

    let cases = [
        (LampDirection::Bottom, rect(900.0, 900.0, 64.0, 64.0)),
        (LampDirection::Top, rect(900.0, 12.0, 64.0, 64.0)),
        (LampDirection::Right, rect(1840.0, 500.0, 64.0, 64.0)),
        (LampDirection::Left, rect(12.0, 500.0, 64.0, 64.0)),
    ];
    let source = rect(400.0, 300.0, 800.0, 600.0);
    for (direction, directional_anchor) in cases {
        let portal = lamp_portal_rect(source, directional_anchor, direction)
            .expect("valid directional portal");
        let sink = lamp_sink_rect(
            directional_anchor,
            portal,
            direction,
            ASTREA_LAMP_ABSORB_DEPTH,
            ASTREA_LAMP_SINK_THICKNESS,
        )
        .expect("valid directional sink");
        assert!(rect_contains_rect(directional_anchor, sink));
        assert_eq!(
            match direction {
                LampDirection::Top | LampDirection::Bottom => sink.width(),
                LampDirection::Left | LampDirection::Right => sink.height(),
            },
            match direction {
                LampDirection::Top | LampDirection::Bottom => portal.width(),
                LampDirection::Left | LampDirection::Right => portal.height(),
            }
        );
        let (anchor_start, anchor_end) = axis_bounds(directional_anchor, direction);
        let center = match direction {
            LampDirection::Top | LampDirection::Bottom => sink.y() + sink.height() * 0.5,
            LampDirection::Left | LampDirection::Right => sink.x() + sink.width() * 0.5,
        };
        let expected_depth = match direction {
            LampDirection::Bottom | LampDirection::Right => {
                (center - anchor_start) / (anchor_end - anchor_start)
            }
            LampDirection::Top | LampDirection::Left => {
                (anchor_end - center) / (anchor_end - anchor_start)
            }
        };
        assert!((expected_depth - ASTREA_LAMP_ABSORB_DEPTH).abs() < 1.0e-12);
    }

    let tiny_anchor = rect(-0.003, -0.004, 0.003, 0.004);
    let tiny_portal =
        lamp_portal_rect(source, tiny_anchor, LampDirection::Bottom).expect("valid tiny portal");
    let tiny_sink = lamp_sink_rect(
        tiny_anchor,
        tiny_portal,
        LampDirection::Bottom,
        ASTREA_LAMP_ABSORB_DEPTH,
        ASTREA_LAMP_SINK_THICKNESS,
    )
    .expect("valid tiny sink");
    assert!(rect_contains_rect(tiny_anchor, tiny_sink));
    assert!(tiny_sink.width() > 0.0 && tiny_sink.height() > 0.0);
}

#[test]
fn cubic_funnel_profile_is_bounded_monotonic_and_wider_than_linear_mid_funnel() {
    let samples = [0.00, 0.10, 0.25, 0.50, 0.75, 0.90, 1.00];
    let expected = [
        [0.0, 0.04987, 0.15203125, 0.38375, 0.67359375, 0.86643, 1.0],
        [
            0.0,
            0.033535,
            0.109140625,
            0.306875,
            0.601171875,
            0.827415,
            1.0,
        ],
        [0.0, 0.0172, 0.06625, 0.23, 0.52875, 0.7884, 1.0],
    ];
    for (shape, expected_values) in [
        (ASTREA_LAMP_INITIAL_SHAPE_FACTOR, expected[0]),
        (0.50, expected[1]),
        (ASTREA_LAMP_MAX_SHAPE_FACTOR, expected[2]),
    ] {
        let values = samples.map(|t| cubic_funnel_profile(t, shape));
        assert_eq!(values[0], 0.0);
        assert_eq!(values[6], 1.0);
        assert!(
            values
                .into_iter()
                .zip(expected_values)
                .all(|(value, expected)| {
                    value.is_finite()
                        && (0.0..=1.0).contains(&value)
                        && (value - expected).abs() < 1.0e-12
                })
        );
        assert!(values.windows(2).all(|pair| pair[1] >= pair[0]));
        assert!(cubic_funnel_profile(0.50, shape) < 0.50);
    }
}

#[test]
fn lamp_footprint_covers_ssd_above_client_after_visual_group_fix() {
    let client = rect(400.0, 100.0, 800.0, 600.0);
    let ssd_outer = rect(384.0, 60.0, 832.0, 640.0);
    let anchor = rect(900.0, 900.0, 64.0, 64.0);

    let visual = LifecycleVisualGroup::from_bounds(client, ssd_outer, client, anchor, 1920, 1080)
        .expect("valid visual group");
    let footprint = lamp_footprint(visual).expect("valid footprint");

    assert!(footprint.y() <= ssd_outer.y());
    assert!(footprint.y() + footprint.height() >= ssd_outer.y() + ssd_outer.height());
}

#[test]
fn canonical_visual_group_unions_client_subsurface_and_ssd_outer_bounds() {
    let actual = canonical_visual_rect(
        rect(400.0, 100.0, 800.0, 600.0),
        [rect(360.0, 120.0, 32.0, 760.0)],
        Some(rect(384.0, 60.0, 832.0, 640.0)),
    );
    assert_eq!(actual, Some(rect(360.0, 60.0, 856.0, 820.0)));
}

#[test]
fn canonical_visual_group_does_not_expand_csd_window() {
    let actual = canonical_visual_rect(rect(400.0, 100.0, 800.0, 600.0), [], None);
    assert_eq!(actual, Some(rect(400.0, 100.0, 800.0, 600.0)));
}

#[test]
fn presented_visual_group_reuses_canonical_to_presented_client_affine() {
    let actual = presented_visual_rect(
        rect(400.0, 100.0, 800.0, 600.0),
        rect(360.0, 60.0, 832.0, 640.0),
        rect(200.0, 160.0, 960.0, 720.0),
    );
    assert_eq!(actual, Some(rect(152.0, 112.0, 998.4, 768.0)));
}

#[test]
fn active_transition_ignores_live_visual_mutations_on_reversal() {
    let window = WindowId::from_raw(18).expect("valid window id");
    let first_group = LifecycleVisualGroup::from_bounds(
        rect(400.0, 100.0, 800.0, 600.0),
        rect(384.0, 60.0, 832.0, 640.0),
        rect(400.0, 100.0, 800.0, 600.0),
        rect(900.0, 900.0, 64.0, 64.0),
        1920,
        1080,
    )
    .expect("valid visual group");
    // These changed bounds model a relaid-out SSD/subsurface group and a
    // newly reported Dock anchor while the first transition still owns
    // physical presentation. The animator must ignore both snapshots.
    let changed_group = LifecycleVisualGroup::from_bounds(
        rect(400.0, 100.0, 800.0, 600.0),
        rect(384.0, 20.0, 832.0, 680.0),
        rect(400.0, 100.0, 800.0, 600.0),
        rect(1000.0, 900.0, 64.0, 64.0),
        1920,
        1080,
    )
    .expect("valid visual group");
    let mut animator = WindowLifecycleAnimator::new(true);
    let first = animator
        .start_or_reverse(
            request_with_group(window, 18, first_group, LifecycleDirection::Minimize),
            None,
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("minimize starts");
    let before = animator
        .sample(first, AnimationTime::from_nanos(100_000_000))
        .expect("sample before reversal");
    let second = animator
        .start_or_reverse(
            request_with_group(window, 18, changed_group, LifecycleDirection::Restore),
            Some(first),
            AnimationTime::from_nanos(100_000_000),
            1.0,
        )
        .expect("restore reverses");
    let after = animator
        .sample(second, AnimationTime::from_nanos(100_000_000))
        .expect("sample after reversal");

    assert_ne!(first, second);
    assert_eq!(before.visual_group, after.visual_group);
    assert_eq!(
        before.visual_group.anchor_rect,
        after.visual_group.anchor_rect
    );
}

#[test]
fn settled_new_transition_captures_new_visual_bounds() {
    let window = WindowId::from_raw(19).expect("valid window id");
    let first_group = LifecycleVisualGroup::from_bounds(
        rect(400.0, 100.0, 800.0, 600.0),
        rect(384.0, 60.0, 832.0, 640.0),
        rect(400.0, 100.0, 800.0, 600.0),
        rect(900.0, 900.0, 64.0, 64.0),
        1920,
        1080,
    )
    .expect("valid visual group");
    let second_group = LifecycleVisualGroup::from_bounds(
        rect(400.0, 100.0, 800.0, 600.0),
        rect(384.0, 20.0, 832.0, 680.0),
        rect(400.0, 100.0, 800.0, 600.0),
        rect(900.0, 900.0, 64.0, 64.0),
        1920,
        1080,
    )
    .expect("valid visual group");
    let mut animator = WindowLifecycleAnimator::new(true);
    let first = animator
        .start_or_reverse(
            request_with_group(window, 19, first_group, LifecycleDirection::Minimize),
            None,
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("minimize starts");
    assert!(animator.snap_to_endpoint(first, AnimationTime::from_nanos(280_000_000),));
    assert_eq!(animator.acknowledge(first, true), Some(first));
    let second = animator
        .start_or_reverse(
            request_with_group(window, 19, second_group, LifecycleDirection::Minimize),
            None,
            AnimationTime::from_nanos(300_000_000),
            1.0,
        )
        .expect("new transition starts");
    let sample = animator
        .sample(second, AnimationTime::from_nanos(300_000_000))
        .expect("new transition sample");
    assert_eq!(sample.visual_group.canonical_visual_rect.y(), 20.0);
}

#[test]
fn lifecycle_snapshot_signature_includes_sink_geometry() {
    let window_id = WindowId::from_raw(20).expect("valid window id");
    let visual_group = LifecycleVisualGroup::from_bounds(
        rect(400.0, 100.0, 800.0, 600.0),
        rect(384.0, 60.0, 832.0, 640.0),
        rect(400.0, 100.0, 800.0, 600.0),
        rect(900.0, 900.0, 64.0, 64.0),
        1920,
        1080,
    )
    .expect("valid visual group");
    let sample = LifecycleSceneSample {
        sampled_at: AnimationTime::from_nanos(1),
        lamps: vec![LampWindowSample {
            window_id,
            root_surface_id: 20,
            presentation_identity: test_identity(window_id),
            visual_group,
            progress: 0.5,
            opacity: 1.0,
            direction: LifecycleDirection::Minimize,
            mathematically_settled: false,
        }],
        visual_sources: Vec::new(),
    };
    let mut snapshot = LifecycleFrameSnapshot::from_sample(&sample);
    let original_signature = snapshot.signature;
    snapshot.lamps[0].visual_group.sink_rect = PresentationRect::new(
        visual_group.sink_rect.x(),
        visual_group.sink_rect.y() + 1.0,
        visual_group.sink_rect.width(),
        visual_group.sink_rect.height(),
    )
    .expect("valid changed sink rectangle");
    snapshot.refresh_signature();
    assert_ne!(snapshot.signature, original_signature);
}

#[test]
fn lifecycle_footprint_covers_subsurface_outside_root_and_anchor() {
    let visual_group = LifecycleVisualGroup::from_bounds(
        rect(400.0, 100.0, 800.0, 600.0),
        canonical_visual_rect(
            rect(400.0, 100.0, 800.0, 600.0),
            [rect(360.0, 120.0, 32.0, 760.0)],
            None,
        )
        .expect("valid visual bounds"),
        rect(400.0, 100.0, 800.0, 600.0),
        rect(1200.0, 900.0, 64.0, 64.0),
        1920,
        1080,
    )
    .expect("valid visual group");
    let footprint = lamp_footprint(visual_group).expect("valid footprint");
    assert!(footprint.x() <= 360.0);
    assert!(footprint.y() <= 100.0);
    assert!(footprint.x() + footprint.width() >= 1264.0);
    assert!(footprint.y() + footprint.height() >= 964.0);
}

#[test]
fn lifecycle_footprint_includes_overlap_bump_excursion() {
    let source = rect(400.0, 800.0, 800.0, 200.0);
    let anchor = rect(900.0, 900.0, 64.0, 64.0);
    let visual_group =
        LifecycleVisualGroup::from_bounds(source, source, source, anchor, 1920, 1080)
            .expect("valid visual group");
    assert!(visual_group.bump_distance > 0.0);
    let footprint = lamp_footprint(visual_group).expect("valid footprint");
    assert!(footprint.y() < source.y());
    assert!(footprint.y() + footprint.height() > source.y() + source.height());
}

#[test]
fn lifecycle_footprint_intersection_is_finite_at_every_output_edge() {
    let cases = [
        (
            rect(-90.0, 100.0, 120.0, 120.0),
            rect(-30.0, 110.0, 20.0, 20.0),
        ),
        (
            rect(1870.0, 100.0, 120.0, 120.0),
            rect(1900.0, 110.0, 20.0, 20.0),
        ),
        (
            rect(100.0, -90.0, 120.0, 120.0),
            rect(110.0, -30.0, 20.0, 20.0),
        ),
        (
            rect(100.0, 1070.0, 120.0, 120.0),
            rect(110.0, 1090.0, 20.0, 20.0),
        ),
    ];
    for (source, anchor) in cases {
        let visual_group =
            LifecycleVisualGroup::from_bounds(source, source, source, anchor, 1920, 1080)
                .expect("valid visual group");
        assert!(
            lamp_footprint(visual_group)
                .expect("finite footprint")
                .x()
                .is_finite()
        );
        assert!(lamp_footprint_intersects_output(visual_group, 1920, 1080));
    }
}

#[test]
fn lamp_opacity_stays_full_until_narrow_endpoint_interval() {
    assert_eq!(lamp_opacity(0.97), 1.0);
    assert!(lamp_opacity(0.99) > 0.0);
    assert_eq!(lamp_opacity(1.0), 0.0);
}

#[test]
fn lamp_continuous_funnel_has_no_internal_aggregate_stop() {
    let source = rect(300.0, 200.0, 640.0, 480.0);
    let anchor = rect(700.0, 900.0, 64.0, 64.0);
    let direction = LampDirection::Bottom;
    let shape_factor = 0.60;
    let points = [
        [620.0, 680.0], // near-edge center
        [620.0, 440.0], // window center
        [620.0, 200.0], // trailing-edge center
        [300.0, 440.0], // left visual edge
        [940.0, 440.0], // right visual edge
    ];
    let aggregate_velocity = |progress: f64, bump_distance: f64| {
        let half_step = 1.0e-4;
        let sum_squared_displacement = points
            .into_iter()
            .map(|point| {
                let before = lamp_warp_point_directional(
                    source,
                    anchor,
                    direction,
                    shape_factor,
                    bump_distance,
                    lamp_motion_channels(progress - half_step, bump_distance),
                    point,
                );
                let after = lamp_warp_point_directional(
                    source,
                    anchor,
                    direction,
                    shape_factor,
                    bump_distance,
                    lamp_motion_channels(progress + half_step, bump_distance),
                    point,
                );
                (after[0] - before[0]).powi(2) + (after[1] - before[1]).powi(2)
            })
            .sum::<f64>();
        (sum_squared_displacement / points.len() as f64).sqrt() / (2.0 * half_step)
    };

    let sample_count = 512;
    let velocities = (1..sample_count)
        .map(|step| {
            let progress = 0.08 + 0.84 * f64::from(step) / f64::from(sample_count);
            aggregate_velocity(progress, 0.0)
        })
        .collect::<Vec<_>>();
    let maximum = velocities.iter().copied().fold(0.0, f64::max);
    let minimum = velocities.iter().copied().fold(f64::INFINITY, f64::min);
    assert!(maximum > 1.0);
    assert!(
        minimum > 0.01,
        "aggregate mesh velocity stopped internally: minimum={minimum}, maximum={maximum}"
    );

    for boundary in [0.29577464788732394, 0.07792207792207792] {
        let bump_distance = if boundary < 0.1 { 48.0 } else { 0.0 };
        let before = aggregate_velocity(boundary - 0.01, bump_distance);
        let at_boundary = aggregate_velocity(boundary, bump_distance);
        let after = aggregate_velocity(boundary + 0.01, bump_distance);
        assert!(before > 1.0);
        assert!(after > 1.0);
        assert!(at_boundary > before.min(after) * 0.10);
    }
}

#[test]
fn lamp_motion_channels_use_one_overlapping_timeline() {
    let at_zero = lamp_motion_channels(0.0, 0.0);
    assert_eq!(at_zero.temporal_progress, 0.0);
    assert_eq!(at_zero.contraction_progress, 0.0);
    assert_eq!(at_zero.translation_progress, 0.0);
    assert_eq!(at_zero.retreat_progress, 0.0);

    let representative = lamp_motion_channels(0.4, 0.0);
    assert!((representative.temporal_progress - 0.256).abs() < 1.0e-12);
    assert!(representative.contraction_progress > 0.0);
    assert!(representative.translation_progress > 0.0);
    assert_eq!(representative.retreat_progress, 0.0);

    let overlap = lamp_motion_channels(0.4, 48.0);
    assert!(overlap.contraction_progress > 0.0);
    assert!(overlap.translation_progress > 0.0);
    assert!(overlap.retreat_progress > 0.0);
    assert!(overlap.retreat_progress <= 1.0);
    assert_eq!(lamp_motion_channels(0.5, 0.0).retreat_progress, 0.0);
    assert_eq!(lamp_motion_channels(1.0, 48.0).temporal_progress, 1.0);
    assert_eq!(lamp_motion_channels(1.0, 48.0).translation_progress, 1.0);
}

#[test]
fn lamp_translation_soft_start_is_c1_at_its_internal_join() {
    let start = ASTREA_LAMP_TRANSLATION_START;
    let blend = ASTREA_LAMP_TRANSLATION_BLEND;
    let join = start + (1.0 - start) * blend;
    let step = 1.0e-6;
    let left_slope = (translation_soft_start(join) - translation_soft_start(join - step)) / step;
    let right_slope = (translation_soft_start(join + step) - translation_soft_start(join)) / step;
    assert!((left_slope - right_slope).abs() < 1.0e-4);
    assert_eq!(translation_soft_start(start), 0.0);
    assert_eq!(translation_soft_start(1.0), 1.0);
}

#[test]
fn lamp_motion_channels_have_c1_boundaries_without_hidden_stages() {
    let finite_difference_slope = |function: &dyn Fn(f64) -> f64, value: f64| {
        let step = 1.0e-6;
        (function(value + step) - function(value - step)) / (2.0 * step)
    };

    let translation_start = |value| translation_soft_start(value);
    let translation_start_slope =
        finite_difference_slope(&translation_start, ASTREA_LAMP_TRANSLATION_START);
    assert!(translation_start_slope.abs() < 1.0e-4);

    let translation_join = ASTREA_LAMP_TRANSLATION_START
        + (1.0 - ASTREA_LAMP_TRANSLATION_START) * ASTREA_LAMP_TRANSLATION_BLEND;
    let translation_join_left = {
        let step = 1.0e-6;
        (translation_soft_start(translation_join) - translation_soft_start(translation_join - step))
            / step
    };
    let translation_join_right = {
        let step = 1.0e-6;
        (translation_soft_start(translation_join + step) - translation_soft_start(translation_join))
            / step
    };
    assert!((translation_join_left - translation_join_right).abs() < 1.0e-4);

    let contraction = |value| smoothstep01(value / ASTREA_LAMP_CONTRACTION_END);
    let contraction_slope = finite_difference_slope(&contraction, ASTREA_LAMP_CONTRACTION_END);
    assert!(contraction_slope.abs() < 1.0e-4);

    let retreat = |value| smoothstep01(value / ASTREA_LAMP_RETREAT_END);
    let retreat_slope = finite_difference_slope(&retreat, ASTREA_LAMP_RETREAT_END);
    assert!(retreat_slope.abs() < 1.0e-4);
}

#[test]
fn lamp_reversal_samples_the_same_frozen_geometry_function() {
    let group = LifecycleVisualGroup::from_bounds(
        rect(400.0, 100.0, 800.0, 600.0),
        rect(384.0, 60.0, 832.0, 640.0),
        rect(440.0, 140.0, 720.0, 540.0),
        rect(1500.0, 500.0, 64.0, 64.0),
        1920,
        1080,
    )
    .expect("valid frozen Lamp visual group");
    let points = [
        [
            group.presented_source_visual_rect.x(),
            group.presented_source_visual_rect.y(),
        ],
        [
            group.presented_source_visual_rect.x()
                + group.presented_source_visual_rect.width() * 0.37,
            group.presented_source_visual_rect.y()
                + group.presented_source_visual_rect.height() * 0.61,
        ],
        [
            group.presented_source_visual_rect.x() + group.presented_source_visual_rect.width(),
            group.presented_source_visual_rect.y() + group.presented_source_visual_rect.height(),
        ],
    ];
    for progress in [0.01, 0.17, 0.37, 0.73, 0.99] {
        for point in points {
            let minimize_sample = lamp_warp_visual_point(group, point, progress);
            let restore_sample = lamp_warp_visual_point(group, point, progress);
            assert_eq!(minimize_sample, restore_sample);
        }
    }
}

#[test]
fn lamp_funnel_moves_near_edge_before_trailing_edge() {
    let source = rect(300.0, 200.0, 640.0, 480.0);
    let anchor = rect(700.0, 900.0, 64.0, 64.0);
    let channels = lamp_motion_channels(0.4, 0.0);
    let near_edge = lamp_warp_point_directional(
        source,
        anchor,
        LampDirection::Bottom,
        0.6,
        0.0,
        channels,
        [620.0, 680.0],
    );
    let trailing_edge = lamp_warp_point_directional(
        source,
        anchor,
        LampDirection::Bottom,
        0.6,
        0.0,
        channels,
        [620.0, 200.0],
    );
    let near_axis_fraction = (near_edge[1] - 680.0) / (964.0 - 680.0);
    let trailing_axis_fraction = (trailing_edge[1] - 200.0) / (900.0 - 200.0);
    assert!(near_axis_fraction > trailing_axis_fraction);

    let near_cross_fraction = (near_edge[0] - 620.0) / (732.0 - 620.0);
    let trailing_cross_fraction = (trailing_edge[0] - 620.0) / (732.0 - 620.0);
    assert!(near_cross_fraction > trailing_cross_fraction);
}

#[test]
fn lamp_retreat_is_bounded_overlapping_and_disabled_without_overlap() {
    let source = rect(100.0, 100.0, 200.0, 200.0);
    let overlapping_anchor = rect(100.0, 250.0, 64.0, 64.0);
    let bump_distance = lamp_bump_distance(source, overlapping_anchor, LampDirection::Bottom);
    assert_eq!(bump_distance, 50.0);

    let progress = 0.15;
    let channels = lamp_motion_channels(progress, bump_distance);
    assert!(channels.retreat_progress > 0.0 && channels.retreat_progress < 1.0);
    let retreated = lamp_warp_point_directional(
        source,
        overlapping_anchor,
        LampDirection::Bottom,
        0.6,
        bump_distance,
        channels,
        [200.0, 300.0],
    );
    let without_retreat = lamp_warp_point_directional(
        source,
        overlapping_anchor,
        LampDirection::Bottom,
        0.6,
        0.0,
        lamp_motion_channels(progress, 0.0),
        [200.0, 300.0],
    );
    assert!(retreated[1] < without_retreat[1]);
    assert!((without_retreat[1] - retreated[1]) <= bump_distance);

    let separated_anchor = rect(100.0, 500.0, 64.0, 64.0);
    assert_eq!(
        lamp_bump_distance(source, separated_anchor, LampDirection::Bottom),
        0.0
    );
    assert_eq!(lamp_motion_channels(progress, 0.0).retreat_progress, 0.0);
}

#[test]
fn lamp_no_retreat_main_axis_motion_is_monotonic() {
    let source = rect(300.0, 200.0, 640.0, 480.0);
    let anchor = rect(700.0, 900.0, 64.0, 64.0);
    for point in [[300.0, 200.0], [620.0, 440.0], [940.0, 680.0]] {
        let mut previous = lamp_warp_point(source, anchor, point, 0.0)[1];
        for step in 1..=100 {
            let progress = f64::from(step) / 100.0;
            let current = lamp_warp_point(source, anchor, point, progress)[1];
            assert!(
                current + 1.0e-9 >= previous,
                "point {point:?} moved away from the Bottom anchor at progress {progress}"
            );
            previous = current;
        }
    }
}

#[test]
fn lamp_directional_endpoints_are_exact_for_all_output_edges() {
    let source = rect(700.0, 400.0, 200.0, 200.0);
    let point = [760.0, 520.0];
    for (anchor, expected_direction) in [
        (rect(760.0, 12.0, 64.0, 32.0), LampDirection::Top),
        (rect(1840.0, 520.0, 64.0, 64.0), LampDirection::Right),
        (rect(760.0, 1036.0, 64.0, 32.0), LampDirection::Bottom),
        (rect(12.0, 520.0, 64.0, 64.0), LampDirection::Left),
    ] {
        let group = LifecycleVisualGroup::from_bounds(source, source, source, anchor, 1920, 1080)
            .expect("valid directional visual group");
        assert_eq!(group.lamp_direction, expected_direction);
        assert_eq!(lamp_warp_visual_point(group, point, 0.0), point);
        let endpoint = lamp_warp_visual_point(group, point, 1.0);
        assert_eq!(
            endpoint,
            [
                group.sink_rect.x() + 0.3 * group.sink_rect.width(),
                group.sink_rect.y() + 0.6 * group.sink_rect.height(),
            ]
        );
    }
}

#[test]
fn lamp_direction_selection_prefers_each_output_edge() {
    let source = rect(700.0, 400.0, 200.0, 200.0);
    let anchors = [
        (rect(900.0, 12.0, 64.0, 32.0), LampDirection::Top),
        (rect(1840.0, 480.0, 64.0, 64.0), LampDirection::Right),
        (rect(900.0, 1036.0, 64.0, 32.0), LampDirection::Bottom),
        (rect(12.0, 480.0, 64.0, 64.0), LampDirection::Left),
    ];
    for (anchor, expected) in anchors {
        let group = LifecycleVisualGroup::from_bounds(source, source, source, anchor, 1920, 1080)
            .expect("valid visual group");
        assert_eq!(group.lamp_direction, expected);
    }
}

#[test]
fn directional_warp_has_exact_endpoints_and_finite_intermediates() {
    let source = rect(300.0, 200.0, 640.0, 480.0);
    let anchor = rect(700.0, 900.0, 64.0, 64.0);
    let point = [620.0, 440.0];
    let shape = 0.4;
    let bump = 0.0;
    let channels = lamp_motion_channels(0.5, bump);
    let intermediate = lamp_warp_point_directional(
        source,
        anchor,
        LampDirection::Bottom,
        shape,
        bump,
        channels,
        point,
    );
    assert!(intermediate.into_iter().all(f64::is_finite));
    assert_eq!(lamp_warp_point(source, anchor, point, 0.0), point);
    let portal = lamp_portal_rect(source, anchor, LampDirection::Bottom).expect("portal");
    let sink = lamp_sink_rect(
        anchor,
        portal,
        LampDirection::Bottom,
        ASTREA_LAMP_ABSORB_DEPTH,
        ASTREA_LAMP_SINK_THICKNESS,
    )
    .expect("sink");
    assert_eq!(
        lamp_warp_point(source, anchor, point, 1.0),
        [
            sink.x() + 0.5 * sink.width(),
            sink.y() + 0.5 * sink.height()
        ]
    );
}

#[test]
fn directional_warp_is_equivalent_under_axis_rotation() {
    let bottom_source = rect(0.0, 0.0, 100.0, 100.0);
    let bottom_anchor = rect(20.0, 150.0, 40.0, 40.0);
    let bottom_point = [30.0, 40.0];
    let rotated_source = rect(0.0, 0.0, 100.0, 100.0);
    let rotated_anchor = rect(150.0, 40.0, 40.0, 40.0);
    let rotated_point = [40.0, 70.0];
    let shape = lamp_shape_factor(bottom_source, bottom_anchor, LampDirection::Bottom);
    let bump = lamp_bump_distance(bottom_source, bottom_anchor, LampDirection::Bottom);
    let channels = lamp_motion_channels(0.55, bump);
    let bottom = lamp_warp_point_directional(
        bottom_source,
        bottom_anchor,
        LampDirection::Bottom,
        shape,
        bump,
        channels,
        bottom_point,
    );
    let rotated = lamp_warp_point_directional(
        rotated_source,
        rotated_anchor,
        LampDirection::Right,
        shape,
        bump,
        channels,
        rotated_point,
    );
    assert!((rotated[0] - bottom[1]).abs() < 1e-9);
    assert!((rotated[1] - (100.0 - bottom[0])).abs() < 1e-9);
}

#[test]
fn directional_warp_handles_tiny_and_huge_geometry_without_nonfinite_values() {
    for (source, anchor, point) in [
        (
            rect(0.0, 0.0, 0.001, 0.001),
            rect(0.002, 0.002, 0.001, 0.001),
            [0.0, 0.0],
        ),
        (
            rect(-1.0e6, -1.0e6, 2.0e6, 2.0e6),
            rect(1.0e6, 1.0e6, 1.0, 1.0),
            [0.0, 0.0],
        ),
    ] {
        let group = LifecycleVisualGroup::from_bounds(source, source, source, anchor, 1920, 1080)
            .expect("valid extreme visual group");
        let warped = lamp_warp_visual_point(group, point, 0.5);
        assert!(warped.into_iter().all(f64::is_finite));
    }
}

#[test]
fn lamp_is_identity_at_zero_and_reaches_sink_at_one() {
    let source = rect(100.0, 80.0, 800.0, 600.0);
    let anchor = rect(1200.0, 900.0, 64.0, 64.0);
    let point = [500.0, 320.0];

    assert_eq!(lamp_warp_point(source, anchor, point, 0.0), point);
    let warped = lamp_warp_point(source, anchor, point, 1.0);
    let portal = lamp_portal_rect(source, anchor, LampDirection::Bottom).expect("portal");
    let sink = lamp_sink_rect(
        anchor,
        portal,
        LampDirection::Bottom,
        ASTREA_LAMP_ABSORB_DEPTH,
        ASTREA_LAMP_SINK_THICKNESS,
    )
    .expect("sink");
    let expected = [
        sink.x() + 0.5 * sink.width(),
        sink.y() + 0.4 * sink.height(),
    ];
    assert_eq!(warped, expected);
    assert_eq!(lamp_opacity(0.0), 1.0);
    assert_eq!(lamp_opacity(1.0), 0.0);
}

#[test]
fn full_window_reference_freezes_the_presented_affine_basis() {
    let full = rect(100.0, 100.0, 800.0, 600.0);
    let source = rect(200.0, 160.0, 960.0, 720.0);
    let anchor = rect(1200.0, 900.0, 64.0, 64.0);
    let point = [500.0, 400.0];
    assert_eq!(
        lamp_warp_window_point(full, source, anchor, point, 0.0),
        [680.0, 520.0]
    );
    assert_eq!(
        lamp_warp_window_point(full, source, anchor, point, 1.0),
        [1232.0, 938.4]
    );
}

#[test]
fn lamp_is_finite_and_direction_independent_for_anchor_positions() {
    let source = rect(300.0, 200.0, 640.0, 480.0);
    let points = [[300.0, 200.0], [620.0, 440.0], [940.0, 680.0]];
    let anchors = [
        rect(500.0, 800.0, 64.0, 64.0),
        rect(-120.0, 300.0, 64.0, 64.0),
        rect(1100.0, 300.0, 64.0, 64.0),
        rect(500.0, -80.0, 64.0, 64.0),
        rect(1100.0, 800.0, 64.0, 64.0),
    ];

    for anchor in anchors {
        for point in points {
            let warped = lamp_warp_point(source, anchor, point, 0.5);
            assert!(warped.into_iter().all(f64::is_finite));
        }
    }
}

#[test]
fn lamp_progress_and_opacity_are_monotonic() {
    let source = rect(0.0, 0.0, 1000.0, 700.0);
    let anchor = rect(1200.0, 800.0, 48.0, 48.0);
    let point = [1000.0, 700.0];
    let mut previous_distance = f64::INFINITY;
    let mut previous_opacity = 1.0;

    for step in 0..=100 {
        let t = f64::from(step) / 100.0;
        let warped = lamp_warp_point(source, anchor, point, t);
        let target = [anchor.x() + anchor.width(), anchor.y() + anchor.height()];
        let distance = (target[0] - warped[0]).hypot(target[1] - warped[1]);
        assert!(distance <= previous_distance + 1e-9);
        let opacity = lamp_opacity(t);
        assert!(opacity <= previous_opacity + 1e-9);
        previous_distance = distance;
        previous_opacity = opacity;
    }
}

#[test]
fn retained_presentation_identities_are_exact_versions() {
    let scene_node_id = SceneNodeId::from_raw(15).expect("scene node");
    let first = retained_identity(15, 1, 2);
    let second = retained_identity(15, 3, 4);
    assert_ne!(first, second);
    assert_eq!(first.scene_node_id(), scene_node_id);
    assert_eq!(second.scene_node_id(), scene_node_id);
    assert_ne!(first.transaction_id(), second.transaction_id());
    assert_ne!(first.revision_id(), second.revision_id());
}

#[test]
fn reversal_preserves_progress_and_rejects_stale_ack() {
    let window = WindowId::from_raw(7).expect("valid window id");
    let source = rect(0.0, 0.0, 800.0, 600.0);
    let anchor = rect(1000.0, 700.0, 64.0, 64.0);
    let mut animator = WindowLifecycleAnimator::new(true);
    let first = animator
        .start_or_reverse(
            request(window, 7, source, anchor, LifecycleDirection::Minimize),
            None,
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("minimize starts");
    let before = animator
        .sample(first, AnimationTime::from_nanos(103_600_000))
        .expect("active sample");
    let second = animator
        .start_or_reverse(
            request(window, 7, source, anchor, LifecycleDirection::Restore),
            Some(first),
            AnimationTime::from_nanos(103_600_000),
            1.0,
        )
        .expect("restore reverses");
    let after = animator
        .sample(second, AnimationTime::from_nanos(103_600_000))
        .expect("reversed sample");

    assert!((before.progress - 0.37).abs() < 1e-9);
    assert_eq!(before.progress, after.progress);
    assert_ne!(first, second);
    assert_eq!(animator.acknowledge(first, true), None);
    assert!(
        animator
            .sample(second, AnimationTime::from_nanos(103_600_000))
            .is_some()
    );
}

#[test]
fn exact_endpoint_stays_owned_until_matching_ack() {
    let window = WindowId::from_raw(9).expect("valid window id");
    let source = rect(20.0, 20.0, 400.0, 300.0);
    let anchor = rect(900.0, 700.0, 48.0, 48.0);
    let mut animator = WindowLifecycleAnimator::new(true);
    let transition = animator
        .start_or_reverse(
            request(window, 9, source, anchor, LifecycleDirection::Minimize),
            None,
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("minimize starts");
    let endpoint = animator.sample_scene(&[transition], AnimationTime::from_nanos(280_000_000));
    assert!(endpoint.lamps[0].mathematically_settled);
    assert_eq!(animator.active_count(), 1);
    assert_eq!(animator.acknowledge(transition, true), Some(transition));
    assert_eq!(animator.active_count(), 0);
}

#[test]
fn policy_endpoint_snap_preserves_exact_transition_ownership() {
    let window = WindowId::from_raw(10).expect("valid window id");
    let source = rect(20.0, 20.0, 400.0, 300.0);
    let anchor = rect(900.0, 700.0, 48.0, 48.0);
    let mut animator = WindowLifecycleAnimator::new(true);
    let transition = animator
        .start_or_reverse(
            request(window, 10, source, anchor, LifecycleDirection::Minimize),
            None,
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("minimize starts");

    assert!(animator.snap_to_endpoint(transition, AnimationTime::from_nanos(100_000_000),));
    let endpoint = animator
        .sample(transition, AnimationTime::from_nanos(100_000_000))
        .expect("snapped transition remains active");
    assert_eq!(endpoint.presentation_identity, transition);
    assert_eq!(endpoint.progress, 1.0);
    assert_eq!(endpoint.visual_group.presented_source_client_rect, source);
    assert_eq!(endpoint.visual_group.anchor_rect, anchor);
    assert!(endpoint.mathematically_settled);
    assert_eq!(animator.active_count(), 1);
    assert_eq!(animator.acknowledge(transition, true), Some(transition));
}

#[test]
fn render_evidence_qualifies_only_consumed_transition_identities() {
    let first_window = WindowId::from_raw(15).expect("valid window id");
    let second_window = WindowId::from_raw(16).expect("valid window id");
    let source = rect(0.0, 0.0, 100.0, 100.0);
    let second_source = rect(1000.0, 1000.0, 100.0, 100.0);
    let anchor = rect(200.0, 200.0, 10.0, 10.0);
    let second_anchor = rect(1200.0, 1200.0, 10.0, 10.0);
    let mut animator = WindowLifecycleAnimator::new(true);
    let first = animator
        .start_or_reverse(
            request(
                first_window,
                15,
                source,
                anchor,
                LifecycleDirection::Minimize,
            ),
            None,
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("first transition starts");
    let second = animator
        .start_or_reverse(
            request(
                second_window,
                16,
                second_source,
                second_anchor,
                LifecycleDirection::Minimize,
            ),
            None,
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("second transition starts");
    assert!(lamp_footprint_intersects_output(
        LifecycleVisualGroup::from_bounds(source, source, source, anchor, 800, 600)
            .expect("valid visual group"),
        800,
        600,
    ));
    assert!(!lamp_footprint_intersects_output(
        LifecycleVisualGroup::from_bounds(
            second_source,
            second_source,
            second_source,
            second_anchor,
            800,
            600,
        )
        .expect("valid visual group"),
        800,
        600,
    ));
    let sample = animator.sample_scene(&[first, second], AnimationTime::from_nanos(100_000_000));
    let evidence = LifecycleRenderEvidence::from_consumed([LifecycleRenderEvidenceEntry {
        window_id: first_window,
        root_surface_id: 15,
        presentation_identity: first,
    }]);
    let qualified = LifecycleFrameSnapshot::qualified_from_sample(&sample, &evidence);
    assert_eq!(qualified.lamps.len(), 1);
    assert_eq!(qualified.lamps[0].window_id, first_window);
    assert_eq!(qualified.lamps[0].presentation_identity, first);
    assert_ne!(first, second);
}

#[test]
fn stale_render_evidence_does_not_qualify_a_reversed_identity() {
    let window_id = WindowId::from_raw(601).expect("valid window id");
    let root_surface_id = 801;
    let scene_node_id = SceneNodeId::from_raw(701).expect("scene node");
    let old_identity = retained_identity(701, 901, 1001);
    let current_identity = retained_identity(701, 902, 1002);
    let visual_group = LifecycleVisualGroup::from_bounds(
        rect(20.0, 30.0, 400.0, 300.0),
        rect(20.0, 30.0, 400.0, 300.0),
        rect(20.0, 30.0, 400.0, 300.0),
        rect(700.0, 500.0, 40.0, 40.0),
        1920,
        1080,
    )
    .expect("visual group");
    let sample = LifecycleSceneSample {
        sampled_at: AnimationTime::from_nanos(10),
        lamps: vec![LampWindowSample {
            window_id,
            root_surface_id,
            presentation_identity: current_identity,
            visual_group,
            progress: 0.43,
            opacity: lamp_opacity(0.43),
            direction: LifecycleDirection::Restore,
            mathematically_settled: false,
        }],
        visual_sources: vec![LifecycleVisualSource {
            window_id,
            root_surface_id,
            presentation_identity: current_identity,
            kind: LifecycleVisualSourceKind::NoOwnedEffects,
            effect_scene: Arc::new(ResolvedEffectScene::default()),
        }],
    };
    assert_eq!(current_identity.scene_node_id(), scene_node_id);
    let old_evidence = LifecycleRenderEvidence::from_consumed([LifecycleRenderEvidenceEntry {
        window_id,
        root_surface_id,
        presentation_identity: old_identity,
    }]);

    let qualified = LifecycleFrameSnapshot::qualified_from_sample(&sample, &old_evidence);

    assert!(qualified.is_empty());
    assert_eq!(sample.visual_source_for_identity(old_identity), None);
    assert!(
        sample
            .visual_source_for_identity(current_identity)
            .is_some()
    );
}

#[test]
fn backing_replacement_does_not_rewrite_a_frozen_transition_adapter() {
    let window = WindowId::from_raw(602).expect("valid window id");
    let old_root_surface_id = 802;
    let replacement_root_surface_id = 803;
    let visual_group = LifecycleVisualGroup::from_bounds(
        rect(20.0, 30.0, 400.0, 300.0),
        rect(20.0, 30.0, 400.0, 300.0),
        rect(20.0, 30.0, 400.0, 300.0),
        rect(700.0, 500.0, 40.0, 40.0),
        1920,
        1080,
    )
    .expect("visual group");
    let mut animator = WindowLifecycleAnimator::new(true);
    let old_identity = animator
        .start_or_reverse(
            request_with_group(
                window,
                old_root_surface_id,
                visual_group,
                LifecycleDirection::Minimize,
            ),
            None,
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("initial transition");
    let new_identity = animator
        .start_or_reverse(
            request_with_group(
                window,
                replacement_root_surface_id,
                visual_group,
                LifecycleDirection::Restore,
            ),
            Some(old_identity),
            AnimationTime::from_nanos(100_000_000),
            1.0,
        )
        .expect("reversed transition");

    let sample = animator.sample_scene(&[new_identity], AnimationTime::from_nanos(100_000_000));
    assert_eq!(old_identity.scene_node_id(), new_identity.scene_node_id());
    assert_ne!(old_identity, new_identity);
    assert_eq!(sample.lamps[0].root_surface_id, old_root_surface_id);
    assert_eq!(sample.lamps[0].presentation_identity, new_identity);
    assert_eq!(
        sample.visual_sources[0].root_surface_id,
        old_root_surface_id
    );
    assert_eq!(sample.visual_sources[0].presentation_identity, new_identity);
}

#[test]
fn lamp_footprint_intersection_is_conservative_and_output_aware() {
    let source = rect(300.0, 300.0, 20.0, 20.0);
    let full = rect(310.0, 310.0, 30.0, 30.0);
    let anchor = rect(330.0, 330.0, 10.0, 10.0);
    let visual_group = LifecycleVisualGroup::from_bounds(source, full, source, anchor, 400, 400)
        .expect("valid visual group");
    assert!(!lamp_footprint_intersects_output(visual_group, 100, 100,));
    assert!(lamp_footprint_intersects_output(visual_group, 400, 400,));
}

#[test]
fn no_visual_change_settlement_is_separate_from_physical_ack() {
    let window = WindowId::from_raw(17).expect("valid window id");
    let mut animator = WindowLifecycleAnimator::new(true);
    let transition = animator
        .start_or_reverse(
            request(
                window,
                17,
                rect(300.0, 300.0, 100.0, 100.0),
                rect(500.0, 500.0, 10.0, 10.0),
                LifecycleDirection::Minimize,
            ),
            None,
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("transition starts");
    assert_eq!(
        animator.settle_no_visual_change(transition),
        Some(transition)
    );
    assert_eq!(animator.active_count(), 0);
    assert_eq!(animator.acknowledge(transition, true), None);
}

#[test]
fn reversal_keeps_the_original_anchor_frozen() {
    let window = WindowId::from_raw(11).expect("valid window id");
    let source = rect(20.0, 20.0, 400.0, 300.0);
    let first_anchor = rect(900.0, 700.0, 48.0, 48.0);
    let second_anchor = rect(-80.0, 400.0, 64.0, 64.0);
    let mut animator = WindowLifecycleAnimator::new(true);
    let first = animator
        .start_or_reverse(
            request(
                window,
                11,
                source,
                first_anchor,
                LifecycleDirection::Minimize,
            ),
            None,
            AnimationTime::from_nanos(0),
            1.0,
        )
        .expect("first transition starts");
    let second = animator
        .start_or_reverse(
            request(
                window,
                11,
                source,
                second_anchor,
                LifecycleDirection::Restore,
            ),
            Some(first),
            AnimationTime::from_nanos(100_000_000),
            1.0,
        )
        .expect("reverse transition starts");
    assert_eq!(
        animator
            .sample(second, AnimationTime::from_nanos(100_000_000))
            .expect("reversed sample")
            .visual_group
            .anchor_rect,
        first_anchor
    );
}

#[test]
fn speed_scales_the_linear_timeline_and_disable_snaps_to_target() {
    let window = WindowId::from_raw(13).expect("valid window id");
    let source = rect(20.0, 20.0, 400.0, 300.0);
    let anchor = rect(900.0, 700.0, 48.0, 48.0);
    let mut animator = WindowLifecycleAnimator::new(true);
    let transition = animator
        .start_or_reverse(
            request(window, 13, source, anchor, LifecycleDirection::Minimize),
            None,
            AnimationTime::from_nanos(0),
            2.0,
        )
        .expect("minimize starts");
    assert_eq!(
        animator
            .sample(transition, AnimationTime::from_nanos(70_000_000))
            .expect("active sample")
            .progress,
        0.5
    );
    animator.set_enabled(false, &[transition], AnimationTime::from_nanos(70_000_000));
    let endpoint = animator
        .sample(transition, AnimationTime::from_nanos(70_000_000))
        .expect("disabled animation retains endpoint");
    assert_eq!(endpoint.progress, 1.0);
    assert!(endpoint.mathematically_settled);
}
