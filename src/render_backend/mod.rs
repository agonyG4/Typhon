pub mod buffer;
pub mod egl_gles;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderBackendKind {
    EglGles,
}

impl RenderBackendKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EglGles => "egl-gles",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuApi {
    GlGles,
}

impl GpuApi {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "gles" | "gl" | "egl-gles" | "gl-gles" => Some(Self::GlGles),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::GlGles => "gl/gles",
        }
    }

    pub const fn arg_value(self) -> &'static str {
        match self {
            Self::GlGles => "gles",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderCapability {
    GpuComposition,
    ShmImportFallback,
    DamageTrackedShmUpload,
    ModifierAwareDmabufImport,
    DmabufFeedback,
    ExplicitSync,
    DirectScanout,
    MultiGpuImport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderBackendProfile {
    pub kind: RenderBackendKind,
    pub preferred_api: GpuApi,
    pub capabilities: &'static [RenderCapability],
}

impl RenderBackendProfile {
    pub const fn egl_gles() -> Self {
        Self {
            kind: RenderBackendKind::EglGles,
            preferred_api: GpuApi::GlGles,
            capabilities: &[
                RenderCapability::GpuComposition,
                RenderCapability::ShmImportFallback,
                RenderCapability::DamageTrackedShmUpload,
                RenderCapability::ModifierAwareDmabufImport,
                RenderCapability::DmabufFeedback,
                RenderCapability::ExplicitSync,
                RenderCapability::DirectScanout,
                RenderCapability::MultiGpuImport,
            ],
        }
    }

    pub const fn smithay_egl_gles() -> Self {
        Self::egl_gles()
    }

    pub fn supports(self, capability: RenderCapability) -> bool {
        self.capabilities.contains(&capability)
    }

    pub fn missing_for(self, target: Self) -> Vec<RenderCapability> {
        target
            .capabilities
            .iter()
            .copied()
            .filter(|capability| !self.supports(*capability))
            .collect()
    }
}

pub fn browser_gpu_acceleration_ready(backend: &RenderBackendProfile) -> bool {
    backend.supports(RenderCapability::GpuComposition)
        && backend.supports(RenderCapability::ModifierAwareDmabufImport)
        && backend.supports(RenderCapability::DmabufFeedback)
        && backend.supports(RenderCapability::ExplicitSync)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn smithay_egl_backend_aliases_the_official_egl_gles_target() {
        let backend = RenderBackendProfile::smithay_egl_gles();

        assert_eq!(backend.kind, RenderBackendKind::EglGles);
        assert!(backend.supports(RenderCapability::GpuComposition));
        assert!(backend.supports(RenderCapability::ShmImportFallback));
        assert!(backend.supports(RenderCapability::ModifierAwareDmabufImport));
        assert!(backend.supports(RenderCapability::DmabufFeedback));
        assert!(backend.supports(RenderCapability::ExplicitSync));
    }

    #[test]
    fn egl_gles_backend_is_the_official_real_gpu_target() {
        let backend = RenderBackendProfile::egl_gles();

        assert_eq!(backend.kind, RenderBackendKind::EglGles);
        assert_eq!(backend.kind.as_str(), "egl-gles");
        assert_eq!(backend.preferred_api, GpuApi::GlGles);
        assert!(backend.supports(RenderCapability::ModifierAwareDmabufImport));
        assert!(backend.supports(RenderCapability::ExplicitSync));
    }

    #[test]
    fn gpu_api_parser_accepts_cli_aliases() {
        assert_eq!(GpuApi::parse("gles"), Some(GpuApi::GlGles));
        assert_eq!(GpuApi::parse("egl-gles"), Some(GpuApi::GlGles));
        assert_eq!(GpuApi::parse("vulkan"), None);
        assert_eq!(GpuApi::parse("metal"), None);
    }

    #[test]
    fn egl_gles_import_plan_requires_modifier_aware_dmabuf() {
        let plan = egl_gles::EglGlesImportPlan::from_profile(RenderBackendProfile::egl_gles());

        assert!(plan.accepts_dmabuf_buffers());
        assert!(plan.requires_dmabuf_feedback());
        assert!(!plan.requires_cpu_pixels_for_import());
    }

    #[test]
    fn committed_surface_buffers_keep_shm_and_dmabuf_separate() {
        let shm = buffer::CommittedSurfaceBuffer::shm_snapshot(
            buffer::BufferSize::new(2, 2).unwrap(),
            vec![0xff00_0000; 4],
        );
        let dmabuf = buffer::CommittedSurfaceBuffer::dmabuf_handle(
            buffer::DmabufBufferHandle::new(
                buffer::BufferSize::new(2, 2).unwrap(),
                buffer::DrmFormat::Argb8888,
                vec![buffer::DmabufPlane::new(
                    File::open("/dev/null").unwrap().into(),
                    buffer::DmabufPlaneDescriptor {
                        plane_index: 0,
                        offset: 0,
                        stride: 8,
                        modifier: buffer::DrmModifier::LINEAR,
                    },
                )],
            )
            .unwrap(),
        );

        assert_eq!(shm.source(), buffer::SurfaceBufferSource::Shm);
        assert_eq!(dmabuf.source(), buffer::SurfaceBufferSource::Dmabuf);
        assert!(shm.cpu_pixels().is_some());
        assert!(dmabuf.cpu_pixels().is_none());
    }

    #[test]
    fn brave_default_acceleration_requires_modifier_aware_dmabuf() {
        assert!(browser_gpu_acceleration_ready(
            &RenderBackendProfile::egl_gles()
        ));
        let incomplete_backend = RenderBackendProfile {
            kind: RenderBackendKind::EglGles,
            preferred_api: GpuApi::GlGles,
            capabilities: &[
                RenderCapability::GpuComposition,
                RenderCapability::ShmImportFallback,
                RenderCapability::DamageTrackedShmUpload,
            ],
        };
        assert!(!browser_gpu_acceleration_ready(&incomplete_backend));
    }
}
