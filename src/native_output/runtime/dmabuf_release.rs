use std::{
    collections::HashMap,
    io,
    os::fd::{AsRawFd, OwnedFd},
};

use oblivion_one::compositor::{DmabufGpuReleaseLeaseId, OwnCompositorServer};
use oblivion_one::native::event_loop::{NativeEventLoop, NativeEventSource, ReactorToken};

use crate::native_output::NativePerfField;
use crate::native_output::scanout::AtomicEglGbmScanout;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DmabufGpuReleaseMetrics {
    pub(crate) leases_registered: u64,
    pub(crate) leases_completed: u64,
    pub(crate) leases_requeued: u64,
    pub(crate) obligations_armed: u64,
    pub(crate) obligations_completed: u64,
    pub(crate) fences_created: u64,
    pub(crate) fences_signaled: u64,
    pub(crate) no_visual_fence_only: u64,
    pub(crate) completion_fd_failures: u64,
    pub(crate) registration_failures: u64,
    pub(crate) active_leases: u64,
    pub(crate) peak_active_leases: u64,
}

#[derive(Debug)]
struct DmabufGpuReleaseWatch {
    lease_id: DmabufGpuReleaseLeaseId,
    completion_fd: OwnedFd,
}

#[derive(Debug, Default)]
pub(crate) struct DmabufGpuReleaseRegistry {
    next_lease_id: u64,
    watches: HashMap<ReactorToken, DmabufGpuReleaseWatch>,
    metrics: DmabufGpuReleaseMetrics,
}

impl DmabufGpuReleaseRegistry {
    pub(crate) fn allocate_lease_id(&mut self) -> io::Result<DmabufGpuReleaseLeaseId> {
        let id = std::num::NonZeroU64::new(self.next_lease_id.max(1))
            .ok_or_else(|| io::Error::other("DMA-BUF GPU release lease IDs exhausted"))?;
        self.next_lease_id = self
            .next_lease_id
            .max(1)
            .checked_add(1)
            .ok_or_else(|| io::Error::other("DMA-BUF GPU release lease IDs exhausted"))?;
        Ok(DmabufGpuReleaseLeaseId::new(id))
    }

    pub(crate) fn register(
        &mut self,
        lease_id: DmabufGpuReleaseLeaseId,
        completion_fd: OwnedFd,
        event_loop: &mut NativeEventLoop,
    ) -> io::Result<ReactorToken> {
        if self
            .watches
            .values()
            .any(|watch| watch.lease_id == lease_id)
        {
            return Err(io::Error::other(
                "DMA-BUF GPU release lease ID is already active",
            ));
        }
        let token = event_loop.register(
            completion_fd.as_raw_fd(),
            NativeEventSource::DmabufGpuRelease,
        )?;
        if self
            .watches
            .insert(
                token,
                DmabufGpuReleaseWatch {
                    lease_id,
                    completion_fd,
                },
            )
            .is_some()
        {
            let _ = event_loop.unregister(token);
            return Err(io::Error::other(
                "native reactor returned a duplicate DMA-BUF release token",
            ));
        }
        self.metrics.leases_registered = self.metrics.leases_registered.saturating_add(1);
        self.metrics.fences_created = self.metrics.fences_created.saturating_add(1);
        self.metrics.active_leases = self.metrics.active_leases.saturating_add(1);
        self.metrics.peak_active_leases = self
            .metrics
            .peak_active_leases
            .max(self.metrics.active_leases);
        Ok(token)
    }

    pub(crate) fn note_registration_failure(&mut self) {
        self.metrics.registration_failures = self.metrics.registration_failures.saturating_add(1);
    }

    pub(crate) fn note_completion_fd_failure(&mut self) {
        self.metrics.completion_fd_failures = self.metrics.completion_fd_failures.saturating_add(1);
    }

    pub(crate) fn note_obligations_armed(&mut self, count: usize) {
        self.metrics.obligations_armed =
            self.metrics.obligations_armed.saturating_add(count as u64);
    }

    pub(crate) fn note_no_visual_fence_only(&mut self) {
        self.metrics.no_visual_fence_only = self.metrics.no_visual_fence_only.saturating_add(1);
    }

    pub(crate) fn metrics(&self) -> DmabufGpuReleaseMetrics {
        self.metrics
    }

    #[cfg(test)]
    pub(crate) fn active_count(&self) -> usize {
        self.watches.len()
    }

