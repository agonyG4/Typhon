use super::*;

fn rect(x: i32, y: i32, width: u32, height: u32) -> OutputRect {
    OutputRect::new(x, y, width, height)
}

fn partial_capabilities() -> EglPartialRepaintCapabilities {
    EglPartialRepaintCapabilities {
        buffer_age: true,
        partial_render_repair: true,
        swap_buffers_with_damage: true,
    }
}

fn partial_planner(
    output_size: (u32, u32),
    capabilities: EglPartialRepaintCapabilities,
) -> PartialRepaintPlanner {
    let mut planner = PartialRepaintPlanner::new(output_size, capabilities);
    planner.partial_enabled = true;
    planner
}

#[test]
fn output_damage_clips_all_edges_and_discards_empty_rectangles() {
    let damage = OutputDamage::rects(
        100,
        80,
        [
            rect(-5, 10, 10, 10),
            rect(95, 10, 10, 10),
            rect(10, -5, 10, 10),
            rect(10, 75, 10, 10),
            rect(0, 0, 0, 5),
        ],
    );
    assert_eq!(
        damage,
        OutputDamage::Rects(vec![
            rect(0, 10, 5, 10),
            rect(95, 10, 5, 10),
            rect(10, 0, 10, 5),
            rect(10, 75, 10, 5),
        ])
    );
}

#[test]
fn output_damage_coalesces_overlapping_and_touching_rectangles() {
    assert_eq!(
        OutputDamage::rects(
            100,
            100,
            [rect(5, 5, 10, 10), rect(15, 5, 5, 10), rect(8, 8, 4, 4)],
        ),
        OutputDamage::Rects(vec![rect(5, 5, 15, 10)])
    );
}

#[test]
fn output_damage_converts_top_left_rectangles_for_gl_and_egl() {
    let damage = OutputDamage::rects(100, 80, [rect(4, 7, 9, 11)]);
    assert_eq!(
        damage
            .to_gl_scissors(100, 80, OutputFramebufferOrigin::BottomLeft)
            .unwrap(),
        vec![[4, 62, 9, 11]]
    );
    assert_eq!(
        damage.to_egl_rects(100, 80).unwrap().as_slice(),
        &[4, 62, 9, 11]
    );
}

#[test]
fn output_damage_converts_one_pixel_rectangles_at_every_edge() {
    let damage = OutputDamage::rects(
        8,
        6,
        [
            rect(0, 0, 1, 1),
            rect(0, 5, 1, 1),
            rect(7, 2, 1, 1),
            rect(3, 0, 1, 1),
        ],
    );
    assert_eq!(
        damage
            .to_gl_scissors(8, 6, OutputFramebufferOrigin::BottomLeft)
            .unwrap(),
        vec![[0, 5, 1, 1], [0, 0, 1, 1], [7, 3, 1, 1], [3, 5, 1, 1]]
    );
}

#[test]
fn first_frame_and_unsupported_buffer_age_force_full_repaint() {
    let current = OutputDamage::rects(100, 80, [rect(2, 3, 4, 5)]);
    let mut planner = partial_planner((100, 80), partial_capabilities());
    assert_eq!(
        planner.plan(current.clone(), BufferAge::Value(1)).mode,
        RepaintMode::Full
    );
    let mut unsupported = partial_planner(
        (100, 80),
        EglPartialRepaintCapabilities {
            buffer_age: false,
            partial_render_repair: true,
            swap_buffers_with_damage: true,
        },
    );
    assert_eq!(
        unsupported.plan(current, BufferAge::Unsupported).mode,
        RepaintMode::Full
    );
}

#[test]
fn software_buffer_age_uses_output_presentation_serials() {
    assert_eq!(software_buffer_age(10, None), BufferAge::Value(0));
    assert_eq!(software_buffer_age(10, Some(9)), BufferAge::Value(2));
    assert_eq!(software_buffer_age(10, Some(8)), BufferAge::Value(3));
    assert_eq!(software_buffer_age(10, Some(10)), BufferAge::Value(1));
    assert_eq!(software_buffer_age(10, Some(11)), BufferAge::Value(-1));
}

#[test]
fn pending_presentation_does_not_invalidate_unrelated_slot_age() {
    assert_eq!(render_target_buffer_age(10, Some(8)), BufferAge::Value(3));
    assert_eq!(render_target_buffer_age(10, Some(8)), BufferAge::Value(3));
}

