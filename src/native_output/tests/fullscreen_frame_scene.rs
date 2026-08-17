use super::output::test_renderable_surface;
use super::*;

#[test]
fn solitary_fullscreen_snapshot_matches_the_filtered_renderer_scene() {
    let rear = test_renderable_surface(101, 80, 80, 500, 300, RenderableSurfaceDamage::Empty);
    let owner = test_renderable_surface(102, 0, 0, 1280, 800, RenderableSurfaceDamage::Full);
    let renderer_surfaces = vec![owner.clone()];
    let raw_surfaces = vec![rear.clone(), owner.clone()];
    let decorations = vec![
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

    // This is the current pre-fix construction: the renderer has activated
    // the solitary fullscreen tree, while the snapshot still sees the raw
    // logical desktop and all of its decorations.
    let snapshot = NativeFrameSceneSnapshot {
        frame_id: 1,
        render_generation: 1,
        scene: NativeSceneSnapshot::from_surfaces(&raw_surfaces, decorations),
        cursor_damage: NativeCursorDamageBounds::default(),
    };
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

    assert_eq!(renderer_ids, snapshot_ids);
}
