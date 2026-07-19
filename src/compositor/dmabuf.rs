use std::{
    fs::{self, File, OpenOptions},
    io::{self, Seek, Write},
    os::fd::{AsFd, OwnedFd},
    sync::Mutex,
};

use wayland_protocols::wp::linux_dmabuf::zv1::server::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_feedback_v1, zwp_linux_dmabuf_v1,
};
use wayland_server::Resource;

use crate::render_backend::buffer::{
    BufferIdentity, BufferSize, DmabufBufferHandle, DmabufPlane as RenderDmabufPlane,
    DmabufPlaneDescriptor, DrmFormat, DrmModifier,
};
use crate::render_backend::egl_gles::{EglGlesDmabufFeedback, EglGlesDmabufFormat};
use crate::wayland_drm::server::wl_drm;

use super::{
    CompositorState, CoreComplianceMetrics, gpu_protocol_capabilities::GpuFormat,
    unique_runtime_file_path,
};

const WL_DRM_CAPABILITIES_SINCE: u32 = 2;

pub(super) fn send_wl_drm_capabilities(drm: &wl_drm::WlDrm, state: &CompositorState) {
    if let Some(path) = state.gpu_protocol_capabilities.wl_drm_device() {
        drm.device(path.to_string());
    }
    if drm.version() >= WL_DRM_CAPABILITIES_SINCE && state.gpu_protocol_capabilities.wl_drm_prime()
    {
        drm.capabilities(1);
    }
    for fourcc in state.gpu_protocol_capabilities.wl_drm_formats() {
        drm.format(*fourcc);
    }
}

pub(super) fn send_dmabuf_format_modifiers(
    dmabuf: &zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
    formats: &[GpuFormat],
) {
    let mut announced_formats = Vec::new();
    for format in formats {
        let fourcc = format.fourcc;
        if !announced_formats.contains(&fourcc) {
            dmabuf.format(fourcc);
            announced_formats.push(fourcc);
        }
    }

    if dmabuf.version() >= zwp_linux_dmabuf_v1::EVT_MODIFIER_SINCE {
        for format in formats {
            let modifier = format.modifier;
            dmabuf.modifier(format.fourcc, (modifier >> 32) as u32, modifier as u32);
        }
    }
}

pub(super) struct DmabufFeedbackData {
    format_table: File,
    format_table_size: u32,
    main_device: u64,
    tranches: Vec<(Vec<u16>, bool)>,
}

impl DmabufFeedbackData {
    pub(super) fn new(
        feedback: &EglGlesDmabufFeedback,
        main_device: u64,
        allowed_formats: &[GpuFormat],
    ) -> io::Result<Self> {
        if main_device == 0 || feedback.format_table_formats().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "dmabuf feedback has no valid main device or format table",
            ));
        }
        let format_table_formats = feedback
            .format_table_formats()
            .iter()
            .copied()
            .filter(|format| gpu_format_is_allowed(*format, allowed_formats))
            .collect::<Vec<_>>();
        if format_table_formats.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "dmabuf feedback has no format supported by the selected importer",
            ));
        }
        let scanout = dmabuf_tranche_indices(
            &format_table_formats,
            &feedback
                .scanout_formats()
                .iter()
                .copied()
                .filter(|format| gpu_format_is_allowed(*format, allowed_formats))
                .collect::<Vec<_>>(),
        );
        let render = dmabuf_tranche_indices(
            &format_table_formats,
            &feedback
                .formats()
                .iter()
                .copied()
                .filter(|format| gpu_format_is_allowed(*format, allowed_formats))
                .collect::<Vec<_>>(),
        );
        let tranches = if scanout.is_empty() {
            vec![(render, false)]
        } else {
            vec![(scanout, true), (render, false)]
        };
        let (format_table, format_table_size) = dmabuf_format_table_file(&format_table_formats)?;
        Ok(Self {
            format_table,
            format_table_size,
            main_device,
            tranches,
        })
    }
}

fn gpu_format_is_allowed(format: EglGlesDmabufFormat, allowed_formats: &[GpuFormat]) -> bool {
    allowed_formats.iter().any(|allowed| {
        allowed.fourcc == format.format.as_fourcc() && allowed.modifier == format.modifier.0
    })
}

