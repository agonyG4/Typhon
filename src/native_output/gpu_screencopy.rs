//! Zero-copy wlr-screencopy into client DMA-BUFs.
//!
//! The reactor stages DMA-BUF requests on the Atomic scanout; the next
//! composited display frame carries them to the render domain, which blits the
//! finished slot into each client buffer before exporting the frame's render
//! fence. The render thread never waits for that fence: a blitted target
//! reports its frame id and a duplicate of the fence, and the reactor sends
//! `ready` when that frame is presented, since KMS has waited on the same
//! fence by then.
//!
//! Every target reports exactly once through [`GpuScreencopyCompletions`] when
//! it is dropped, so a target lost on any error or teardown path fails its
//! request instead of leaking it. A blitted frame that is superseded, dropped
//! or never presented falls back to a non-blocking fence check: signaled
//! delivers, anything else fails and the client retries.

use std::{
    collections::HashMap,
    fmt,
    os::fd::{AsRawFd, OwnedFd},
    sync::{Arc, Mutex, PoisonError},
};

use oblivion_one::render_backend::buffer::{DmabufBufferHandle, WeakBufferIdentity};

pub(crate) use crate::egl_renderer::screencopy_blit::ScreencopyBlitRegion;

/// Frames that may come back unrendered (renderer skip or lifecycle
/// fallback) before a staged target is failed instead of retried.
const MAX_RENDER_ATTEMPTS: u8 = 3;

/// A blitted frame not presented within this long uses the fence fallback.
const PRESENTATION_WAIT_NS: u64 = 100_000_000;

enum TargetReport {
    Failed,
    Blitted { frame_id: u64, fence: OwnedFd },
}

#[derive(Default)]
pub(crate) struct GpuScreencopyCompletions {
    reports: Mutex<Vec<(u64, TargetReport)>>,
}

impl fmt::Debug for GpuScreencopyCompletions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GpuScreencopyCompletions")
            .finish_non_exhaustive()
    }
}

impl GpuScreencopyCompletions {
    fn push(&self, id: u64, report: TargetReport) {
        self.reports
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((id, report));
    }

    fn drain(&self) -> Vec<(u64, TargetReport)> {
        std::mem::take(&mut *self.reports.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// One client DMA-BUF a composited frame must be copied into.
pub(crate) struct GpuScreencopyTarget {
    id: u64,
    buffer: DmabufBufferHandle,
    buffer_identity: WeakBufferIdentity,
    region: ScreencopyBlitRegion,
    overlay_cursor: bool,
    render_attempts: u8,
    blitted: bool,
    rendered: Option<(u64, OwnedFd)>,
    completions: Arc<GpuScreencopyCompletions>,
}

impl fmt::Debug for GpuScreencopyTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GpuScreencopyTarget")
            .field("id", &self.id)
            .field("buffer_id", &self.buffer_identity.id().get())
            .field("region", &self.region)
            .field("overlay_cursor", &self.overlay_cursor)
            .field("blitted", &self.blitted)
            .finish_non_exhaustive()
    }
}

impl GpuScreencopyTarget {
    pub(crate) fn buffer(&self) -> &DmabufBufferHandle {
        &self.buffer
    }

    pub(crate) fn buffer_identity(&self) -> &WeakBufferIdentity {
        &self.buffer_identity
    }

    pub(crate) const fn region(&self) -> ScreencopyBlitRegion {
        self.region
    }

    /// wlr-screencopy `overlay_cursor`: the copy must show the cursor even
    /// while the output shows it on the hardware cursor plane.
    pub(crate) const fn overlay_cursor(&self) -> bool {
        self.overlay_cursor
    }

    pub(crate) fn note_blitted(&mut self) {
        self.blitted = true;
    }

    /// The blit was submitted in output frame `frame_id`, completed by `fence`.
    pub(crate) fn note_rendered(&mut self, frame_id: u64, fence: OwnedFd) {
        if self.blitted {
            self.rendered = Some((frame_id, fence));
        }
    }

    /// Account one unrendered frame; `false` once the target should fail.
    pub(crate) fn retry_after_unrendered_frame(&mut self) -> bool {
        self.render_attempts = self.render_attempts.saturating_add(1);
        self.blitted = false;
        self.rendered = None;
        self.render_attempts < MAX_RENDER_ATTEMPTS
    }
}

impl Drop for GpuScreencopyTarget {
    fn drop(&mut self) {
        let report = match self.rendered.take() {
            Some((frame_id, fence)) if self.blitted => TargetReport::Blitted { frame_id, fence },
            _ => TargetReport::Failed,
        };
        self.completions.push(self.id, report);
    }
}

/// Protocol side of a GPU screencopy request.
pub(crate) trait GpuScreencopyRequest {
    /// `presented_at_ns` is the CLOCK_MONOTONIC presentation time of the
    /// copied frame, or `None` when it was not observed.
    fn deliver(self, presented_at_ns: Option<u64>);
    fn fail(self);
}

impl GpuScreencopyRequest for oblivion_one::compositor::ScreencopyPending {
    fn deliver(self, presented_at_ns: Option<u64>) {
        self.deliver_gpu_copy(presented_at_ns);
    }

