use wayland_protocols::wp::color_management::v1::server::{
    wp_color_management_output_v1, wp_color_management_surface_feedback_v1,
    wp_color_management_surface_v1, wp_color_manager_v1, wp_image_description_creator_icc_v1,
    wp_image_description_creator_params_v1, wp_image_description_info_v1, wp_image_description_v1,
};
use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};

use super::{CompositorState, protocols::versions};

pub(super) type PendingColorInfo = wp_image_description_info_v1::WpImageDescriptionInfoV1;

const SRGB_IMAGE_DESCRIPTION_ID: u32 = 1;
const RENDER_INTENT_PERCEPTUAL: u32 = 0;
const FEATURE_PARAMETRIC: u32 = 1;
const PRIMARIES_SRGB: u32 = 1;
const TRANSFER_FUNCTION_GAMMA22: u32 = 2;
const TRANSFER_FUNCTION_SRGB: u32 = 9;
const ERROR_UNSUPPORTED_FEATURE: u32 = 0;
const CAUSE_UNSUPPORTED: u32 = 1;

pub(super) fn register_color_management_global(display: &DisplayHandle) {
    display.create_global::<CompositorState, wp_color_manager_v1::WpColorManagerV1, _>(
        versions::WP_COLOR_MANAGER_V1,
        (),
    );
}

impl GlobalDispatch<wp_color_manager_v1::WpColorManagerV1, ()> for CompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<wp_color_manager_v1::WpColorManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        send_supported_color_manager_state(&manager);
    }
}

impl Dispatch<wp_color_manager_v1::WpColorManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &wp_color_manager_v1::WpColorManagerV1,
        request: wp_color_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_color_manager_v1::Request::Destroy => {}
            wp_color_manager_v1::Request::GetOutput { id, .. } => {
                data_init.init(id, ());
            }
            wp_color_manager_v1::Request::GetSurface { id, .. } => {
                data_init.init(id, ());
            }
            wp_color_manager_v1::Request::GetSurfaceFeedback { id, .. } => {
                let feedback = data_init.init(id, ());
                send_preferred_surface_color_description(&feedback);
            }
            wp_color_manager_v1::Request::CreateParametricCreator { obj } => {
                data_init.init(obj, ());
            }
            wp_color_manager_v1::Request::CreateIccCreator { obj } => {
                data_init.init(obj, ());
                state.post_protocol_error(
                    client,
                    resource,
                    ERROR_UNSUPPORTED_FEATURE,
                    "ICC color descriptions are not supported yet".to_string(),
                );
            }
            _ => {
                state.post_protocol_error(
                    client,
                    resource,
                    ERROR_UNSUPPORTED_FEATURE,
                    "color management request is not supported yet".to_string(),
                );
            }
        }
    }
}

impl Dispatch<wp_color_management_output_v1::WpColorManagementOutputV1, ()> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wp_color_management_output_v1::WpColorManagementOutputV1,
        request: wp_color_management_output_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_color_management_output_v1::Request::Destroy => {}
            wp_color_management_output_v1::Request::GetImageDescription { image_description } => {
                let image_description = data_init.init(image_description, ());
                send_image_description_ready(&image_description);
            }
            _ => {}
        }
    }
}

impl Dispatch<wp_color_management_surface_v1::WpColorManagementSurfaceV1, ()> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wp_color_management_surface_v1::WpColorManagementSurfaceV1,
        _request: wp_color_management_surface_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<wp_color_management_surface_feedback_v1::WpColorManagementSurfaceFeedbackV1, ()>
    for CompositorState
{
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wp_color_management_surface_feedback_v1::WpColorManagementSurfaceFeedbackV1,
        request: wp_color_management_surface_feedback_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_color_management_surface_feedback_v1::Request::Destroy => {}
            wp_color_management_surface_feedback_v1::Request::GetPreferred {
                image_description,
            } => {
                let image_description = data_init.init(image_description, ());
                send_image_description_ready(&image_description);
            }
            _ => {}
        }
    }
}

