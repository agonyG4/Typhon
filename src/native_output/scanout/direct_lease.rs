use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use oblivion_one::compositor::{
    DirectScanoutSceneCandidate, PresentationRect, SurfaceCommitSequence, SurfaceDamagePresentation,
};
use oblivion_one::core::SceneNodeId;
use oblivion_one::render_backend::buffer::DmabufBufferHandle;

use super::{DirectPlaneValidationKey, DirectScanoutCandidateKey, ImportedDirectFramebuffer};

#[derive(Debug)]
pub(crate) struct DirectPrimaryLease {
    key: DirectScanoutCandidateKey,
    validation_key: DirectPlaneValidationKey,
    surface_id: u32,
    root_surface_id: u32,
    buffer_id: oblivion_one::render_backend::buffer::BufferId,
    surface_scene_node_id: SceneNodeId,
    window_scene_node_id: SceneNodeId,
    presented_window_rect: PresentationRect,
    render_generation: u64,
    effect_identity_signature: u64,
    surface_presentation_generation: u64,
    commit_sequence: SurfaceCommitSequence,
    _buffer: DmabufBufferHandle,
    framebuffer: Arc<ImportedDirectFramebuffer>,
    surface_damage: Option<SurfaceDamagePresentation>,
    live_lease_count: Arc<AtomicU64>,
}

impl DirectPrimaryLease {
    pub(crate) fn new(
        candidate: DirectScanoutSceneCandidate,
        key: DirectScanoutCandidateKey,
        validation_key: DirectPlaneValidationKey,
        framebuffer: Arc<ImportedDirectFramebuffer>,
        surface_damage: SurfaceDamagePresentation,
        live_lease_count: Arc<AtomicU64>,
    ) -> Self {
        live_lease_count.fetch_add(1, Ordering::AcqRel);
        Self {
            key,
            validation_key,
            surface_id: candidate.surface_id,
            root_surface_id: candidate.root_surface_id,
            buffer_id: candidate.buffer_identity.id(),
            surface_scene_node_id: candidate.surface_scene_node_id,
            window_scene_node_id: candidate.window_scene_node_id,
            presented_window_rect: candidate.presented_window_rect,
            render_generation: candidate.render_generation,
            effect_identity_signature: candidate.effect_identity_signature,
            surface_presentation_generation: candidate.surface_presentation_generation,
            commit_sequence: candidate.commit_sequence,
            _buffer: candidate.buffer,
            framebuffer,
            surface_damage: Some(surface_damage),
            live_lease_count,
        }
    }

    pub(crate) const fn key(&self) -> DirectScanoutCandidateKey {
        self.key
    }

    pub(crate) const fn surface_id(&self) -> u32 {
        self.surface_id
    }

    pub(crate) const fn presented_window_rect(&self) -> PresentationRect {
        self.presented_window_rect
    }

    pub(crate) const fn surface_scene_node_id(&self) -> SceneNodeId {
        self.surface_scene_node_id
    }

    pub(crate) const fn window_scene_node_id(&self) -> SceneNodeId {
        self.window_scene_node_id
    }

    pub(crate) const fn render_generation(&self) -> u64 {
        self.render_generation
    }

    pub(crate) const fn effect_identity_signature(&self) -> u64 {
        self.effect_identity_signature
    }

    pub(crate) const fn root_surface_id(&self) -> u32 {
        self.root_surface_id
    }

    pub(crate) const fn buffer_id(&self) -> oblivion_one::render_backend::buffer::BufferId {
        self.buffer_id
    }

    pub(crate) const fn surface_presentation_generation(&self) -> u64 {
        self.surface_presentation_generation
    }

    pub(crate) const fn commit_sequence(&self) -> SurfaceCommitSequence {
        self.commit_sequence
    }

    pub(crate) const fn validation_key(&self) -> DirectPlaneValidationKey {
        self.validation_key
    }

    pub(crate) fn validate_against(
        &self,
        expected_key: DirectScanoutCandidateKey,
        expected_surface_id: u32,
        expected_framebuffer_id: u32,
    ) -> bool {
        self.key == expected_key
            && self.key.content.surface_id == self.surface_id
            && self.surface_id == expected_surface_id
            && self.framebuffer_id() == expected_framebuffer_id
    }

