use super::super::*;

#[derive(Debug, Clone)]
struct RelativePointerData {
    source_pointer: wl_pointer::WlPointer,
}

#[derive(Debug, Clone)]
struct LockedPointerData {
    constraint_id: u64,
}

#[derive(Debug, Clone)]
struct ConfinedPointerData {
    constraint_id: u64,
}

impl GlobalDispatch<zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1, ()>
    for CompositorState
{
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1, ()>
    for CompositorState
{
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
        request: zwp_relative_pointer_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_relative_pointer_manager_v1::Request::Destroy => {}
            zwp_relative_pointer_manager_v1::Request::GetRelativePointer { id, pointer } => {
                let relative_pointer = data_init.init(
                    id,
                    RelativePointerData {
                        source_pointer: pointer.clone(),
                    },
                );
                state.add_relative_pointer_resource(relative_pointer, pointer);
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_relative_pointer_v1::ZwpRelativePointerV1, RelativePointerData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &zwp_relative_pointer_v1::ZwpRelativePointerV1,
        request: zwp_relative_pointer_v1::Request,
        data: &RelativePointerData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let zwp_relative_pointer_v1::Request::Destroy = request {
            let _ = data.source_pointer.is_alive();
            state.remove_relative_pointer_resource(resource);
        }
    }
}

impl GlobalDispatch<zwp_pointer_constraints_v1::ZwpPointerConstraintsV1, ()> for CompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<zwp_pointer_constraints_v1::ZwpPointerConstraintsV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<zwp_pointer_constraints_v1::ZwpPointerConstraintsV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
        request: zwp_pointer_constraints_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_pointer_constraints_v1::Request::Destroy => {}
            zwp_pointer_constraints_v1::Request::LockPointer {
                id,
                surface,
                pointer,
                region,
                lifetime,
            } => {
                pointer_debug_log(format!(
                    "pointer.lock request surface={} pointer={} client={}",
                    compositor_surface_id(&surface),
                    pointer.id().protocol_id(),
                    wayland_resource_client_label(&pointer)
                ));
                let lifetime = pointer_constraint_lifetime(lifetime);
                let region = region
                    .as_ref()
                    .and_then(|region| region.data::<RegionData>())
                    .map(RegionData::snapshot)
                    .unwrap_or_default();
                let constraint_id = state.allocate_internal_pointer_constraint_id();
                let locked_pointer = data_init.init(id, LockedPointerData { constraint_id });
                if !state.register_pointer_constraint(PointerConstraintRegistration {
                    id: constraint_id,
                    mode: PointerConstraintMode::Locked,
                    lifetime,
                    surface,
                    pointer,
                    locked_resource: Some(locked_pointer.clone()),
                    confined_resource: None,
                    region,
                }) {
                    resource.post_error(
                        zwp_pointer_constraints_v1::Error::AlreadyConstrained,
                        "surface and pointer already have a pointer constraint".to_string(),
                    );
                }
            }
            zwp_pointer_constraints_v1::Request::ConfinePointer {
                id,
                surface,
                pointer,
                region,
                lifetime,
            } => {
                pointer_debug_log(format!(
                    "pointer.confine request surface={} pointer={} client={}",
                    compositor_surface_id(&surface),
                    pointer.id().protocol_id(),
                    wayland_resource_client_label(&pointer)
                ));
                let lifetime = pointer_constraint_lifetime(lifetime);
                let region = region
                    .as_ref()
                    .and_then(|region| region.data::<RegionData>())
                    .map(RegionData::snapshot)
                    .unwrap_or_default();
                let constraint_id = state.allocate_internal_pointer_constraint_id();
                let confined_pointer = data_init.init(id, ConfinedPointerData { constraint_id });
                if !state.register_pointer_constraint(PointerConstraintRegistration {
                    id: constraint_id,
                    mode: PointerConstraintMode::Confined,
                    lifetime,
                    surface,
                    pointer,
                    locked_resource: None,
                    confined_resource: Some(confined_pointer.clone()),
                    region,
                }) {
                    resource.post_error(
                        zwp_pointer_constraints_v1::Error::AlreadyConstrained,
                        "surface and pointer already have a pointer constraint".to_string(),
                    );
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_locked_pointer_v1::ZwpLockedPointerV1, LockedPointerData> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &zwp_locked_pointer_v1::ZwpLockedPointerV1,
        request: zwp_locked_pointer_v1::Request,
        data: &LockedPointerData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_locked_pointer_v1::Request::Destroy => {
                state.remove_pointer_constraint(data.constraint_id);
            }
            zwp_locked_pointer_v1::Request::SetRegion { region } => {
                let region = region
                    .as_ref()
                    .and_then(|region| region.data::<RegionData>())
                    .map(RegionData::snapshot)
                    .unwrap_or_default();
                state.set_pointer_constraint_pending_region(data.constraint_id, region);
            }
            zwp_locked_pointer_v1::Request::SetCursorPositionHint {
                surface_x,
                surface_y,
            } => {
                pointer_debug_log(format!(
                    "pointer.lock cursor_hint pending id={} hint=({},{})",
                    data.constraint_id, surface_x, surface_y
                ));
                state.set_pointer_constraint_pending_cursor_position_hint(
                    data.constraint_id,
                    surface_x,
                    surface_y,
                );
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_confined_pointer_v1::ZwpConfinedPointerV1, ConfinedPointerData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &zwp_confined_pointer_v1::ZwpConfinedPointerV1,
        request: zwp_confined_pointer_v1::Request,
        data: &ConfinedPointerData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_confined_pointer_v1::Request::Destroy => {
                state.remove_pointer_constraint(data.constraint_id);
            }
            zwp_confined_pointer_v1::Request::SetRegion { region } => {
                let region = region
                    .as_ref()
                    .and_then(|region| region.data::<RegionData>())
                    .map(RegionData::snapshot)
                    .unwrap_or_default();
                state.set_pointer_constraint_pending_region(data.constraint_id, region);
            }
            _ => {}
        }
    }
}

impl GlobalDispatch<wp_pointer_warp_v1::WpPointerWarpV1, ()> for CompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<wp_pointer_warp_v1::WpPointerWarpV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<wp_pointer_warp_v1::WpPointerWarpV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &wp_pointer_warp_v1::WpPointerWarpV1,
        request: wp_pointer_warp_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_pointer_warp_v1::Request::Destroy => {}
            wp_pointer_warp_v1::Request::WarpPointer {
                surface,
                pointer,
                x,
                y,
                serial,
            } => {
                state.warp_pointer_protocol_request(surface, pointer, x, y, serial);
            }
            _ => {}
        }
    }
}

