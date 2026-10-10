use oblivion_one::{
    native::kms::FramebufferId,
    render_backend::buffer::{
        BufferSize, DmabufBufferHandle, DmabufPlane, DmabufPlaneDescriptor, DrmFormat, DrmModifier,
    },
};
use std::{io, os::fd::BorrowedFd};

use crate::egl_renderer::{BufferAge, render_target_buffer_age};

use super::{
    EXPLICIT_OUTPUT_SLOT_CAPACITY, ExplicitFramebufferDescriptor, ExplicitFramebufferPlane,
    OutputSlotId,
};

/// Reactor/DRM-owned state for one composited output slot.
pub(crate) struct AtomicOutputSlotBacking {
    pub(crate) id: OutputSlotId,
    pub(crate) pool_generation: u64,
    pub(crate) bo: gbm::BufferObject<()>,
    pub(crate) framebuffer: FramebufferId,
    content_validity: AtomicOutputSlotContentValidity,
}

/// Owned setup payload exported once when an output pool is created or replaced.
/// Its DMA-BUF planes keep their file descriptors alive through `Arc<OwnedFd>`.
#[derive(Debug)]
pub(crate) struct AtomicRenderSlotDescriptor {
    pub(crate) id: OutputSlotId,
    pub(crate) pool_generation: u64,
    pub(crate) buffer: DmabufBufferHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RenderTargetRequest {
    pub(crate) slot: OutputSlotId,
    pub(crate) pool_generation: u64,
    pub(crate) buffer_age: BufferAge,
}

#[allow(dead_code)]
fn assert_atomic_render_slot_descriptor_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<AtomicRenderSlotDescriptor>();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AtomicOutputSlotContentValidity {
    // `Some` is valid only while the BO still contains the presentation at
    // this serial. An abandoned render clears the lineage before reuse.
    last_presented_serial: Option<u64>,
}

impl AtomicOutputSlotContentValidity {
    pub(crate) const fn presented_at(last_presented_serial: u64) -> Self {
        Self {
            last_presented_serial: Some(last_presented_serial),
        }
    }

    pub(crate) const fn unpresented() -> Self {
        Self {
            last_presented_serial: None,
        }
    }

    pub(crate) fn buffer_age(&self, presentation_serial: u64) -> BufferAge {
        render_target_buffer_age(presentation_serial, self.last_presented_serial)
    }

    pub(crate) fn invalidate_after_unpresented_render(&mut self) {
        self.last_presented_serial = None;
    }

    pub(crate) fn record_presentation(&mut self, presentation_serial: u64) {
        self.last_presented_serial = Some(presentation_serial);
    }

    pub(crate) const fn last_presented_serial(&self) -> Option<u64> {
        self.last_presented_serial
    }
}

impl AtomicOutputSlotBacking {
    pub(crate) fn new(
        id: OutputSlotId,
        pool_generation: u64,
        bo: gbm::BufferObject<()>,
        framebuffer: FramebufferId,
    ) -> Self {
        Self {
            id,
            pool_generation,
            bo,
            framebuffer,
            content_validity: AtomicOutputSlotContentValidity::unpresented(),
        }
    }

    pub(crate) fn buffer_age(&self, presentation_serial: u64) -> BufferAge {
        self.content_validity.buffer_age(presentation_serial)
    }

    pub(crate) fn invalidate_after_unpresented_render(&mut self) {
        self.content_validity.invalidate_after_unpresented_render();
    }

    pub(crate) fn record_presentation(&mut self, presentation_serial: u64) {
        self.content_validity
            .record_presentation(presentation_serial);
    }