    fn fail(self) {
        oblivion_one::compositor::ScreencopyPending::fail(&self);
    }
}

struct AwaitingPresentation {
    id: u64,
    frame_id: u64,
    fence: OwnedFd,
    since_ns: u64,
}

/// Reactor-owned requests whose targets are staged, inside a render job, or
/// blitted and waiting for their frame to be presented.
pub(crate) struct GpuScreencopyInflight<R> {
    completions: Arc<GpuScreencopyCompletions>,
    requests: HashMap<u64, R>,
    awaiting: Vec<AwaitingPresentation>,
    last_presented_frame_id: Option<u64>,
    next_id: u64,
    unflushed_events: usize,
}

impl<R> Default for GpuScreencopyInflight<R> {
    fn default() -> Self {
        Self {
            completions: Arc::default(),
            requests: HashMap::new(),
            awaiting: Vec::new(),
            last_presented_frame_id: None,
            next_id: 1,
            unflushed_events: 0,
        }
    }
}

impl<R: GpuScreencopyRequest> GpuScreencopyInflight<R> {
    pub(crate) fn admit(
        &mut self,
        request: R,
        buffer: DmabufBufferHandle,
        buffer_identity: WeakBufferIdentity,
        region: ScreencopyBlitRegion,
        overlay_cursor: bool,
    ) -> GpuScreencopyTarget {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.requests.insert(id, request);
        GpuScreencopyTarget {
            id,
            buffer,
            buffer_identity,
            region,
            overlay_cursor,
            render_attempts: 0,
            blitted: false,
            rendered: None,
            completions: Arc::clone(&self.completions),
        }
    }

    /// Fail unblitted targets, queue blitted ones for presentation, and use
    /// the fence fallback for any that waited too long. Returns the number of
    /// protocol events sent since the previous call, for flushing.
    pub(crate) fn settle(&mut self, now_ns: u64) -> usize {
        self.collect(now_ns);
        let (expired, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.awaiting)
            .into_iter()
            .partition(|entry| now_ns.saturating_sub(entry.since_ns) >= PRESENTATION_WAIT_NS);
        self.awaiting = waiting;
        for entry in expired {
            self.fallback(entry);
        }
        std::mem::take(&mut self.unflushed_events)
    }

    /// Output frame `frame_id` reached the screen at `presented_at_ns`
    /// (CLOCK_MONOTONIC). Its copies are complete; older frames were
    /// superseded and fall back to the fence check.
    pub(crate) fn note_presented(&mut self, frame_id: u64, presented_at_ns: u64, now_ns: u64) {
        self.collect(now_ns);
        self.last_presented_frame_id = Some(
            self.last_presented_frame_id
                .map_or(frame_id, |last| last.max(frame_id)),
        );
        for entry in std::mem::take(&mut self.awaiting) {
            if entry.frame_id == frame_id {
                self.deliver(entry.id, Some(presented_at_ns));
            } else if entry.frame_id < frame_id {
                self.fallback(entry);
            } else {
                self.awaiting.push(entry);
            }
        }
    }

    /// Settle everything outstanding: blitted copies use the fence fallback,
    /// everything else fails; late target reports are ignored.
    pub(crate) fn fail_all(&mut self) {
        self.collect(0);
        for entry in std::mem::take(&mut self.awaiting) {
            self.fallback(entry);
        }
        for (_, request) in self.requests.drain() {
            request.fail();
            self.unflushed_events += 1;
        }
    }

    fn collect(&mut self, now_ns: u64) {
        for (id, report) in self.completions.drain() {
            if !self.requests.contains_key(&id) {
                continue;
            }
            match report {
                TargetReport::Failed => self.fail(id),
                TargetReport::Blitted { frame_id, fence } => {
                    let entry = AwaitingPresentation {
                        id,
                        frame_id,
                        fence,
                        since_ns: now_ns,
                    };
                    if self
                        .last_presented_frame_id
                        .is_some_and(|presented| frame_id <= presented)
                    {
                        self.fallback(entry);
                    } else {
                        self.awaiting.push(entry);
                    }
                }
            }
        }
    }

    fn fallback(&mut self, entry: AwaitingPresentation) {
        if fence_signaled(&entry.fence) {
            self.deliver(entry.id, None);
        } else {
            self.fail(entry.id);
        }
    }

    fn deliver(&mut self, id: u64, presented_at_ns: Option<u64>) {
        if let Some(request) = self.requests.remove(&id) {
            request.deliver(presented_at_ns);
            self.unflushed_events += 1;
        }
    }

