use std::{
    collections::HashMap,
    io,
    os::fd::{AsRawFd, BorrowedFd, RawFd},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use oblivion_one::{
    native::{drm, kms::FramebufferId},
    render_backend::buffer::{
        BufferIdentity, DmabufBufferHandle, DmabufImageKey, DrmFormat, DrmModifier,
        WeakBufferIdentity,
    },
};

use super::{ExplicitFramebufferDescriptor, ExplicitFramebufferPlane, add_explicit_framebuffer};

pub(crate) trait DirectFramebufferIo: Send + Sync {
    fn prime_fd_to_handle(&self, dma_buf: BorrowedFd<'_>) -> io::Result<u32>;
    fn add_framebuffer(
        &self,
        descriptor: &ExplicitFramebufferDescriptor,
    ) -> io::Result<FramebufferId>;
    fn remove_framebuffer(&self, framebuffer: FramebufferId) -> io::Result<()>;
    fn gem_close(&self, handle: u32) -> io::Result<()>;
}

#[derive(Debug)]
struct RealDirectFramebufferIo {
    drm_fd: RawFd,
}

impl DirectFramebufferIo for RealDirectFramebufferIo {
    fn prime_fd_to_handle(&self, dma_buf: BorrowedFd<'_>) -> io::Result<u32> {
        let drm = unsafe { BorrowedFd::borrow_raw(self.drm_fd) };
        drm::prime_fd_to_handle(drm, dma_buf)
    }

    fn add_framebuffer(
        &self,
        descriptor: &ExplicitFramebufferDescriptor,
    ) -> io::Result<FramebufferId> {
        let drm = unsafe { BorrowedFd::borrow_raw(self.drm_fd) };
        add_explicit_framebuffer(drm, descriptor)
    }

    fn remove_framebuffer(&self, framebuffer: FramebufferId) -> io::Result<()> {
        let drm = unsafe { BorrowedFd::borrow_raw(self.drm_fd) };
        drm_ffi::mode::rm_fb(drm, framebuffer.get()).map(|_| ())
    }

    fn gem_close(&self, handle: u32) -> io::Result<()> {
        let drm = unsafe { BorrowedFd::borrow_raw(self.drm_fd) };
        drm::gem_close(drm, handle)
    }
}

pub(crate) struct ImportedDirectFramebuffer {
    pub(crate) key: DmabufImageKey,
    pub(crate) framebuffer: FramebufferId,
    pub(crate) format: u32,
    pub(crate) modifier: u64,
    gem_handles: Vec<u32>,
    drm_cleanup_armed: AtomicBool,
    io: Arc<dyn DirectFramebufferIo>,
    cleanup_failures: Arc<AtomicU64>,
}

impl std::fmt::Debug for ImportedDirectFramebuffer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImportedDirectFramebuffer")
            .field("key", &self.key)
            .field("framebuffer", &self.framebuffer)
            .field("format", &self.format)
            .field("modifier", &self.modifier)
            .field("gem_handles", &self.gem_handles)
            .finish()
    }
}

impl ImportedDirectFramebuffer {
    fn import(
        io: Arc<dyn DirectFramebufferIo>,
        cleanup_failures: Arc<AtomicU64>,
        identity: &BufferIdentity,
        buffer: &DmabufBufferHandle,
    ) -> io::Result<Self> {
        validate_direct_dma_buf(buffer)?;
        let key = DmabufImageKey::from_handle(identity.id(), buffer);
        let mut planes = Vec::with_capacity(buffer.planes().len());
        let mut gem_handles = Vec::with_capacity(buffer.planes().len());

        for plane in buffer.planes() {
            let descriptor = plane.descriptor();
            let handle = match io.prime_fd_to_handle(plane.fd()) {
                Ok(handle) => handle,
                Err(error) => {
                    close_gem_handles(&io, &gem_handles);
                    return Err(error);
                }
            };
            if !gem_handles.contains(&handle) {
                gem_handles.push(handle);
            }
            planes.push(ExplicitFramebufferPlane {
                handle,
                pitch: descriptor.stride,
                offset: descriptor.offset,
                modifier: descriptor.modifier.0,
            });
        }

        let descriptor = match ExplicitFramebufferDescriptor::new(
            buffer.size().width,
            buffer.size().height,
            buffer.format().as_fourcc(),
            &planes,
        ) {
            Ok(descriptor) => descriptor,
            Err(error) => {
                close_gem_handles(&io, &gem_handles);
                return Err(error);
            }
        };
        let framebuffer = match io.add_framebuffer(&descriptor) {
            Ok(framebuffer) => framebuffer,
            Err(error) => {
                close_gem_handles(&io, &gem_handles);
                return Err(error);
            }
        };
        if framebuffer.get() == 0 {
            close_gem_handles(&io, &gem_handles);
            return Err(io::Error::other("AddFB2 returned framebuffer ID zero"));
        }

        Ok(Self {
            key,
            framebuffer,
            format: buffer.format().as_fourcc(),
            modifier: buffer.planes()[0].descriptor().modifier.0,
            gem_handles,
            drm_cleanup_armed: AtomicBool::new(true),
            io,
            cleanup_failures,
        })
    }

