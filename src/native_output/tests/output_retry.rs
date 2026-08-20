use super::output::test_renderable_surface;
use super::*;

fn contains(rect: NativeDamageRect, x: i32, y: i32) -> bool {
    x >= rect.x
        && y >= rect.y
        && x < rect.x.saturating_add(rect.width as i32)
        && y < rect.y.saturating_add(rect.height as i32)
}

#[test]
fn rejected_same_generation_retry_repairs_from_presented_scene() {
    let mut presented =
        test_renderable_surface(91, -200, 160, 2200, 420, RenderableSurfaceDamage::Full);
    presented.generation = 10;
    let mut retry =
        test_renderable_surface(91, -200, 160, 1400, 420, RenderableSurfaceDamage::Empty);
    retry.generation = 11;
    let presented_scene = NativeSceneSnapshot::from_surfaces(&[presented], Vec::new());
    let retry_scene = NativeSceneSnapshot::from_surfaces(&[retry], Vec::new());
    let mut history = NativeSceneHistory::new(NativeFrameSceneSnapshot {
        frame_id: 1,
        render_generation: 1,
        scene: presented_scene,
        cursor_damage: NativeCursorDamageBounds::default(),
    });
    history.replace_ready(NativeFrameSceneSnapshot {
        frame_id: 2,
        render_generation: 7,
        scene: retry_scene.clone(),
        cursor_damage: NativeCursorDamageBounds::default(),
    });
    assert!(history.queue_submission(200));
    assert!(history.discard_submission(200));
    assert_eq!(
        history.presented_scene().surfaces[0].bounds.unwrap().width,
        2200
    );
    let damage = native_output_damage_for_scene_snapshots(
        1920,
        1080,
        history.presented_scene(),
        &retry_scene,
        NativeCursorDamageBounds::default(),
    );
    assert!(!damage.rects.is_empty());
}

#[test]
fn rejected_decoration_only_retry_keeps_decoration_damage() {
    let surface = test_renderable_surface(95, 108, 28, 500, 300, RenderableSurfaceDamage::Empty);
    let window_id = WindowId::from_raw(95).unwrap();
    let scene = |signature| {
        NativeSceneSnapshot::from_surfaces(
            std::slice::from_ref(&surface),
            vec![DecorationSceneSnapshot::from_bounds(
                window_id, 95, 179, 74, 502, 328, signature,
            )],
        )
    };
    let damage = native_output_damage_for_scene_snapshots(
        960,
        640,
        &scene(1),
        &scene(2),
        NativeCursorDamageBounds::default(),
    );
    assert!(damage.rects.iter().any(|rect| contains(*rect, 180, 90)));
}

#[test]
fn oversized_width_matrix_repairs_shrink_and_expand_at_each_output_edge() {
    const WIDTH: u32 = 1920;
    const HEIGHT: u32 = 1080;
    const ROOT_Y: i32 = 160;
    const TITLEBAR_HEIGHT: i32 = 26;
    let widths = [2200_u32, 2050, 1900, 1700, 1400, 1000, 800];
    let placements = [
        ("both-offscreen", -200_i32),
        ("left-offscreen", -900),
        ("right-offscreen", 900),
        ("inside", 250),
    ];
    let window_id = WindowId::from_raw(97).unwrap();
    for (placement, root_x) in placements {
        let scene = |width: u32, generation: u64| {
            let mut surface = test_renderable_surface(
                97,
                root_x - 72,
                ROOT_Y - 72,
                width,
                220,
                RenderableSurfaceDamage::Empty,
            );
            surface.generation = generation;
            NativeSceneSnapshot::from_surfaces(
                std::slice::from_ref(&surface),
                vec![DecorationSceneSnapshot::from_bounds(
                    window_id,
                    97,
                    root_x - 1,
                    ROOT_Y - TITLEBAR_HEIGHT,
                    width + 2,
                    248,
                    1,
                )],
            )
        };
        let reversed = widths.iter().rev().copied().collect::<Vec<_>>();
        for (direction, sequence) in [
            ("shrink", widths.as_slice()),
            ("expand", reversed.as_slice()),
        ] {
            for pair in sequence.windows(2) {
                let damage = native_output_damage_for_scene_snapshots(
                    WIDTH,
                    HEIGHT,
                    &scene(pair[0], 1),
                    &scene(pair[1], 2),
                    NativeCursorDamageBounds::default(),
                );
                let current_x = root_x.clamp(0, WIDTH as i32 - 1);
                let old_edge_x = (root_x + pair[0] as i32 - 1).clamp(0, WIDTH as i32 - 1);
                assert!(
                    damage
                        .rects
                        .iter()
                        .any(|rect| contains(*rect, current_x, ROOT_Y)),
                    "{placement} {direction} current width {}",
                    pair[1]
                );
                assert!(
                    damage.rects.iter().any(|rect| contains(
                        *rect,
                        old_edge_x,
                        ROOT_Y - TITLEBAR_HEIGHT
                    )),
                    "{placement} {direction} old width {}",
                    pair[0]
                );
            }
        }
    }
}
