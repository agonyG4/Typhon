use super::super::*;
use wayland_protocols::ext::background_effect::v1::server::{
    ext_background_effect_manager_v1, ext_background_effect_surface_v1,
};

#[derive(Debug, Clone)]
pub(super) struct BackgroundEffectResourceData {
    pub(super) surface_id: u32,
    pub(super) surface: wl_surface::WlSurface,
}

impl GlobalDispatch<ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1, ()>
    for CompositorState
{
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        let _ = manager.send_event(ext_background_effect_manager_v1::Event::Capabilities {
            flags: WEnum::Value(ext_background_effect_manager_v1::Capability::Blur),
        });
    }
}

impl Dispatch<ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1, ()>
    for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1,
        request: ext_background_effect_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_background_effect_manager_v1::Request::Destroy => {}
            ext_background_effect_manager_v1::Request::GetBackgroundEffect { id, surface } => {
                if !surface.id().same_client_as(&resource.id()) {
                    return;
                }
                let surface_id = compositor_surface_id(&surface);
                if state.background_effect_resources.contains_key(&surface_id) {
                    state.post_protocol_error_deferred(
                        client,
                        resource,
                        ext_background_effect_manager_v1::Error::BackgroundEffectExists,
                        "a background effect object already exists for this surface",
                    );
                    return;
                }
                if surface.data::<SurfaceData>().is_none() {
                    return;
                }
                let effect = data_init.init(
                    id,
                    BackgroundEffectResourceData {
                        surface_id,
                        surface: surface.clone(),
                    },
                );
                state
                    .background_effect_resources
                    .insert(surface_id, effect.id());
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "ext_background_effect_manager_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl
    Dispatch<
        ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
        BackgroundEffectResourceData,
    > for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
        request: ext_background_effect_surface_v1::Request,
        data: &BackgroundEffectResourceData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_background_effect_surface_v1::Request::Destroy => {
                state.remove_background_effect_resource(resource, data.surface_id);
            }
            ext_background_effect_surface_v1::Request::SetBlurRegion { region } => {
                if !data.surface.is_alive() {
                    state.post_protocol_error_deferred_with_details(
                        client,
                        resource,
                        ext_background_effect_surface_v1::Error::SurfaceDestroyed,
                        "associated wl_surface was destroyed",
                        Some(data.surface_id),
                        ProtocolErrorCategory::SurfaceDestroyed,
                    );
                    return;
                }
                let region = region
                    .as_ref()
                    .and_then(|region| region.data::<RegionData>())
                    .map(RegionData::snapshot)
                    .map(BackgroundEffectRegion::from_surface_input_region)
                    .unwrap_or_default();
                if let Some(surface) = state.surface_resource_by_id(data.surface_id)
                    && let Some(surface_data) = surface.data::<SurfaceData>()
                {
                    surface_data.set_pending_background_effect(region);
                }
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "ext_background_effect_surface_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
        data: &BackgroundEffectResourceData,
    ) {
        state.remove_background_effect_resource(resource, data.surface_id);
    }
}

impl CompositorState {
    fn remove_background_effect_resource(
        &mut self,
        resource: &ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
        surface_id: u32,
    ) {
        if self
            .background_effect_resources
            .get(&surface_id)
            .is_some_and(|existing| *existing == resource.id())
        {
            self.background_effect_resources.remove(&surface_id);
            if let Some(surface) = self.surface_resource_by_id(surface_id)
                && let Some(data) = surface.data::<SurfaceData>()
            {
                data.clear_pending_background_effect();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_background_effect_is_applied_only_at_commit() {
        let surface = SurfaceData::new(1);
        let region =
            BackgroundEffectRegion::from_surface_input_region(SurfaceInputRegion::Custom(vec![
                InputRegionOp::Add(InputRegionRect::new(2, 3, 10, 11).expect("valid blur region")),
            ]));
        surface.set_pending_background_effect(region.clone());
        assert_eq!(
            surface.committed_background_effect(),
            BackgroundEffectRegion::default()
        );

        let pending = surface
            .take_pending_background_effect()
            .expect("pending blur region");
        assert!(surface.apply_background_effect_change(Some(pending)));
        assert_eq!(surface.committed_background_effect(), region);
    }

    #[test]
    fn destroying_the_effect_object_queues_empty_state() {
        let surface = SurfaceData::new(1);
        let region =
            BackgroundEffectRegion::from_surface_input_region(SurfaceInputRegion::Custom(vec![
                InputRegionOp::Add(InputRegionRect::new(0, 0, 4, 4).expect("valid blur region")),
            ]));
        surface.set_pending_background_effect(region);
        assert!(surface.apply_background_effect_change(surface.take_pending_background_effect()));
        surface.clear_pending_background_effect();
        assert!(surface.apply_background_effect_change(surface.take_pending_background_effect()));
        assert_eq!(
            surface.committed_background_effect(),
            BackgroundEffectRegion::default()
        );
    }
}