    pub(crate) fn render_descriptor(&self) -> io::Result<AtomicRenderSlotDescriptor> {
        let bo = &self.bo;
        let size = BufferSize::new(bo.width(), bo.height())
            .ok_or_else(|| io::Error::other("explicit output BO dimensions are zero"))?;
        let plane_count = usize::try_from(bo.plane_count())
            .map_err(|_| io::Error::other("explicit output BO plane count overflow"))?;
        if !(1..=4).contains(&plane_count) {
            return Err(io::Error::other(
                "explicit output BO plane count must be between one and four",
            ));
        }
        let modifier = DrmModifier(u64::from(bo.modifier()));
        if modifier == DrmModifier::INVALID {
            return Err(io::Error::other(
                "explicit output BO unexpectedly has DRM_FORMAT_MOD_INVALID",
            ));
        }
        let mut planes = Vec::with_capacity(plane_count);
        for plane_index in 0..plane_count {
            let raw_index = i32::try_from(plane_index)
                .map_err(|_| io::Error::other("explicit output plane index overflow"))?;
            let fd = bo
                .fd_for_plane(raw_index)
                .map_err(|_| io::Error::other("failed to export explicit output plane FD"))?;
            planes.push(DmabufPlane::new(
                fd,
                DmabufPlaneDescriptor {
                    plane_index: u32::try_from(plane_index).unwrap(),
                    offset: bo.offset(raw_index),
                    stride: bo.stride_for_plane(raw_index),
                    modifier,
                },
            ));
        }
        let handle =
            DmabufBufferHandle::new(size, DrmFormat::from_fourcc(bo.format() as u32), planes)
                .map_err(|error| {
                    io::Error::other(format!("invalid explicit output BO: {error:?}"))
                })?;
        Ok(AtomicRenderSlotDescriptor {
            id: self.id,
            pool_generation: self.pool_generation,
            buffer: handle,
        })
    }
}

pub(crate) fn explicit_framebuffer_descriptor(
    bo: &gbm::BufferObject<()>,
) -> io::Result<ExplicitFramebufferDescriptor> {
    let plane_count = usize::try_from(bo.plane_count())
        .map_err(|_| io::Error::other("explicit output BO plane count overflow"))?;
    if !(1..=4).contains(&plane_count) {
        return Err(io::Error::other(
            "explicit output BO plane count must be between one and four",
        ));
    }
    let modifier = u64::from(bo.modifier());
    let mut planes = Vec::with_capacity(plane_count);
    for plane_index in 0..plane_count {
        let raw_index = i32::try_from(plane_index)
            .map_err(|_| io::Error::other("explicit output BO plane index overflow"))?;
        planes.push(ExplicitFramebufferPlane {
            handle: unsafe { bo.handle_for_plane(raw_index).u32_ },
            pitch: bo.stride_for_plane(raw_index),
            offset: bo.offset(raw_index),
            modifier,
        });
    }
    ExplicitFramebufferDescriptor::new(bo.width(), bo.height(), bo.format() as u32, &planes)
}

pub(crate) fn add_explicit_framebuffer(
    drm: BorrowedFd<'_>,
    descriptor: &ExplicitFramebufferDescriptor,
) -> io::Result<FramebufferId> {
    let framebuffer = drm_ffi::mode::add_fb2(
        drm,
        descriptor.width(),
        descriptor.height(),
        descriptor.format(),
        descriptor.handles(),
        descriptor.pitches(),
        descriptor.offsets(),
        descriptor.modifiers(),
        descriptor.flags(),
    )?;
    FramebufferId::new(framebuffer.fb_id)
        .ok_or_else(|| io::Error::other("AddFB2 returned framebuffer ID zero"))
}

pub(crate) struct AtomicPhysicalOutputPool {
    slots: [AtomicOutputSlotBacking; EXPLICIT_OUTPUT_SLOT_CAPACITY],
    pool_generation: u64,
}

impl AtomicPhysicalOutputPool {
    pub(crate) fn validate_slots(
        slots: &[AtomicOutputSlotBacking],
        pool_generation: u64,
    ) -> io::Result<()> {
        let first = slots
            .first()
            .ok_or_else(|| io::Error::other("explicit physical output pool has no slots"))?;
        let identities = slots
            .iter()
            .map(|slot| (slot.id, slot.pool_generation))
            .collect::<Vec<_>>();
        validate_slot_identities(&identities, pool_generation, "physical")?;
        if slots.len() != EXPLICIT_OUTPUT_SLOT_CAPACITY
            || slots.iter().any(|slot| {
                slot.bo.width() != first.bo.width()
                    || slot.bo.height() != first.bo.height()
                    || slot.bo.format() != first.bo.format()
                    || slot.bo.modifier() != first.bo.modifier()
                    || slot.bo.plane_count() != first.bo.plane_count()
            })
        {
            return Err(io::Error::other(
                "explicit physical output pool slots do not share matching metadata",
            ));
        }
        Ok(())
    }

    pub(crate) fn from_validated_slots(
        slots: [AtomicOutputSlotBacking; EXPLICIT_OUTPUT_SLOT_CAPACITY],
        pool_generation: u64,
    ) -> Self {
        Self {
            slots,
            pool_generation,
        }
    }

    pub(crate) fn physical_slot(&self, slot: OutputSlotId) -> io::Result<&AtomicOutputSlotBacking> {
        self.slots
            .iter()
            .find(|candidate| candidate.id == slot)
            .filter(|candidate| candidate.pool_generation == self.pool_generation)
            .ok_or_else(|| io::Error::other("explicit physical output slot is unavailable"))
    }

