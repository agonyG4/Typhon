use super::super::*;
use crate::astrea_background_effect_coverage::server::{
    astrea_background_effect_coverage_manager_v1, astrea_background_effect_coverage_v1,
};
use crate::effects::{EffectCoverage, EffectCoverageRoundedRect, EffectCoverageTriangle};

const MAX_LOGICAL_COORDINATE: f64 = 32_768.0;

#[derive(Debug, Clone)]
pub(super) struct BackgroundEffectCoverageResourceData {
    surface_id: u32,
    surface: wl_surface::WlSurface,
}

impl
    GlobalDispatch<
        astrea_background_effect_coverage_manager_v1::AstreaBackgroundEffectCoverageManagerV1,
        (),
    > for CompositorState
{
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<
            astrea_background_effect_coverage_manager_v1::AstreaBackgroundEffectCoverageManagerV1,
        >,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl
    Dispatch<
        astrea_background_effect_coverage_manager_v1::AstreaBackgroundEffectCoverageManagerV1,
        (),
    > for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &astrea_background_effect_coverage_manager_v1::AstreaBackgroundEffectCoverageManagerV1,
        request: astrea_background_effect_coverage_manager_v1::Request,
        _data: &(),
        handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            astrea_background_effect_coverage_manager_v1::Request::Destroy => {}
            astrea_background_effect_coverage_manager_v1::Request::GetCoverage { id, surface } => {
                if !state.astrea_shell_client_allowed(client, handle) {
                    state.post_protocol_error_deferred(
                        client,
                        resource,
                        astrea_background_effect_coverage_manager_v1::Error::Unauthorized,
                        "client is not an authorized Astrea shell client",
                    );
                    return;
                }
                if let Err(error) = validate_coverage_surface(resource, &surface) {
                    state.post_protocol_error_deferred(
                        client,
                        resource,
                        error,
                        "wl_surface is not owned by this client or has no compositor surface data",
                    );
                    return;
                }
                let surface_id = compositor_surface_id(&surface);
                if state
                    .background_effect_coverage_resources
                    .contains_key(&surface_id)
                {
                    state.post_protocol_error_deferred(
                        client,
                        resource,
                        astrea_background_effect_coverage_manager_v1::Error::CoverageExists,
                        "an analytic coverage object already exists for this surface",
                    );
                    return;
                }
                let coverage = data_init.init(
                    id,
                    BackgroundEffectCoverageResourceData {
                        surface_id,
                        surface,
                    },
                );
                state
                    .background_effect_coverage_resources
                    .insert(surface_id, coverage.id());
            }
        }
    }
}

fn validate_coverage_surface(
    manager: &astrea_background_effect_coverage_manager_v1::AstreaBackgroundEffectCoverageManagerV1,
    surface: &wl_surface::WlSurface,
) -> Result<(), astrea_background_effect_coverage_manager_v1::Error> {
    if !surface.id().same_client_as(&manager.id()) || surface.data::<SurfaceData>().is_none() {
        return Err(astrea_background_effect_coverage_manager_v1::Error::InvalidSurface);
    }
    Ok(())
}

impl
    Dispatch<
        astrea_background_effect_coverage_v1::AstreaBackgroundEffectCoverageV1,
        BackgroundEffectCoverageResourceData,
    > for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &astrea_background_effect_coverage_v1::AstreaBackgroundEffectCoverageV1,
        request: astrea_background_effect_coverage_v1::Request,
        data: &BackgroundEffectCoverageResourceData,
        handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if !matches!(
            &request,
            astrea_background_effect_coverage_v1::Request::Destroy
        ) && !state.astrea_shell_client_allowed(client, handle)
        {
            state.post_protocol_error_deferred(
                client,
                resource,
                astrea_background_effect_coverage_v1::Error::Unauthorized,
                "client is not an authorized Astrea shell client",
            );
            return;
        }
        if matches!(
            &request,
            astrea_background_effect_coverage_v1::Request::Destroy
        ) {
            state.remove_background_effect_coverage_resource(resource, data.surface_id);
            return;
        }
        if !data.surface.is_alive() {
            state.post_protocol_error_deferred(
                client,
                resource,
                astrea_background_effect_coverage_v1::Error::SurfaceDestroyed,
                "associated wl_surface is destroyed",
            );
            return;
        }
        let Some(surface) = data.surface.data::<SurfaceData>() else {
            return;
        };
        match request {
            astrea_background_effect_coverage_v1::Request::SetRoundedRect {
                x,
                y,
                width,
                height,
                radius,
            } => {
                let Some(rect) = validated_rounded_rect(x, y, width, height, radius) else {
                    state.post_protocol_error_deferred(
                        client,
                        resource,
                        astrea_background_effect_coverage_v1::Error::InvalidGeometry,
                        "rounded rectangle geometry is invalid or outside the bounded coordinate policy",
                    );
                    return;
                };
                surface.update_pending_background_effect_coverage(|coverage| {
                    coverage
                        .get_or_insert_with(EffectCoverage::default)
                        .rounded_rect = Some(rect);
                });
            }
            astrea_background_effect_coverage_v1::Request::UnsetRoundedRect => {
                surface.update_pending_background_effect_coverage(|coverage| {
                    if let Some(coverage) = coverage {
                        coverage.rounded_rect = None;
                    }
                });
            }
            astrea_background_effect_coverage_v1::Request::SetTriangle {
                ax,
                ay,
                bx,
                by,
                cx,
                cy,
            } => {
                let Some(triangle) = validated_triangle(ax, ay, bx, by, cx, cy) else {
                    state.post_protocol_error_deferred(
                        client,
                        resource,
                        astrea_background_effect_coverage_v1::Error::InvalidGeometry,
                        "triangle geometry is degenerate or outside the bounded coordinate policy",
                    );
                    return;
                };
                surface.update_pending_background_effect_coverage(|coverage| {
                    coverage
                        .get_or_insert_with(EffectCoverage::default)
                        .triangle = Some(triangle);
                });
            }
            astrea_background_effect_coverage_v1::Request::UnsetTriangle => {
                surface.update_pending_background_effect_coverage(|coverage| {
                    if let Some(coverage) = coverage {
                        coverage.triangle = None;
                    }
                });
            }
            astrea_background_effect_coverage_v1::Request::Clear => {
                surface.set_pending_background_effect_coverage(None);
            }
            astrea_background_effect_coverage_v1::Request::Destroy => unreachable!(),
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &astrea_background_effect_coverage_v1::AstreaBackgroundEffectCoverageV1,
        data: &BackgroundEffectCoverageResourceData,
    ) {
        state.remove_background_effect_coverage_resource(resource, data.surface_id);
    }
}

