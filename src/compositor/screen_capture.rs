use crate::astrea_screen_capture::server::{
    astrea_screen_capture_manager_v1, astrea_screen_capture_v1,
};
use std::collections::HashMap;

use super::*;

#[derive(Debug, Clone)]
pub struct AstreaScreenCapturePending {
    pub capture: astrea_screen_capture_v1::AstreaScreenCaptureV1,
    pub output: wl_output::WlOutput,
}

#[derive(Debug, Clone)]
pub(in crate::compositor) struct AstreaScreenCaptureResourceData {
    pub(in crate::compositor) _client_id: ClientId,
    pub(in crate::compositor) _output_id: u32,
}

impl GlobalDispatch<astrea_screen_capture_manager_v1::AstreaScreenCaptureManagerV1, ()>
    for CompositorState
{
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<astrea_screen_capture_manager_v1::AstreaScreenCaptureManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<astrea_screen_capture_manager_v1::AstreaScreenCaptureManagerV1, ()>
    for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &astrea_screen_capture_manager_v1::AstreaScreenCaptureManagerV1,
        request: astrea_screen_capture_manager_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            astrea_screen_capture_manager_v1::Request::Destroy => {}
            astrea_screen_capture_manager_v1::Request::CaptureOutput { capture, output } => {
                let client_id = client.id();
                let capture = data_init.init(
                    capture,
                    AstreaScreenCaptureResourceData {
                        _client_id: client_id.clone(),
                        _output_id: output.id().protocol_id(),
                    },
                );
                if !state.astrea_shell_mutation_allowed(client) {
                    state.post_protocol_error(
                        client,
                        resource,
                        astrea_screen_capture_manager_v1::Error::Unauthorized,
                        "client is not an authorized Astrea shell client",
                    );
                    return;
                }
                if !state.output_resource_is_current(&output) {
                    state.post_protocol_error(
                        client,
                        resource,
                        astrea_screen_capture_manager_v1::Error::InvalidOutput,
                        "capture output is not a current wl_output resource",
                    );
                    return;
                }
                if state.astrea_screen_captures.contains_key(&client_id) {
                    capture.failed("busy".to_string());
                    return;
                }
                state.astrea_screen_captures.insert(
                    client_id.clone(),
                    AstreaScreenCapturePending { capture, output },
                );
            }
        }
    }
}

impl Dispatch<astrea_screen_capture_v1::AstreaScreenCaptureV1, AstreaScreenCaptureResourceData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &astrea_screen_capture_v1::AstreaScreenCaptureV1,
        request: astrea_screen_capture_v1::Request,
        _data: &AstreaScreenCaptureResourceData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, astrea_screen_capture_v1::Request::Destroy) {
            state.remove_astrea_screen_capture(resource);
        }
    }

    fn destroyed(
        state: &mut Self,
        _client_id: ClientId,
        resource: &astrea_screen_capture_v1::AstreaScreenCaptureV1,
        _data: &AstreaScreenCaptureResourceData,
    ) {
        state.remove_astrea_screen_capture(resource);
    }
}

impl CompositorState {
    pub(in crate::compositor) fn output_resource_is_current(
        &self,
        output: &wl_output::WlOutput,
    ) -> bool {
        self.output_resources.iter().any(|candidate| {
            candidate.id().protocol_id() == output.id().protocol_id()
                && candidate.id().same_client_as(&output.id())
                && candidate.is_alive()
        })
    }

    pub(in crate::compositor) fn has_pending_astrea_screen_capture(&self) -> bool {
        !self.astrea_screen_captures.is_empty()
    }

    pub(in crate::compositor) fn take_pending_astrea_screen_capture(
        &mut self,
    ) -> Option<AstreaScreenCapturePending> {
        let client_id = self.astrea_screen_captures.keys().next().cloned()?;
        self.astrea_screen_captures.remove(&client_id)
    }

    pub(in crate::compositor) fn remove_astrea_screen_capture(
        &mut self,
        resource: &astrea_screen_capture_v1::AstreaScreenCaptureV1,
    ) {
        self.astrea_screen_captures.retain(|_, pending| {
            pending.capture.id().protocol_id() != resource.id().protocol_id()
                || !pending.capture.id().same_client_as(&resource.id())
        });
    }

    pub(in crate::compositor) fn clear_astrea_screen_capture_client(
        &mut self,
        client_id: &ClientId,
    ) {
        self.astrea_screen_captures.remove(client_id);
    }

    pub(in crate::compositor) fn fail_astrea_screen_captures_for_output(
        &mut self,
        output: &wl_output::WlOutput,
        reason: &str,
    ) {
        // ClientId is the compositor's exact authenticated identity key; it is
        // intentionally retained as the pending-capture map key.
        #[allow(clippy::mutable_key_type)]
        let mut failed = HashMap::new();
        std::mem::swap(&mut failed, &mut self.astrea_screen_captures);
        for (client_id, pending) in failed {
            if pending.output.id().protocol_id() == output.id().protocol_id()
                && pending.output.id().same_client_as(&output.id())
            {
                pending.capture.failed(reason.to_string());
            } else {
                self.astrea_screen_captures.insert(client_id, pending);
            }
        }
    }
}

impl OwnCompositorServer {
    pub fn has_pending_astrea_screen_capture(&self) -> bool {
        self.state.has_pending_astrea_screen_capture()
    }

    pub fn take_pending_astrea_screen_capture(&mut self) -> Option<AstreaScreenCapturePending> {
        self.state.take_pending_astrea_screen_capture()
    }

    pub fn astrea_screen_capture_output_is_current(&self, output: &wl_output::WlOutput) -> bool {
        self.state.output_resource_is_current(output)
    }
}