    pub(crate) fn physical_slot_mut(
        &mut self,
        slot: OutputSlotId,
    ) -> io::Result<&mut AtomicOutputSlotBacking> {
        self.slots
            .iter_mut()
            .find(|candidate| candidate.id == slot)
            .filter(|candidate| candidate.pool_generation == self.pool_generation)
            .ok_or_else(|| io::Error::other("explicit physical output slot is unavailable"))
    }

    pub(crate) fn render_target_request(
        &self,
        slot: OutputSlotId,
        presentation_serial: u64,
    ) -> io::Result<RenderTargetRequest> {
        let physical = self.physical_slot(slot)?;
        Ok(RenderTargetRequest {
            slot,
            pool_generation: physical.pool_generation,
            buffer_age: physical.buffer_age(presentation_serial),
        })
    }

    pub(crate) const fn pool_generation(&self) -> u64 {
        self.pool_generation
    }

    pub(crate) fn plane_count(&self) -> io::Result<u32> {
        self.slots
            .first()
            .map(|slot| slot.bo.plane_count())
            .ok_or_else(|| io::Error::other("explicit physical output pool is unavailable"))
    }

    pub(crate) fn identities(&self) -> Vec<(OutputSlotId, u64)> {
        self.slots
            .iter()
            .map(|slot| (slot.id, slot.pool_generation))
            .collect()
    }

    /// Export owned render-side DMA-BUF descriptors once during pool setup.
    pub(crate) fn export_render_slot_descriptors(
        &self,
    ) -> io::Result<Vec<AtomicRenderSlotDescriptor>> {
        self.slots
            .iter()
            .map(AtomicOutputSlotBacking::render_descriptor)
            .collect()
    }

    pub(crate) fn rebind_pool_generation(&mut self, pool_generation: u64) {
        self.pool_generation = pool_generation;
        for slot in &mut self.slots {
            slot.pool_generation = pool_generation;
        }
    }

    pub(crate) fn teardown(self, drm: BorrowedFd<'_>, drm_cleanup_armed: bool) {
        if drm_cleanup_armed {
            Self::remove_framebuffers(&self.slots, drm);
        }
        drop(self);
    }

