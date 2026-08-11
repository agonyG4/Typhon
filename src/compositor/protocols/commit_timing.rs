use super::super::*;

#[derive(Debug, Clone)]
pub(super) struct CommitTimerResourceData {
    pub(super) surface_id: u32,
    pub(super) surface: wl_surface::WlSurface,
}

impl GlobalDispatch<wp_commit_timing_manager_v1::WpCommitTimingManagerV1, ()> for CompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<wp_commit_timing_manager_v1::WpCommitTimingManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<wp_commit_timing_manager_v1::WpCommitTimingManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &wp_commit_timing_manager_v1::WpCommitTimingManagerV1,
        request: wp_commit_timing_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_commit_timing_manager_v1::Request::Destroy => {}
            wp_commit_timing_manager_v1::Request::GetTimer { id, surface } => {
                let surface_id = compositor_surface_id(&surface);
                if state.commit_timer_resources.contains_key(&surface_id) {
                    resource.post_error(
                        wp_commit_timing_manager_v1::Error::CommitTimerExists,
                        "a commit timer already exists for this surface",
                    );
                    return;
                }
                let timer = data_init.init(
                    id,
                    CommitTimerResourceData {
                        surface_id,
                        surface: surface.clone(),
                    },
                );
                state.commit_timer_resources.insert(surface_id, timer);
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "wp_commit_timing_manager_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<wp_commit_timer_v1::WpCommitTimerV1, CommitTimerResourceData> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &wp_commit_timer_v1::WpCommitTimerV1,
        request: wp_commit_timer_v1::Request,
        data: &CommitTimerResourceData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_commit_timer_v1::Request::Destroy => {
                state.remove_commit_timer_resource(resource, data.surface_id)
            }
            wp_commit_timer_v1::Request::SetTimestamp {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            } => {
                if !data.surface.is_alive() {
                    state.note_protocol_error_metric();
                    resource.post_error(
                        wp_commit_timer_v1::Error::SurfaceDestroyed,
                        "associated wl_surface was destroyed",
                    );
                    return;
                }
                let seconds = (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo);
                let Some(timestamp) = CommitTimingConstraint::from_protocol(seconds, tv_nsec)
                else {
                    state.surface_pacing_metrics.timing_protocol_errors = state
                        .surface_pacing_metrics
                        .timing_protocol_errors
                        .saturating_add(1);
                    resource.post_error(
                        wp_commit_timer_v1::Error::InvalidTimestamp,
                        "tv_nsec must be less than one billion",
                    );
                    return;
                };
                if !state.set_pending_commit_timing(data.surface_id, timestamp) {
                    state.surface_pacing_metrics.timing_protocol_errors = state
                        .surface_pacing_metrics
                        .timing_protocol_errors
                        .saturating_add(1);
                    resource.post_error(
                        wp_commit_timer_v1::Error::TimestampExists,
                        "a commit timestamp is already pending",
                    );
                }
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "wp_commit_timer_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &wp_commit_timer_v1::WpCommitTimerV1,
        data: &CommitTimerResourceData,
    ) {
        state.remove_commit_timer_resource(resource, data.surface_id);
    }
}
