use super::*;

#[test]
fn scene_work_preservation_plan_counts_dock_pixels_for_both_origins() {
    let extra_scene_work = [OutputRect::new(762, 976, 396, 104)];

    for framebuffer_origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        let plan = SceneWorkPreservationPlan::from_extra_scene_work(
            &extra_scene_work,
            (1920, 1080),
            framebuffer_origin,
        );

        assert_eq!(plan.transfers.len(), 1);
        assert_eq!(plan.pixels, 41_184);
    }
}

#[test]
fn scene_work_preservation_plan_counts_topbar_pixels() {
    let plan = SceneWorkPreservationPlan::from_extra_scene_work(
        &[OutputRect::new(0, 0, 120, 65)],
        (1920, 1080),
        OutputFramebufferOrigin::TopLeftScanout,
    );

    assert_eq!(plan.transfers.len(), 1);
    assert_eq!(plan.pixels, 7_800);
}

#[test]
fn scene_work_preservation_plan_keeps_disjoint_rectangles_separate() {
    let plan = SceneWorkPreservationPlan::from_extra_scene_work(
        &[
            OutputRect::new(762, 976, 396, 104),
            OutputRect::new(0, 0, 120, 65),
        ],
        (1920, 1080),
        OutputFramebufferOrigin::BottomLeft,
    );

    assert_eq!(plan.transfers.len(), 2);
    assert_eq!(plan.pixels, 48_984);
}

#[test]
fn scene_work_preservation_plan_counts_only_clipped_pixels() {
    let plan = SceneWorkPreservationPlan::from_extra_scene_work(
        &[OutputRect::new(-10, -5, 30, 20)],
        (100, 80),
        OutputFramebufferOrigin::BottomLeft,
    );

    assert_eq!(plan.transfers.len(), 1);
    assert_eq!(plan.pixels, 300);
}

#[test]
fn scene_work_preservation_plan_represents_empty_work_without_transfers() {
    let plan = SceneWorkPreservationPlan::from_extra_scene_work(
        &[],
        (1920, 1080),
        OutputFramebufferOrigin::BottomLeft,
    );

    assert!(plan.transfers.is_empty());
    assert_eq!(plan.pixels, 0);
}

#[test]
fn scene_work_preservation_maps_framebuffer_origins() {
    let rect = OutputRect::new(10, 5, 30, 10);
    let bottom_left =
        scene_work_preservation_blit_rects(rect, (100, 80), OutputFramebufferOrigin::BottomLeft)
            .expect("bottom-left preservation rects");
    assert_eq!(bottom_left.source, GlBlitRect::new(10, 65, 40, 75));
    assert_eq!(bottom_left.destination, GlBlitRect::new(10, 65, 40, 75));
    assert_eq!(bottom_left.inverse().source, bottom_left.destination);
    assert_eq!(bottom_left.inverse().destination, bottom_left.source);

    let top_left = scene_work_preservation_blit_rects(
        rect,
        (100, 80),
        OutputFramebufferOrigin::TopLeftScanout,
    )
    .expect("top-left preservation rects");
    assert_eq!(top_left.source, GlBlitRect::new(10, 5, 40, 15));
    assert_eq!(top_left.destination, GlBlitRect::new(10, 75, 40, 65));
    assert_eq!(top_left.inverse().source, top_left.destination);
    assert_eq!(top_left.inverse().destination, top_left.source);
}

#[test]
fn scene_work_preservation_source_matches_direct_capture_for_asymmetric_domains() {
    let rects = [
        OutputRect::new(10, 5, 30, 10),
        OutputRect::new(41, 27, 13, 11),
        OutputRect::new(3, 68, 26, 12),
    ];

    for framebuffer_origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        for rect in rects {
            let preservation =
                scene_work_preservation_blit_rects(rect, (100, 80), framebuffer_origin)
                    .expect("in-bounds preservation region");
            let domain =
                oblivion_one::effects::EffectRect::new(rect.x, rect.y, rect.width, rect.height)
                    .expect("in-bounds direct-capture domain");
            let direct_capture = plan_graph_texture_capture(
                (100, 80),
                domain,
                (rect.width, rect.height),
                framebuffer_origin,
            )
            .expect("in-bounds direct-capture transfer");

            assert_eq!(
                preservation.source, direct_capture.source,
                "source mismatch for {framebuffer_origin:?}, {rect:?}"
            );
        }
    }
}
