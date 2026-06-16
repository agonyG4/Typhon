use super::super::*;

impl Dispatch<wl_compositor::WlCompositor, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &wl_compositor::WlCompositor,
        request: wl_compositor::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_compositor::Request::CreateSurface { id } => {
                let surface_id = state.allocate_surface_id();
                let surface = data_init.init(id, SurfaceData::new(surface_id));
                state.register_surface_resource(surface_id, surface.clone());
                state.focused_surface.get_or_insert_with(|| surface.clone());
            }
            wl_compositor::Request::CreateRegion { id } => {
                data_init.init(id, RegionData::default());
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_surface::WlSurface, SurfaceData> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &wl_surface::WlSurface,
        request: wl_surface::Request,
        data: &SurfaceData,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_surface::Request::Frame { callback } => {
                let callback = data_init.init(callback, ());
                data.push_frame_callback(callback);
            }
            wl_surface::Request::Attach { buffer, x, y } => {
                data.set_pending(buffer, x, y);
            }
            wl_surface::Request::Commit => {
                let surface_id = data.surface_id();
                let damage = data.take_damage();
                let explicit_sync = data.explicit_sync();
                let input_region_changed = data.commit_pending_input_region();
                match data.take_pending() {
                    Some(PendingSurfaceAttachment::Buffer(mut pending)) => {
                        if let Some((x, y)) = data.take_pending_offset() {
                            pending.x = x;
                            pending.y = y;
                        }
                        let viewport_destination = data.commit_pending_viewport();
                        let buffer_scale = data.commit_pending_buffer_scale();
                        if pending
                            .apply_committed_surface_state(viewport_destination, buffer_scale)
                            .is_err()
                        {
                            return;
                        }
                        let frame_callbacks = data.take_frame_callbacks();
                        state.commit_surface_request(
                            surface_id,
                            pending,
                            damage.damage,
                            frame_callbacks,
                            explicit_sync,
                        );
                    }
                    Some(PendingSurfaceAttachment::RemoveContent) => {
                        let _ = data.take_pending_offset();
                        data.commit_pending_viewport();
                        data.commit_pending_buffer_scale();
                        if explicit_sync.is_some() {
                            state.commit_surface_without_buffer(
                                surface_id,
                                data,
                                None,
                                explicit_sync,
                            );
                        } else {
                            state.unmap_surface_content(surface_id);
                            state.complete_frame_callbacks_now(data);
                        }
                    }
                    None => {
                        state.commit_surface_without_buffer(
                            surface_id,
                            data,
                            damage.explicit(),
                            explicit_sync,
                        );
                    }
                }
                state.apply_pending_pointer_constraint_state_for_surface(surface_id);
                state.apply_pending_subsurface_stack_for_parent(surface_id);
                if input_region_changed {
                    state.refresh_pointer_focus_at_last_position();
                }
            }
            wl_surface::Request::Destroy => {
                state.unregister_surface_resource(data.surface_id());
            }
            wl_surface::Request::Damage {
                x,
                y,
                width,
                height,
            }
            | wl_surface::Request::DamageBuffer {
                x,
                y,
                width,
                height,
            } => {
                data.push_damage(x, y, width, height);
            }
            wl_surface::Request::SetInputRegion { region } => {
                let input_region = region
                    .as_ref()
                    .and_then(|region| region.data::<RegionData>())
                    .map(RegionData::snapshot)
                    .unwrap_or_default();
                data.set_pending_input_region(input_region);
            }
            wl_surface::Request::SetBufferScale { scale } => {
                if scale <= 0 {
                    resource.post_error(
                        wl_surface::Error::InvalidScale,
                        "buffer scale must be greater than zero".to_string(),
                    );
                    return;
                }
                data.set_pending_buffer_scale(scale as u32);
            }
            wl_surface::Request::SetOpaqueRegion { .. }
            | wl_surface::Request::SetBufferTransform { .. } => {}
            wl_surface::Request::Offset { x, y } => {
                data.set_pending_offset(x, y);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wl_callback::WlCallback,
        _request: wl_callback::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<wl_region::WlRegion, RegionData> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wl_region::WlRegion,
        request: wl_region::Request,
        data: &RegionData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_region::Request::Add {
                x,
                y,
                width,
                height,
            } => {
                if let Some(rect) = InputRegionRect::new(x, y, width, height) {
                    data.push(InputRegionOp::Add(rect));
                }
            }
            wl_region::Request::Subtract {
                x,
                y,
                width,
                height,
            } => {
                if let Some(rect) = InputRegionRect::new(x, y, width, height) {
                    data.push(InputRegionOp::Subtract(rect));
                }
            }
            wl_region::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<wl_subcompositor::WlSubcompositor, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &wl_subcompositor::WlSubcompositor,
        request: wl_subcompositor::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_subcompositor::Request::GetSubsurface {
                id,
                surface,
                parent,
            } => {
                let surface_id = compositor_surface_id(&surface);
                let parent_id = compositor_surface_id(&parent);
                state.set_surface_placement(
                    surface_id,
                    SurfacePlacement::subsurface(parent_id, 0, 0),
                );
                state.register_subsurface_relationship(surface_id, parent_id);
                data_init.init(id, SubsurfaceData { surface, parent });
            }
            wl_subcompositor::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<wl_subsurface::WlSubsurface, SubsurfaceData> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &wl_subsurface::WlSubsurface,
        request: wl_subsurface::Request,
        data: &SubsurfaceData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_subsurface::Request::SetPosition { x, y } => {
                state.set_surface_placement(
                    compositor_surface_id(&data.surface),
                    SurfacePlacement::subsurface(compositor_surface_id(&data.parent), x, y),
                );
            }
            wl_subsurface::Request::Destroy => {
                let surface_id = compositor_surface_id(&data.surface);
                state.unmap_surface_content(surface_id);
                state.set_surface_placement(surface_id, SurfacePlacement::root());
                state.cleanup_subsurface_stack_state_for_surface(surface_id);
            }
            wl_subsurface::Request::PlaceAbove { sibling } => {
                let surface_id = compositor_surface_id(&data.surface);
                let parent_id = compositor_surface_id(&data.parent);
                let sibling_id = compositor_surface_id(&sibling);
                if !state.restack_subsurface(surface_id, parent_id, sibling_id, true) {
                    _resource.post_error(
                        wl_subsurface::Error::BadSurface,
                        "place_above reference must be the parent or a sibling".to_string(),
                    );
                }
            }
            wl_subsurface::Request::PlaceBelow { sibling } => {
                let surface_id = compositor_surface_id(&data.surface);
                let parent_id = compositor_surface_id(&data.parent);
                let sibling_id = compositor_surface_id(&sibling);
                if !state.restack_subsurface(surface_id, parent_id, sibling_id, false) {
                    _resource.post_error(
                        wl_subsurface::Error::BadSurface,
                        "place_below reference must be the parent or a sibling".to_string(),
                    );
                }
            }
            wl_subsurface::Request::SetSync | wl_subsurface::Request::SetDesync => {}
            _ => {}
        }
    }
}
