use std::{io, sync::Arc};

use oblivion_one::compositor::{DirectScanoutSceneCandidate, SurfaceDamagePresentation};
use oblivion_one::render_backend::buffer::DmabufBufferHandle;

use super::{DirectScanoutCandidateKey, ImportedDirectFramebuffer};

#[derive(Debug)]
pub(crate) struct DirectPrimaryLease {
    key: DirectScanoutCandidateKey,
    surface_id: u32,
    _buffer: DmabufBufferHandle,
    framebuffer: Arc<ImportedDirectFramebuffer>,
    surface_damage: Option<SurfaceDamagePresentation>,
}

impl DirectPrimaryLease {
    pub(crate) fn new(
        candidate: DirectScanoutSceneCandidate,
        key: DirectScanoutCandidateKey,
        framebuffer: Arc<ImportedDirectFramebuffer>,
        surface_damage: SurfaceDamagePresentation,
    ) -> Self {
        Self {
            key,
            surface_id: candidate.surface_id,
            _buffer: candidate.buffer,
            framebuffer,
            surface_damage: Some(surface_damage),
        }
    }

    pub(crate) const fn key(&self) -> DirectScanoutCandidateKey {
        self.key
    }

    pub(crate) const fn surface_id(&self) -> u32 {
        self.surface_id
    }

    pub(crate) fn framebuffer_id(&self) -> u32 {
        self.framebuffer.framebuffer.get()
    }

    pub(crate) fn take_surface_damage(&mut self) -> io::Result<SurfaceDamagePresentation> {
        self.surface_damage
            .take()
            .ok_or_else(|| io::Error::other("direct surface damage already settled"))
    }

    pub(crate) const fn has_surface_damage(&self) -> bool {
        self.surface_damage.is_some()
    }

    pub(crate) fn into_parts(
        mut self,
    ) -> io::Result<(
        DirectScanoutCandidateKey,
        u32,
        DmabufBufferHandle,
        Arc<ImportedDirectFramebuffer>,
        SurfaceDamagePresentation,
    )> {
        if !self.has_surface_damage() {
            return Err(io::Error::other("direct surface damage already settled"));
        }
        let surface_damage = self.take_surface_damage()?;
        let Self {
            key,
            surface_id,
            _buffer,
            framebuffer,
            surface_damage: None,
        } = self
        else {
            unreachable!("surface damage was consumed above");
        };
        Ok((key, surface_id, _buffer, framebuffer, surface_damage))
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
        let (framebuffer, buffer, cleanup_count) =
            super::test_direct_primary_framebuffer(framebuffer_id);
        (
            Self {
                key,
                surface_id: key.content.surface_id,
                _buffer: buffer,
                framebuffer,
                surface_damage: None,
            },
            cleanup_count,
        )
    }
}
