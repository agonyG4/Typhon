use super::super::*;

impl Dispatch<wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1,
        request: wp_linux_drm_syncobj_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_linux_drm_syncobj_manager_v1::Request::Destroy => {}
            wp_linux_drm_syncobj_manager_v1::Request::ImportTimeline { id, fd } => {
                let Some(device) = state.syncobj_device.as_ref() else {
                    resource.post_error(
                        SYNCOBJ_MANAGER_ERROR_INVALID_TIMELINE,
                        "no DRM device supports timeline syncobj",
                    );
                    return;
                };
                match device.import_timeline_fd(fd) {
                    Ok(timeline) => {
                        data_init.init(id, SyncobjTimelineData { timeline });
                    }
                    Err(_) => {
                        resource.post_error(
                            SYNCOBJ_MANAGER_ERROR_INVALID_TIMELINE,
                            "failed to import DRM syncobj timeline",
                        );
                    }
                }
            }
            wp_linux_drm_syncobj_manager_v1::Request::GetSurface { id, surface } => {
                let Some(surface_data) = surface.data::<SurfaceData>() else {
                    resource.post_error(SYNCOBJ_MANAGER_ERROR_SURFACE_EXISTS, "invalid surface");
                    return;
                };
                let state = Arc::new(SyncobjSurfaceState::new(surface.downgrade()));
                if !surface_data.attach_explicit_sync(state.clone()) {
                    resource.post_error(
                        SYNCOBJ_MANAGER_ERROR_SURFACE_EXISTS,
                        "surface already has an explicit sync object",
                    );
                    return;
                }
                let sync_surface = data_init.init(id, state.clone());
                state.set_resource(sync_surface);
            }
            _ => {}
        }
    }
}

impl Dispatch<wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1, SyncobjTimelineData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1,
        request: wp_linux_drm_syncobj_timeline_v1::Request,
        data: &SyncobjTimelineData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, wp_linux_drm_syncobj_timeline_v1::Request::Destroy) {
            state.cancel_pending_acquire_commits_for_timeline(
                &data.timeline,
                AcquireWatchCancelReason::TimelineDestroyed,
            );
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        _resource: &wp_linux_drm_syncobj_timeline_v1::WpLinuxDrmSyncobjTimelineV1,
        data: &SyncobjTimelineData,
    ) {
        state.cancel_pending_acquire_commits_for_timeline(
            &data.timeline,
            AcquireWatchCancelReason::TimelineDestroyed,
        );
    }
}

impl Dispatch<wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1, Arc<SyncobjSurfaceState>>
    for CompositorState
{
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1,
        request: wp_linux_drm_syncobj_surface_v1::Request,
        data: &Arc<SyncobjSurfaceState>,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_linux_drm_syncobj_surface_v1::Request::Destroy => {
                data.clear_resource();
                if let Some(surface_id) = data.surface_id() {
                    state.cancel_pending_surface_trees_for_surface(
                        surface_id,
                        AcquireWatchCancelReason::SyncSurfaceDestroyed,
                    );
                    state.cancel_pending_acquire_commits_for_surface(
                        surface_id,
                        AcquireWatchCancelReason::SyncSurfaceDestroyed,
                    );
                }
            }
            wp_linux_drm_syncobj_surface_v1::Request::SetAcquirePoint {
                timeline,
                point_hi,
                point_lo,
            } => {
                if !data.surface_is_alive() {
                    data.post_error(
                        SYNCOBJ_SURFACE_ERROR_NO_SURFACE,
                        "associated wl_surface was destroyed",
                    );
                    return;
                }
                if let Some(timeline) = timeline.data::<SyncobjTimelineData>() {
                    data.set_pending_acquire(ExplicitSyncPoint::new(
                        timeline.timeline.clone(),
                        point_hi,
                        point_lo,
                    ));
                }
            }
            wp_linux_drm_syncobj_surface_v1::Request::SetReleasePoint {
                timeline,
                point_hi,
                point_lo,
            } => {
                if !data.surface_is_alive() {
                    data.post_error(
                        SYNCOBJ_SURFACE_ERROR_NO_SURFACE,
                        "associated wl_surface was destroyed",
                    );
                    return;
                }
                if let Some(timeline) = timeline.data::<SyncobjTimelineData>() {
                    data.set_pending_release(ExplicitSyncPoint::new(
                        timeline.timeline.clone(),
                        point_hi,
                        point_lo,
                    ));
                }
            }
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        _resource: &wp_linux_drm_syncobj_surface_v1::WpLinuxDrmSyncobjSurfaceV1,
        data: &Arc<SyncobjSurfaceState>,
    ) {
        data.clear_resource();
        if let Some(surface_id) = data.surface_id() {
            state.cancel_pending_surface_trees_for_surface(
                surface_id,
                AcquireWatchCancelReason::SyncSurfaceDestroyed,
            );
            state.cancel_pending_acquire_commits_for_surface(
                surface_id,
                AcquireWatchCancelReason::SyncSurfaceDestroyed,
            );
        }
    }
}
