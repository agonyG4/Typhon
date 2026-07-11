use crate::astrea_shortcuts::server::{astrea_shortcut_v1, astrea_shortcuts_manager_v1};

use super::super::*;

#[derive(Debug, Clone)]
pub(in crate::compositor) struct AstreaShortcutResourceData {
    pub(in crate::compositor) _namespace: String,
    pub(in crate::compositor) _name: String,
}

impl GlobalDispatch<astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1, ()> for CompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        _resource: &astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1,
        request: astrea_shortcuts_manager_v1::Request,
        _data: &(),
        dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            astrea_shortcuts_manager_v1::Request::Destroy => {}
            astrea_shortcuts_manager_v1::Request::RegisterShortcut {
                id,
                namespace,
                name,
                description: _,
            } => {
                let shortcut = data_init.init(
                    id,
                    AstreaShortcutResourceData {
                        _namespace: namespace.clone(),
                        _name: name.clone(),
                    },
                );
                if state.astrea_shortcut_registration_allowed(&namespace, client, dhandle) {
                    state.register_astrea_shortcut(shortcut, namespace, name);
                } else {
                    shortcut.cancelled(state.next_configure_serial());
                }
            }
        }
    }
}

impl Dispatch<astrea_shortcut_v1::AstreaShortcutV1, AstreaShortcutResourceData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &astrea_shortcut_v1::AstreaShortcutV1,
        request: astrea_shortcut_v1::Request,
        _data: &AstreaShortcutResourceData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            astrea_shortcut_v1::Request::Destroy => {
                state.unregister_astrea_shortcut(resource);
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &astrea_shortcut_v1::AstreaShortcutV1,
        _data: &AstreaShortcutResourceData,
    ) {
        state.unregister_astrea_shortcut(resource);
    }
}