fn dmabuf_tranche_indices(
    format_table_formats: &[EglGlesDmabufFormat],
    tranche_formats: &[EglGlesDmabufFormat],
) -> Vec<u16> {
    tranche_formats
        .iter()
        .filter_map(|tranche_format| {
            format_table_formats
                .iter()
                .position(|table_format| table_format == tranche_format)
        })
        .filter_map(|index| u16::try_from(index).ok())
        .collect()
}

fn dmabuf_format_table_file(formats: &[EglGlesDmabufFormat]) -> io::Result<(File, u32)> {
    let path = unique_runtime_file_path("oblivion-one-dmabuf-formats");
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    let _ = fs::remove_file(&path);

    for format in formats {
        file.write_all(&format.format.as_fourcc().to_ne_bytes())?;
        file.write_all(&0u32.to_ne_bytes())?;
        file.write_all(&format.modifier.0.to_ne_bytes())?;
    }
    file.flush()?;
    file.rewind()?;
    Ok((file, (formats.len() * 16) as u32))
}

pub(super) fn send_dmabuf_feedback(
    feedback: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
) {
    let Some(data) = feedback.data::<DmabufFeedbackData>() else {
        return;
    };
    let device = data.main_device.to_ne_bytes().to_vec();
    feedback.format_table(data.format_table.as_fd(), data.format_table_size);
    feedback.main_device(device.clone());
    for (indices, scanout) in &data.tranches {
        let tranche_indices = indices
            .iter()
            .copied()
            .flat_map(u16::to_ne_bytes)
            .collect::<Vec<_>>();
        feedback.tranche_target_device(device.clone());
        feedback.tranche_flags(if *scanout {
            zwp_linux_dmabuf_feedback_v1::TrancheFlags::Scanout
        } else {
            zwp_linux_dmabuf_feedback_v1::TrancheFlags::empty()
        });
        feedback.tranche_formats(tranche_indices);
        feedback.tranche_done();
    }
    feedback.done();
}

#[derive(Debug, Default)]
pub(super) struct DmabufParamsData {
    used: Mutex<bool>,
    planes: Mutex<Vec<PendingDmabufPlane>>,
}