fn pointer_constraint_lifetime(
    lifetime: WEnum<zwp_pointer_constraints_v1::Lifetime>,
) -> PointerConstraintLifetime {
    match lifetime {
        WEnum::Value(zwp_pointer_constraints_v1::Lifetime::Oneshot) => {
            PointerConstraintLifetime::Oneshot
        }
        _ => PointerConstraintLifetime::Persistent,
    }
}

impl GlobalDispatch<zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1, ()> for CompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1,
        request: zwp_idle_inhibit_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_idle_inhibit_manager_v1::Request::Destroy => {}
            zwp_idle_inhibit_manager_v1::Request::CreateInhibitor { id, .. } => {
                let inhibitor = data_init.init(id, ());
                state.add_idle_inhibitor(inhibitor);
            }
            _ => {}
        }
    }
}

impl Dispatch<zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1,
        request: zwp_idle_inhibitor_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let zwp_idle_inhibitor_v1::Request::Destroy = request {
            state.remove_idle_inhibitor(resource);
        }
    }
}

impl GlobalDispatch<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1, ()>
    for CompositorState
{
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1, ()>
    for CompositorState
{
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1,
        request: zwp_primary_selection_device_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_primary_selection_device_manager_v1::Request::CreateSource { id } => {
                data_init.init(id, ());
            }
            zwp_primary_selection_device_manager_v1::Request::GetDevice { id, .. } => {
                data_init.init(id, ());
            }
            zwp_primary_selection_device_manager_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, ()>
    for CompositorState
{
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
        _request: zwp_primary_selection_device_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1, ()>
    for CompositorState
{
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
        _request: zwp_primary_selection_source_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, ()> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1,
        _request: zwp_primary_selection_offer_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<ext_data_control_manager_v1::ExtDataControlManagerV1, ()> for CompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ext_data_control_manager_v1::ExtDataControlManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ext_data_control_manager_v1::ExtDataControlManagerV1, ()> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ext_data_control_manager_v1::ExtDataControlManagerV1,
        request: ext_data_control_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_data_control_manager_v1::Request::CreateDataSource { id } => {
                data_init.init(id, ());
            }
            ext_data_control_manager_v1::Request::GetDataDevice { id, .. } => {
                data_init.init(id, ());
            }
            ext_data_control_manager_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<ext_data_control_device_v1::ExtDataControlDeviceV1, ()> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ext_data_control_device_v1::ExtDataControlDeviceV1,
        _request: ext_data_control_device_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<ext_data_control_source_v1::ExtDataControlSourceV1, ()> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ext_data_control_source_v1::ExtDataControlSourceV1,
        _request: ext_data_control_source_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<ext_data_control_offer_v1::ExtDataControlOfferV1, ()> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ext_data_control_offer_v1::ExtDataControlOfferV1,
        _request: ext_data_control_offer_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}
