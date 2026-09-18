use std::{
    fs::{self, File, OpenOptions},
    io::{self, Seek, Write},
    os::fd::{AsFd, OwnedFd},
    sync::{Arc, Mutex},
};

use wayland_protocols::wp::linux_dmabuf::zv1::server::{
    zwp_linux_buffer_params_v1, zwp_linux_dmabuf_feedback_v1, zwp_linux_dmabuf_v1,
};
use wayland_server::{Resource, Weak};

use crate::render_backend::buffer::{
    BufferIdentity, BufferSize, DmabufBufferHandle, DmabufPlane as RenderDmabufPlane,
    DmabufPlaneDescriptor, DrmFormat, DrmModifier,
};
use crate::render_backend::egl_gles::{EglGlesDmabufFeedback, EglGlesDmabufFormat};
use crate::wayland_drm::server::wl_drm;

use super::{
    CompositorState, CoreComplianceMetrics, ProtocolErrorCategory, ProtocolErrorTrace,
    gpu_protocol_capabilities::GpuFormat, post_fatal_protocol_error, unique_runtime_file_path,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DmabufFeedbackSnapshot {
    main_device: u64,
    format_table_formats: Vec<EglGlesDmabufFormat>,
    tranches: Vec<DmabufFeedbackSnapshotTranche>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DmabufFeedbackSnapshotTranche {
    formats: Vec<EglGlesDmabufFormat>,
    scanout: bool,
    target_device: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DmabufFeedbackScope {
    Default,
    Surface(u32),
    InertSurface,
}

pub(super) struct DmabufFeedbackBinding {
    scope: DmabufFeedbackScope,
    last_snapshot: Option<DmabufFeedbackSnapshot>,
    payload: Option<DmabufFeedbackData>,
}

impl DmabufFeedbackBinding {
    pub(super) fn new(
        scope: DmabufFeedbackScope,
        snapshot: DmabufFeedbackSnapshot,
    ) -> io::Result<Self> {
        Ok(Self {
            scope,
            last_snapshot: Some(snapshot.clone()),
            payload: Some(DmabufFeedbackData::from_snapshot(snapshot)?),
        })
    }

    pub(super) fn scope(&self) -> DmabufFeedbackScope {
        self.scope
    }

    pub(super) fn make_inert(&mut self) {
        self.scope = DmabufFeedbackScope::InertSurface;
    }

    pub(super) fn replace_snapshot(
        &mut self,
        snapshot: DmabufFeedbackSnapshot,
    ) -> io::Result<bool> {
        if self.last_snapshot.as_ref() == Some(&snapshot) {
            return Ok(false);
        }
        self.payload = Some(DmabufFeedbackData::from_snapshot(snapshot.clone())?);
        self.last_snapshot = Some(snapshot);
        Ok(true)
    }
}

pub(super) struct DmabufFeedbackResourceData {
    pub(super) binding: Arc<Mutex<DmabufFeedbackBinding>>,
}

#[derive(Debug)]
pub(super) struct LiveDmabufFeedbackResource {
    pub(super) resource: Weak<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1>,
}

pub(super) struct DmabufFeedbackData {
    format_table: File,
    format_table_size: u32,
    main_device: u64,
    tranches: Vec<DmabufFeedbackTranche>,
}

impl DmabufFeedbackData {
    #[cfg(test)]
    pub(super) fn new(
        feedback: &EglGlesDmabufFeedback,
        main_device: u64,
        allowed_formats: &[GpuFormat],
        scanout_capabilities: Option<&DirectScanoutFeedbackCapabilities>,
        scanout_target_device_override: Option<u64>,
    ) -> io::Result<Self> {
        let snapshot = Self::build_snapshot(
            feedback,
            main_device,
            allowed_formats,
            scanout_capabilities,
            scanout_target_device_override,
        )?;
        Self::from_snapshot(snapshot)
    }

    pub(super) fn build_snapshot(
        feedback: &EglGlesDmabufFeedback,
        main_device: u64,
        allowed_formats: &[GpuFormat],
        scanout_capabilities: Option<&DirectScanoutFeedbackCapabilities>,
        scanout_target_device_override: Option<u64>,
    ) -> io::Result<DmabufFeedbackSnapshot> {
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
        let format_table_formats = normalized_formats(format_table_formats);
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
            .unwrap_or_default();
        let render_formats = feedback
            .formats()
            .iter()
            .copied()
            .filter(|format| gpu_format_is_allowed(*format, allowed_formats))
            .collect::<Vec<_>>();
        let scanout_formats = normalized_formats(scanout_formats);
        let render_formats = normalized_formats(render_formats);
        let scanout_device = scanout_target_device_override
            .or_else(|| scanout_capabilities.map(|capabilities| capabilities.drm_device))
            .filter(|device| *device != 0)
            .unwrap_or(main_device);
        let tranches = if scanout_formats.is_empty() {
            vec![DmabufFeedbackSnapshotTranche {
                formats: render_formats,
                scanout: false,
                target_device: main_device,
            }]
        } else {
            vec![
                DmabufFeedbackSnapshotTranche {
                    formats: scanout_formats,
                    scanout: true,
                    target_device: scanout_device,
                },
                DmabufFeedbackSnapshotTranche {
                    formats: render_formats,
                    scanout: false,
                    target_device: main_device,
                },
            ]
        };
        Ok(DmabufFeedbackSnapshot {
            main_device,
            format_table_formats,
            tranches,
        })
    }

    pub(super) fn from_snapshot(snapshot: DmabufFeedbackSnapshot) -> io::Result<Self> {
        let format_table_formats = snapshot.format_table_formats.clone();
        let tranches = snapshot
            .tranches
            .iter()
            .map(|tranche| DmabufFeedbackTranche {
                indices: dmabuf_tranche_indices(&format_table_formats, &tranche.formats),
                scanout: tranche.scanout,
                target_device: tranche.target_device,
            })
            .collect();
        let (format_table, format_table_size) = dmabuf_format_table_file(&format_table_formats)?;
        Ok(Self {
            format_table,
            format_table_size,
            main_device: snapshot.main_device,
            tranches,
        })
    }
}

fn gpu_format_is_allowed(format: EglGlesDmabufFormat, allowed_formats: &[GpuFormat]) -> bool {
    allowed_formats.iter().any(|allowed| {
        allowed.fourcc == format.format.as_fourcc() && allowed.modifier == format.modifier.0
    })
}

fn normalized_formats(mut formats: Vec<EglGlesDmabufFormat>) -> Vec<EglGlesDmabufFormat> {
    formats.sort_unstable_by_key(|format| (format.format.as_fourcc(), format.modifier.0));
    formats.dedup();
    formats
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

pub(super) fn send_dmabuf_feedback_resource(
    feedback: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
) {
    let Some(resource_data) = feedback.data::<DmabufFeedbackResourceData>() else {
        return;
    };
    let Ok(binding) = resource_data.binding.lock() else {
        return;
    };
    let Some(data) = binding.payload.as_ref() else {
        return;
    };
    send_dmabuf_feedback_payload(feedback, data);
}

fn send_dmabuf_feedback_payload(
    feedback: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
    data: &DmabufFeedbackData,
) {
    feedback.format_table(data.format_table.as_fd(), data.format_table_size);
    feedback.main_device(data.main_device.to_ne_bytes().to_vec());
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

#[allow(clippy::mutable_key_type)]
impl DmabufParamsData {
    pub(super) fn add_plane(
        &self,
        params: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        plane: PendingDmabufPlane,
        metrics: &mut CoreComplianceMetrics,
        trace: &mut ProtocolErrorTrace,
        terminal_client_ids: &mut std::collections::HashSet<wayland_server::backend::ClientId>,
    ) {
        if self.is_used() {
            post_dmabuf_protocol_error(
                metrics,
                trace,
                terminal_client_ids,
                params,
                zwp_linux_buffer_params_v1::Error::AlreadyUsed,
                "linux-dmabuf params already used",
            );
            return;
        }
        if plane.plane_idx > 3 {
            post_dmabuf_protocol_error(
                metrics,
                trace,
                terminal_client_ids,
                params,
                zwp_linux_buffer_params_v1::Error::PlaneIdx,
                "dmabuf plane index is outside the supported EGL import range",
            );
            return;
        }
        if plane.stride == 0 {
            post_dmabuf_protocol_error(
                metrics,
                trace,
                terminal_client_ids,
                params,
                zwp_linux_buffer_params_v1::Error::OutOfBounds,
                "invalid dmabuf plane offset or stride",
            );
            return;
        }
        let mut planes = self.planes.lock().unwrap();
        if planes
            .iter()
            .any(|existing| existing.plane_idx == plane.plane_idx)
        {
            post_dmabuf_protocol_error(
                metrics,
                trace,
                terminal_client_ids,
                params,
                zwp_linux_buffer_params_v1::Error::PlaneSet,
                "dmabuf plane index was already provided",
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
        trace: &mut ProtocolErrorTrace,
        terminal_client_ids: &mut std::collections::HashSet<wayland_server::backend::ClientId>,
    ) -> bool {
        if !self.mark_used(params, metrics, trace, terminal_client_ids) {
            return false;
        }
        if width <= 0 || height <= 0 {
            post_dmabuf_protocol_error(
                metrics,
                trace,
                terminal_client_ids,
                params,
                zwp_linux_buffer_params_v1::Error::InvalidDimensions,
                "dmabuf width and height must be positive",
            );
            return false;
        }
        let planes = self.planes.lock().unwrap();
        let Some(plane) = planes.first() else {
            post_dmabuf_protocol_error(
                metrics,
                trace,
                terminal_client_ids,
                params,
                zwp_linux_buffer_params_v1::Error::Incomplete,
                "dmabuf create requires at least one plane",
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
            post_dmabuf_protocol_error(
                metrics,
                trace,
                terminal_client_ids,
                params,
                zwp_linux_buffer_params_v1::Error::InvalidFormat,
                "dmabuf format + modifier pair is not advertised by compositor feedback",
            );
            return false;
        }
        let _fd = plane.fd.as_fd();
        if plane.offset % 4 != 0 {
            post_dmabuf_protocol_error(
                metrics,
                trace,
                terminal_client_ids,
                params,
                zwp_linux_buffer_params_v1::Error::OutOfBounds,
                "dmabuf plane offset is not aligned",
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
        trace: &mut ProtocolErrorTrace,
        terminal_client_ids: &mut std::collections::HashSet<wayland_server::backend::ClientId>,
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
            trace,
            terminal_client_ids,
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
                post_dmabuf_protocol_error(
                    metrics,
                    trace,
                    terminal_client_ids,
                    params,
                    zwp_linux_buffer_params_v1::Error::InvalidWlBuffer,
                    "invalid dmabuf buffer metadata",
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
        trace: &mut ProtocolErrorTrace,
        terminal_client_ids: &mut std::collections::HashSet<wayland_server::backend::ClientId>,
    ) -> bool {
        let mut used = self.used.lock().unwrap();
        if *used {
            post_dmabuf_protocol_error(
                metrics,
                trace,
                terminal_client_ids,
                params,
                zwp_linux_buffer_params_v1::Error::AlreadyUsed,
                "linux-dmabuf params already used",
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

#[allow(clippy::mutable_key_type)]
fn post_dmabuf_protocol_error(
    metrics: &mut CoreComplianceMetrics,
    trace: &mut ProtocolErrorTrace,
    terminal_client_ids: &mut std::collections::HashSet<wayland_server::backend::ClientId>,
    resource: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
    code: zwp_linux_buffer_params_v1::Error,
    message: &str,
) {
    let _ = post_fatal_protocol_error(
        metrics,
        trace,
        terminal_client_ids,
        resource,
        code,
        message,
        None,
        ProtocolErrorCategory::Wire,
        None,
    );
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
        let capabilities = scanout_capabilities();
        let data = DmabufFeedbackData::new(&feedback, 0x1234, &allowed, Some(&capabilities), None)
            .unwrap();

        assert_eq!(data.tranches.len(), 2);
        assert!(data.tranches[0].scanout);
        assert!(!data.tranches[1].scanout);
        assert_eq!(data.tranches[0].indices, vec![1]);
        assert_eq!(data.tranches[1].indices, vec![0]);
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
        let capabilities = scanout_capabilities();
        let data =
            DmabufFeedbackData::new(&feedback, 0x100, &allowed, Some(&capabilities), Some(0x100))
                .unwrap();

        assert!(data.tranches[0].scanout);
        assert_eq!(data.tranches[0].indices, vec![1]);
        assert_eq!(data.tranches[1].indices, vec![0]);
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
        let capabilities = scanout_capabilities();
        let data =
            DmabufFeedbackData::new(&feedback, 0x100, &allowed, Some(&capabilities), Some(0x100))
                .unwrap();

        assert_eq!(data.tranches[0].indices, vec![1]);
        assert_eq!(data.tranches[1].indices, vec![0]);
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

    #[test]
    fn dmabuf_feedback_snapshot_is_comparable_before_materialization() {
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
        let capabilities = scanout_capabilities();
        let first = DmabufFeedbackData::build_snapshot(
            &feedback,
            0x100,
            &allowed,
            Some(&capabilities),
            None,
        )
        .unwrap();
        let second = DmabufFeedbackData::build_snapshot(
            &feedback,
            0x100,
            &allowed,
            Some(&capabilities),
            None,
        )
        .unwrap();
        let changed_target = DmabufFeedbackData::build_snapshot(
            &feedback,
            0x100,
            &allowed,
            Some(&capabilities),
            Some(0x100),
        )
        .unwrap();

        assert_eq!(first, second);
        assert_ne!(first, changed_target);
    }

    #[test]
    fn surface_bindings_keep_scanout_preference_scoped_and_deduplicated() {
        let feedback = EglGlesDmabufFeedback::with_scanout_tranche(
            [],
            [
                EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier(7)),
                EglGlesDmabufFormat::new(DrmFormat::Xbgr8888, DrmModifier(9)),
            ],
        );
        let allowed = [
            GpuFormat::new(DrmFormat::Xrgb8888.as_fourcc(), 7),
            GpuFormat::new(DrmFormat::Xbgr8888.as_fourcc(), 9),
        ];
        let capabilities = DirectScanoutFeedbackCapabilities::new(
            0x200,
            1,
            42,
            vec![
                DirectScanoutFormatCapability {
                    format: DrmFormat::Xrgb8888.as_fourcc(),
                    modifier: 7,
                },
                DirectScanoutFormatCapability {
                    format: DrmFormat::Xbgr8888.as_fourcc(),
                    modifier: 9,
                },
            ],
        );
        let renderer_snapshot =
            DmabufFeedbackData::build_snapshot(&feedback, 0x100, &allowed, None, None).unwrap();
        let scanout_snapshot = DmabufFeedbackData::build_snapshot(
            &feedback,
            0x100,
            &allowed,
            Some(&capabilities),
            None,
        )
        .unwrap();
        let mut surface_a =
            DmabufFeedbackBinding::new(DmabufFeedbackScope::Surface(1), renderer_snapshot.clone())
                .unwrap();
        let surface_b =
            DmabufFeedbackBinding::new(DmabufFeedbackScope::Surface(2), renderer_snapshot).unwrap();

        assert!(
            surface_a
                .replace_snapshot(scanout_snapshot.clone())
                .unwrap()
        );
        assert!(!surface_a.replace_snapshot(scanout_snapshot).unwrap());
        assert!(
            surface_a
                .payload
                .as_ref()
                .is_some_and(|payload| payload.tranches[0].scanout)
        );
        assert!(
            surface_b
                .payload
                .as_ref()
                .is_some_and(|payload| !payload.tranches[0].scanout)
        );
    }

    #[test]
    fn destroyed_surface_binding_becomes_inert() {
        let snapshot = DmabufFeedbackData::build_snapshot(
            &EglGlesDmabufFeedback::with_scanout_tranche(
                [],
                [EglGlesDmabufFormat::new(
                    DrmFormat::Xrgb8888,
                    DrmModifier(7),
                )],
            ),
            0x100,
            &[GpuFormat::new(DrmFormat::Xrgb8888.as_fourcc(), 7)],
            None,
            None,
        )
        .unwrap();
        let mut binding =
            DmabufFeedbackBinding::new(DmabufFeedbackScope::Surface(7), snapshot).unwrap();

        binding.make_inert();

        assert_eq!(binding.scope(), DmabufFeedbackScope::InertSurface);
    }

    fn scanout_capabilities() -> DirectScanoutFeedbackCapabilities {
        DirectScanoutFeedbackCapabilities::new(
            0x200,
            1,
            42,
            vec![DirectScanoutFormatCapability {
                format: DrmFormat::Xrgb8888.as_fourcc(),
                modifier: 7,
            }],
        )
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