    pub(crate) fn framebuffer_id(&self) -> u32 {
        self.framebuffer.framebuffer.get()
    }

    pub(crate) fn clone_surface_damage(&self) -> io::Result<SurfaceDamagePresentation> {
        self.surface_damage
            .clone()
            .ok_or_else(|| io::Error::other("direct surface damage is already settled"))
    }

    pub(crate) fn take_surface_damage(&mut self) -> io::Result<SurfaceDamagePresentation> {
        self.surface_damage
            .take()
            .ok_or_else(|| io::Error::other("direct surface damage already settled"))
    }

    pub(crate) fn disarm_drm_cleanup(&self) {
        self.framebuffer.disarm_drm_cleanup();
    }

    #[cfg(test)]
    pub(crate) fn test_fixture(key: DirectScanoutCandidateKey, framebuffer_id: u32) -> Self {
        Self::test_fixture_with_probe(key, framebuffer_id).0
    }

    #[cfg(test)]
    pub(crate) fn test_fixture_with_probe(
        key: DirectScanoutCandidateKey,
        framebuffer_id: u32,
    ) -> (Self, Arc<std::sync::atomic::AtomicU64>) {
        Self::test_fixture_with_probe_and_damage(key, framebuffer_id, None)
    }

    #[cfg(test)]
    pub(crate) fn test_fixture_with_probe_and_root(
        key: DirectScanoutCandidateKey,
        framebuffer_id: u32,
        root_surface_id: u32,
    ) -> (Self, Arc<std::sync::atomic::AtomicU64>) {
        let (mut lease, cleanup_count) = Self::test_fixture_with_probe(key, framebuffer_id);
        lease.root_surface_id = root_surface_id;
        (lease, cleanup_count)
    }

    #[cfg(test)]
    pub(crate) fn test_fixture_with_probe_and_damage(
        key: DirectScanoutCandidateKey,
        framebuffer_id: u32,
        surface_damage: Option<SurfaceDamagePresentation>,
    ) -> (Self, Arc<std::sync::atomic::AtomicU64>) {
        Self::test_fixture_with_probe_and_damage_and_rect(
            key,
            framebuffer_id,
            surface_damage,
            PresentationRect::new(0.0, 0.0, 1.0, 1.0).expect("valid direct test window rect"),
        )
    }

    #[cfg(test)]
    pub(crate) fn test_fixture_with_probe_and_damage_and_rect(
        key: DirectScanoutCandidateKey,
        framebuffer_id: u32,
        surface_damage: Option<SurfaceDamagePresentation>,
        presented_window_rect: PresentationRect,
    ) -> (Self, Arc<std::sync::atomic::AtomicU64>) {
        let (framebuffer, buffer, cleanup_count) =
            super::test_direct_primary_framebuffer(framebuffer_id);
        let imported_buffer_id = framebuffer.key.buffer_id();
        assert_eq!(
            imported_buffer_id.get(),
            key.content.buffer_id.get(),
            "direct test candidate key must describe the imported test buffer",
        );
        (
            Self {
                key,
                validation_key: super::test_validation_key(key.output_generation),
                surface_id: key.content.surface_id,
                root_surface_id: key.content.surface_id,
                buffer_id: imported_buffer_id,
                surface_scene_node_id: SceneNodeId::from_raw(u64::from(key.content.surface_id))
                    .expect("test surface scene node"),
                window_scene_node_id: SceneNodeId::from_raw(u64::from(key.content.surface_id))
                    .expect("test window scene node"),
                presented_window_rect,
                render_generation: 0,
                effect_identity_signature: 0,
                surface_presentation_generation: 1,
                commit_sequence: SurfaceCommitSequence::initial(),
                _buffer: buffer,
                framebuffer,
                surface_damage,
                live_lease_count: Arc::new(AtomicU64::new(1)),
            },
            cleanup_count,
        )
    }
}

impl Drop for DirectPrimaryLease {
    fn drop(&mut self) {
        let _ = self
            .live_lease_count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                Some(count.saturating_sub(1))
            });
    }
}
