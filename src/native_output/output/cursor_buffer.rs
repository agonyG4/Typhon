//! Cursor framebuffer allocation, upload, leasing, caching, and retirement.

use super::*;
use oblivion_one::cursor_theme::CompositorCursorImage;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeCursorSourceKey {
    Theme,
    Client(NativeCursorImageKey),
}

#[derive(Debug)]
pub(super) struct AtomicCursorBuffer {
    pub(super) fd: RawFd,
    pub(super) handle: u32,
    pub(super) framebuffer: FramebufferId,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) pitch: u32,
    pub(super) size: usize,
    pub(super) mapping: *mut c_void,
    pub(super) drm_cleanup_armed: bool,
    /// Shared only as a lightweight lease marker. The DRM/CPU buffer itself
    /// remains compositor-thread-owned; worker jobs carry a clone of this
    /// marker so retirement cannot drop the buffer while a queued ioctl may
    /// still reference its framebuffer ID.
    pub(super) lease: Arc<()>,
}

impl AtomicCursorBuffer {
    pub(super) fn create(file: &fs::File, width: u32, height: u32) -> io::Result<Self> {
        let dumb = drm_ffi::mode::dumbbuffer::create(file.as_fd(), width, height, 32, 0)?;
        let descriptor = ExplicitFramebufferDescriptor::new(
            width,
            height,
            DRM_FORMAT_ARGB8888,
            &[ExplicitFramebufferPlane {
                handle: dumb.handle,
                pitch: dumb.pitch,
                offset: 0,
                modifier: 0,
            }],
        )?;
        let framebuffer = match add_explicit_framebuffer(file.as_fd(), &descriptor) {
            Ok(framebuffer) => framebuffer,
            Err(error) => {
                let _ = drm_ffi::mode::dumbbuffer::destroy(file.as_fd(), dumb.handle);
                return Err(error);
            }
        };
        let map = match drm_ffi::mode::dumbbuffer::map(file.as_fd(), dumb.handle, 0, 0) {
            Ok(map) => map,
            Err(error) => {
                let _ = drm_ffi::mode::rm_fb(file.as_fd(), framebuffer.get());
                let _ = drm_ffi::mode::dumbbuffer::destroy(file.as_fd(), dumb.handle);
                return Err(error);
            }
        };
        let size = match usize::try_from(dumb.size) {
            Ok(size) => size,
            Err(_) => {
                let _ = drm_ffi::mode::rm_fb(file.as_fd(), framebuffer.get());
                let _ = drm_ffi::mode::dumbbuffer::destroy(file.as_fd(), dumb.handle);
                return Err(io::Error::other("Atomic cursor dumb buffer size overflow"));
            }
        };
        let mapping = unsafe {
            libc::mmap(
                ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                map.offset as libc::off_t,
            )
        };
        if mapping == libc::MAP_FAILED {
            let error = io::Error::last_os_error();
            let _ = drm_ffi::mode::rm_fb(file.as_fd(), framebuffer.get());
            let _ = drm_ffi::mode::dumbbuffer::destroy(file.as_fd(), dumb.handle);
            return Err(error);
        }
        Ok(Self {
            fd: file.as_raw_fd(),
            handle: dumb.handle,
            framebuffer,
            width,
            height,
            pitch: dumb.pitch,
            size,
            mapping,
            drm_cleanup_armed: true,
            lease: Arc::new(()),
        })
    }

    pub(super) fn upload_image(&mut self, image: &CompositorCursorImage) -> io::Result<()> {
        let bytes = native_cursor_argb_bytes(
            &image.pixels_argb8888,
            image.width,
            image.height,
            self.width,
            self.height,
            self.pitch,
        )?;
        let destination =
            unsafe { slice::from_raw_parts_mut(self.mapping.cast::<u8>(), self.size) };
        destination.copy_from_slice(&bytes);
        Ok(())
    }

