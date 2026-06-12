use super::super::*;

impl Dispatch<wl_shm::WlShm, ()> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wl_shm::WlShm,
        request: wl_shm::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_shm::Request::CreatePool { id, fd, size } = request {
            data_init.init(
                id,
                ShmPoolData {
                    file: Arc::new(File::from(fd)),
                    size,
                },
            );
        }
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ShmPoolData> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wl_shm_pool::WlShmPool,
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
                data_init.init(
                    id,
                    ShmBufferData {
                        pool_size: data.size,
                        file: Arc::clone(&data.file),
                        offset,
                        width,
                        height,
                        stride,
                        format,
                    },
                );
            }
            wl_shm_pool::Request::Destroy | wl_shm_pool::Request::Resize { .. } => {}
            _ => {}
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
}

impl Dispatch<wl_drm::WlDrm, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &wl_drm::WlDrm,
        request: wl_drm::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_drm::Request::Authenticate { .. } => {
                resource.authenticated();
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
                if let Some(data) =
                    wl_drm_prime_buffer_data(resource, request, &state.dmabuf_feedback)
                {
                    data_init.init(id, data);
                }
            }
            wl_drm::Request::CreateBuffer { .. } | wl_drm::Request::CreatePlanarBuffer { .. } => {
                resource.post_error(
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
) -> Option<DmabufBufferData> {
    if request.width <= 0 || request.height <= 0 || request.offset0 < 0 || request.stride0 <= 0 {
        drm.post_error(
            wl_drm::Error::InvalidName,
            "wl_drm prime buffer dimensions are invalid".to_string(),
        );
        return None;
    }

    let drm_format = DrmFormat::from_fourcc(request.format);
    if !matches!(drm_format, DrmFormat::Argb8888 | DrmFormat::Xrgb8888) {
        drm.post_error(
            wl_drm::Error::InvalidFormat,
            "unsupported wl_drm prime buffer format".to_string(),
        );
        return None;
    }
    if !feedback.supports(drm_format, DrmModifier::LINEAR) {
        drm.post_error(
            wl_drm::Error::InvalidFormat,
            "wl_drm prime buffers require a linear advertised format".to_string(),
        );
        return None;
    }

    let minimum_stride = (request.width as u32).saturating_mul(4);
    let stride = request.stride0 as u32;
    if stride < minimum_stride || request.offset0 % 4 != 0 {
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
    Some(DmabufBufferData { handle })
}

impl Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
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
                match DmabufFeedbackData::new(&state.dmabuf_feedback, state.dmabuf_main_device) {
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
            _ => {}
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
                );
            }
            zwp_linux_buffer_params_v1::Request::Create {
                width,
                height,
                format,
                ..
            } => {
                let Some(buffer_data) = data.take_buffer_data_for_create(
                    resource,
                    width,
                    height,
                    format,
                    &state.dmabuf_feedback,
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
                if let Some(buffer_data) = data.take_buffer_data_for_create(
                    resource,
                    width,
                    height,
                    format,
                    &state.dmabuf_feedback,
                ) {
                    _data_init.init(buffer_id, buffer_data);
                }
            }
            zwp_linux_buffer_params_v1::Request::Destroy => {}
            _ => {}
        }
    }
}
