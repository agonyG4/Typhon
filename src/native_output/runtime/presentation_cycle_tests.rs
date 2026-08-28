use super::presentation_cycle::complete_compatibility_no_visual_change;
use super::*;
use crate::native_output::runtime::commit_timing::logical_scene_changed;
use crate::native_output::scanout::{NativePaintOutcome, NativePaintStats, NativeScanoutKind};
use oblivion_one::compositor::{DesktopFrameCopyKind, DesktopSceneRebuildKind};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_COMPATIBILITY_SKIP_SOCKET: AtomicU64 = AtomicU64::new(0);

fn skipped_outcome() -> NativePaintOutcome {
    NativePaintOutcome::Skipped(NativePaintStats {
        backend: NativeScanoutKind::NativeEglGbmOpaqueCompatibility,
        scanout_format: None,
        width: 1,
        height: 1,
        bytes: 0,
        copy_bytes: 0,
        write_bytes: 0,
        gpu_draw_us: 0,
        egl_swap_us: 0,
        shm_upload_bytes: 0,
        dmabuf_imports: 0,
        dmabuf_reuses: 0,
        dmabuf_import_failures: 0,
        dmabuf_cache_entries: 0,
        dmabuf_cache_peak_entries: 0,
        dmabuf_cache_evictions: 0,
        scene_rebuild: DesktopSceneRebuildKind::None,
        frame_copy: DesktopFrameCopyKind::None,
        total_us: 0,
        render_us: 0,
        copy_us: 0,
        write_us: 0,
        gles_repaint: None,
        swap_with_damage_used: false,
    })
}

#[test]
fn compatibility_renderer_skip_retires_logical_generation_and_terminally_owns_batch() {
    let socket_name = format!(
        "typhon-compatibility-skip-test-{}-{}",
        std::process::id(),
        NEXT_COMPATIBILITY_SKIP_SOCKET.fetch_add(1, Ordering::Relaxed)
    );
    let mut server = OwnCompositorServer::bind_cpu_composition(socket_name)
        .expect("bind compatibility skip test compositor");
    server.capture_frame_callbacks_for_render();
    let mut frame_scheduler = NativeFrameScheduler::new(60, 0);
    frame_scheduler.queue_visual_work();
    let mut last_logical_scene_generation = 41;
    let scene_generation = 42;
    let mut queued_redraw_requested = true;
    let previous_client_cursor_damage = NativeClientCursorDamageState {
        surface_id: 7,
        generation: 1,
        hotspot_x: 0,
        hotspot_y: 0,
        rect: None,
    };
    let current_client_cursor_damage = NativeClientCursorDamageState {
        surface_id: 7,
        generation: 2,
        hotspot_x: 1,
        hotspot_y: 2,
        rect: None,
    };
    let mut last_client_cursor_damage = Some(previous_client_cursor_damage);
    let mut last_software_cursor_damage = Some(NativeDamageRect {
        x: 1,
        y: 2,
        width: 3,
        height: 4,
    });
    let current_software_cursor_damage = Some(NativeDamageRect {
        x: 5,
        y: 6,
        width: 7,
        height: 8,
    });
    let prepared_batch = server
        .prepared_frame_batch_id()
        .expect("compatibility render owns a prepared batch");

    assert!(complete_compatibility_no_visual_change(
        skipped_outcome(),
        &mut server,
        &mut frame_scheduler,
        &mut last_logical_scene_generation,
        scene_generation,
        &mut queued_redraw_requested,
        &mut last_client_cursor_damage,
        &mut last_software_cursor_damage,
        Some(current_client_cursor_damage),
        current_software_cursor_damage,
    ));

    assert_eq!(last_logical_scene_generation, scene_generation);
    assert!(!logical_scene_changed(
        last_logical_scene_generation,
        scene_generation
    ));
    assert!(logical_scene_changed(
        last_logical_scene_generation,
        scene_generation + 1
    ));
    assert!(!frame_scheduler.visual_work_queued());
    assert!(!queued_redraw_requested);
    assert_eq!(server.prepared_frame_batch_id(), None);
    assert_eq!(server.frame_batch_count(), 0);
    assert_eq!(
        last_client_cursor_damage,
        Some(current_client_cursor_damage)
    );
    assert_eq!(last_software_cursor_damage, current_software_cursor_damage);
    assert_ne!(server.prepared_frame_batch_id(), Some(prepared_batch));
}