    fn disarm_drm_cleanup(&mut self) {
        self.drm_cleanup_armed = false;
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CursorFramebufferPin {
    framebuffer: FramebufferId,
    #[allow(dead_code)]
    lease: Arc<()>,
}

impl CursorFramebufferPin {
    pub(crate) fn framebuffer_id(&self) -> FramebufferId {
        self.framebuffer
    }

    #[cfg(test)]
    pub(crate) fn is_job_owned(&self) -> bool {
        Arc::strong_count(&self.lease) > 1
    }
}

impl Drop for AtomicCursorBuffer {
    fn drop(&mut self) {
        if self.drm_cleanup_armed {
            let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
            let _ = drm_ffi::mode::rm_fb(fd, self.framebuffer.get());
        }
        let _ = unsafe { libc::munmap(self.mapping, self.size) };
        if self.drm_cleanup_armed {
            let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
            let _ = drm_ffi::mode::dumbbuffer::destroy(fd, self.handle);
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct AtomicCursorResources {
    pub(super) current: Option<AtomicCursorBuffer>,
    pub(super) retired: Vec<AtomicCursorBuffer>,
    pub(super) theme_cache: Option<AtomicCursorBuffer>,
    pub(super) client_cache: Option<(NativeCursorImageKey, AtomicCursorBuffer)>,
}

impl AtomicCursorResources {
    pub(super) fn take_cached(
        &mut self,
        source_key: NativeCursorSourceKey,
    ) -> Option<AtomicCursorBuffer> {
        match source_key {
            NativeCursorSourceKey::Theme => self.theme_cache.take(),
            NativeCursorSourceKey::Client(key) => self
                .client_cache
                .take()
                .and_then(|(cached_key, buffer)| (cached_key == key).then_some(buffer)),
        }
    }

    pub(super) fn cache_current(
        &mut self,
        source_key: NativeCursorSourceKey,
        buffer: AtomicCursorBuffer,
    ) {
        match source_key {
            NativeCursorSourceKey::Theme => {
                if let Some(previous) = self.theme_cache.replace(buffer) {
                    self.retired.push(previous);
                }
            }
            NativeCursorSourceKey::Client(key) => {
                if let Some((previous_key, previous)) = self.client_cache.replace((key, buffer))
                    && previous_key != key
                {
                    self.retired.push(previous);
                }
            }
        }
    }

    pub(super) fn retire_cached_mismatch(&mut self, source_key: NativeCursorSourceKey) {
        if let NativeCursorSourceKey::Client(key) = source_key
            && self
                .client_cache
                .as_ref()
                .is_some_and(|(cached_key, _)| *cached_key != key)
            && let Some((_, buffer)) = self.client_cache.take()
        {
            self.retired.push(buffer);
        }
    }

    pub(super) fn retire_safe(&mut self, keep: &[Option<u32>]) {
        self.retired.retain(|buffer| {
            Arc::strong_count(&buffer.lease) > 1
                || keep
                    .iter()
                    .flatten()
                    .any(|framebuffer| *framebuffer == buffer.framebuffer.get())
        });
    }

    pub(super) fn pin_framebuffer(
        &self,
        framebuffer: FramebufferId,
    ) -> Option<CursorFramebufferPin> {
        let matches = |buffer: &AtomicCursorBuffer| buffer.framebuffer == framebuffer;
        self.current
            .as_ref()
            .filter(|buffer| matches(buffer))
            .or_else(|| self.theme_cache.as_ref().filter(|buffer| matches(buffer)))
            .or_else(|| {
                self.client_cache
                    .as_ref()
                    .map(|(_, buffer)| buffer)
                    .filter(|buffer| matches(buffer))
            })
            .or_else(|| self.retired.iter().find(|buffer| matches(buffer)))
            .map(|buffer| CursorFramebufferPin {
                framebuffer,
                lease: Arc::clone(&buffer.lease),
            })
    }

    pub(super) fn disarm_drm_cleanup(&mut self) {
        if let Some(current) = self.current.as_mut() {
            current.disarm_drm_cleanup();
        }
        if let Some(theme) = self.theme_cache.as_mut() {
            theme.disarm_drm_cleanup();
        }
        if let Some((_, client)) = self.client_cache.as_mut() {
            client.disarm_drm_cleanup();
        }
        for retired in &mut self.retired {
            retired.disarm_drm_cleanup();
        }
    }
}
