use crate::compositor::RenderSceneElement;

use super::{
    FrameRenderPolicy, RenderBackendKind, RenderBackendProfile, RenderCapability,
    buffer::SurfaceBufferSource,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeEglGbmCompositionMode {
    GpuComposition,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeEglGbmFramePlan {
    pub backend: RenderBackendKind,
    pub element_count: usize,
    pub shm_uploads: usize,
    pub dmabuf_imports: usize,
    pub composition_mode: NativeEglGbmCompositionMode,
    pub direct_scanout_enabled: bool,
}

impl NativeEglGbmFramePlan {
    pub fn for_elements(
        profile: RenderBackendProfile,
        policy: FrameRenderPolicy,
        elements: &[RenderSceneElement],
    ) -> Self {
        let mut shm_uploads = 0;
        let mut dmabuf_imports = 0;
        for element in elements {
            match element.buffer_source() {
                SurfaceBufferSource::Shm => shm_uploads += 1,
                SurfaceBufferSource::Dmabuf => dmabuf_imports += 1,
            }
        }

        let composition_mode = if profile.supports(RenderCapability::GpuComposition) {
            NativeEglGbmCompositionMode::GpuComposition
        } else {
            NativeEglGbmCompositionMode::Unsupported
        };
        let direct_scanout_enabled = elements.len() == 1
            && policy.allows_primary_scanout()
            && profile.supports(RenderCapability::DirectScanout);

        Self {
            backend: profile.kind,
            element_count: elements.len(),
            shm_uploads,
            dmabuf_imports,
            composition_mode,
            direct_scanout_enabled,
        }
    }
}