    fn fail(&mut self, id: u64) {
        if let Some(request) = self.requests.remove(&id) {
            request.fail();
            self.unflushed_events += 1;
        }
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.requests.is_empty() && self.awaiting.is_empty()
    }
}

/// Non-blocking sync_file readiness check.
fn fence_signaled(fence: &OwnedFd) -> bool {
    let mut pollfd = libc::pollfd {
        fd: fence.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut pollfd, 1, 0) };
    ready > 0 && pollfd.revents & libc::POLLIN != 0
}

/// A 4x4 `/dev/null`-backed DMA-BUF stand-in and its (dead) identity.
#[cfg(test)]
fn test_dmabuf() -> (DmabufBufferHandle, WeakBufferIdentity) {
    use oblivion_one::render_backend::buffer::{
        BufferIdAllocator, BufferSize, DmabufPlane, DmabufPlaneDescriptor, DrmFormat, DrmModifier,
    };
    let identity = BufferIdAllocator::default().allocate().unwrap();
    let file = std::fs::File::open("/dev/null").unwrap();
    let buffer = DmabufBufferHandle::new(
        BufferSize::new(4, 4).unwrap(),
        DrmFormat::Xrgb8888,
        vec![DmabufPlane::new(
            file.into(),
            DmabufPlaneDescriptor {
                plane_index: 0,
                offset: 0,
                stride: 16,
                modifier: DrmModifier::LINEAR,
            },
        )],
    )
    .unwrap();
    (buffer, identity.downgrade())
}

