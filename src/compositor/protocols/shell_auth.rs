use crate::astrea_shell_auth::server::astrea_shell_auth_manager_v1;
use std::sync::atomic::{AtomicBool, Ordering};

use super::super::*;

#[derive(Debug, Default)]
pub(in crate::compositor) struct AstreaShellAuthData {
    attempted: AtomicBool,
}

impl GlobalDispatch<astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1, ()>
    for CompositorState
{
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, AstreaShellAuthData::default());
    }
}

impl Dispatch<astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1, AstreaShellAuthData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1,
        request: astrea_shell_auth_manager_v1::Request,
        data: &AstreaShellAuthData,
        handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            astrea_shell_auth_manager_v1::Request::Destroy => {}
            astrea_shell_auth_manager_v1::Request::Authenticate { capability } => {
                if data.attempted.swap(true, Ordering::AcqRel) {
                    resource.rejected();
                    return;
                }
                if state.authenticate_astrea_shell_client(client, handle, &capability) {
                    resource.authenticated();
                } else {
                    resource.rejected();
                }
            }
        }
    }
}
