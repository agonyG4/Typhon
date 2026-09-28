use crate::astrea_screen_capture::server::{
    astrea_screen_capture_manager_v1, astrea_screen_capture_v1,
};
use std::collections::HashMap;

use super::*;

#[derive(Debug, Clone)]
pub struct AstreaScreenCapturePending {
    pub capture: astrea_screen_capture_v1::AstreaScreenCaptureV1,
    pub output_id: OutputId,
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
                let capture = data_init.init(capture, ());
                if !state.astrea_shell_mutation_allowed(client) {
                    state.post_protocol_error(
                        client,
                        resource,
                        astrea_screen_capture_manager_v1::Error::Unauthorized,
                        "client is not an authorized Astrea shell client",
                    );
                    return;
                }
                let Some(output_id) = state.output_id_for_binding(&output) else {
                    state.post_protocol_error(
                        client,
                        resource,
                        astrea_screen_capture_manager_v1::Error::InvalidOutput,
                        "capture output is not a current wl_output resource",
                    );
                    return;
                };
                if state.astrea_screen_captures.contains_key(&client_id) {
                    capture.failed("busy".to_string());
                    return;
                }
                state.astrea_screen_captures.insert(
                    client_id.clone(),
                    AstreaScreenCapturePending { capture, output_id },
                );
            }
        }
    }
}

impl Dispatch<astrea_screen_capture_v1::AstreaScreenCaptureV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &astrea_screen_capture_v1::AstreaScreenCaptureV1,
        request: astrea_screen_capture_v1::Request,
        _data: &(),
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
        _data: &(),
    ) {
        state.remove_astrea_screen_capture(resource);
    }
}

impl CompositorState {
    pub(in crate::compositor) fn output_id_for_binding(
        &self,
        output: &wl_output::WlOutput,
    ) -> Option<OutputId> {
        let object_id = output.id();
        self.output_resources
            .iter()
            .find(|binding| {
                binding.resource.id() == object_id
                    && binding.resource.is_alive()
                    && self.logical_output_ids.contains(&binding.output_id)
            })
            .map(|binding| binding.output_id)
    }

    pub(in crate::compositor) fn logical_output_is_current(&self, output_id: OutputId) -> bool {
        self.logical_output_ids.contains(&output_id)
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

    #[allow(dead_code)]
    pub(in crate::compositor) fn fail_astrea_screen_captures_for_output(
        &mut self,
        output_id: OutputId,
        reason: &str,
    ) {
        // ClientId is the compositor's exact authenticated identity key; it is
        // intentionally retained as the pending-capture map key.
        #[allow(clippy::mutable_key_type)]
        let mut failed = HashMap::new();
        std::mem::swap(&mut failed, &mut self.astrea_screen_captures);
        for (client_id, pending) in failed {
            if pending.output_id == output_id {
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

    pub fn astrea_screen_capture_output_is_current(&self, output_id: OutputId) -> bool {
        self.state.logical_output_is_current(output_id)
    }
}
