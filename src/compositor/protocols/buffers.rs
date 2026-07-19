use super::super::*;

impl Dispatch<wl_shm::WlShm, ()> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &wl_shm::WlShm,
        request: wl_shm::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_shm::Request::CreatePool { id, fd, size } = request {
            if size <= 0 {
                state.post_protocol_error(
                    client,
                    resource,
                    wl_shm::Error::InvalidStride,
                    "wl_shm_pool size must be positive".to_string(),
                );
                return;
            }

            let file = Arc::new(File::from(fd));
            if !file
                .metadata()
                .is_ok_and(|metadata| metadata.len() >= u64::try_from(size).unwrap_or(u64::MAX))
            {
                state.post_protocol_error(
                    client,
                    resource,
                    wl_shm::Error::InvalidFd,
                    "wl_shm_pool backing file is not usable for its initial size".to_string(),
                );
                return;
            }

            data_init.init(id, ShmPoolData::new(file, size));
        }
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ShmPoolData> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &wl_shm_pool::WlShmPool,
        request: wl_shm_pool::Request,
        data: &ShmPoolData,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_shm_pool::Request::CreateBuffer {
                id,
                offset,
                width,
                height,
                stride,
                format,
            } => {
                let Some(format_descriptor) = shm_format_descriptor(format) else {
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_shm::Error::InvalidFormat,
                        "wl_shm format was not advertised by Typhon".to_string(),
                    );
                    return;
                };
                let Some(row_bytes) = width.checked_mul(format_descriptor.bytes_per_pixel) else {
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_shm::Error::InvalidStride,
                        "wl_shm row-byte calculation overflowed".to_string(),
                    );
                    return;
                };
                let valid_dimensions = offset >= 0 && width > 0 && height > 0;
                let valid_stride = stride >= row_bytes;
                let end = height
                    .checked_sub(1)
                    .and_then(|rows| rows.checked_mul(stride))
                    .and_then(|last_row| offset.checked_add(last_row))
                    .and_then(|last_row_offset| last_row_offset.checked_add(row_bytes));
                let valid_pool_range = end.is_some_and(|end| end <= data.size());
                let valid_backing_range = end.is_some_and(|end| {
                    data.has_backing_range(u64::try_from(end).unwrap_or(u64::MAX))
                });
                if !valid_dimensions || !valid_stride || !valid_pool_range || !valid_backing_range {
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_shm::Error::InvalidStride,
                        "wl_shm buffer metadata is outside the advertised pool".to_string(),
                    );
                    return;
                }
                let Some(identity) = state.allocate_buffer_identity() else {
                    return;
                };
                data_init.init(
                    id,
                    ShmBufferData {
                        identity,
                        pool: Arc::new(data.clone()),
                        offset,
                        width,
                        height,
                        stride,
                        format,
                    },
                );
            }
            wl_shm_pool::Request::Resize { size } => {
                if size <= 0 {
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_shm::Error::InvalidStride,
                        "wl_shm_pool size must be positive".to_string(),
                    );
                } else if data.grow_to(size).is_err() {
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_shm::Error::InvalidStride,
                        "shrinking wl_shm_pool is invalid".to_string(),
                    );
                }
            }
            wl_shm_pool::Request::Destroy => {}
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "wl_shm_pool",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, ShmBufferData> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wl_buffer::WlBuffer,
        _request: wl_buffer::Request,
        data: &ShmBufferData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let _ = data.fits_in_pool();
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &wl_buffer::WlBuffer,
        _data: &ShmBufferData,
    ) {
        state.cancel_pending_acquire_commits_for_buffer(
            resource,
            AcquireWatchCancelReason::BufferDestroyed,
        );
    }
}

impl Dispatch<wl_buffer::WlBuffer, DmabufBufferData> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wl_buffer::WlBuffer,
        _request: wl_buffer::Request,
        _data: &DmabufBufferData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &wl_buffer::WlBuffer,
        _data: &DmabufBufferData,
    ) {
        state.cancel_pending_acquire_commits_for_buffer(
            resource,
            AcquireWatchCancelReason::BufferDestroyed,
        );
    }
}

impl Dispatch<wl_drm::WlDrm, ()> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &wl_drm::WlDrm,
        request: wl_drm::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_drm::Request::Authenticate { .. } => {
                if state.gpu_protocol_capabilities.wl_drm_authentication() {
                    resource.authenticated();
                } else {
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_drm::Error::AuthenticateFail,
                        "wl_drm authentication is not supported for the selected render-node contract"
                            .to_string(),
                    );
                }
            }
            wl_drm::Request::CreatePrimeBuffer {
                id,
                name,
                width,
                height,
                format,
                offset0,
                stride0,
                ..
            } => {
                let request = WlDrmPrimeBufferRequest {
                    fd: name,
                    width,
                    height,
                    format,
                    offset0,
                    stride0,
                };
                if let Some(data) = state.allocate_buffer_identity().and_then(|identity| {
                    wl_drm_prime_buffer_data(
                        resource,
                        request,
                        &state.dmabuf_feedback,
                        state.gpu_protocol_capabilities.wl_drm_formats(),
                        &mut state.compliance_metrics,
                        identity,
                    )
                }) {
                    data_init.init(id, data);
                }
            }
            wl_drm::Request::CreateBuffer { .. } | wl_drm::Request::CreatePlanarBuffer { .. } => {
                state.post_protocol_error(
                    client,
                    resource,
                    wl_drm::Error::InvalidName,
                    "wl_drm flink buffers are not supported; use linux-dmabuf".to_string(),
                );
            }
        }
    }
}