#[test]
fn explicit_render_repair_does_not_require_egl_swap_damage() {
    let first = OutputDamage::Full;
    let current = OutputDamage::rects(100, 80, [rect(20, 20, 3, 3)]);
    let mut planner = partial_planner(
        (100, 80),
        EglPartialRepaintCapabilities {
            buffer_age: true,
            partial_render_repair: true,
            swap_buffers_with_damage: false,
        },
    );
    let first_plan = planner.plan(first, BufferAge::Value(0));
    planner.commit_presented(&first_plan);

    assert_eq!(
        planner.plan(current, BufferAge::Value(1)).mode,
        RepaintMode::Partial
    );
}

#[test]
fn buffer_age_beyond_three_slot_history_forces_full_repaint() {
    let mut planner = partial_planner((100, 80), partial_capabilities());
    for x in [1, 10, 20] {
        let plan = planner.plan(
            OutputDamage::rects(100, 80, [rect(x, 1, 2, 2)]),
            BufferAge::Value(1),
        );
        planner.commit_presented(&plan);
    }

    let unsupported = planner.plan(
        OutputDamage::rects(100, 80, [rect(30, 1, 2, 2)]),
        BufferAge::Value(4),
    );

    assert_eq!(unsupported.mode, RepaintMode::Full);
    assert_eq!(
        unsupported.fallback_reason,
        Some(FullRepaintReason::InsufficientHistory)
    );
}

#[test]
fn empty_logical_damage_skips_even_when_history_is_invalid() {
    let mut planner = partial_planner((100, 80), partial_capabilities());

    assert_eq!(
        planner.plan(OutputDamage::Empty, BufferAge::Value(0)).mode,
        RepaintMode::Skip
    );
    assert_eq!(planner.history_depth(), 0);
}

#[test]
fn usable_ages_accumulate_only_required_logical_damage() {
    let first = OutputDamage::rects(100, 80, [rect(1, 1, 3, 3)]);
    let second = OutputDamage::rects(100, 80, [rect(20, 20, 3, 3)]);
    let third = OutputDamage::rects(100, 80, [rect(40, 40, 3, 3)]);
    let mut planner = partial_planner((100, 80), partial_capabilities());
    let plan = planner.plan(first, BufferAge::Value(0));
    planner.commit_presented(&plan);
    let plan = planner.plan(second.clone(), BufferAge::Value(1));
    assert_eq!(plan.repair_damage, second);
    planner.commit_presented(&plan);
    let plan = planner.plan(third, BufferAge::Value(2));
    assert_eq!(
        plan.repair_damage,
        OutputDamage::Rects(vec![rect(40, 40, 3, 3), rect(20, 20, 3, 3)])
    );
    planner.commit_presented(&plan);
    let fourth = OutputDamage::rects(100, 80, [rect(60, 60, 3, 3)]);
    assert_eq!(
        planner
            .plan(fourth, BufferAge::Value(3))
            .repair_damage
            .rect_count(),
        3
    );
}

#[test]
fn invalid_age_history_and_resize_force_full_repaint() {
    let current = OutputDamage::rects(100, 80, [rect(2, 3, 4, 5)]);
    let mut planner = partial_planner((100, 80), partial_capabilities());
    let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    planner.commit_presented(&first);
    assert_eq!(
        planner.plan(current.clone(), BufferAge::Value(0)).mode,
        RepaintMode::Full
    );
    assert_eq!(
        planner.plan(current.clone(), BufferAge::Value(9)).mode,
        RepaintMode::Full
    );
    planner.resize((120, 80));
    assert_eq!(
        planner.plan(current, BufferAge::Value(1)).mode,
        RepaintMode::Full
    );
}

#[test]
fn failed_swap_does_not_advance_history_and_empty_stays_empty() {
    let current = OutputDamage::rects(100, 80, [rect(2, 3, 4, 5)]);
    let mut planner = partial_planner((100, 80), partial_capabilities());
    let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    planner.commit_presented(&first);
    let failed = planner.plan(current, BufferAge::Value(1));
    planner.swap_failed();
    assert_eq!(planner.history_depth(), 0);
    assert_eq!(
        planner
            .plan(OutputDamage::Empty, BufferAge::Value(1))
            .current_damage,
        OutputDamage::Empty
    );
    assert_eq!(failed.current_damage.rect_count(), 1);
}

#[test]
fn rendered_candidate_does_not_advance_history_until_matching_commit() {
    let mut planner = partial_planner((100, 80), partial_capabilities());
    let candidate = planner.plan(OutputDamage::Full, BufferAge::Value(0));

    assert_eq!(planner.history_depth(), 0);
    planner.commit_presented(&candidate);
    assert_eq!(planner.history_depth(), 1);
}

