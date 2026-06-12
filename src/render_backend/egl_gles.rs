use super::{
    RenderBackendProfile, RenderCapability,
    buffer::{BufferSize, DmabufBufferHandle, DrmFormat, DrmModifier},
};
use std::os::fd::AsRawFd;

pub const EGL_NONE: usize = 0x3038;
pub const EGL_HEIGHT: usize = 0x3056;
pub const EGL_WIDTH: usize = 0x3057;
pub const EGL_LINUX_DMA_BUF_EXT: u32 = 0x3270;
pub const EGL_LINUX_DRM_FOURCC_EXT: usize = 0x3271;
pub const EGL_DMA_BUF_PLANE0_FD_EXT: usize = 0x3272;
pub const EGL_DMA_BUF_PLANE0_OFFSET_EXT: usize = 0x3273;
pub const EGL_DMA_BUF_PLANE0_PITCH_EXT: usize = 0x3274;
pub const EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT: usize = 0x3443;
pub const EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT: usize = 0x3444;

const EGL_DMA_BUF_PLANE_FD_EXT: [usize; 4] = [EGL_DMA_BUF_PLANE0_FD_EXT, 0x3275, 0x3278, 0x3440];
const EGL_DMA_BUF_PLANE_OFFSET_EXT: [usize; 4] =
    [EGL_DMA_BUF_PLANE0_OFFSET_EXT, 0x3276, 0x3279, 0x3441];
const EGL_DMA_BUF_PLANE_PITCH_EXT: [usize; 4] =
    [EGL_DMA_BUF_PLANE0_PITCH_EXT, 0x3277, 0x327a, 0x3442];
const EGL_DMA_BUF_PLANE_MODIFIER_LO_EXT: [usize; 4] =
    [EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT, 0x3445, 0x3447, 0x3449];
const EGL_DMA_BUF_PLANE_MODIFIER_HI_EXT: [usize; 4] =
    [EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT, 0x3446, 0x3448, 0x344a];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EglGlesImportPlan {
    profile: RenderBackendProfile,
    dmabuf_feedback: EglGlesDmabufFeedback,
}

impl EglGlesImportPlan {
    pub fn from_profile(profile: RenderBackendProfile) -> Self {
        Self::with_dmabuf_feedback(profile, EglGlesDmabufFeedback::linear_argb_xrgb())
    }

    pub const fn with_dmabuf_feedback(
        profile: RenderBackendProfile,
        dmabuf_feedback: EglGlesDmabufFeedback,
    ) -> Self {
        Self {
            profile,
            dmabuf_feedback,
        }
    }

    pub fn accepts_dmabuf_buffers(&self) -> bool {
        self.profile
            .supports(RenderCapability::ModifierAwareDmabufImport)
    }

    pub fn requires_dmabuf_feedback(&self) -> bool {
        self.profile.supports(RenderCapability::DmabufFeedback)
    }

    pub fn requires_cpu_pixels_for_import(&self) -> bool {
        !self.accepts_dmabuf_buffers()
    }

    pub fn import_candidate_for(
        &self,
        handle: &DmabufBufferHandle,
    ) -> Result<EglGlesImportCandidate, EglGlesImportError> {
        self.require_capability(RenderCapability::ModifierAwareDmabufImport)?;
        self.require_capability(RenderCapability::DmabufFeedback)?;

        let modifier = handle
            .planes()
            .first()
            .map(|plane| plane.descriptor().modifier)
            .ok_or(EglGlesImportError::InvalidPlaneSet)?;
        let format = handle.format();
        if !self.dmabuf_feedback.advertises(format, modifier) {
            return Err(EglGlesImportError::UnsupportedFormatModifier { format, modifier });
        }

        Ok(EglGlesImportCandidate {
            size: handle.size(),
            format,
            modifier,
            plane_count: handle.planes().len(),
        })
    }

