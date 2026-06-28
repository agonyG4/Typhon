pub mod buffer;
pub mod egl_gles;
pub mod native_egl_gbm;

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameFlags(u32);

impl FrameFlags {
    pub const ALLOW_PRIMARY_PLANE_SCANOUT: Self = Self(1 << 0);
    pub const ALLOW_PRIMARY_PLANE_SCANOUT_ANY: Self = Self(1 << 1);
    pub const ALLOW_OVERLAY_PLANE_SCANOUT: Self = Self(1 << 2);
    pub const ALLOW_CURSOR_PLANE_SCANOUT: Self = Self(1 << 3);
    pub const SKIP_CURSOR_ONLY_UPDATES: Self = Self(1 << 4);
    pub const ALLOW_SCANOUT: Self = Self(
        Self::ALLOW_PRIMARY_PLANE_SCANOUT.0
            | Self::ALLOW_OVERLAY_PLANE_SCANOUT.0
            | Self::ALLOW_CURSOR_PLANE_SCANOUT.0,
    );
    pub const DEFAULT: Self = Self::ALLOW_SCANOUT;

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRenderPolicy {
    pub flags: FrameFlags,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorCompositionMode {
    HardwarePlane,
    Composited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardwareCursorPlan {
    pub max_width: u32,
    pub max_height: u32,
    pub mode: CursorCompositionMode,
}

impl HardwareCursorPlan {
    pub fn choose(
        policy: FrameRenderPolicy,
        cursor_width: u32,
        cursor_height: u32,
        max_width: u32,
        max_height: u32,
    ) -> Self {
        let cursor_fits = cursor_width > 0
            && cursor_height > 0
            && cursor_width <= max_width
            && cursor_height <= max_height;
        let mode = if policy.allows_cursor_plane() && cursor_fits {
            CursorCompositionMode::HardwarePlane
        } else {
            CursorCompositionMode::Composited
        };
        Self {
            max_width,
            max_height,
            mode,
        }
    }

    pub const fn uses_hardware_plane(self) -> bool {
        matches!(self.mode, CursorCompositionMode::HardwarePlane)
    }
}

impl FrameRenderPolicy {
    pub const fn composited_only() -> Self {
        Self {
            flags: FrameFlags::empty(),
        }
    }

    pub const fn smithay_default() -> Self {
        Self {
            flags: FrameFlags::DEFAULT,
        }
    }

    pub fn for_backend(backend: &RenderBackendProfile) -> Self {
        let mut flags = FrameFlags::DEFAULT;
        if !backend.supports(RenderCapability::DirectScanout) {
            flags = flags
                .without(FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT)
                .without(FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY)
                .without(FrameFlags::ALLOW_OVERLAY_PLANE_SCANOUT);
        }
        Self { flags }
    }

    pub const fn allows_cursor_plane(self) -> bool {
        self.flags.contains(FrameFlags::ALLOW_CURSOR_PLANE_SCANOUT)
    }

    pub const fn allows_primary_scanout(self) -> bool {
        self.flags.contains(FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT)
            || self
                .flags
                .contains(FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY)
    }

    pub const fn allows_overlay_scanout(self) -> bool {
        self.flags.contains(FrameFlags::ALLOW_OVERLAY_PLANE_SCANOUT)
    }
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

    fn test_buffer_identity() -> buffer::BufferIdentity {
        buffer::BufferIdAllocator::default()
            .allocate()
            .expect("test buffer identity")
    }

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
        assert!(!backend.supports(RenderCapability::DirectScanout));
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
            test_buffer_identity(),
            buffer::BufferSize::new(2, 2).unwrap(),
            vec![0xff00_0000; 4],
        );
        let dmabuf = buffer::CommittedSurfaceBuffer::dmabuf_handle(
            test_buffer_identity(),
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

    #[test]
    fn frame_flags_match_smithay_scanout_defaults() {
        let flags = FrameFlags::DEFAULT;

        assert!(flags.contains(FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT));
        assert!(flags.contains(FrameFlags::ALLOW_OVERLAY_PLANE_SCANOUT));
        assert!(flags.contains(FrameFlags::ALLOW_CURSOR_PLANE_SCANOUT));
        assert!(!flags.contains(FrameFlags::SKIP_CURSOR_ONLY_UPDATES));
    }

    #[test]
    fn frame_render_policy_disables_surface_scanout_without_backend_capability() {
        let policy = FrameRenderPolicy::for_backend(&RenderBackendProfile::egl_gles());

        assert!(!policy.allows_primary_scanout());
        assert!(!policy.allows_overlay_scanout());
        assert!(policy.allows_cursor_plane());
    }

    #[test]
    fn native_egl_gbm_plan_accepts_render_scene_elements_for_gpu_composition() {
        use crate::compositor::{
            RenderableSurface, RenderableSurfaceDamage, SurfaceCommitSequence, SurfacePlacement,
            render_scene_elements_for_surfaces,
        };

        let surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            placement: SurfacePlacement::root(),
            render_placement: None,
            visual_clip: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: buffer::CommittedSurfaceBuffer::shm_snapshot(
                test_buffer_identity(),
                buffer::BufferSize::new(2, 2).unwrap(),
                vec![0xff00_0000; 4],
            ),
            damage: RenderableSurfaceDamage::full(),
        };
        let elements = render_scene_elements_for_surfaces(std::slice::from_ref(&surface), 1.0);

        let plan = native_egl_gbm::NativeEglGbmFramePlan::for_elements(
            RenderBackendProfile::egl_gles(),
            FrameRenderPolicy::for_backend(&RenderBackendProfile::egl_gles()),
            &elements,
        );

        assert_eq!(plan.element_count, 1);
        assert_eq!(plan.shm_uploads, 1);
        assert_eq!(plan.dmabuf_imports, 0);
        assert_eq!(
            plan.composition_mode,
            native_egl_gbm::NativeEglGbmCompositionMode::GpuComposition
        );
        assert!(!plan.direct_scanout_enabled);
    }

    #[test]
    fn hardware_cursor_plan_uses_cursor_plane_only_when_cursor_fits() {
        let policy = FrameRenderPolicy::smithay_default();

        let small = HardwareCursorPlan::choose(policy, 32, 32, 64, 64);
        let large = HardwareCursorPlan::choose(policy, 128, 32, 64, 64);

        assert!(small.uses_hardware_plane());
        assert!(!large.uses_hardware_plane());
    }

    #[test]
    fn hardware_cursor_plan_falls_back_when_policy_disables_cursor_plane() {
        let policy = FrameRenderPolicy::composited_only();

        let plan = HardwareCursorPlan::choose(policy, 32, 32, 64, 64);

        assert_eq!(plan.mode, CursorCompositionMode::Composited);
    }
}
