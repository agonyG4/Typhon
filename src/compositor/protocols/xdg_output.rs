use super::super::output::{
    OUTPUT_DESCRIPTION, OUTPUT_LOGICAL_POSITION, OUTPUT_NAME, OUTPUT_TRANSFORM, logical_output_size,
};
use super::super::*;
use wayland_protocols::xdg::xdg_output::zv1::server::{zxdg_output_manager_v1, zxdg_output_v1};

/// xdg_output version that deprecates `zxdg_output_v1.done` in favour of
/// `wl_output.done`.
const XDG_OUTPUT_DONE_DEPRECATED_SINCE: u32 = 3;
const WL_OUTPUT_DONE_SINCE: u32 = 2;
const XDG_OUTPUT_NAME_SINCE: u32 = 2;

pub(in crate::compositor) struct XdgOutputData {
    output: wl_output::WlOutput,
}

impl CompositorState {
    fn wl_output_is_bound(&self, output: &wl_output::WlOutput) -> bool {
        output.is_alive()
            && self
                .output_resources
                .iter()
                .any(|binding| binding.resource.id() == output.id())
    }

    fn send_xdg_output_logical_geometry(&self, xdg_output: &zxdg_output_v1::ZxdgOutputV1) {
        let (x, y) = OUTPUT_LOGICAL_POSITION;
        let (width, height) =
            logical_output_size(self.output_size, self.output_scale, OUTPUT_TRANSFORM);
        let _ = xdg_output.send_event(zxdg_output_v1::Event::LogicalPosition { x, y });
        let _ = xdg_output.send_event(zxdg_output_v1::Event::LogicalSize { width, height });
    }

    /// Re-sends logical geometry to every live xdg_output whose wl_output is
    /// still bound. Called from the wl_output mode and scale update paths.
    pub(in crate::compositor) fn send_xdg_output_geometry_to_bound_outputs(&self) {
        for xdg_output in &self.xdg_output_resources {
            if !xdg_output.is_alive() {
                continue;
            }
            let Some(data) = xdg_output.data::<XdgOutputData>() else {
                continue;
            };
            if self.wl_output_is_bound(&data.output) {
                self.send_xdg_output_logical_geometry(xdg_output);
                send_xdg_output_done(xdg_output, &data.output);
            }
        }
    }
}

/// `wl_output.done` is the source of truth for xdg_output v3 and later; older
/// xdg_output resources (or a wl_output too old to carry `done`) get the
/// deprecated `zxdg_output_v1.done` instead.
fn send_xdg_output_done(xdg_output: &zxdg_output_v1::ZxdgOutputV1, output: &wl_output::WlOutput) {
    if xdg_output.version() >= XDG_OUTPUT_DONE_DEPRECATED_SINCE
        && output.version() >= WL_OUTPUT_DONE_SINCE
    {
        let _ = output.send_event(wl_output::Event::Done);
    } else {
        let _ = xdg_output.send_event(zxdg_output_v1::Event::Done);
    }
}

impl GlobalDispatch<zxdg_output_manager_v1::ZxdgOutputManagerV1, ()> for CompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<zxdg_output_manager_v1::ZxdgOutputManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &zxdg_output_manager_v1::ZxdgOutputManagerV1,
        request: zxdg_output_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zxdg_output_manager_v1::Request::Destroy => {}
            zxdg_output_manager_v1::Request::GetXdgOutput { id, output } => {
                let xdg_output = data_init.init(
                    id,
                    XdgOutputData {
                        output: output.clone(),
                    },
                );
                state
                    .xdg_output_resources
                    .retain(|resource| resource.is_alive());
                state.xdg_output_resources.push(xdg_output.clone());
                if !state.wl_output_is_bound(&output) {
                    return;
                }
                state.send_xdg_output_logical_geometry(&xdg_output);
                if xdg_output.version() >= XDG_OUTPUT_NAME_SINCE {
                    let _ = xdg_output.send_event(zxdg_output_v1::Event::Name {
                        name: OUTPUT_NAME.to_string(),
                    });
                    let _ = xdg_output.send_event(zxdg_output_v1::Event::Description {
                        description: OUTPUT_DESCRIPTION.to_string(),
                    });
                }
                send_xdg_output_done(&xdg_output, &output);
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "zxdg_output_manager_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, XdgOutputData> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &zxdg_output_v1::ZxdgOutputV1,
        request: zxdg_output_v1::Request,
        _data: &XdgOutputData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zxdg_output_v1::Request::Destroy => {}
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "zxdg_output_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}