#[test]
fn discarded_rendered_candidate_does_not_advance_or_invalidate_history() {
    let mut planner = partial_planner((100, 80), partial_capabilities());
    let presented = planner.plan(
        OutputDamage::rects(100, 80, [rect(1, 2, 2, 2)]),
        BufferAge::Value(0),
    );
    planner.commit_presented(&presented);
    let discarded = planner.plan(
        OutputDamage::rects(100, 80, [rect(4, 5, 6, 7)]),
        BufferAge::Value(2),
    );

    planner.discard_rendered(&discarded);

    assert_eq!(planner.history_depth(), 1);
    assert_eq!(
        planner
            .plan(
                OutputDamage::rects(100, 80, [rect(20, 21, 2, 2)]),
                BufferAge::Value(2),
            )
            .repair_damage
            .rect_count(),
        2
    );
}

#[test]
fn render_ahead_keeps_presented_history_until_pageflip_and_repairs_all_changes() {
    let first_damage = OutputDamage::rects(100, 80, [rect(1, 2, 2, 2)]);
    let render_ahead_damage = OutputDamage::rects(100, 80, [rect(20, 21, 2, 2)]);
    let current_damage = OutputDamage::rects(100, 80, [rect(20, 21, 2, 2), rect(40, 41, 2, 2)]);
    let mut planner = partial_planner((100, 80), partial_capabilities());

    let presented = planner.plan(first_damage.clone(), BufferAge::Value(0));
    planner.commit_presented(&presented);

    // B is rendered while A is presented, but its damage is not presented
    // history until B's pageflip is confirmed.
    let render_ahead = planner.plan(render_ahead_damage, BufferAge::Value(1));
    assert_eq!(planner.history_depth(), 1);
    assert_eq!(render_ahead.mode, RepaintMode::Partial);

    // C's scene damage includes the B change because B never became the
    // confirmed scene. C's slot is older, so repair also needs A's history.
    let render_ahead_ready = planner.plan(current_damage, BufferAge::Value(2));
    assert_eq!(render_ahead_ready.mode, RepaintMode::Partial);
    for required in first_damage
        .rects_slice()
        .iter()
        .chain(render_ahead_ready.current_damage.rects_slice())
    {
        assert!(
            render_ahead_ready
                .repair_damage
                .rects_slice()
                .contains(required),
            "partial repair omitted required visual difference {required:?}"
        );
    }
    planner.commit_presented(&render_ahead_ready);
    assert_eq!(planner.history_depth(), 2);
}

#[test]
fn two_rendered_candidates_can_coexist_before_one_is_committed() {
    let mut planner = partial_planner((100, 80), partial_capabilities());
    let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    let second = planner.plan(
        OutputDamage::rects(100, 80, [rect(8, 9, 3, 3)]),
        BufferAge::Value(0),
    );

    assert_eq!(planner.history_depth(), 0);
    planner.discard_rendered(&first);
    planner.commit_presented(&second);
    assert_eq!(planner.history_depth(), 1);
}

#[test]
fn policy_falls_back_for_many_rectangles_or_near_full_area() {
    let mut planner = partial_planner((100, 100), partial_capabilities());
    let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    planner.commit_presented(&first);
    let many = OutputDamage::Rects(
        (0..=MAX_PARTIAL_REPAINT_RECTS)
            .map(|index| rect((index * 3) as i32, 1, 1, 1))
            .collect(),
    );
    assert_eq!(
        planner.plan(many, BufferAge::Value(1)).mode,
        RepaintMode::Full
    );
    let near_full = OutputDamage::rects(100, 100, [rect(0, 0, 90, 90)]);
    assert_eq!(
        planner.plan(near_full, BufferAge::Value(1)).mode,
        RepaintMode::Full
    );
}

#[test]
fn partial_repaint_can_be_disabled_by_force_full_policy() {
    let mut planner = PartialRepaintPlanner::new((100, 80), partial_capabilities());
    planner.partial_enabled = false;
    let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    planner.commit_presented(&first);

    let plan = planner.plan(
        OutputDamage::rects(100, 80, [rect(4, 7, 9, 11)]),
        BufferAge::Value(1),
    );

    assert_eq!(plan.mode, RepaintMode::Full);
}

