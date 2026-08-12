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
        scene_generation != last_rendered_scene_generation,
        server.has_unowned_frame_work(),
    )
}