#[cfg(test)]
impl GpuScreencopyTarget {
    /// A detached full-buffer target whose report nobody collects.
    pub(crate) fn for_test(overlay_cursor: bool) -> Self {
        let (buffer, buffer_identity) = test_dmabuf();
        Self {
            id: 0,
            buffer,
            buffer_identity,
            region: ScreencopyBlitRegion {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            },
            overlay_cursor,
            render_attempts: 0,
            blitted: false,
            rendered: None,
            completions: Arc::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, io::Write, rc::Rc};

    /// `(tag, Some(presented_at) | None for "now", delivered)`.
    type Outcome = (u32, Option<u64>, bool);

    #[derive(Clone, Default)]
    struct Outcomes(Rc<RefCell<Vec<Outcome>>>);

    impl Outcomes {
        fn sorted(&self) -> Vec<Outcome> {
            let mut outcomes = self.0.borrow().clone();
            outcomes.sort_unstable();
            outcomes
        }
    }

    struct FakeRequest {
        tag: u32,
        outcomes: Outcomes,
    }

    impl GpuScreencopyRequest for FakeRequest {
        fn deliver(self, presented_at_ns: Option<u64>) {
            self.outcomes
                .0
                .borrow_mut()
                .push((self.tag, presented_at_ns, true));
        }

        fn fail(self) {
            self.outcomes.0.borrow_mut().push((self.tag, None, false));
        }
    }

    /// A pipe read end stands in for a sync_file: readable means signaled.
    fn fence(signaled: bool) -> OwnedFd {
        let (read, write) = std::io::pipe().unwrap();
        if signaled {
            let mut write = write;
            write.write_all(b"x").unwrap();
            std::mem::forget(write);
        } else {
            std::mem::forget(write);
        }
        read.into()
    }

    fn admit(
        inflight: &mut GpuScreencopyInflight<FakeRequest>,
        outcomes: &Outcomes,
        tag: u32,
        overlay_cursor: bool,
    ) -> GpuScreencopyTarget {
        let (buffer, identity) = test_dmabuf();
        inflight.admit(
            FakeRequest {
                tag,
                outcomes: outcomes.clone(),
            },
            buffer,
            identity,
            ScreencopyBlitRegion {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            },
            overlay_cursor,
        )
    }

    fn rendered(mut target: GpuScreencopyTarget, frame_id: u64, signaled: bool) {
        target.note_blitted();
        target.note_rendered(frame_id, fence(signaled));
    }

    #[test]
    fn presented_frame_delivers_with_its_presentation_time() {
        let outcomes = Outcomes::default();
        let mut inflight = GpuScreencopyInflight::default();
        rendered(admit(&mut inflight, &outcomes, 1, false), 7, false);
        rendered(admit(&mut inflight, &outcomes, 2, false), 8, false);

        assert_eq!(inflight.settle(1_000), 0);
        assert!(outcomes.sorted().is_empty());
        inflight.note_presented(7, 5_000, 1_000);
        assert_eq!(outcomes.sorted(), [(1, Some(5_000), true)]);
        inflight.note_presented(8, 6_000, 1_000);
        assert_eq!(
            outcomes.sorted(),
            [(1, Some(5_000), true), (2, Some(6_000), true)]
        );
        assert_eq!(inflight.settle(1_000), 2);
        assert!(inflight.is_empty());
    }

    #[test]
    fn presentation_before_settle_still_matches_the_frame() {
        let outcomes = Outcomes::default();
        let mut inflight = GpuScreencopyInflight::default();
        rendered(admit(&mut inflight, &outcomes, 1, false), 3, false);
        inflight.note_presented(3, 9_000, 0);
        assert_eq!(outcomes.sorted(), [(1, Some(9_000), true)]);
    }

    #[test]
    fn superseded_frame_falls_back_to_the_fence() {
        let outcomes = Outcomes::default();
        let mut inflight = GpuScreencopyInflight::default();
        rendered(admit(&mut inflight, &outcomes, 1, false), 4, true);
        rendered(admit(&mut inflight, &outcomes, 2, false), 5, false);
        inflight.settle(0);
        // Frame 6 presented: 4 and 5 were superseded and never reached KMS.
        inflight.note_presented(6, 10_000, 0);
        assert_eq!(outcomes.sorted(), [(1, None, true), (2, None, false)]);
        assert!(inflight.is_empty());
    }

    #[test]
    fn unpresented_frame_falls_back_after_the_wait_and_late_reports_too() {
        let outcomes = Outcomes::default();
        let mut inflight = GpuScreencopyInflight::default();
        rendered(admit(&mut inflight, &outcomes, 1, false), 2, true);
        rendered(admit(&mut inflight, &outcomes, 2, false), 3, false);
        inflight.settle(0);
        assert!(outcomes.sorted().is_empty());
        assert_eq!(inflight.settle(PRESENTATION_WAIT_NS), 2);
        assert_eq!(outcomes.sorted(), [(1, None, true), (2, None, false)]);

        // A report for a frame already presented cannot wait for it.
        inflight.note_presented(9, 1, PRESENTATION_WAIT_NS);
        rendered(admit(&mut inflight, &outcomes, 3, false), 9, true);
        inflight.settle(PRESENTATION_WAIT_NS);
        assert_eq!(outcomes.0.borrow().last(), Some(&(3, None, true)));
        assert!(inflight.is_empty());
    }

    #[test]
    fn unblitted_or_unrendered_targets_fail() {
        let outcomes = Outcomes::default();
        let mut inflight = GpuScreencopyInflight::default();
        let dropped = admit(&mut inflight, &outcomes, 1, false);
        let mut not_blitted = admit(&mut inflight, &outcomes, 2, false);
        not_blitted.note_rendered(1, fence(true));
        let mut not_rendered = admit(&mut inflight, &outcomes, 3, false);
        not_rendered.note_blitted();
        drop((dropped, not_blitted, not_rendered));
        assert_eq!(inflight.settle(0), 3);
        assert_eq!(
            outcomes.sorted(),
            [(1, None, false), (2, None, false), (3, None, false)]
        );
        assert!(inflight.is_empty());
    }

    #[test]
    fn unrendered_frames_retry_a_bounded_number_of_times() {
        let outcomes = Outcomes::default();
        let mut inflight = GpuScreencopyInflight::default();
        let mut target = admit(&mut inflight, &outcomes, 1, false);
        target.note_blitted();
        target.note_rendered(1, fence(true));
        assert!(target.retry_after_unrendered_frame());
        assert!(target.retry_after_unrendered_frame());
        assert!(!target.retry_after_unrendered_frame());
        // A retried target must blit and render again before it can succeed.
        drop(target);
        inflight.settle(0);
        assert_eq!(outcomes.sorted(), [(1, None, false)]);
    }

    #[test]
    fn fail_all_settles_each_request_once_and_ignores_late_reports() {
        let outcomes = Outcomes::default();
        let mut inflight = GpuScreencopyInflight::default();
        rendered(admit(&mut inflight, &outcomes, 1, false), 1, true);
        rendered(admit(&mut inflight, &outcomes, 2, false), 1, false);
        inflight.settle(0);
        let staged = admit(&mut inflight, &outcomes, 3, false);
        inflight.fail_all();
        assert_eq!(
            outcomes.sorted(),
            [(1, None, true), (2, None, false), (3, None, false)]
        );
        rendered(staged, 2, true);
        inflight.settle(0);
        inflight.note_presented(2, 1, 0);
        assert_eq!(outcomes.0.borrow().len(), 3);
        assert!(inflight.is_empty());
    }

    #[test]
    fn targets_carry_the_overlay_cursor_request() {
        let outcomes = Outcomes::default();
        let mut inflight = GpuScreencopyInflight::default();
        assert!(!admit(&mut inflight, &outcomes, 1, false).overlay_cursor());
        assert!(admit(&mut inflight, &outcomes, 2, true).overlay_cursor());
    }
}
