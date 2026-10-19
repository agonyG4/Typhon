//! Cursor framebuffer allocation, upload, leasing, caching, and retirement.

use super::*;
use oblivion_one::cursor_theme::CompositorCursorImage;
use std::collections::VecDeque;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CursorFramebufferLifecycleEvent {
    #[cfg(test)]
    BeforeFramebufferAcquired,
    AddFb2Succeeded,
    SetupFailedAfterAddFb2 {
        rm_fb_succeeded: bool,
    },
    ArmedOwnerDrop {
        rm_fb_succeeded: bool,
    },
    DisarmedOwnerDrop,
    CursorBackingReady,
    CursorBackingDrop,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CursorFramebufferLifecyclePlan {
    drm_allocations: u8,
    drm_retirements_via_remove: u8,
    drm_retirements_without_remove: u8,
    remove_framebuffer: bool,
    cursor_backing_allocations: u8,
    cursor_backing_retirements: u8,
}

const fn cursor_framebuffer_lifecycle_plan(
    event: CursorFramebufferLifecycleEvent,
) -> CursorFramebufferLifecyclePlan {
    match event {
        #[cfg(test)]
        CursorFramebufferLifecycleEvent::BeforeFramebufferAcquired => {
            CursorFramebufferLifecyclePlan {
                drm_allocations: 0,
                drm_retirements_via_remove: 0,
                drm_retirements_without_remove: 0,
                remove_framebuffer: false,
                cursor_backing_allocations: 0,
                cursor_backing_retirements: 0,
            }
        }
        CursorFramebufferLifecycleEvent::AddFb2Succeeded => CursorFramebufferLifecyclePlan {
            drm_allocations: 1,
            ..CursorFramebufferLifecyclePlan::zeroed()
        },
        CursorFramebufferLifecycleEvent::SetupFailedAfterAddFb2 { rm_fb_succeeded }
        | CursorFramebufferLifecycleEvent::ArmedOwnerDrop { rm_fb_succeeded } => {
            CursorFramebufferLifecyclePlan {
                drm_retirements_via_remove: if rm_fb_succeeded { 1 } else { 0 },
                remove_framebuffer: true,
                ..CursorFramebufferLifecyclePlan::zeroed()
            }
        }
        CursorFramebufferLifecycleEvent::DisarmedOwnerDrop => CursorFramebufferLifecyclePlan {
            drm_retirements_without_remove: 1,
            ..CursorFramebufferLifecyclePlan::zeroed()
        },
        CursorFramebufferLifecycleEvent::CursorBackingReady => CursorFramebufferLifecyclePlan {
            cursor_backing_allocations: 1,
            ..CursorFramebufferLifecyclePlan::zeroed()
        },
        CursorFramebufferLifecycleEvent::CursorBackingDrop => CursorFramebufferLifecyclePlan {
            cursor_backing_retirements: 1,
            ..CursorFramebufferLifecyclePlan::zeroed()
        },
    }
}

impl CursorFramebufferLifecyclePlan {
    const fn zeroed() -> Self {
        Self {
            drm_allocations: 0,
            drm_retirements_via_remove: 0,
            drm_retirements_without_remove: 0,
            remove_framebuffer: false,
            cursor_backing_allocations: 0,
            cursor_backing_retirements: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeCursorSourceKey {
    Theme,
    Client(NativeCursorImageKey),
}

#[cfg(test)]
mod lifecycle_plan_tests {
    use super::*;

    #[test]
    fn successful_addfb2_ownership_plans_one_drm_framebuffer_allocation() {
        let plan =
            cursor_framebuffer_lifecycle_plan(CursorFramebufferLifecycleEvent::AddFb2Succeeded);

        assert_eq!(plan.drm_allocations, 1);
        assert_eq!(plan.drm_retirements_via_remove, 0);
        assert_eq!(plan.drm_retirements_without_remove, 0);
        assert_eq!(plan.cursor_backing_allocations, 0);
    }

    #[test]
    fn post_addfb2_construction_failure_plans_one_matching_retirement_on_successful_rm_fb() {
        let acquired =
            cursor_framebuffer_lifecycle_plan(CursorFramebufferLifecycleEvent::AddFb2Succeeded);
        let retired = cursor_framebuffer_lifecycle_plan(
            CursorFramebufferLifecycleEvent::SetupFailedAfterAddFb2 {
                rm_fb_succeeded: true,
            },
        );

        assert!(retired.remove_framebuffer);
        assert_eq!(acquired.drm_allocations, 1);
        assert_eq!(retired.drm_retirements_via_remove, 1);
        assert_eq!(retired.drm_retirements_without_remove, 0);
    }

    #[test]
    fn failed_rm_fb_does_not_manufacture_a_drm_framebuffer_retirement() {
        let plan =
            cursor_framebuffer_lifecycle_plan(CursorFramebufferLifecycleEvent::ArmedOwnerDrop {
                rm_fb_succeeded: false,
            });

        assert!(plan.remove_framebuffer);
        assert_eq!(plan.drm_retirements_via_remove, 0);
        assert_eq!(plan.drm_retirements_without_remove, 0);
    }

    #[test]
    fn armed_drop_has_one_remove_retirement_path_and_no_second_retirement() {
        let plan =
            cursor_framebuffer_lifecycle_plan(CursorFramebufferLifecycleEvent::ArmedOwnerDrop {
                rm_fb_succeeded: true,
            });

        assert!(plan.remove_framebuffer);
        assert_eq!(plan.drm_retirements_via_remove, 1);
        assert_eq!(plan.drm_retirements_without_remove, 0);
    }

    #[test]
    fn disarmed_drop_skips_rm_fb_and_retires_the_tracked_owner_once() {
        let plan =
            cursor_framebuffer_lifecycle_plan(CursorFramebufferLifecycleEvent::DisarmedOwnerDrop);

        assert!(!plan.remove_framebuffer);
        assert_eq!(plan.drm_retirements_via_remove, 0);
        assert_eq!(plan.drm_retirements_without_remove, 1);
    }

    #[test]
    fn pre_allocation_failure_cannot_retire_an_unrecorded_framebuffer() {
        let plan = cursor_framebuffer_lifecycle_plan(
            CursorFramebufferLifecycleEvent::BeforeFramebufferAcquired,
        );

        assert_eq!(plan.drm_allocations, 0);
        assert_eq!(plan.drm_retirements_via_remove, 0);
        assert_eq!(plan.drm_retirements_without_remove, 0);
        assert!(!plan.remove_framebuffer);
    }

    #[test]
    fn cursor_backing_events_are_independent_from_drm_framebuffer_events() {
        let created =
            cursor_framebuffer_lifecycle_plan(CursorFramebufferLifecycleEvent::CursorBackingReady);
        let retired =
            cursor_framebuffer_lifecycle_plan(CursorFramebufferLifecycleEvent::CursorBackingDrop);

        assert_eq!(created.cursor_backing_allocations, 1);
        assert_eq!(created.drm_allocations, 0);
        assert_eq!(retired.cursor_backing_retirements, 1);
        assert_eq!(retired.drm_retirements_via_remove, 0);
        assert_eq!(retired.drm_retirements_without_remove, 0);
    }
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
    pub(super) image_owner: Option<Arc<CompositorCursorImage>>,
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
        let framebuffer_plan =
            cursor_framebuffer_lifecycle_plan(CursorFramebufferLifecycleEvent::AddFb2Succeeded);
        for _ in 0..framebuffer_plan.drm_allocations {
            crate::gpu_residency::record_gpu_resource_allocation(
                crate::gpu_residency::GpuResourceClass::DrmFramebuffer,
                None,
            );
        }
        let map = match drm_ffi::mode::dumbbuffer::map(file.as_fd(), dumb.handle, 0, 0) {
            Ok(map) => map,
            Err(error) => {
                remove_failed_cursor_framebuffer(file.as_fd(), framebuffer);
                let _ = drm_ffi::mode::dumbbuffer::destroy(file.as_fd(), dumb.handle);
                return Err(error);
            }
        };
        let size = match usize::try_from(dumb.size) {
            Ok(size) => size,
            Err(_) => {
                remove_failed_cursor_framebuffer(file.as_fd(), framebuffer);
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
            remove_failed_cursor_framebuffer(file.as_fd(), framebuffer);
            let _ = drm_ffi::mode::dumbbuffer::destroy(file.as_fd(), dumb.handle);
            return Err(error);
        }
        let cursor_backing_plan =
            cursor_framebuffer_lifecycle_plan(CursorFramebufferLifecycleEvent::CursorBackingReady);
        for _ in 0..cursor_backing_plan.cursor_backing_allocations {
            crate::gpu_residency::record_gpu_resource_allocation(
                crate::gpu_residency::GpuResourceClass::CursorBacking,
                Some(dumb.size),
            );
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
            image_owner: None,
            lease: Arc::new(()),
        })
    }

    pub(super) fn upload_image(&mut self, image: &Arc<CompositorCursorImage>) -> io::Result<()> {
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
        self.image_owner = Some(image.clone());
        Ok(())
    }

    fn disarm_drm_cleanup(&mut self) {
        self.drm_cleanup_armed = false;
    }
}

fn remove_failed_cursor_framebuffer(drm: BorrowedFd<'_>, framebuffer: FramebufferId) {
    let rm_fb_succeeded = super::super::scanout::remove_explicit_framebuffer(drm, framebuffer);
    let plan = cursor_framebuffer_lifecycle_plan(
        CursorFramebufferLifecycleEvent::SetupFailedAfterAddFb2 { rm_fb_succeeded },
    );
    debug_assert_eq!(plan.drm_retirements_via_remove, u8::from(rm_fb_succeeded));
    debug_assert!(plan.remove_framebuffer);
    let _ = plan;
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
    pub(crate) fn for_test(framebuffer: u32, lease: Arc<()>) -> Self {
        Self {
            framebuffer: FramebufferId::new(framebuffer).expect("test framebuffer is nonzero"),
            lease,
        }
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
            let rm_fb_succeeded =
                super::super::scanout::remove_explicit_framebuffer(fd, self.framebuffer);
            let plan = cursor_framebuffer_lifecycle_plan(
                CursorFramebufferLifecycleEvent::ArmedOwnerDrop { rm_fb_succeeded },
            );
            debug_assert_eq!(plan.drm_retirements_via_remove, u8::from(rm_fb_succeeded));
            debug_assert!(plan.remove_framebuffer);
            let _ = plan;
        } else {
            let plan = cursor_framebuffer_lifecycle_plan(
                CursorFramebufferLifecycleEvent::DisarmedOwnerDrop,
            );
            for _ in 0..plan.drm_retirements_without_remove {
                crate::gpu_residency::record_gpu_resource_retirement(
                    crate::gpu_residency::GpuResourceClass::DrmFramebuffer,
                    None,
                );
            }
            debug_assert!(!plan.remove_framebuffer);
        }
        let _ = unsafe { libc::munmap(self.mapping, self.size) };
        if self.drm_cleanup_armed {
            let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
            let _ = drm_ffi::mode::dumbbuffer::destroy(fd, self.handle);
        }
        let cursor_backing_plan =
            cursor_framebuffer_lifecycle_plan(CursorFramebufferLifecycleEvent::CursorBackingDrop);
        for _ in 0..cursor_backing_plan.cursor_backing_retirements {
            crate::gpu_residency::record_gpu_resource_retirement(
                crate::gpu_residency::GpuResourceClass::CursorBacking,
                Some(self.size as u64),
            );
        }
    }
}

/// Upper bound on DRM cursor buffers owned by one output's resources. Only the
/// recycle pool is trimmed to it: the other lists are bounded by construction
/// and may hold buffers that are still on screen.
pub(super) const MAX_CURSOR_BUFFERS: usize = 8;
pub(super) const CLIENT_CURSOR_CACHE_CAPACITY: usize = 4;
const CURSOR_POOL_PREWARM_COUNT: usize = 2;

#[derive(Debug, Default)]
pub(super) struct AtomicCursorResources {
    pub(super) current: Option<AtomicCursorBuffer>,
    pub(super) retired: Vec<AtomicCursorBuffer>,
    pub(super) theme_cache: Option<AtomicCursorBuffer>,
    /// Most recently cached client image first.
    pub(super) client_cache: VecDeque<(NativeCursorImageKey, AtomicCursorBuffer)>,
    /// Buffers no KMS state or queued job references, oldest first. Same-size
    /// images reuse these so a cursor image change is a memcpy.
    pub(super) free: Vec<AtomicCursorBuffer>,
}

impl AtomicCursorResources {
    pub(super) fn take_cached(
        &mut self,
        source_key: NativeCursorSourceKey,
    ) -> Option<AtomicCursorBuffer> {
        match source_key {
            NativeCursorSourceKey::Theme => self.theme_cache.take(),
            NativeCursorSourceKey::Client(key) => {
                let index = self
                    .client_cache
                    .iter()
                    .position(|(cached_key, _)| *cached_key == key)?;
                self.client_cache.remove(index).map(|(_, buffer)| buffer)
            }
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
                // Evicted buffers may still be on screen, so they only move to
                // `retired`; retirement decides when they become reusable.
                if let Some(index) = self
                    .client_cache
                    .iter()
                    .position(|(cached_key, _)| *cached_key == key)
                    && let Some((_, stale)) = self.client_cache.remove(index)
                {
                    self.retired.push(stale);
                }
                self.client_cache.push_front((key, buffer));
                while self.client_cache.len() > CLIENT_CURSOR_CACHE_CAPACITY {
                    if let Some((_, evicted)) = self.client_cache.pop_back() {
                        self.retired.push(evicted);
                    }
                }
            }
        }
    }

    pub(super) fn retire_theme_cache(&mut self) {
        if let Some(buffer) = self.theme_cache.take() {
            self.retired.push(buffer);
        }
    }

    /// Takes a pooled buffer of the requested size. Pooled buffers are never
    /// presented or leased, so the caller may overwrite them.
    pub(super) fn take_free(&mut self, width: u32, height: u32) -> Option<AtomicCursorBuffer> {
        let index = self
            .free
            .iter()
            .rposition(|buffer| buffer.width == width && buffer.height == height)?;
        let buffer = self.free.remove(index);
        debug_assert_eq!(Arc::strong_count(&buffer.lease), 1);
        Some(buffer)
    }

    /// Moves retired buffers that KMS and queued jobs no longer reference into
    /// the pool. Each buffer keeps its DRM framebuffer, so moving it is not a
    /// DRM allocation or retirement.
    pub(super) fn retire_safe(&mut self, keep: &[Option<u32>]) {
        let (safe, pending): (Vec<_>, Vec<_>) = std::mem::take(&mut self.retired)
            .into_iter()
            .partition(|buffer| {
                !(Arc::strong_count(&buffer.lease) > 1
                    || keep
                        .iter()
                        .flatten()
                        .any(|framebuffer| *framebuffer == buffer.framebuffer.get()))
            });
        self.retired = pending;
        self.free.extend(safe);
        self.trim_free_pool();
    }

    /// Drops the oldest pooled buffers so the owned total stays within the cap.
    pub(super) fn trim_free_pool(&mut self) {
        let owned_outside_pool = usize::from(self.current.is_some())
            + usize::from(self.theme_cache.is_some())
            + self.client_cache.len()
            + self.retired.len();
        let allowed = MAX_CURSOR_BUFFERS.saturating_sub(owned_outside_pool);
        if self.free.len() > allowed {
            let excess = self.free.len() - allowed;
            self.free.drain(..excess);
        }
    }

    /// Best effort: spare buffers let the first image changes after startup
    /// avoid DRM allocation on the reactor.
    pub(super) fn prewarm_free(&mut self, file: &fs::File, width: u32, height: u32) {
        for _ in 0..CURSOR_POOL_PREWARM_COUNT {
            let Ok(buffer) = AtomicCursorBuffer::create(file, width, height) else {
                break;
            };
            self.free.push(buffer);
        }
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
                    .iter()
                    .map(|(_, buffer)| buffer)
                    .find(|buffer| matches(buffer))
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
        for (_, client) in &mut self.client_cache {
            client.disarm_drm_cleanup();
        }
        for buffer in self.retired.iter_mut().chain(self.free.iter_mut()) {
            buffer.disarm_drm_cleanup();
        }
    }
}

#[cfg(test)]
pub(super) fn fake_cursor_buffer(framebuffer: u32, width: u32, height: u32) -> AtomicCursorBuffer {
    let pitch = width * 4;
    let size = pitch as usize * height as usize;
    let mapping = unsafe {
        libc::mmap(
            ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert_ne!(mapping, libc::MAP_FAILED);
    AtomicCursorBuffer {
        fd: -1,
        handle: 1,
        framebuffer: FramebufferId::new(framebuffer).expect("test framebuffer is nonzero"),
        width,
        height,
        pitch,
        size,
        mapping,
        drm_cleanup_armed: false,
        image_owner: None,
        lease: Arc::new(()),
    }
}
