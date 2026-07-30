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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectScanoutFormatCapability {
    pub format: u32,
    pub modifier: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectScanoutFeedbackCapabilities {
    pub drm_device: u64,
    pub output_generation: u64,
    pub primary_plane_id: u32,
    pub formats: Vec<DirectScanoutFormatCapability>,
}

impl DirectScanoutFeedbackCapabilities {
    pub fn new(
        drm_device: u64,
        output_generation: u64,
        primary_plane_id: u32,
        formats: Vec<DirectScanoutFormatCapability>,
    ) -> Self {
        let mut formats = formats;
        formats.sort_unstable_by_key(|format| (format.format, format.modifier));
        formats.dedup_by_key(|format| (format.format, format.modifier));
        Self {
            drm_device,
            output_generation,
            primary_plane_id,
            formats,
        }
    }

    pub fn supports(&self, format: u32, modifier: u64) -> bool {
        self.formats
            .binary_search_by_key(&(format, modifier), |capability| {
                (capability.format, capability.modifier)
            })
            .is_ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DmabufFeedbackTranche {
    indices: Vec<u16>,
    scanout: bool,
    target_device: u64,
}

pub(super) struct DmabufFeedbackData {
    format_table: File,
    format_table_size: u32,
    main_device: u64,
    tranches: Vec<DmabufFeedbackTranche>,
}

impl DmabufFeedbackData {
    pub(super) fn new(
        feedback: &EglGlesDmabufFeedback,
        main_device: u64,
        allowed_formats: &[GpuFormat],
        scanout_capabilities: Option<&DirectScanoutFeedbackCapabilities>,
        scanout_target_device_override: Option<u64>,
    ) -> io::Result<Self> {
        if main_device == 0 || feedback.format_table_formats().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "dmabuf feedback has no valid main device or format table",
            ));
        }
        let mut format_table_formats = Vec::new();
        if let Some(capabilities) = scanout_capabilities {
            for capability in &capabilities.formats {
                let format = EglGlesDmabufFormat::new(
                    DrmFormat::from_fourcc(capability.format),
                    DrmModifier(capability.modifier),
                );
                if gpu_format_is_allowed(format, allowed_formats)
                    && !format_table_formats.contains(&format)
                {
                    format_table_formats.push(format);
                }
            }
        }
        for format in feedback.format_table_formats() {
            if gpu_format_is_allowed(*format, allowed_formats)
                && !format_table_formats.contains(format)
            {
                format_table_formats.push(*format);
            }
        }
        if format_table_formats.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "dmabuf feedback has no format supported by the selected importer",
            ));
        }
        let scanout_formats = scanout_capabilities
            .map(|capabilities| {
                capabilities
                    .formats
                    .iter()
                    .map(|capability| {
                        EglGlesDmabufFormat::new(
                            DrmFormat::from_fourcc(capability.format),
                            DrmModifier(capability.modifier),
                        )
                    })
                    .filter(|format| gpu_format_is_allowed(*format, allowed_formats))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                feedback
                    .scanout_formats()
                    .iter()
                    .copied()
                    .filter(|format| gpu_format_is_allowed(*format, allowed_formats))
                    .collect::<Vec<_>>()
            });
        let scanout = dmabuf_tranche_indices(&format_table_formats, &scanout_formats);
        let render = dmabuf_tranche_indices(
            &format_table_formats,
            &feedback
                .formats()
                .iter()
                .copied()
                .filter(|format| gpu_format_is_allowed(*format, allowed_formats))
                .collect::<Vec<_>>(),
        );
        let scanout_device = scanout_target_device_override
            .or_else(|| scanout_capabilities.map(|capabilities| capabilities.drm_device))
            .filter(|device| *device != 0)
            .unwrap_or(main_device);
        let tranches = if scanout.is_empty() {
            vec![DmabufFeedbackTranche {
                indices: render,
                scanout: false,
                target_device: main_device,
            }]
        } else {
            vec![
                DmabufFeedbackTranche {
                    indices: scanout,
                    scanout: true,
                    target_device: scanout_device,
                },
                DmabufFeedbackTranche {
                    indices: render,
                    scanout: false,
                    target_device: main_device,
                },
            ]
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
    for tranche in &data.tranches {
        let tranche_indices = tranche
            .indices
            .iter()
            .copied()
            .flat_map(u16::to_ne_bytes)
            .collect::<Vec<_>>();
        feedback.tranche_target_device(tranche.target_device.to_ne_bytes().to_vec());
        feedback.tranche_flags(if tranche.scanout {
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
        let data = DmabufFeedbackData::new(&feedback, 0x1234, &allowed, None, None).unwrap();

        assert_eq!(data.tranches.len(), 2);
        assert!(data.tranches[0].scanout);
        assert!(!data.tranches[1].scanout);
        assert_eq!(data.tranches[0].indices, vec![0]);
        assert_eq!(data.tranches[1].indices, vec![1]);
    }

    #[test]
    fn strict_feedback_keeps_primary_scanout_target() {
        let data = feedback_data(None);

        assert_eq!(data.tranches[0].target_device, 0x200);
        assert_eq!(data.tranches[1].target_device, 0x100);
    }

    #[test]
    fn forced_compat_normalizes_same_gpu_scanout_target() {
        let data = feedback_data(Some(0x100));

        assert_eq!(data.tranches[0].target_device, 0x100);
        assert_eq!(data.tranches[1].target_device, 0x100);
    }

    #[test]
    fn compat_preserves_scanout_tranche_flag_and_exact_format_subset() {
        let scanout = EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier(7));
        let render = EglGlesDmabufFormat::new(DrmFormat::Argb8888, DrmModifier::LINEAR);
        let feedback = EglGlesDmabufFeedback::with_scanout_tranche([scanout], [render]);
        let allowed = [
            GpuFormat::new(DrmFormat::Xrgb8888.as_fourcc(), 7),
            GpuFormat::new(DrmFormat::Argb8888.as_fourcc(), DrmModifier::LINEAR.0),
        ];
        let data = DmabufFeedbackData::new(&feedback, 0x100, &allowed, None, Some(0x100)).unwrap();

        assert!(data.tranches[0].scanout);
        assert_eq!(data.tranches[0].indices, vec![0]);
        assert_eq!(data.tranches[1].indices, vec![1]);
        assert_eq!(
            data.tranches[0].target_device,
            data.tranches[1].target_device
        );
    }

    #[test]
    fn compat_does_not_duplicate_format_modifier_pairs() {
        let scanout = EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier(7));
        let render = EglGlesDmabufFormat::new(DrmFormat::Argb8888, DrmModifier::LINEAR);
        let feedback =
            EglGlesDmabufFeedback::with_scanout_tranche([scanout, scanout], [render, render]);
        let allowed = [
            GpuFormat::new(DrmFormat::Xrgb8888.as_fourcc(), 7),
            GpuFormat::new(DrmFormat::Argb8888.as_fourcc(), DrmModifier::LINEAR.0),
        ];
        let data = DmabufFeedbackData::new(&feedback, 0x100, &allowed, None, Some(0x100)).unwrap();

        assert_eq!(data.tranches[0].indices, vec![0]);
        assert_eq!(data.tranches[1].indices, vec![1]);
        assert_eq!(data.format_table_size, 32);
    }

    #[test]
    fn compat_does_not_expand_direct_scanout_eligibility() {
        let capabilities = DirectScanoutFeedbackCapabilities::new(
            0x200,
            1,
            42,
            vec![DirectScanoutFormatCapability {
                format: DrmFormat::Xrgb8888.as_fourcc(),
                modifier: 7,
            }],
        );
        let before = capabilities.clone();
        let feedback = EglGlesDmabufFeedback::with_scanout_tranche(
            [EglGlesDmabufFormat::new(
                DrmFormat::Xrgb8888,
                DrmModifier(7),
            )],
            [EglGlesDmabufFormat::new(
                DrmFormat::Argb8888,
                DrmModifier::LINEAR,
            )],
        );
        let allowed = [
            GpuFormat::new(DrmFormat::Xrgb8888.as_fourcc(), 7),
            GpuFormat::new(DrmFormat::Argb8888.as_fourcc(), DrmModifier::LINEAR.0),
        ];

        let _ =
            DmabufFeedbackData::new(&feedback, 0x100, &allowed, Some(&capabilities), Some(0x100))
                .unwrap();

        assert_eq!(capabilities, before);
    }

    #[test]
    fn strict_and_compat_modes_produce_deterministic_feedback() {
        let strict = feedback_data(None);
        let strict_again = feedback_data(None);
        let compat = feedback_data(Some(0x100));
        let compat_again = feedback_data(Some(0x100));

        assert_eq!(
            feedback_shape(&strict),
            feedback_shape(&strict_again),
            "strict feedback must be deterministic"
        );
        assert_eq!(
            feedback_shape(&compat),
            feedback_shape(&compat_again),
            "compatibility feedback must be deterministic"
        );
        assert_ne!(feedback_shape(&strict), feedback_shape(&compat));
    }

    fn feedback_data(scanout_target_override: Option<u64>) -> DmabufFeedbackData {
        let feedback = EglGlesDmabufFeedback::with_scanout_tranche(
            [EglGlesDmabufFormat::new(
                DrmFormat::Xrgb8888,
                DrmModifier(7),
            )],
            [EglGlesDmabufFormat::new(
                DrmFormat::Argb8888,
                DrmModifier::LINEAR,
            )],
        );
        let allowed = [
            GpuFormat::new(DrmFormat::Xrgb8888.as_fourcc(), 7),
            GpuFormat::new(DrmFormat::Argb8888.as_fourcc(), DrmModifier::LINEAR.0),
        ];
        let capabilities = DirectScanoutFeedbackCapabilities::new(
            0x200,
            1,
            42,
            vec![DirectScanoutFormatCapability {
                format: DrmFormat::Xrgb8888.as_fourcc(),
                modifier: 7,
            }],
        );
        DmabufFeedbackData::new(
            &feedback,
            0x100,
            &allowed,
            Some(&capabilities),
            scanout_target_override,
        )
        .unwrap()
    }

    fn feedback_shape(data: &DmabufFeedbackData) -> Vec<(Vec<u16>, bool, u64)> {
        data.tranches
            .iter()
            .map(|tranche| {
                (
                    tranche.indices.clone(),
                    tranche.scanout,
                    tranche.target_device,
                )
            })
            .collect()
    }
}
