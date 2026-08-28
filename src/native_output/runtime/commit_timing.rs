use oblivion_one::compositor::OwnCompositorServer;
use oblivion_one::native::presentation_deadline::{
    PresentationDeadlinePlanner, PresentationTarget,
};

pub(super) fn defer_after_timing(
    presentation_deadline: &mut PresentationDeadlinePlanner,
    scheduled_presentation_target: &mut Option<PresentationTarget>,
    queued_redraw_requested: &mut bool,
) {
    presentation_deadline.clear_scheduled_target();
    *scheduled_presentation_target = None;
    *queued_redraw_requested = true;
}

pub(super) fn reset_after_same_buffer(
    server: &mut OwnCompositorServer,
    presentation_deadline: &mut PresentationDeadlinePlanner,
    scheduled_presentation_target: &mut Option<PresentationTarget>,
) {
    presentation_deadline.clear_scheduled_target();
    *scheduled_presentation_target = None;
    server.invalidate_commit_timing_targets();
}

pub(super) fn refreshed_published_state(
    server: &OwnCompositorServer,
    last_rendered_scene_generation: u64,
) -> (u64, u64, bool, bool) {
    let scene_generation = server.scene_render_generation();
    (
        server.render_generation(),
        scene_generation,
        logical_scene_changed(last_rendered_scene_generation, scene_generation),
        server.has_unowned_frame_work(),
    )
}

pub(super) const fn logical_scene_changed(logical_generation: u64, scene_generation: u64) -> bool {
    logical_generation != scene_generation
}

pub(super) fn retire_logical_scene_generation(
    logical_generation: &mut u64,
    scene_generation: u64,
    terminal_no_visual_change: bool,
) -> bool {
    if !terminal_no_visual_change {
        return false;
    }
    *logical_generation = scene_generation;
    true
}

pub(super) const fn no_visual_change_completes_cycle(
    scene_changed: bool,
    protocol_work: bool,
) -> bool {
    scene_changed || protocol_work
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_primary_no_visual_change_retires_logical_generation_without_physical_presentation() {
        let mut logical_generation = 4;

        assert!(retire_logical_scene_generation(
            &mut logical_generation,
            5,
            true,
        ));
        assert_eq!(logical_generation, 5);
        assert!(!logical_scene_changed(logical_generation, 5));
        assert!(logical_scene_changed(logical_generation, 6));
    }

    #[test]
    fn atomic_no_logical_damage_retires_logical_generation_without_physical_presentation() {
        let mut logical_generation = 9;

        assert!(retire_logical_scene_generation(
            &mut logical_generation,
            10,
            true,
        ));
        assert_eq!(logical_generation, 10);
        assert!(!logical_scene_changed(logical_generation, 10));
    }

    #[test]
    fn idle_no_visual_tick_is_not_completed_work() {
        assert!(!no_visual_change_completes_cycle(false, false));
        assert!(no_visual_change_completes_cycle(true, false));
        assert!(no_visual_change_completes_cycle(false, true));
    }
}