    fn require_capability(&self, capability: RenderCapability) -> Result<(), EglGlesImportError> {
        if self.profile.supports(capability) {
            Ok(())
        } else {
            Err(EglGlesImportError::MissingCapability(capability))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EglGlesDmabufFeedback {
    formats: Vec<EglGlesDmabufFormat>,
    format_table_formats: Vec<EglGlesDmabufFormat>,
}

impl Default for EglGlesDmabufFeedback {
    fn default() -> Self {
        Self::renderer_default()
    }
}

impl EglGlesDmabufFeedback {
    pub const fn new(formats: Vec<EglGlesDmabufFormat>) -> Self {
        Self {
            format_table_formats: Vec::new(),
            formats,
        }
    }

    pub fn renderer_default() -> Self {
        Self::from_formats([
            EglGlesDmabufFormat::new(DrmFormat::Argb8888, DrmModifier::LINEAR),
            EglGlesDmabufFormat::new(DrmFormat::Argb8888, DrmModifier::INVALID),
            EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier::LINEAR),
            EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier::INVALID),
        ])
    }

    pub fn linear_argb_xrgb() -> Self {
        Self::from_formats([
            EglGlesDmabufFormat::new(DrmFormat::Argb8888, DrmModifier::LINEAR),
            EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier::LINEAR),
        ])
    }

    pub fn from_formats(formats: impl IntoIterator<Item = EglGlesDmabufFormat>) -> Self {
        let mut formats = formats.into_iter().collect::<Vec<_>>();
        formats.sort_unstable_by_key(|format| (format.format.as_fourcc(), format.modifier.0));
        formats.dedup_by_key(|format| (format.format.as_fourcc(), format.modifier.0));
        Self {
            format_table_formats: formats.clone(),
            formats,
        }
    }

    pub fn from_preferred_formats(formats: impl IntoIterator<Item = EglGlesDmabufFormat>) -> Self {
        let mut unique_formats = Vec::new();
        for format in formats {
            if unique_formats.iter().any(|existing: &EglGlesDmabufFormat| {
                existing.format == format.format && existing.modifier == format.modifier
            }) {
                continue;
            }
            unique_formats.push(format);
        }
        Self {
            format_table_formats: unique_formats.clone(),
            formats: unique_formats,
        }
    }

    pub fn from_table_and_tranche_formats(
        format_table_formats: impl IntoIterator<Item = EglGlesDmabufFormat>,
        tranche_formats: impl IntoIterator<Item = EglGlesDmabufFormat>,
    ) -> Self {
        let format_table_formats = unique_dmabuf_formats(format_table_formats);
        let formats = unique_dmabuf_formats(tranche_formats);
        Self {
            format_table_formats,
            formats,
        }
    }

    pub fn supports(&self, format: DrmFormat, modifier: DrmModifier) -> bool {
        self.formats
            .iter()
            .any(|supported| supported.format == format && supported.modifier == modifier)
    }

    pub fn advertises(&self, format: DrmFormat, modifier: DrmModifier) -> bool {
        self.format_table_formats()
            .iter()
            .any(|supported| supported.format == format && supported.modifier == modifier)
    }

    pub fn formats(&self) -> &[EglGlesDmabufFormat] {
        &self.formats
    }

    pub fn format_table_formats(&self) -> &[EglGlesDmabufFormat] {
        if self.format_table_formats.is_empty() {
            &self.formats
        } else {
            &self.format_table_formats
        }
    }
}

