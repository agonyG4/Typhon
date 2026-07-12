use super::super::*;

impl Dispatch<wp_presentation::WpPresentation, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &wp_presentation::WpPresentation,
        request: wp_presentation::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_presentation::Request::Destroy => {}
            wp_presentation::Request::Feedback { surface, callback } => {
                let feedback = data_init.init(callback, ());
                let surface_id = compositor_surface_id(&surface);
                client_pacing_log(
                    "presentation_feedback_requested",
                    &[
                        ("surface", surface_id.to_string()),
                        (
                            "root",
                            state.root_surface_id_for_surface(surface_id).to_string(),
                        ),
                        (
                            "client",
                            format!("{:?}", state.surface_client_ids.get(&surface_id)),
                        ),
                        ("feedback", format!("{:?}", feedback.id())),
                    ],
                );
                state
                    .pending_surface_presentation_feedbacks
                    .entry(surface_id)
                    .or_default()
                    .push(PendingPresentationFeedback {
                        surface_id,
                        surface,
                        feedback,
                    });
            }
            _ => {}
        }
    }
}

impl Dispatch<wp_presentation_feedback::WpPresentationFeedback, ()> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wp_presentation_feedback::WpPresentationFeedback,
        _request: wp_presentation_feedback::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}