    pub(crate) fn disarm_drm_cleanup(&self) {
        self.drm_cleanup_armed.store(false, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn gem_handles(&self) -> &[u32] {
        &self.gem_handles
    }
}

impl Drop for ImportedDirectFramebuffer {
    fn drop(&mut self) {
        if !self.drm_cleanup_armed.load(Ordering::Acquire) {
            return;
        }
        // KMS must stop referring to the framebuffer before its imported GEM
        // handles are closed. Keep this order even when one cleanup operation
        // fails so the remaining resources are still attempted.
        if let Err(error) = self.io.remove_framebuffer(self.framebuffer) {
            self.cleanup_failures.fetch_add(1, Ordering::Relaxed);
            eprintln!("direct scanout: failed to remove framebuffer: {error}");
        }
        for handle in &self.gem_handles {
            if let Err(error) = self.io.gem_close(*handle) {
                self.cleanup_failures.fetch_add(1, Ordering::Relaxed);
                eprintln!("direct scanout: failed to close GEM handle {handle}: {error}");
            }
        }
    }
}

fn close_gem_handles(io: &Arc<dyn DirectFramebufferIo>, handles: &[u32]) {
    for handle in handles {
        let _ = io.gem_close(*handle);
    }
}

pub(crate) fn validate_direct_dma_buf(buffer: &DmabufBufferHandle) -> io::Result<()> {
    let planes = buffer.planes();
    if !(1..=4).contains(&planes.len()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "direct scanout requires one to four dma-buf planes",
        ));
    }
    let first_modifier = planes[0].descriptor().modifier;
    if first_modifier == DrmModifier::INVALID {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "direct scanout rejects DRM_FORMAT_MOD_INVALID",
        ));
    }
    for (index, plane) in planes.iter().enumerate() {
        let descriptor = plane.descriptor();
        if descriptor.plane_index != u32::try_from(index).unwrap_or(u32::MAX) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "direct scanout dma-buf plane indices must be contiguous",
            ));
        }
        if descriptor.stride == 0
            || descriptor.modifier == DrmModifier::INVALID
            || descriptor.offset % 4 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "direct scanout dma-buf plane has invalid stride or modifier",
            ));
        }
        if descriptor.modifier != first_modifier {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "direct scanout dma-buf modifiers must agree",
            ));
        }
    }
    if buffer.format() != DrmFormat::Xrgb8888 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "direct scanout importer only accepts XRGB8888",
        ));
    }
    if planes.len() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "XRGB8888 direct scanout must have one plane",
        ));
    }
    let descriptor = planes[0].descriptor();
    let minimum_stride = buffer
        .size()
        .width
        .checked_mul(4)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "XRGB stride overflow"))?;
    if descriptor.offset != 0 || descriptor.stride < minimum_stride {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "impossible XRGB8888 dma-buf layout",
        ));
    }
    Ok(())
}

struct DirectFramebufferCacheEntry {
    framebuffer: Arc<ImportedDirectFramebuffer>,
    identity: WeakBufferIdentity,
    last_used: u64,
}

pub(crate) struct DirectFramebufferCache {
    entries: HashMap<DmabufImageKey, DirectFramebufferCacheEntry>,
    capacity: usize,
    drm_generation: u64,
    io: Arc<dyn DirectFramebufferIo>,
    use_counter: u64,
    cleanup_failures: Arc<AtomicU64>,
}

