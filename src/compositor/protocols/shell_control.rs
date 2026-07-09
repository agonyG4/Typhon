use crate::astrea_shell_control::server::{
    astrea_launch_request_v1, astrea_shell_control_manager_v1,
};
use crate::{desktop_launch_for_id, validate_desktop_id};

use super::super::*;

#[derive(Debug, Clone)]
pub(in crate::compositor) struct AstreaLaunchRequestData;

impl GlobalDispatch<astrea_shell_control_manager_v1::AstreaShellControlManagerV1, ()>
    for CompositorState
{
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<astrea_shell_control_manager_v1::AstreaShellControlManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<astrea_shell_control_manager_v1::AstreaShellControlManagerV1, ()>
    for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &astrea_shell_control_manager_v1::AstreaShellControlManagerV1,
        request: astrea_shell_control_manager_v1::Request,
        _data: &(),
        dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            astrea_shell_control_manager_v1::Request::Destroy => {}
            astrea_shell_control_manager_v1::Request::LaunchDesktop {
                request,
                desktop_id,
            } => {
                let launch = data_init.init(request, AstreaLaunchRequestData);
                if !state.astrea_shell_client_allowed(client, dhandle) {
                    resource.post_error(
                        astrea_shell_control_manager_v1::Error::Unauthorized,
                        "client is not an authorized Astrea shell descendant".to_string(),
                    );
                    return;
                }
                if let Err(message) = validate_desktop_id(&desktop_id) {
                    launch.failed(1, message);
                    return;
                }
                match desktop_launch_for_id(&desktop_id).and_then(|desktop| {
                    state.queue_shell_control_launch(desktop.argv, launch.clone())
                }) {
                    Ok(()) => {}
                    Err(message) => launch.failed(2, message),
                }
            }
            astrea_shell_control_manager_v1::Request::LaunchArgvJson { request, argv_json } => {
                let launch = data_init.init(request, AstreaLaunchRequestData);
                if !state.astrea_shell_client_allowed(client, dhandle) {
                    resource.post_error(
                        astrea_shell_control_manager_v1::Error::Unauthorized,
                        "client is not an authorized Astrea shell descendant".to_string(),
                    );
                    return;
                }
                let argv = match serde_json::from_str::<Vec<String>>(&argv_json) {
                    Ok(argv)
                        if !argv.is_empty()
                            && argv.len() <= 256
                            && argv
                                .iter()
                                .all(|arg| !arg.is_empty() && !arg.contains('\0')) =>
                    {
                        argv
                    }
                    _ => {
                        launch.failed(3, "invalid argv JSON".to_string());
                        return;
                    }
                };
                match state.queue_shell_control_launch(argv, launch.clone()) {
                    Ok(()) => {}
                    Err(message) => launch.failed(4, message),
                }
            }
        }
    }
}

impl Dispatch<astrea_launch_request_v1::AstreaLaunchRequestV1, AstreaLaunchRequestData>
    for CompositorState
{
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &astrea_launch_request_v1::AstreaLaunchRequestV1,
        _request: astrea_launch_request_v1::Request,
        _data: &AstreaLaunchRequestData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl CompositorState {
    fn queue_shell_control_launch(
        &mut self,
        argv: Vec<String>,
        request: astrea_launch_request_v1::AstreaLaunchRequestV1,
    ) -> Result<(), String> {
        if self.typhon_socket_name.is_none() {
            return Err("Typhon socket name is not available".to_string());
        }
        if argv.is_empty() {
            return Err("empty command".to_string());
        }
        self.queue_pending_process_launch(PendingProcessLaunch { argv, request });
        Ok(())
    }
}
