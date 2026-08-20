use super::output::test_renderable_surface;
use super::*;
use crate::egl_renderer::{
    BufferAge, EglPartialRepaintCapabilities, OutputDamage, PartialRepaintPlanner, RepaintMode,
};
use oblivion_one::compositor::FullscreenRenderPlanMetrics;
use std::borrow::Cow;

#[test]
fn solitary_fullscreen_snapshot_matches_the_filtered_renderer_scene() {
    let rear = test_renderable_surface(101, 80, 80, 500, 300, RenderableSurfaceDamage::Empty);
    let owner = test_renderable_surface(102, 0, 0, 1280, 800, RenderableSurfaceDamage::Full);
    let renderer_surfaces = vec![owner.clone()];
    let raw_surfaces = vec![rear.clone(), owner.clone()];
    let raw_decorations = vec![
        DecorationSceneSnapshot::from_bounds(
            WindowId::from_raw(101).unwrap(),
            rear.surface_id,
            74,
            54,
            512,
            328,
            1,
        ),
        DecorationSceneSnapshot::from_bounds(
            WindowId::from_raw(102).unwrap(),
            owner.surface_id,
            0,
            0,
            1280,
            800,
            1,
        ),
    ];
    let resolved_scene = ResolvedNativeFrameScene {
        surfaces: Cow::Owned(renderer_surfaces.clone()),
        decorations: Vec::new(),
        popup_surface_ids: Cow::Owned(Vec::new()),
        external_overlay_surface_ids: Vec::new(),
        render_generation: 1,
        visibility: FullscreenRenderPlanMetrics {
            fullscreen_active: true,
            owner_root_surface_id: Some(owner.surface_id),
            solitary_tree_active: true,
            culled_surface_count: 1,
            wallpaper_culled: true,
            visible_overlay_count: 0,
            rejection: None,
        },
        snapshot: NativeSceneSnapshot::from_surfaces(&renderer_surfaces, Vec::new()),
    };
    let snapshot = NativeFrameSceneSnapshot::from_resolved_frame_scene(
        1,
        &resolved_scene,
        NativeCursorDamageBounds::default(),
    );
    let renderer_ids = renderer_surfaces
        .iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    let snapshot_ids = snapshot
        .scene
        .surfaces
        .iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();

    let raw_snapshot = NativeSceneSnapshot::from_surfaces(&raw_surfaces, raw_decorations);
    assert_ne!(
        renderer_ids,
        raw_snapshot
            .surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(renderer_ids, snapshot_ids);
    assert!(snapshot.scene.decorations.is_empty());
    assert_eq!(raw_snapshot.decorations.len(), 2);
}

#[test]
fn fullscreen_restore_damage_repairs_the_culled_window_and_ssd() {
    let rear = test_renderable_surface(201, 80, 80, 500, 300, RenderableSurfaceDamage::Empty);
    let owner = test_renderable_surface(202, 0, 0, 1280, 800, RenderableSurfaceDamage::Empty);
    let fullscreen = NativeSceneSnapshot::from_surfaces(std::slice::from_ref(&owner), Vec::new());
    let restore_decoration = DecorationSceneSnapshot::from_bounds(
        WindowId::from_raw(201).unwrap(),
        rear.surface_id,
        74,
        54,
        512,
        328,
        1,
    );
    let restored = NativeSceneSnapshot::from_surfaces(&[rear, owner], vec![restore_decoration]);
    let damage = native_output_damage_for_scene_snapshots(
        1280,
        800,
        &fullscreen,
        &restored,
        NativeCursorDamageBounds::default(),
    );

    assert!(!damage.is_empty());
    assert!(
        damage.rects.iter().any(|rect| {
            rect.x <= 74
                && rect.y <= 54
                && rect.x.saturating_add(rect.width as i32) >= 586
                && rect.y.saturating_add(rect.height as i32) >= 382
        }),
        "restore damage must include the rear window's returning SSD"
    );
}

fn fullscreen_scene(owner_generation: u64) -> NativeSceneSnapshot {
    let mut owner = test_renderable_surface(302, 0, 0, 64, 40, RenderableSurfaceDamage::Empty);
    owner.generation = owner_generation;
    NativeSceneSnapshot::from_surfaces(std::slice::from_ref(&owner), Vec::new())
}

fn restored_scene() -> NativeSceneSnapshot {
    let rear = test_renderable_surface(301, 5, 7, 22, 14, RenderableSurfaceDamage::Empty);
    let owner = test_renderable_surface(302, 37, 9, 22, 25, RenderableSurfaceDamage::Empty);
    NativeSceneSnapshot::from_surfaces(
        &[rear, owner],
        vec![
            DecorationSceneSnapshot::from_bounds(
                WindowId::from_raw(301).unwrap(),
                301,
                4,
                4,
                24,
                18,
                1,
            ),
            DecorationSceneSnapshot::from_bounds(
                WindowId::from_raw(302).unwrap(),
                302,
                36,
                5,
                24,
                29,
                1,
            ),
        ],
    )
}

fn scene_pixel(scene: &NativeSceneSnapshot, x: i32, y: i32) -> u32 {
    let mut pixel = 0xff10_1018;
    for surface in &scene.surfaces {
        if surface.bounds.is_some_and(|bounds| {
            x >= bounds.x
                && y >= bounds.y
                && x < bounds.x.saturating_add(bounds.width as i32)
                && y < bounds.y.saturating_add(bounds.height as i32)
        }) {
            pixel = 0xff00_0000
                | ((surface.surface_id & 0xff) << 8)
                | (surface.content_generation as u32 & 0xff);
        }
    }
    for decoration in &scene.decorations {
        let (window_id, _) = decoration.identity();
        let (decoration_x, decoration_y, width, height) = decoration.bounds();
        if x >= decoration_x
            && y >= decoration_y
            && x < decoration_x.saturating_add(width as i32)
            && y < decoration_y.saturating_add(height as i32)
        {
            pixel = 0xffff_0000 | (window_id.get() as u32 & 0xff);
        }
    }
    pixel
}

fn paint_scene(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    scene: &NativeSceneSnapshot,
    damage: Option<&[NativeDamageRect]>,
) {
    let full = [NativeDamageRect {
        x: 0,
        y: 0,
        width,
        height,
    }];
    for rect in damage.unwrap_or(&full) {
        let Some(rect) = rect.clipped_to_output(width, height) else {
            continue;
        };
        for y in rect.y..rect.y.saturating_add(rect.height as i32) {
            for x in rect.x..rect.x.saturating_add(rect.width as i32) {
                pixels[y as usize * width as usize + x as usize] = scene_pixel(scene, x, y);
            }
        }
    }
}

#[test]
fn fullscreen_restore_matches_full_reference_for_buffer_ages_one_two_three() {
    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 40;
    let normal = NativeSceneSnapshot::from_surfaces(
        &[
            test_renderable_surface(301, 5, 7, 22, 14, RenderableSurfaceDamage::Empty),
            test_renderable_surface(302, 37, 9, 22, 25, RenderableSurfaceDamage::Empty),
        ],
        Vec::new(),
    );
    let restored = restored_scene();

    for age in 1..=3_u32 {
        let mut history = NativeSceneHistory::new(NativeFrameSceneSnapshot {
            frame_id: 0,
            render_generation: 0,
            scene: normal.clone(),
            cursor_damage: NativeCursorDamageBounds::default(),
        });
        for frame_id in 1..=20_u64 {
            let scene = fullscreen_scene(frame_id);
            history.replace_ready(NativeFrameSceneSnapshot {
                frame_id,
                render_generation: frame_id,
                scene,
                cursor_damage: NativeCursorDamageBounds::default(),
            });
            let token = 700 + frame_id;
            assert!(history.queue_submission(token));
            assert!(history.promote_pageflip(token));
        }

        let mut planner = PartialRepaintPlanner::new(
            (WIDTH, HEIGHT),
            EglPartialRepaintCapabilities {
                buffer_age: true,
                partial_render_repair: true,
                swap_buffers_with_damage: true,
            },
        );
        for _ in 0..age {
            let plan = planner.plan(OutputDamage::Full, BufferAge::Value(0));
            planner.commit_presented_transition(plan.render_damage.clone());
        }
        let current_damage = native_output_damage_for_scene_snapshots(
            WIDTH,
            HEIGHT,
            history.presented_scene(),
            &restored,
            NativeCursorDamageBounds::default(),
        );
        let plan = planner.plan(
            current_damage.as_renderer_damage(WIDTH, HEIGHT),
            BufferAge::Value(age as i32),
        );
        let repair_rects = plan
            .repair_damage
            .rects_slice()
            .iter()
            .map(|rect| NativeDamageRect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            })
            .collect::<Vec<_>>();
        let fullscreen = history.presented_scene().clone();
        let mut reused = vec![0; (WIDTH * HEIGHT) as usize];
        paint_scene(&mut reused, WIDTH, HEIGHT, &fullscreen, None);
        if plan.mode == RepaintMode::Full {
            paint_scene(&mut reused, WIDTH, HEIGHT, &restored, None);
        } else {
            paint_scene(&mut reused, WIDTH, HEIGHT, &restored, Some(&repair_rects));
        }
        let mut reference = vec![0; (WIDTH * HEIGHT) as usize];
        paint_scene(&mut reference, WIDTH, HEIGHT, &restored, None);

        assert_eq!(
            reused, reference,
            "age {age} restore differs from reference"
        );
        for (x, y) in [(5, 7), (4, 4), (37, 9), (36, 5), (0, 0)] {
            assert_eq!(
                reused[y * WIDTH as usize + x],
                reference[y * WIDTH as usize + x],
                "stale pixel at ({x},{y}) for age {age}"
            );
        }
    }
}