impl DirectFramebufferCache {
    pub(crate) fn new(drm: BorrowedFd<'_>, drm_generation: u64) -> Self {
        Self::with_io(
            Arc::new(RealDirectFramebufferIo {
                drm_fd: drm.as_raw_fd(),
            }),
            drm_generation,
        )
    }

    pub(crate) fn with_io(io: Arc<dyn DirectFramebufferIo>, drm_generation: u64) -> Self {
        Self {
            entries: HashMap::new(),
            capacity: 64,
            drm_generation,
            io,
            use_counter: 0,
            cleanup_failures: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(crate) fn get_or_import(
        &mut self,
        identity: &BufferIdentity,
        buffer: &DmabufBufferHandle,
    ) -> io::Result<(Arc<ImportedDirectFramebuffer>, bool)> {
        let key = DmabufImageKey::from_handle(identity.id(), buffer);
        self.use_counter = self.use_counter.wrapping_add(1);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.last_used = self.use_counter;
            return Ok((Arc::clone(&entry.framebuffer), true));
        }
        if !self.evict_if_needed() {
            return Err(io::Error::other(
                "direct scanout framebuffer cache is full of live frames",
            ));
        }
        let framebuffer = Arc::new(ImportedDirectFramebuffer::import(
            Arc::clone(&self.io),
            Arc::clone(&self.cleanup_failures),
            identity,
            buffer,
        )?);
        self.entries.insert(
            key,
            DirectFramebufferCacheEntry {
                framebuffer: Arc::clone(&framebuffer),
                identity: identity.downgrade(),
                last_used: self.use_counter,
            },
        );
        Ok((framebuffer, false))
    }

    fn evict_if_needed(&mut self) -> bool {
        self.entries.retain(|_, entry| {
            Arc::strong_count(&entry.framebuffer) != 1 || entry.identity.is_alive()
        });
        while self.entries.len() >= self.capacity {
            let Some(key) = self
                .entries
                .iter()
                .filter(|(_, entry)| Arc::strong_count(&entry.framebuffer) == 1)
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            else {
                return false;
            };
            self.entries.remove(&key);
        }
        true
    }

    pub(crate) fn clear_for_generation(&mut self, drm_generation: u64) {
        if self.drm_generation == drm_generation {
            return;
        }
        let mut entries = self
            .entries
            .drain()
            .map(|(_, entry)| entry)
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.last_used);
        for entry in entries {
            drop(entry);
        }
        self.drm_generation = drm_generation;
    }

    pub(crate) fn disarm_drm_cleanup(&mut self) {
        for entry in self.entries.values() {
            entry.framebuffer.disarm_drm_cleanup();
        }
    }

    pub(crate) fn clear_disarmed(&mut self) {
        self.disarm_drm_cleanup();
        self.entries.clear();
    }

    pub(crate) fn cleanup_failures(&self) -> u64 {
        self.cleanup_failures.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::fd::OwnedFd, sync::Mutex};

    #[derive(Default)]
    struct FakeIo {
        next_handle: Mutex<u32>,
        next_fb: Mutex<u32>,
        events: Mutex<Vec<String>>,
        fail_add: Mutex<bool>,
    }

    impl DirectFramebufferIo for FakeIo {
        fn prime_fd_to_handle(&self, _dma_buf: BorrowedFd<'_>) -> io::Result<u32> {
            let mut next = self.next_handle.lock().unwrap();
            *next = next.saturating_add(1).max(1);
            Ok(*next)
        }

        fn add_framebuffer(
            &self,
            _descriptor: &ExplicitFramebufferDescriptor,
        ) -> io::Result<FramebufferId> {
            if *self.fail_add.lock().unwrap() {
                return Err(io::Error::other("injected AddFB2 failure"));
            }
            let mut next = self.next_fb.lock().unwrap();
            *next = next.saturating_add(1).max(1);
            self.events.lock().unwrap().push("add_fb".into());
            Ok(FramebufferId::new(*next).unwrap())
        }

        fn remove_framebuffer(&self, _framebuffer: FramebufferId) -> io::Result<()> {
            self.events.lock().unwrap().push("rm_fb".into());
            Ok(())
        }

        fn gem_close(&self, _handle: u32) -> io::Result<()> {
            self.events.lock().unwrap().push("gem_close".into());
            Ok(())
        }
    }