struct WlDrmPrimeBufferRequest {
    fd: OwnedFd,
    width: i32,
    height: i32,
    format: u32,
    offset0: i32,
    stride0: i32,
}

fn wl_drm_prime_buffer_data(
    drm: &wl_drm::WlDrm,
    request: WlDrmPrimeBufferRequest,
    feedback: &EglGlesDmabufFeedback,
    allowed_formats: &[u32],
    metrics: &mut CoreComplianceMetrics,
    identity: BufferIdentity,
) -> Option<DmabufBufferData> {
    if request.width <= 0 || request.height <= 0 || request.offset0 < 0 || request.stride0 <= 0 {
        metrics.note_protocol_error();
        drm.post_error(
            wl_drm::Error::InvalidName,
            "wl_drm prime buffer dimensions are invalid".to_string(),
        );
        return None;
    }

    let drm_format = DrmFormat::from_fourcc(request.format);
    if !matches!(drm_format, DrmFormat::Argb8888 | DrmFormat::Xrgb8888) {
        metrics.note_protocol_error();
        drm.post_error(
            wl_drm::Error::InvalidFormat,
            "unsupported wl_drm prime buffer format".to_string(),
        );
        return None;
    }
    if !feedback.supports(drm_format, DrmModifier::LINEAR)
        || !allowed_formats.contains(&request.format)
    {
        metrics.note_protocol_error();
        drm.post_error(
            wl_drm::Error::InvalidFormat,
            "wl_drm prime buffers require a linear advertised format".to_string(),
        );
        return None;
    }

    let minimum_stride = (request.width as u32).saturating_mul(4);
    let stride = request.stride0 as u32;
    if stride < minimum_stride || request.offset0 % 4 != 0 {
        metrics.note_protocol_error();
        drm.post_error(
            wl_drm::Error::InvalidName,
            "wl_drm prime buffer plane metadata is out of bounds".to_string(),
        );
        return None;
    }

    let size = BufferSize::new(request.width as u32, request.height as u32)?;
    let handle = DmabufBufferHandle::new(
        size,
        drm_format,
        vec![RenderDmabufPlane::new(
            request.fd,
            DmabufPlaneDescriptor {
                plane_index: 0,
                offset: request.offset0 as u32,
                stride,
                modifier: DrmModifier::LINEAR,
            },
        )],
    )
    .ok()?;
    Some(DmabufBufferData { identity, handle })
}

impl Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        request: zwp_linux_dmabuf_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_linux_dmabuf_v1::Request::CreateParams { params_id } => {
                data_init.init(params_id, DmabufParamsData::default());
            }
            zwp_linux_dmabuf_v1::Request::Destroy => {}
            zwp_linux_dmabuf_v1::Request::GetDefaultFeedback { id }
            | zwp_linux_dmabuf_v1::Request::GetSurfaceFeedback { id, .. } => {
                match DmabufFeedbackData::new(
                    &state.dmabuf_feedback,
                    state.dmabuf_main_device,
                    state.gpu_protocol_capabilities.dmabuf_formats(),
                ) {
                    Ok(data) => {
                        let feedback = data_init.init(id, data);
                        send_dmabuf_feedback(&feedback);
                    }
                    Err(error) => {
                        eprintln!(
                            "oblivion-one compositor: failed to build dmabuf feedback: {error}"
                        );
                    }
                }
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "zwp_linux_dmabuf_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, DmabufFeedbackData>
    for CompositorState
{
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        _request: zwp_linux_dmabuf_feedback_v1::Request,
        _data: &DmabufFeedbackData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, DmabufParamsData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        request: zwp_linux_buffer_params_v1::Request,
        data: &DmabufParamsData,
        dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_linux_buffer_params_v1::Request::Add {
                fd,
                plane_idx,
                offset,
                stride,
                modifier_hi,
                modifier_lo,
            } => {
                data.add_plane(
                    resource,
                    PendingDmabufPlane {
                        fd,
                        plane_idx,
                        offset,
                        stride,
                        modifier: ((modifier_hi as u64) << 32) | u64::from(modifier_lo),
                    },
                    &mut state.compliance_metrics,
                );
            }
            zwp_linux_buffer_params_v1::Request::Create {
                width,
                height,
                format,
                ..
            } => {
                let Some(identity) = state.allocate_buffer_identity() else {
                    resource.failed();
                    return;
                };
                let Some(buffer_data) = data.take_buffer_data_for_create(
                    resource,
                    width,
                    height,
                    format,
                    &state.dmabuf_feedback,
                    state.gpu_protocol_capabilities.dmabuf_formats(),
                    &mut state.compliance_metrics,
                    identity,
                ) else {
                    resource.failed();
                    return;
                };
                match client.create_resource::<wl_buffer::WlBuffer, DmabufBufferData, Self>(
                    dhandle,
                    1,
                    buffer_data,
                ) {
                    Ok(buffer) => resource.created(&buffer),
                    Err(_) => resource.failed(),
                }
            }
            zwp_linux_buffer_params_v1::Request::CreateImmed {
                buffer_id,
                width,
                height,
                format,
                ..
            } => {
                let Some(identity) = state.allocate_buffer_identity() else {
                    return;
                };
                if let Some(buffer_data) = data.take_buffer_data_for_create(
                    resource,
                    width,
                    height,
                    format,
                    &state.dmabuf_feedback,
                    state.gpu_protocol_capabilities.dmabuf_formats(),
                    &mut state.compliance_metrics,
                    identity,
                ) {
                    _data_init.init(buffer_id, buffer_data);
                }
            }
            zwp_linux_buffer_params_v1::Request::Destroy => {}
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "zwp_linux_buffer_params_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}