#[test]
fn render_execution_plan_clears_each_partial_scissor_and_restores_state() {
    let plan = RepaintPlan {
        current_damage: OutputDamage::rects(100, 80, [rect(4, 7, 9, 11)]),
        repair_damage: OutputDamage::rects(100, 80, [rect(4, 7, 9, 11), rect(30, 40, 5, 6)]),
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
    };

    assert_eq!(
        plan.render_execution(100, 80, OutputFramebufferOrigin::BottomLeft)
            .unwrap(),
        RenderExecution::Scissored {
            scissors: vec![[4, 62, 9, 11], [30, 34, 5, 6]],
            disable_scissor_after: true,
        }
    );
    assert_eq!(plan.swap_damage(), &plan.repair_damage);
}

#[test]
fn skipped_plan_has_no_gl_execution() {
    let plan = RepaintPlan {
        current_damage: OutputDamage::Empty,
        repair_damage: OutputDamage::Empty,
        buffer_age: Some(1),
        mode: RepaintMode::Skip,
        fallback_reason: None,
    };

    assert_eq!(
        plan.render_execution(100, 80, OutputFramebufferOrigin::BottomLeft),
        None
    );
}

#[test]
fn successful_swap_records_logical_damage_instead_of_expanded_repair() {
    let mut planner = partial_planner((100, 80), partial_capabilities());
    let initial = planner.plan(
        OutputDamage::rects(100, 80, [rect(1, 1, 2, 2)]),
        BufferAge::Value(0),
    );
    planner.commit_presented(&initial);
    let second = planner.plan(
        OutputDamage::rects(100, 80, [rect(20, 20, 2, 2)]),
        BufferAge::Value(2),
    );
    planner.commit_presented(&second);

    let third = planner.plan(
        OutputDamage::rects(100, 80, [rect(40, 40, 2, 2)]),
        BufferAge::Value(2),
    );
    assert_eq!(third.repair_damage.rect_count(), 2);
    assert!(
        !third
            .repair_damage
            .rects_slice()
            .contains(&rect(1, 1, 2, 2))
    );
}

#[test]
fn full_damage_conversion_and_checked_area_are_explicit() {
    assert_eq!(
        OutputDamage::Full
            .to_gl_scissors(8, 6, OutputFramebufferOrigin::BottomLeft)
            .unwrap(),
        vec![[0, 0, 8, 6]]
    );
    let overflowing = OutputDamage::Rects(vec![
        rect(0, 0, u32::MAX, u32::MAX),
        rect(0, 0, u32::MAX, u32::MAX),
    ]);
    assert_eq!(overflowing.pixels(u32::MAX, u32::MAX), None);
}

#[test]
fn full_current_damage_wins_and_surface_invalidation_forces_full() {
    let mut planner = partial_planner((100, 80), partial_capabilities());
    let first = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    planner.commit_presented(&first);
    assert_eq!(
        planner.plan(OutputDamage::Full, BufferAge::Value(1)).mode,
        RepaintMode::Full
    );
    planner.invalidate();
    let partial = OutputDamage::rects(100, 80, [rect(1, 2, 3, 4)]);
    assert_eq!(
        planner.plan(partial, BufferAge::Value(1)).fallback_reason,
        Some(FullRepaintReason::FirstFrameOrInvalidated)
    );
}

#[test]
fn histories_are_isolated_per_planner_surface() {
    let mut first = partial_planner((100, 80), partial_capabilities());
    let second = partial_planner((100, 80), partial_capabilities());
    let plan = first.plan(OutputDamage::Full, BufferAge::Value(0));
    first.commit_presented(&plan);

    assert_eq!(first.history_depth(), 1);
    assert_eq!(second.history_depth(), 0);
}