impl CompositorState {
    fn remove_background_effect_coverage_resource(
        &mut self,
        resource: &astrea_background_effect_coverage_v1::AstreaBackgroundEffectCoverageV1,
        surface_id: u32,
    ) {
        if self
            .background_effect_coverage_resources
            .get(&surface_id)
            .is_some_and(|existing| *existing == resource.id())
        {
            self.background_effect_coverage_resources
                .remove(&surface_id);
            if let Some(surface) = self.surface_resource_by_id(surface_id)
                && surface.is_alive()
                && let Some(data) = surface.data::<SurfaceData>()
            {
                data.set_pending_background_effect_coverage(None);
            }
        }
    }
}

fn valid_coordinate(value: f64) -> bool {
    value.is_finite() && value.abs() <= MAX_LOGICAL_COORDINATE
}

fn validated_rounded_rect(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    radius: f64,
) -> Option<EffectCoverageRoundedRect> {
    if ![x, y, width, height, radius]
        .into_iter()
        .all(valid_coordinate)
        || width <= 0.0
        || height <= 0.0
        || radius < 0.0
        || radius > width.min(height) * 0.5
        || !valid_coordinate(x + width)
        || !valid_coordinate(y + height)
    {
        return None;
    }
    Some(EffectCoverageRoundedRect {
        x,
        y,
        width,
        height,
        radius,
    })
}

fn validated_triangle(
    ax: f64,
    ay: f64,
    bx: f64,
    by: f64,
    cx: f64,
    cy: f64,
) -> Option<EffectCoverageTriangle> {
    if ![ax, ay, bx, by, cx, cy].into_iter().all(valid_coordinate) {
        return None;
    }
    let area = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
    if !area.is_finite() || area == 0.0 {
        return None;
    }
    Some(EffectCoverageTriangle {
        a: [ax, ay],
        b: [bx, by],
        c: [cx, cy],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::net::UnixStream, sync::Arc};

    #[test]
    fn geometry_validation_is_bounded_and_rejects_degenerate_values() {
        assert!(validated_rounded_rect(0.25, 0.5, 72.0, 34.0, 17.0).is_some());
        assert!(validated_rounded_rect(0.0, 0.0, 72.0, 34.0, 17.01).is_none());
        assert!(validated_rounded_rect(32_767.0, 0.0, 2.0, 8.0, 0.0).is_none());
        assert!(validated_rounded_rect(f64::NAN, 0.0, 1.0, 1.0, 0.0).is_none());
        assert!(validated_triangle(0.0, 0.0, 10.0, 0.0, 2.0, 4.0).is_some());
        assert!(validated_triangle(0.0, 0.0, 10.0, 0.0, 20.0, 0.0).is_none());
    }

    #[test]
    fn foreign_client_surface_maps_to_typed_invalid_surface_error() {
        let display = wayland_server::Display::<CompositorState>::new().expect("test display");
        let mut handle = display.handle();
        let (manager_stream, _manager_peer) = UnixStream::pair().expect("manager client socket");
        let manager_client = handle
            .insert_client(manager_stream, Arc::new(()))
            .expect("manager client");
        let (surface_stream, _surface_peer) = UnixStream::pair().expect("surface client socket");
        let surface_client = handle
            .insert_client(surface_stream, Arc::new(()))
            .expect("surface client");
        let manager = manager_client
            .create_resource::<
                astrea_background_effect_coverage_manager_v1::AstreaBackgroundEffectCoverageManagerV1,
                (),
                CompositorState,
            >(&handle, 1, ())
            .expect("manager resource");
        let same_client_surface = manager_client
            .create_resource::<wl_surface::WlSurface, SurfaceData, CompositorState>(
                &handle,
                6,
                SurfaceData::new(1),
            )
            .expect("same-client surface resource");
        let foreign_surface = surface_client
            .create_resource::<wl_surface::WlSurface, SurfaceData, CompositorState>(
                &handle,
                6,
                SurfaceData::new(2),
            )
            .expect("foreign surface resource");

        assert_eq!(
            validate_coverage_surface(&manager, &same_client_surface),
            Ok(())
        );
        assert_eq!(
            validate_coverage_surface(&manager, &foreign_surface),
            Err(astrea_background_effect_coverage_manager_v1::Error::InvalidSurface)
        );
    }
}
