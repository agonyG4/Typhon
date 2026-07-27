use std::{io, sync::Arc};

use oblivion_one::compositor::{DirectScanoutSceneCandidate, SurfaceDamagePresentation};
use oblivion_one::render_backend::buffer::DmabufBufferHandle;

use super::{DirectPlaneValidationKey, DirectScanoutCandidateKey, ImportedDirectFramebuffer};

#[derive(Debug)]
pub(crate) struct DirectPrimaryLease {
    key: DirectScanoutCandidateKey,
    validation_key: DirectPlaneValidationKey,
    surface_id: u32,
    _buffer: DmabufBufferHandle,
    framebuffer: Arc<ImportedDirectFramebuffer>,
    surface_damage: Option<SurfaceDamagePresentation>,
}

pub(crate) type DirectPrimaryLeaseParts = (
    DirectScanoutCandidateKey,
    u32,
    DmabufBufferHandle,
    Arc<ImportedDirectFramebuffer>,
    SurfaceDamagePresentation,
);
pub(crate) type DirectPrimaryLeaseTransferError = Box<(io::Error, DirectPrimaryLease)>;

impl DirectPrimaryLease {
    pub(crate) fn new(
        candidate: DirectScanoutSceneCandidate,
        key: DirectScanoutCandidateKey,
        validation_key: DirectPlaneValidationKey,
        framebuffer: Arc<ImportedDirectFramebuffer>,
        surface_damage: SurfaceDamagePresentation,
    ) -> Self {
        Self {
            key,
            validation_key,
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

    pub(crate) const fn has_surface_damage(&self) -> bool {
        self.surface_damage.is_some()
    }

    pub(crate) fn try_into_parts(
        mut self,
    ) -> Result<DirectPrimaryLeaseParts, DirectPrimaryLeaseTransferError> {
        if !self.has_surface_damage() {
            return Err(Box::new((
                io::Error::other("direct surface damage already settled"),
                self,
            )));
        }
        let surface_damage = self
            .surface_damage
            .take()
            .expect("surface damage checked above");
        let Self {
            key,
            surface_id,
            _buffer,
            framebuffer,
            surface_damage: None,
            ..
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
        Self::test_fixture_with_probe_and_damage(key, framebuffer_id, None)
    }

    #[cfg(test)]
    pub(crate) fn test_fixture_with_probe_and_damage(
        key: DirectScanoutCandidateKey,
        framebuffer_id: u32,
        surface_damage: Option<SurfaceDamagePresentation>,
    ) -> (Self, Arc<std::sync::atomic::AtomicU64>) {
        let (framebuffer, buffer, cleanup_count) =
            super::test_direct_primary_framebuffer(framebuffer_id);
        (
            Self {
                key,
                validation_key: super::test_validation_key(key.output_generation),
                surface_id: key.content.surface_id,
                _buffer: buffer,
                framebuffer,
                surface_damage,
            },
            cleanup_count,
        )
    }
}