impl Dispatch<wp_image_description_v1::WpImageDescriptionV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &wp_image_description_v1::WpImageDescriptionV1,
        request: wp_image_description_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_image_description_v1::Request::Destroy => {}
            wp_image_description_v1::Request::GetInformation { information } => {
                let information = data_init.init(information, ());
                state.pending_color_info.push(information);
            }
            _ => {}
        }
    }
}

impl Dispatch<wp_image_description_info_v1::WpImageDescriptionInfoV1, ()> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wp_image_description_info_v1::WpImageDescriptionInfoV1,
        _request: wp_image_description_info_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1, ()>
    for CompositorState
{
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
        request: wp_image_description_creator_params_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wp_image_description_creator_params_v1::Request::Create { image_description } =
            request
        {
            let image_description = data_init.init(image_description, ());
            send_image_description_ready(&image_description);
        }
    }
}

impl Dispatch<wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1, ()>
    for CompositorState
{
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wp_image_description_creator_icc_v1::WpImageDescriptionCreatorIccV1,
        request: wp_image_description_creator_icc_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wp_image_description_creator_icc_v1::Request::Create { image_description } = request
        {
            let image_description = data_init.init(image_description, ());
            let _ = image_description.send_event(wp_image_description_v1::Event::Failed {
                cause: WEnum::Unknown(CAUSE_UNSUPPORTED),
                msg: "ICC color descriptions are not supported yet".to_string(),
            });
        }
    }
}

fn send_supported_color_manager_state(manager: &wp_color_manager_v1::WpColorManagerV1) {
    let _ = manager.send_event(wp_color_manager_v1::Event::SupportedFeature {
        feature: WEnum::Unknown(FEATURE_PARAMETRIC),
    });
    let _ = manager.send_event(wp_color_manager_v1::Event::SupportedPrimariesNamed {
        primaries: WEnum::Unknown(PRIMARIES_SRGB),
    });
    let _ = manager.send_event(wp_color_manager_v1::Event::SupportedTfNamed {
        tf: WEnum::Unknown(TRANSFER_FUNCTION_SRGB),
    });
    let _ = manager.send_event(wp_color_manager_v1::Event::SupportedTfNamed {
        tf: WEnum::Unknown(TRANSFER_FUNCTION_GAMMA22),
    });
    let _ = manager.send_event(wp_color_manager_v1::Event::SupportedIntent {
        render_intent: WEnum::Unknown(RENDER_INTENT_PERCEPTUAL),
    });
    let _ = manager.send_event(wp_color_manager_v1::Event::Done);
}

fn send_preferred_surface_color_description(
    feedback: &wp_color_management_surface_feedback_v1::WpColorManagementSurfaceFeedbackV1,
) {
    let _ = feedback.send_event(
        wp_color_management_surface_feedback_v1::Event::PreferredChanged {
            identity: SRGB_IMAGE_DESCRIPTION_ID,
        },
    );
}

fn send_image_description_ready(image_description: &wp_image_description_v1::WpImageDescriptionV1) {
    let _ = image_description.send_event(wp_image_description_v1::Event::Ready {
        identity: SRGB_IMAGE_DESCRIPTION_ID,
    });
}

fn send_srgb_image_description_info(info: &wp_image_description_info_v1::WpImageDescriptionInfoV1) {
    let _ = info.send_event(wp_image_description_info_v1::Event::PrimariesNamed {
        primaries: WEnum::Unknown(PRIMARIES_SRGB),
    });
    let _ = info.send_event(wp_image_description_info_v1::Event::TfNamed {
        tf: WEnum::Unknown(TRANSFER_FUNCTION_SRGB),
    });
    let _ = info.send_event(wp_image_description_info_v1::Event::Luminances {
        min_lum: 100,
        max_lum: 100,
        reference_lum: 100,
    });
    let _ = info.send_event(wp_image_description_info_v1::Event::TargetLuminance {
        min_lum: 100,
        max_lum: 100,
    });
    let _ = info.send_event(wp_image_description_info_v1::Event::Done);
}

pub(super) fn flush_pending_color_info(state: &mut CompositorState) {
    let pending = std::mem::take(&mut state.pending_color_info);
    for info in pending {
        send_srgb_image_description_info(&info);
    }
}
