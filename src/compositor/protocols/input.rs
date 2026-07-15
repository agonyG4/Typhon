use super::super::*;

impl Dispatch<wl_output::WlOutput, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &wl_output::WlOutput,
        request: wl_output::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, wl_output::Request::Release) {
            state.unregister_output_resource(resource);
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &wl_seat::WlSeat,
        request: wl_seat::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_seat::Request::GetPointer { id } => {
                let pointer = data_init.init(id, ());
                state.register_pointer(pointer);
            }
            wl_seat::Request::GetKeyboard { id } => {
                let keyboard = data_init.init(id, ());
                send_keyboard_initial_state(&keyboard);
                state.register_keyboard(keyboard);
            }
            wl_seat::Request::GetTouch { .. } => {
                state.post_protocol_error(
                    client,
                    resource,
                    wl_seat::Error::MissingCapability,
                    "Typhon does not advertise the wl_seat touch capability".to_string(),
                );
            }
            wl_seat::Request::Release => {}
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "wl_seat",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &wl_pointer::WlPointer,
        request: wl_pointer::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_pointer::Request::SetCursor {
                serial,
                surface,
                hotspot_x,
                hotspot_y,
            } => {
                state.set_pointer_cursor(resource, serial, surface, hotspot_x, hotspot_y);
            }
            wl_pointer::Request::Release => {
                state.unregister_pointer(resource);
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "wl_pointer",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &wl_keyboard::WlKeyboard,
        request: wl_keyboard::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, wl_keyboard::Request::Release) {
            state.unregister_keyboard(resource);
        }
    }
}
