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
                let commit_sequence = state.allocate_surface_commit_sequence();
                let window_geometry = state.pending_surface_window_geometries.remove(&surface_id);
                let explicit_sync = data.explicit_sync();
                let offset = data.take_pending_offset();
                let viewport_change = data.take_pending_viewport();
                let buffer_scale_change = data.take_pending_buffer_scale();
                let input_region_change = data.take_pending_input_region();
                let presentation_feedbacks = state.take_surface_presentation_feedbacks(surface_id);
                let mut attachment = data.take_pending();
                if let Some(PendingSurfaceAttachment::Buffer(buffer)) = attachment.as_mut() {
                    buffer.commit_sequence = commit_sequence;
                }
                let has_attachment_change = attachment.is_some();
                state.record_surface_commit_received(
                    surface_id,
                    commit_sequence,
                    has_attachment_change,
                );
                let buffer_size = match attachment.as_ref() {
                    Some(PendingSurfaceAttachment::Buffer(buffer)) => buffer
                        .data
                        .width()
                        .ok()
                        .zip(buffer.data.height().ok())
                        .and_then(|(width, height)| BufferSize::new(width, height)),
                    _ => state
                        .current_surface_buffers
                        .get(&surface_id)
                        .and_then(|buffer| {
                            buffer
                                .data
                                .width()
                                .ok()
                                .zip(buffer.data.height().ok())
                                .and_then(|(width, height)| BufferSize::new(width, height))
                        }),
                };
                let damage = data.take_damage(
                    buffer_size,
                    data.buffer_scale_for_change(buffer_scale_change),
                    data.viewport_for_change(viewport_change),
                );
                let damage = match attachment {
                    Some(PendingSurfaceAttachment::Buffer(_)) if damage.damage.is_empty() => {
                        Some(RenderableSurfaceDamage::Full)
                    }
                    Some(PendingSurfaceAttachment::Buffer(_)) => Some(damage.damage),
                    _ => damage.explicit(),
                };
                let commit = CachedSubsurfaceCommit {
                    commit_sequence,
                    attachment,
                    damage,
                    frame_callbacks: data.take_frame_callbacks(),
                    explicit_sync: explicit_sync.map(CapturedExplicitSyncState::capture),
                    offset,
                    viewport_destination: viewport_change,
                    buffer_scale: buffer_scale_change,
                    input_region: input_region_change,
                    presentation_feedbacks,
                    resize_commit: None,
                    resize_capture_finalized: false,
                    window_geometry,
                    cached_at: Instant::now(),
                };
                state.commit_surface_tree_request(surface_id, commit);
            }
            wl_surface::Request::Destroy => {
                state.unregister_surface_resource(data.surface_id());
            }
            wl_surface::Request::Damage {
                x,
                y,
                width,
                height,
            } => {
                data.push_surface_damage(x, y, width, height);
            }
            wl_surface::Request::DamageBuffer {
                x,
                y,
                width,
                height,
            } => {
                data.push_buffer_damage(x, y, width, height);
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

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        _resource: &wl_surface::WlSurface,
        data: &SurfaceData,
    ) {
        let _ = state.cancel_pending_acquire_commits_for_surface(
            data.surface_id(),
            AcquireWatchCancelReason::ClientDisconnected,
        );
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
                if let Err(error) =
                    state.assign_surface_role(surface_id, SurfaceRole::Subsurface { parent_id })
                {
                    _resource.post_error(wl_subcompositor::Error::BadSurface, error.message());
                    return;
                }
                if !state.register_subsurface_relationship(surface_id, parent_id) {
                    state.clear_surface_role_if(surface_id, SurfaceRole::Subsurface { parent_id });
                    _resource.post_error(
                        wl_subcompositor::Error::BadSurface,
                        "surface has another role or would create a subsurface cycle".to_string(),
                    );
                    return;
                }
                state.set_surface_placement(
                    surface_id,
                    SurfacePlacement::subsurface(parent_id, 0, 0),
                );
                state.adopt_current_surface_content_for_role(surface_id);
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
                state.set_pending_subsurface_position(compositor_surface_id(&data.surface), x, y);
            }
            wl_subsurface::Request::Destroy => {
                let surface_id = compositor_surface_id(&data.surface);
                state.destroy_subsurface_role(surface_id);
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
            wl_subsurface::Request::SetSync => {
                state.set_subsurface_sync_mode(
                    compositor_surface_id(&data.surface),
                    SubsurfaceSyncMode::Synchronized,
                );
            }
            wl_subsurface::Request::SetDesync => {
                state.set_subsurface_sync_mode(
                    compositor_surface_id(&data.surface),
                    SubsurfaceSyncMode::Desynchronized,
                );
            }
            _ => {}
        }
    }
}