    pub(crate) fn service_ready(
        &mut self,
        tokens: &[ReactorToken],
        event_loop: &mut NativeEventLoop,
        server: &mut OwnCompositorServer,
    ) -> io::Result<usize> {
        self.service_ready_with(tokens, event_loop, |lease_id| {
            server.complete_dmabuf_gpu_release_lease(lease_id)
        })
    }

    fn service_ready_with(
        &mut self,
        tokens: &[ReactorToken],
        event_loop: &mut NativeEventLoop,
        mut complete: impl FnMut(DmabufGpuReleaseLeaseId) -> usize,
    ) -> io::Result<usize> {
        let mut completed = 0;
        for token in tokens.iter().copied() {
            if !self.watches.contains_key(&token) {
                continue;
            }
            // Unregister before removing the owned watch.  If the reactor
            // reports an error, the watch remains the registry's obligation.
            let _ = event_loop.unregister(token)?;
            let Some(watch) = self.watches.remove(&token) else {
                continue;
            };
            let _completion_fd = watch.completion_fd;
            let watch_completed = complete(watch.lease_id);
            completed += watch_completed;
            self.metrics.leases_completed = self.metrics.leases_completed.saturating_add(1);
            self.metrics.obligations_completed = self
                .metrics
                .obligations_completed
                .saturating_add(watch_completed as u64);
            self.metrics.fences_signaled = self.metrics.fences_signaled.saturating_add(1);
            self.metrics.active_leases = self.metrics.active_leases.saturating_sub(1);
        }
        Ok(completed)
    }

    pub(crate) fn cancel_all(
        &mut self,
        event_loop: &mut NativeEventLoop,
        server: &mut OwnCompositorServer,
    ) -> io::Result<usize> {
        self.cancel_all_with(event_loop, |lease_id| {
            server.requeue_dmabuf_gpu_release_lease(lease_id)
        })
    }

    fn cancel_all_with(
        &mut self,
        event_loop: &mut NativeEventLoop,
        mut requeue: impl FnMut(DmabufGpuReleaseLeaseId) -> usize,
    ) -> io::Result<usize> {
        let tokens = self.watches.keys().copied().collect::<Vec<_>>();
        let mut requeued = 0;
        for token in tokens {
            let _ = event_loop.unregister(token)?;
            let Some(watch) = self.watches.remove(&token) else {
                continue;
            };
            let _completion_fd = watch.completion_fd;
            requeued += requeue(watch.lease_id);
            self.metrics.leases_requeued = self.metrics.leases_requeued.saturating_add(1);
            self.metrics.active_leases = self.metrics.active_leases.saturating_sub(1);
        }
        Ok(requeued)
    }
}

pub(crate) fn arm_composited_dmabuf_release(
    registry: &mut DmabufGpuReleaseRegistry,
    server: &mut OwnCompositorServer,
    explicit: &AtomicEglGbmScanout,
    event_loop: &mut NativeEventLoop,
    batch_id: oblivion_one::compositor::CompositorFrameBatchId,
    lease_id: Option<DmabufGpuReleaseLeaseId>,
) -> io::Result<usize> {
    let Some(lease_id) = lease_id else {
        return Ok(0);
    };
    let count = server.frame_batch_dmabuf_release_count(batch_id);
    if count == 0 || explicit.has_live_direct_kms_ownership() {
        return Ok(0);
    }
    let completion_fd = match explicit.duplicate_ready_render_completion_fd() {
        Ok(fd) => fd,
        Err(_) => {
            registry.note_completion_fd_failure();
            return Ok(0);
        }
    };
    let transferred = server.transfer_frame_batch_dmabuf_releases_to_gpu_lease(batch_id, lease_id);
    if transferred == 0 {
        return Ok(0);
    }
    registry.note_obligations_armed(transferred);
    match registry.register(lease_id, completion_fd, event_loop) {
        Ok(_) => Ok(transferred),
        Err(_) => {
            registry.note_registration_failure();
            server.requeue_dmabuf_gpu_release_lease(lease_id);
            Ok(0)
        }
    }
}