#[test]
fn triple_buffer_swapchain_oracle_matches_full_reference() {
    let mut planner = partial_planner((12, 1), partial_capabilities());
    let mut buffers = [vec![0u8; 12], vec![0u8; 12], vec![0u8; 12]];
    let mut last_presented = [None::<u32>; 3];
    let serial = std::cell::Cell::new(0u32);

    let mut present = |planner: &mut PartialRepaintPlanner,
                       buffer_index: usize,
                       reference: &[u8],
                       logical: OutputDamage,
                       fail_swap: bool| {
        let age = last_presented[buffer_index]
            .map(|last| serial.get().saturating_sub(last).saturating_add(1))
            .unwrap_or(0);
        let plan = planner.plan(logical, BufferAge::Value(age as i32));
        assert_ne!(plan.mode, RepaintMode::Skip);
        match &plan.repair_damage {
            OutputDamage::Empty => panic!("rendered oracle plan cannot be empty"),
            OutputDamage::Full => buffers[buffer_index].copy_from_slice(reference),
            OutputDamage::Rects(rects) => {
                for rect in rects {
                    let start = usize::try_from(rect.x.max(0)).unwrap();
                    let end = start
                        .saturating_add(rect.width as usize)
                        .min(reference.len());
                    buffers[buffer_index][start..end].copy_from_slice(&reference[start..end]);
                }
            }
        }
        if fail_swap {
            planner.swap_failed();
            return;
        }
        assert_eq!(buffers[buffer_index], reference);
        serial.set(serial.get().saturating_add(1));
        last_presented[buffer_index] = Some(serial.get());
        planner.commit_presented(&plan);
    };

    let mut reference = vec![0u8; 12];
    reference[1] = 1;
    present(&mut planner, 0, &reference, OutputDamage::Full, false);

    reference[4] = 2;
    present(
        &mut planner,
        1,
        &reference,
        OutputDamage::rects(12, 1, [rect(4, 0, 1, 1)]),
        false,
    );

    reference[7] = 3;
    present(
        &mut planner,
        2,
        &reference,
        OutputDamage::rects(12, 1, [rect(7, 0, 1, 1)]),
        false,
    );

    reference[8] = 4;
    present(
        &mut planner,
        2,
        &reference,
        OutputDamage::rects(12, 1, [rect(8, 0, 1, 1)]),
        false,
    );

    let serial_before_skip = serial.get();
    let skipped = planner.plan(OutputDamage::Empty, BufferAge::Value(3));
    assert_eq!(skipped.mode, RepaintMode::Skip);
    assert_eq!(serial.get(), serial_before_skip);

    reference[1] = 0;
    reference[2] = 5;
    present(
        &mut planner,
        1,
        &reference,
        OutputDamage::rects(12, 1, [rect(1, 0, 2, 1)]),
        false,
    );

    reference[10] = 6;
    present(
        &mut planner,
        2,
        &reference,
        OutputDamage::rects(12, 1, [rect(10, 0, 1, 1)]),
        true,
    );
    assert_eq!(serial.get(), serial_before_skip + 1);
    present(
        &mut planner,
        2,
        &reference,
        OutputDamage::rects(12, 1, [rect(10, 0, 1, 1)]),
        false,
    );

    planner.resize((16, 1));
    let resized_reference = vec![9u8; 16];
    let resized = planner.plan(OutputDamage::Full, BufferAge::Value(0));
    assert_eq!(resized.mode, RepaintMode::Full);
    let mut resized_buffer = vec![0u8; 16];
    resized_buffer.copy_from_slice(&resized_reference);
    assert_eq!(resized_buffer, resized_reference);
    planner.commit_presented(&resized);
}

#[test]
fn logical_top_damage_maps_to_origin_specific_gl_rows() {
    let damage = OutputDamage::rects(100, 80, [rect(4, 0, 9, 11)]);

    assert_eq!(
        damage
            .to_gl_scissors(
                100,
                80,
                crate::egl_renderer::OutputFramebufferOrigin::BottomLeft
            )
            .unwrap(),
        vec![[4, 69, 9, 11]]
    );
    assert_eq!(
        damage
            .to_gl_scissors(
                100,
                80,
                crate::egl_renderer::OutputFramebufferOrigin::TopLeftScanout,
            )
            .unwrap(),
        vec![[4, 0, 9, 11]]
    );
}

#[test]
fn logical_bottom_damage_maps_to_origin_specific_gl_rows() {
    let damage = OutputDamage::rects(100, 80, [rect(4, 69, 9, 11)]);

    assert_eq!(
        damage
            .to_gl_scissors(
                100,
                80,
                crate::egl_renderer::OutputFramebufferOrigin::BottomLeft
            )
            .unwrap(),
        vec![[4, 0, 9, 11]]
    );
    assert_eq!(
        damage
            .to_gl_scissors(
                100,
                80,
                crate::egl_renderer::OutputFramebufferOrigin::TopLeftScanout,
            )
            .unwrap(),
        vec![[4, 69, 9, 11]]
    );
}

#[test]
fn partial_render_execution_uses_scanout_damage_rows() {
    let plan = RepaintPlan {
        current_damage: OutputDamage::rects(100, 80, [rect(4, 0, 9, 11)]),
        repair_damage: OutputDamage::rects(100, 80, [rect(4, 0, 9, 11)]),
        buffer_age: Some(2),
        mode: RepaintMode::Partial,
        fallback_reason: None,
    };

    assert_eq!(
        plan.render_execution(
            100,
            80,
            crate::egl_renderer::OutputFramebufferOrigin::TopLeftScanout,
        )
        .unwrap(),
        RenderExecution::Scissored {
            scissors: vec![[4, 0, 9, 11]],
            disable_scissor_after: true,
        }
    );
}