    fn test_buffer(_identity: &BufferIdentity) -> DmabufBufferHandle {
        let size = oblivion_one::render_backend::buffer::BufferSize::new(4, 4).unwrap();
        DmabufBufferHandle::new(
            size,
            DrmFormat::Xrgb8888,
            vec![oblivion_one::render_backend::buffer::DmabufPlane::new(
                OwnedFd::from(std::fs::File::open("/dev/null").unwrap()),
                oblivion_one::render_backend::buffer::DmabufPlaneDescriptor {
                    plane_index: 0,
                    offset: 0,
                    stride: 16,
                    modifier: DrmModifier::LINEAR,
                },
            )],
        )
        .unwrap()
    }

    #[test]
    fn duplicate_gem_handles_are_closed_once_and_rm_fb_precedes_close() {
        let mut ids = oblivion_one::render_backend::buffer::BufferIdAllocator::default();
        let identity = ids.allocate().unwrap();
        let io = Arc::new(FakeIo::default());
        let buffer = test_buffer(&identity);
        let imported = ImportedDirectFramebuffer::import(
            io.clone(),
            Arc::new(AtomicU64::new(0)),
            &identity,
            &buffer,
        )
        .unwrap();
        assert_eq!(imported.gem_handles(), &[1]);
        drop(imported);
        let events = io.events.lock().unwrap().clone();
        assert_eq!(events, ["add_fb", "rm_fb", "gem_close"]);
    }

    #[test]
    fn cache_hits_and_generation_changes_clean_cache_in_order() {
        let mut ids = oblivion_one::render_backend::buffer::BufferIdAllocator::default();
        let identity = ids.allocate().unwrap();
        let io = Arc::new(FakeIo::default());
        let mut cache = DirectFramebufferCache::with_io(io.clone(), 1);
        let buffer = test_buffer(&identity);
        assert!(!cache.get_or_import(&identity, &buffer).unwrap().1);
        assert!(cache.get_or_import(&identity, &buffer).unwrap().1);
        assert_eq!(cache.len(), 1);
        cache.clear_for_generation(2);
        assert_eq!(cache.len(), 0);
        assert_eq!(*io.events.lock().unwrap(), ["add_fb", "rm_fb", "gem_close"]);
    }

    #[test]
    fn partial_addfb2_failure_closes_imported_gem_handles() {
        let mut ids = oblivion_one::render_backend::buffer::BufferIdAllocator::default();
        let identity = ids.allocate().unwrap();
        let io = Arc::new(FakeIo {
            fail_add: Mutex::new(true),
            ..Default::default()
        });
        let buffer = test_buffer(&identity);
        let result = ImportedDirectFramebuffer::import(
            io.clone(),
            Arc::new(AtomicU64::new(0)),
            &identity,
            &buffer,
        );
        assert!(result.is_err());
        assert_eq!(*io.events.lock().unwrap(), ["gem_close"]);
    }

    #[test]
    fn dead_identity_is_evicted_before_a_new_cache_entry() {
        let mut ids = oblivion_one::render_backend::buffer::BufferIdAllocator::default();
        let io = Arc::new(FakeIo::default());
        let mut cache = DirectFramebufferCache::with_io(io, 1);
        {
            let identity = ids.allocate().unwrap();
            let buffer = test_buffer(&identity);
            let _ = cache.get_or_import(&identity, &buffer).unwrap();
        }
        let identity = ids.allocate().unwrap();
        let buffer = test_buffer(&identity);
        assert!(!cache.get_or_import(&identity, &buffer).unwrap().1);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn current_or_pending_arc_pins_cache_entry_from_eviction() {
        let mut ids = oblivion_one::render_backend::buffer::BufferIdAllocator::default();
        let io = Arc::new(FakeIo::default());
        let mut cache = DirectFramebufferCache::with_io(io, 1);
        cache.capacity = 1;
        let first_identity = ids.allocate().unwrap();
        let first_buffer = test_buffer(&first_identity);
        let pinned = cache
            .get_or_import(&first_identity, &first_buffer)
            .unwrap()
            .0;
        let second_identity = ids.allocate().unwrap();
        let second_buffer = test_buffer(&second_identity);
        assert!(
            cache
                .get_or_import(&second_identity, &second_buffer)
                .is_err()
        );
        drop(pinned);
        assert!(
            cache
                .get_or_import(&second_identity, &second_buffer)
                .is_ok()
        );
    }
}
