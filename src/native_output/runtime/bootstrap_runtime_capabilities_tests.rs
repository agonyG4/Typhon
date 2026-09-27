const BOOTSTRAP_SOURCE: &str = include_str!("bootstrap.rs");

#[test]
fn initial_paint_fallback_republishes_capabilities_from_replacement_scanout() {
    let initial_install = BOOTSTRAP_SOURCE
        .split_once("let initial_material_generation")
        .expect("initial scanout setup must publish its material generation")
        .0;
    assert!(
        initial_install.contains("publish_scanout_runtime_capabilities(&mut server, &scanout);")
    );

    let publisher = BOOTSTRAP_SOURCE
        .split_once("fn publish_scanout_runtime_capabilities(")
        .expect("native bootstrap must define a scanout capability publisher")
        .1
        .split_once("fn keyboard_persistence_snapshot(")
        .expect("scanout capability publisher must end before the next helper")
        .0;
    assert!(publisher.contains("set_lifecycle_animation_renderer_available"));
    assert!(publisher.contains("set_material_runtime_capabilities"));
    assert!(publisher.contains("set_material_program_rendering_available"));
    assert!(publisher.contains("material_program_rendering_available()"));

    let after_scanout_drop = BOOTSTRAP_SOURCE
        .split_once("drop(scanout);")
        .expect("initial-paint fallback must drop the failed scanout")
        .1;
    let after_fallback_open = after_scanout_drop
        .split_once("scanout = NativeScanoutBackend::open(")
        .expect("initial-paint fallback must install a replacement scanout")
        .1;
    let before_paint_retry = after_fallback_open
        .split_once("scanout.paint_server_frame(")
        .expect("initial-paint fallback must retry the initial paint")
        .0;

    assert!(
        before_paint_retry.contains("publish_scanout_runtime_capabilities"),
        "the replacement scanout must republish lifecycle and material capabilities before retrying paint"
    );
}
