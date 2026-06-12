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
                state
                    .pending_presentation_feedbacks
                    .push(PendingPresentationFeedback {
                        surface_id: compositor_surface_id(&surface),
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
