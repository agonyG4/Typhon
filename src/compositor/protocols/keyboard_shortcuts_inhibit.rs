use super::super::*;
use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::server::{
    zwp_keyboard_shortcuts_inhibit_manager_v1, zwp_keyboard_shortcuts_inhibitor_v1,
};

impl
    GlobalDispatch<
        zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1,
        (),
    > for CompositorState
{
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<
            zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1,
        >,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1, ()>
    for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1,
        request: zwp_keyboard_shortcuts_inhibit_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_keyboard_shortcuts_inhibit_manager_v1::Request::Destroy => {}
            zwp_keyboard_shortcuts_inhibit_manager_v1::Request::InhibitShortcuts {
                id,
                surface,
                seat,
            } => {
                if !surface.id().same_client_as(&resource.id())
                    || !seat.id().same_client_as(&resource.id())
                {
                    return;
                }
                let logical_seat = LogicalSeatId::PRIMARY;
                if state
                    .shortcut_inhibition
                    .contains_pair(&surface, logical_seat)
                {
                    state.shortcut_inhibition.note_duplicate();
                    resource.post_error(
                        zwp_keyboard_shortcuts_inhibit_manager_v1::Error::AlreadyInhibited,
                        "keyboard shortcuts are already inhibited for this surface and seat",
                    );
                    return;
                }
                let inhibitor = data_init.init(id, ());
                state.register_keyboard_shortcut_inhibitor(
                    surface,
                    logical_seat,
                    client.id(),
                    inhibitor,
                );
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "zwp_keyboard_shortcuts_inhibit_manager_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1, ()>
    for CompositorState
{
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1,
        request: zwp_keyboard_shortcuts_inhibitor_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let zwp_keyboard_shortcuts_inhibitor_v1::Request::Destroy = request {
            state.remove_keyboard_shortcut_inhibitor_resource(resource);
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1,
        _data: &(),
    ) {
        state.remove_keyboard_shortcut_inhibitor_resource(resource);
    }
}