impl super::NativeRuntime {
    pub(super) fn service_dmabuf_gpu_releases(
        &mut self,
        tokens: &[ReactorToken],
    ) -> super::NativeResult<()> {
        if tokens.is_empty() {
            return Ok(());
        }
        let completed = self.dmabuf_gpu_release_registry.service_ready(
            tokens,
            &mut self.event_loop,
            &mut self.server,
        )?;
        self.perf.log("native.dmabuf_gpu_release", || {
            vec![
                NativePerfField::usize("watches", tokens.len()),
                NativePerfField::usize("obligations_completed", completed),
            ]
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::{AsRawFd, FromRawFd};

    use super::*;

    fn pipe() -> (OwnedFd, OwnedFd) {
        let mut fds = [-1; 2];
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        (unsafe { OwnedFd::from_raw_fd(fds[0]) }, unsafe {
            OwnedFd::from_raw_fd(fds[1])
        })
    }

    fn lease_id(registry: &mut DmabufGpuReleaseRegistry) -> DmabufGpuReleaseLeaseId {
        registry.allocate_lease_id().unwrap()
    }

    #[test]
    fn gpu_release_readiness_is_a_dedicated_reactor_domain() {
        let mut event_loop = NativeEventLoop::new().unwrap();
        let (read, write) = pipe();
        let mut registry = DmabufGpuReleaseRegistry::default();
        let id = lease_id(&mut registry);
        let token = registry.register(id, read, &mut event_loop).unwrap();

        assert_eq!(registry.active_count(), 1);
        assert_eq!(
            event_loop.source_for_token(token),
            Some(NativeEventSource::DmabufGpuRelease)
        );
        unsafe { libc::write(write.as_raw_fd(), [1_u8].as_ptr().cast(), 1) };
        let wakeup = event_loop.wait().unwrap();
        assert!(wakeup.reasons.dmabuf_gpu_release());
        assert_eq!(wakeup.dmabuf_gpu_release_tokens, vec![token]);
        assert!(wakeup.explicit_sync_acquire_tokens.is_empty());

        let mut completed_ids = Vec::new();
        registry
            .service_ready_with(&wakeup.dmabuf_gpu_release_tokens, &mut event_loop, |id| {
                completed_ids.push(id);
                1
            })
            .unwrap();
        assert_eq!(completed_ids, vec![id]);
        assert_eq!(registry.active_count(), 0);
        assert_eq!(event_loop.source_for_token(token), None);
        assert_eq!(registry.metrics().leases_completed, 1);
    }

    #[test]
    fn multiple_release_watches_are_serviced_independently() {
        let mut event_loop = NativeEventLoop::new().unwrap();
        let (read_a, write_a) = pipe();
        let (read_b, write_b) = pipe();
        let mut registry = DmabufGpuReleaseRegistry::default();
        let id_a = lease_id(&mut registry);
        let id_b = lease_id(&mut registry);
        let token_a = registry.register(id_a, read_a, &mut event_loop).unwrap();
        let token_b = registry.register(id_b, read_b, &mut event_loop).unwrap();
        unsafe {
            libc::write(write_a.as_raw_fd(), [1_u8].as_ptr().cast(), 1);
            libc::write(write_b.as_raw_fd(), [1_u8].as_ptr().cast(), 1);
        }
        let wakeup = event_loop.wait().unwrap();
        let mut completed_ids = Vec::new();
        registry
            .service_ready_with(&wakeup.dmabuf_gpu_release_tokens, &mut event_loop, |id| {
                completed_ids.push(id);
                1
            })
            .unwrap();
        assert_eq!(completed_ids.len(), 2);
        assert!(completed_ids.contains(&id_a));
        assert!(completed_ids.contains(&id_b));
        assert_eq!(registry.active_count(), 0);
        assert_eq!(event_loop.source_for_token(token_a), None);
        assert_eq!(event_loop.source_for_token(token_b), None);
    }

    #[test]
    fn cancellation_requeues_and_unregisters_without_a_wake_loop() {
        let mut event_loop = NativeEventLoop::new().unwrap();
        let (read, _write) = pipe();
        let mut registry = DmabufGpuReleaseRegistry::default();
        let id = lease_id(&mut registry);
        let token = registry.register(id, read, &mut event_loop).unwrap();
        let mut requeued_ids = Vec::new();
        registry
            .cancel_all_with(&mut event_loop, |id| {
                requeued_ids.push(id);
                1
            })
            .unwrap();
        assert_eq!(requeued_ids, vec![id]);
        assert_eq!(registry.active_count(), 0);
        assert_eq!(event_loop.source_for_token(token), None);
        assert_eq!(registry.metrics().leases_requeued, 1);
    }
}