fn unique_dmabuf_formats(
    formats: impl IntoIterator<Item = EglGlesDmabufFormat>,
) -> Vec<EglGlesDmabufFormat> {
    let mut unique_formats = Vec::new();
    for format in formats {
        if unique_formats.iter().any(|existing: &EglGlesDmabufFormat| {
            existing.format == format.format && existing.modifier == format.modifier
        }) {
            continue;
        }
        unique_formats.push(format);
    }
    unique_formats
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EglGlesDmabufFormat {
    pub format: DrmFormat,
    pub modifier: DrmModifier,
}

impl EglGlesDmabufFormat {
    pub const fn new(format: DrmFormat, modifier: DrmModifier) -> Self {
        Self { format, modifier }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EglGlesImportCandidate {
    size: BufferSize,
    format: DrmFormat,
    modifier: DrmModifier,
    plane_count: usize,
}

impl EglGlesImportCandidate {
    pub const fn size(&self) -> BufferSize {
        self.size
    }

    pub const fn format(&self) -> DrmFormat {
        self.format
    }

    pub const fn modifier(&self) -> DrmModifier {
        self.modifier
    }

    pub const fn plane_count(&self) -> usize {
        self.plane_count
    }

    pub const fn requires_cpu_pixels(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EglGlesDmabufImportAttributes {
    attributes: Vec<usize>,
}

impl EglGlesDmabufImportAttributes {
    pub fn from_handle(handle: &DmabufBufferHandle) -> Result<Self, EglGlesImportError> {
        let planes = handle.planes();
        if planes.is_empty() || planes.len() > EGL_DMA_BUF_PLANE_FD_EXT.len() {
            return Err(EglGlesImportError::InvalidPlaneSet);
        }

        let size = handle.size();
        let mut attributes = Vec::with_capacity(6 + planes.len() * 10 + 1);
        attributes.extend_from_slice(&[
            EGL_WIDTH,
            size.width as usize,
            EGL_HEIGHT,
            size.height as usize,
            EGL_LINUX_DRM_FOURCC_EXT,
            handle.format().as_fourcc() as usize,
        ]);

        for (expected_index, plane) in planes.iter().enumerate() {
            let descriptor = plane.descriptor();
            let plane_index = usize::try_from(descriptor.plane_index)
                .map_err(|_| EglGlesImportError::InvalidPlaneSet)?;
            if plane_index != expected_index {
                return Err(EglGlesImportError::InvalidPlaneSet);
            }

            let modifier = descriptor.modifier.0;
            attributes.extend_from_slice(&[
                EGL_DMA_BUF_PLANE_FD_EXT[plane_index],
                plane.fd().as_raw_fd() as usize,
                EGL_DMA_BUF_PLANE_OFFSET_EXT[plane_index],
                descriptor.offset as usize,
                EGL_DMA_BUF_PLANE_PITCH_EXT[plane_index],
                descriptor.stride as usize,
                EGL_DMA_BUF_PLANE_MODIFIER_LO_EXT[plane_index],
                modifier as u32 as usize,
                EGL_DMA_BUF_PLANE_MODIFIER_HI_EXT[plane_index],
                (modifier >> 32) as usize,
            ]);
        }

        attributes.push(EGL_NONE);
        Ok(Self { attributes })
    }

    pub fn as_slice(&self) -> &[usize] {
        &self.attributes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EglGlesImportError {
    MissingCapability(RenderCapability),
    UnsupportedFormatModifier {
        format: DrmFormat,
        modifier: DrmModifier,
    },
    InvalidPlaneSet,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_backend::buffer::{
        BufferSize, DmabufBufferHandle, DmabufPlane, DmabufPlaneDescriptor, DrmFormat, DrmModifier,
    };
    use crate::render_backend::{RenderBackendProfile, RenderCapability};
    use std::{fs::File, os::fd::AsRawFd};

    #[test]
    fn egl_gles_import_plan_builds_candidate_for_supported_argb_linear_dmabuf() {
        let plan = EglGlesImportPlan::with_dmabuf_feedback(
            RenderBackendProfile::egl_gles(),
            EglGlesDmabufFeedback::new(vec![EglGlesDmabufFormat::new(
                DrmFormat::Argb8888,
                DrmModifier::LINEAR,
            )]),
        );
        let handle = dmabuf_handle(DrmFormat::Argb8888, DrmModifier::LINEAR);

        let candidate = plan.import_candidate_for(&handle).unwrap();

        assert_eq!(candidate.size(), BufferSize::new(2, 2).unwrap());
        assert_eq!(candidate.format(), DrmFormat::Argb8888);
        assert_eq!(candidate.modifier(), DrmModifier::LINEAR);
        assert_eq!(candidate.plane_count(), 1);
        assert!(!candidate.requires_cpu_pixels());
    }

    #[test]
    fn egl_gles_import_plan_rejects_unsupported_modifier() {
        let unsupported_modifier = DrmModifier(0x0100_0000_0000_0002);
        let plan = EglGlesImportPlan::with_dmabuf_feedback(
            RenderBackendProfile::egl_gles(),
            EglGlesDmabufFeedback::new(vec![EglGlesDmabufFormat::new(
                DrmFormat::Argb8888,
                DrmModifier::LINEAR,
            )]),
        );
        let handle = dmabuf_handle(DrmFormat::Argb8888, unsupported_modifier);

        assert_eq!(
            plan.import_candidate_for(&handle),
            Err(EglGlesImportError::UnsupportedFormatModifier {
                format: DrmFormat::Argb8888,
                modifier: unsupported_modifier,
            })
        );
    }

    #[test]
    fn egl_gles_import_plan_rejects_profiles_without_dmabuf_import_capability() {
        let incomplete_profile = RenderBackendProfile {
            kind: super::super::RenderBackendKind::EglGles,
            preferred_api: super::super::GpuApi::GlGles,
            capabilities: &[
                RenderCapability::GpuComposition,
                RenderCapability::ShmImportFallback,
                RenderCapability::DamageTrackedShmUpload,
            ],
        };
        let plan = EglGlesImportPlan::with_dmabuf_feedback(
            incomplete_profile,
            EglGlesDmabufFeedback::linear_argb_xrgb(),
        );
        let handle = dmabuf_handle(DrmFormat::Argb8888, DrmModifier::LINEAR);

        assert_eq!(
            plan.import_candidate_for(&handle),
            Err(EglGlesImportError::MissingCapability(
                RenderCapability::ModifierAwareDmabufImport
            ))
        );
    }

    #[test]
    fn egl_gles_dmabuf_import_attributes_include_modifier_aware_plane_metadata() {
        let modifier = DrmModifier(0x0100_0000_0000_0002);
        let handle = dmabuf_handle(DrmFormat::Argb8888, modifier);

        let attributes = EglGlesDmabufImportAttributes::from_handle(&handle)
            .expect("single-plane ARGB dmabuf should produce EGL import attributes");

        assert_eq!(
            attributes.as_slice(),
            &[
                EGL_WIDTH,
                2,
                EGL_HEIGHT,
                2,
                EGL_LINUX_DRM_FOURCC_EXT,
                DrmFormat::ARGB8888_FOURCC as usize,
                EGL_DMA_BUF_PLANE0_FD_EXT,
                handle.planes()[0].fd().as_raw_fd() as usize,
                EGL_DMA_BUF_PLANE0_OFFSET_EXT,
                0,
                EGL_DMA_BUF_PLANE0_PITCH_EXT,
                8,
                EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT,
                modifier.0 as u32 as usize,
                EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT,
                (modifier.0 >> 32) as usize,
                EGL_NONE,
            ]
        );
    }

    #[test]
    fn egl_gles_dmabuf_import_attributes_reject_empty_plane_sets() {
        let empty = DmabufBufferHandle::new(
            BufferSize::new(2, 2).unwrap(),
            DrmFormat::Argb8888,
            Vec::new(),
        );

        assert!(matches!(
            empty,
            Err(crate::render_backend::buffer::BufferValidationError::MissingPlane)
        ));
    }

    #[test]
    fn egl_gles_dmabuf_feedback_deduplicates_format_modifier_pairs() {
        let feedback = EglGlesDmabufFeedback::from_formats([
            EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier::LINEAR),
            EglGlesDmabufFormat::new(DrmFormat::Argb8888, DrmModifier::INVALID),
            EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier::LINEAR),
            EglGlesDmabufFormat::new(DrmFormat::Argb8888, DrmModifier::LINEAR),
        ]);

        assert_eq!(
            feedback.formats(),
            &[
                EglGlesDmabufFormat::new(DrmFormat::Argb8888, DrmModifier::LINEAR),
                EglGlesDmabufFormat::new(DrmFormat::Argb8888, DrmModifier::INVALID),
                EglGlesDmabufFormat::new(DrmFormat::Xrgb8888, DrmModifier::LINEAR),
            ]
        );
    }

    fn dmabuf_handle(format: DrmFormat, modifier: DrmModifier) -> DmabufBufferHandle {
        DmabufBufferHandle::new(
            BufferSize::new(2, 2).unwrap(),
            format,
            vec![DmabufPlane::new(
                File::open("/dev/null").unwrap().into(),
                DmabufPlaneDescriptor {
                    plane_index: 0,
                    offset: 0,
                    stride: 8,
                    modifier,
                },
            )],
        )
        .unwrap()
    }
}