    pub(crate) fn remove_framebuffers(slots: &[AtomicOutputSlotBacking], drm: BorrowedFd<'_>) {
        for slot in slots {
            let _ = drm_ffi::mode::rm_fb(drm, slot.framebuffer.get());
        }
    }
}

fn validate_slot_identities(
    identities: &[(OutputSlotId, u64)],
    pool_generation: u64,
    domain: &str,
) -> io::Result<()> {
    if identities.len() != EXPLICIT_OUTPUT_SLOT_CAPACITY {
        return Err(io::Error::other(format!(
            "explicit {domain} output pool has the wrong slot count"
        )));
    }
    let mut ids = std::collections::HashSet::with_capacity(identities.len());
    for (id, generation) in identities {
        if *generation != pool_generation || !ids.insert(id.get()) {
            return Err(io::Error::other(format!(
                "explicit output pool has invalid {domain} slot identity"
            )));
        }
    }
    Ok(())
}

pub(crate) fn validate_split_slot_pairing(
    physical: &[(OutputSlotId, u64)],
    render: &[(OutputSlotId, u64)],
    pool_generation: u64,
) -> io::Result<()> {
    if physical.len() != EXPLICIT_OUTPUT_SLOT_CAPACITY
        || physical.len() != render.len()
        || render.len() != EXPLICIT_OUTPUT_SLOT_CAPACITY
    {
        return Err(io::Error::other(
            "explicit output pool physical/render slot counts differ",
        ));
    }

    validate_slot_identities(physical, pool_generation, "physical")?;
    validate_slot_identities(render, pool_generation, "render")?;

    if physical
        .iter()
        .any(|(id, generation)| !render.contains(&(*id, *generation)))
    {
        return Err(io::Error::other(
            "explicit output pool render slots do not pair by ID and generation",
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn teardown_split_slot_resources<R, P>(
    render_slots: &[R],
    physical_slots: &[P],
    mut delete_render_resources: impl FnMut(&R),
    mut destroy_render_images: impl FnMut(&R),
    mut remove_physical_framebuffers: impl FnMut(&P),
) {
    for slot in render_slots {
        delete_render_resources(slot);
    }
    for slot in render_slots {
        destroy_render_images(slot);
    }
    for slot in physical_slots {
        remove_physical_framebuffers(slot);
    }
}

pub(crate) fn remove_framebuffer_after_render_import_failure<T>(
    render_import: io::Result<T>,
    remove_framebuffer: impl FnOnce(),
) -> io::Result<T> {
    match render_import {
        Ok(render_slot) => Ok(render_slot),
        Err(error) => {
            remove_framebuffer();
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::File,
        os::fd::{AsRawFd, OwnedFd},
    };

    use super::*;

    fn id(value: u8) -> OutputSlotId {
        OutputSlotId::new(value).expect("test slot ID is in range")
    }

    fn test_dmabuf() -> (DmabufBufferHandle, i32) {
        let fd: OwnedFd = File::open("/dev/null").expect("open test fd").into();
        let raw_fd = fd.as_raw_fd();
        let handle = DmabufBufferHandle::new(
            BufferSize::new(8, 8).expect("test buffer dimensions"),
            DrmFormat::Xrgb8888,
            vec![DmabufPlane::new(
                fd,
                DmabufPlaneDescriptor {
                    plane_index: 0,
                    offset: 0,
                    stride: 32,
                    modifier: DrmModifier::LINEAR,
                },
            )],
        )
        .expect("valid test DMA-BUF handle");
        (handle, raw_fd)
    }

    #[test]
    fn render_slot_descriptor_is_send_and_retains_plane_fd_ownership() {
        fn assert_send<T: Send>() {}
        assert_send::<AtomicRenderSlotDescriptor>();

        let (buffer, raw_fd) = test_dmabuf();
        let descriptor = AtomicRenderSlotDescriptor {
            id: id(1),
            pool_generation: 9,
            buffer,
        };
        let retained_buffer = descriptor.buffer.clone();
        drop(descriptor);

        assert_ne!(unsafe { libc::fcntl(raw_fd, libc::F_GETFD) }, -1);
        drop(retained_buffer);
        assert_eq!(unsafe { libc::fcntl(raw_fd, libc::F_GETFD) }, -1);
    }

    #[test]
    fn split_pool_pairing_rejects_mismatched_slot_ids() {
        assert!(
            validate_split_slot_pairing(
                &[(id(0), 5), (id(1), 5), (id(2), 5)],
                &[(id(1), 5), (id(0), 5), (id(2), 5)],
                5,
            )
            .is_ok()
        );
        assert!(
            validate_split_slot_pairing(
                &[(id(0), 5), (id(1), 5), (id(2), 5)],
                &[(id(0), 5), (id(2), 5), (id(2), 5)],
                5,
            )
            .is_err()
        );
    }

    #[test]
    fn split_pool_pairing_rejects_mismatched_generations() {
        assert!(
            validate_split_slot_pairing(
                &[(id(0), 5), (id(1), 5), (id(2), 5)],
                &[(id(0), 5), (id(1), 6), (id(2), 5)],
                5,
            )
            .is_err()
        );
    }

    #[test]
    fn session_recovery_rebinds_matching_physical_and_render_generations() {
        let physical = [(id(0), 5), (id(1), 5), (id(2), 5)];
        let render = [(id(0), 5), (id(1), 5), (id(2), 5)];
        let physical_rebound = [(id(0), 6), (id(1), 6), (id(2), 6)];
        let render_rebound = [(id(0), 6), (id(1), 6), (id(2), 6)];

        assert!(validate_split_slot_pairing(&physical, &render, 5).is_ok());
        assert!(validate_split_slot_pairing(&physical_rebound, &render_rebound, 6).is_ok());
        assert!(validate_split_slot_pairing(&physical_rebound, &render, 6).is_err());
    }

    #[test]
    fn physical_content_validity_owns_buffer_age_and_presentation_serial() {
        let mut validity = AtomicOutputSlotContentValidity::presented_at(4);
        assert_eq!(validity.buffer_age(6), BufferAge::Value(3));
        assert_eq!(validity.last_presented_serial(), Some(4));

        validity.invalidate_after_unpresented_render();
        assert_eq!(validity.buffer_age(6), BufferAge::Value(0));
        assert_eq!(validity.last_presented_serial(), None);

        validity.record_presentation(8);
        assert_eq!(validity.last_presented_serial(), Some(8));
    }

    #[test]
    fn physical_framebuffer_is_removed_when_render_import_fails() {
        let mut removed = false;
        let result = remove_framebuffer_after_render_import_failure::<()>(
            Err(io::Error::other("injected render import failure")),
            || removed = true,
        );

        assert!(result.is_err());
        assert!(removed);
    }
}