impl DmabufParamsData {
    pub(super) fn add_plane(
        &self,
        params: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        plane: PendingDmabufPlane,
        metrics: &mut CoreComplianceMetrics,
    ) {
        if self.is_used() {
            metrics.note_protocol_error();
            params.post_error(
                zwp_linux_buffer_params_v1::Error::AlreadyUsed,
                "linux-dmabuf params already used".to_string(),
            );
            return;
        }
        if plane.plane_idx > 3 {
            metrics.note_protocol_error();
            params.post_error(
                zwp_linux_buffer_params_v1::Error::PlaneIdx,
                "dmabuf plane index is outside the supported EGL import range".to_string(),
            );
            return;
        }
        if plane.stride == 0 {
            metrics.note_protocol_error();
            params.post_error(
                zwp_linux_buffer_params_v1::Error::OutOfBounds,
                "invalid dmabuf plane offset or stride".to_string(),
            );
            return;
        }
        let mut planes = self.planes.lock().unwrap();
        if planes
            .iter()
            .any(|existing| existing.plane_idx == plane.plane_idx)
        {
            metrics.note_protocol_error();
            params.post_error(
                zwp_linux_buffer_params_v1::Error::PlaneSet,
                "dmabuf plane index was already provided".to_string(),
            );
            return;
        }
        planes.push(plane);
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_for_create(
        &self,
        params: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        width: i32,
        height: i32,
        format: u32,
        feedback: &EglGlesDmabufFeedback,
        allowed_formats: &[GpuFormat],
        metrics: &mut CoreComplianceMetrics,
    ) -> bool {
        if !self.mark_used(params, metrics) {
            return false;
        }
        if width <= 0 || height <= 0 {
            metrics.note_protocol_error();
            params.post_error(
                zwp_linux_buffer_params_v1::Error::InvalidDimensions,
                "dmabuf width and height must be positive".to_string(),
            );
            return false;
        }
        let planes = self.planes.lock().unwrap();
        let Some(plane) = planes.first() else {
            metrics.note_protocol_error();
            params.post_error(
                zwp_linux_buffer_params_v1::Error::Incomplete,
                "dmabuf create requires at least one plane".to_string(),
            );
            return false;
        };
        let drm_format = DrmFormat::from_fourcc(format);
        if !feedback.advertises(drm_format, DrmModifier(plane.modifier))
            || !gpu_format_is_allowed(
                EglGlesDmabufFormat::new(drm_format, DrmModifier(plane.modifier)),
                allowed_formats,
            )
        {
            metrics.note_protocol_error();
            params.post_error(
                zwp_linux_buffer_params_v1::Error::InvalidFormat,
                "dmabuf format + modifier pair is not advertised by compositor feedback"
                    .to_string(),
            );
            return false;
        }
        let _fd = plane.fd.as_fd();
        if plane.offset % 4 != 0 {
            metrics.note_protocol_error();
            params.post_error(
                zwp_linux_buffer_params_v1::Error::OutOfBounds,
                "dmabuf plane offset is not aligned".to_string(),
            );
            return false;
        }

        true
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn take_buffer_data_for_create(
        &self,
        params: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        width: i32,
        height: i32,
        format: u32,
        feedback: &EglGlesDmabufFeedback,
        allowed_formats: &[GpuFormat],
        metrics: &mut CoreComplianceMetrics,
        identity: BufferIdentity,
    ) -> Option<DmabufBufferData> {
        if !self.validate_for_create(
            params,
            width,
            height,
            format,
            feedback,
            allowed_formats,
            metrics,
        ) {
            return None;
        }
        let drm_format = DrmFormat::from_fourcc(format);
        let size = BufferSize::new(width as u32, height as u32)?;
        let pending_planes = self.take_planes();
        let planes = pending_planes
            .into_iter()
            .map(|plane| {
                RenderDmabufPlane::new(
                    plane.fd,
                    DmabufPlaneDescriptor {
                        plane_index: plane.plane_idx,
                        offset: plane.offset,
                        stride: plane.stride,
                        modifier: DrmModifier(plane.modifier),
                    },
                )
            })
            .collect::<Vec<_>>();
        match DmabufBufferHandle::new(size, drm_format, planes) {
            Ok(handle) => Some(DmabufBufferData { identity, handle }),
            Err(_) => {
                metrics.note_protocol_error();
                params.post_error(
                    zwp_linux_buffer_params_v1::Error::InvalidWlBuffer,
                    "invalid dmabuf buffer metadata".to_string(),
                );
                None
            }
        }
    }

    fn take_planes(&self) -> Vec<PendingDmabufPlane> {
        self.planes
            .lock()
            .map(|mut planes| std::mem::take(&mut *planes))
            .unwrap_or_default()
    }

    fn mark_used(
        &self,
        params: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        metrics: &mut CoreComplianceMetrics,
    ) -> bool {
        let mut used = self.used.lock().unwrap();
        if *used {
            metrics.note_protocol_error();
            params.post_error(
                zwp_linux_buffer_params_v1::Error::AlreadyUsed,
                "linux-dmabuf params already used".to_string(),
            );
            return false;
        }
        *used = true;
        true
    }

    fn is_used(&self) -> bool {
        *self.used.lock().unwrap()
    }
}

#[derive(Debug, Clone)]
pub(super) struct DmabufBufferData {
    pub(super) identity: BufferIdentity,
    pub(super) handle: DmabufBufferHandle,
}

impl DmabufBufferData {
    pub(super) fn width(&self) -> u32 {
        self.handle.size().width
    }

    pub(super) fn height(&self) -> u32 {
        self.handle.size().height
    }
}

#[derive(Debug)]
pub(super) struct PendingDmabufPlane {
    pub(super) fd: OwnedFd,
    pub(super) plane_idx: u32,
    pub(super) offset: u32,
    pub(super) stride: u32,
    pub(super) modifier: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanout_tranche_is_preferred_and_render_tranche_is_not_scanout() {
        let scanout = EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier(7));
        let render = EglGlesDmabufFormat::new(DrmFormat::Argb8888, DrmModifier::LINEAR);
        let feedback = EglGlesDmabufFeedback::with_scanout_tranche([scanout], [render]);
        let allowed = [
            GpuFormat::new(DrmFormat::Xrgb8888.as_fourcc(), 7),
            GpuFormat::new(DrmFormat::Argb8888.as_fourcc(), DrmModifier::LINEAR.0),
        ];
        let data = DmabufFeedbackData::new(&feedback, 0x1234, &allowed).unwrap();

        assert_eq!(data.tranches.len(), 2);
        assert!(data.tranches[0].1);
        assert!(!data.tranches[1].1);
        assert_eq!(data.tranches[0].0, vec![0]);
        assert_eq!(data.tranches[1].0, vec![1]);
    }
}
