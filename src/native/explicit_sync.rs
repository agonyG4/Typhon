use std::{
    collections::HashMap,
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
};

use crate::{
    compositor::{
        AcquireCommitId, AcquireWatchCancelReason, AcquireWatchRequest, ExplicitSyncPoint,
    },
    syncobj::{SyncobjEventfdErrnoClass, SyncobjEventfdError},
};

use super::event_loop::{NativeEventLoop, NativeEventSource, ReactorToken};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncobjEventfdCapability {
    Unknown,
    Supported,
    Unsupported,
    BrokenOrRejected(i32),
}

#[derive(Debug)]
struct FallbackEntry {
    deadline_ns: u64,
}

#[derive(Debug)]
struct FallbackSchedule {
    retry_interval_ns: u64,
    entries: HashMap<AcquireCommitId, FallbackEntry>,
}

pub trait AcquirePointNotifier {
    fn point_signaled(&self, point: &ExplicitSyncPoint) -> io::Result<bool>;
    fn register_eventfd(
        &self,
        point: &ExplicitSyncPoint,
        event_fd: BorrowedFd<'_>,
    ) -> Result<(), SyncobjEventfdError>;
}

#[derive(Debug, Default)]
pub struct DrmAcquirePointNotifier;

impl AcquirePointNotifier for DrmAcquirePointNotifier {
    fn point_signaled(&self, point: &ExplicitSyncPoint) -> io::Result<bool> {
        point.signaled_result()
    }

    fn register_eventfd(
        &self,
        point: &ExplicitSyncPoint,
        event_fd: BorrowedFd<'_>,
    ) -> Result<(), SyncobjEventfdError> {
        point.timeline.register_eventfd(point.point, event_fd)
    }
}

#[derive(Debug)]
struct AcquireWatch {
    token: ReactorToken,
    event_fd: OwnedFd,
    drm_file_generation: u64,
    request: AcquireWatchRequest,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ExplicitSyncWatchMetrics {
    pub active_eventfd_watches: usize,
    pub active_fallback_watches: usize,
    pub registrations: u64,
    pub already_signaled: u64,
    pub eventfd_wakeups: u64,
    pub stale_wakeups: u64,
    pub duplicate_wakeups: u64,
    pub cancellations: u64,
    pub registration_failures: u64,
    pub fallback_activations: u64,
    pub maximum_simultaneous_watches: usize,
    pub leaked_watch_assertions: u64,
    pub last_registration_errno: i32,
    pub last_commit_to_ready_ns: u64,
    pub cancellations_by_reason: [u64; 8],
}

#[derive(Debug)]
pub enum AcquireRegistrationResult {
    AlreadyReady(AcquireWatchRequest),
    EventfdBacked(AcquireCommitId),
    FallbackBacked(AcquireCommitId),
}

#[derive(Debug)]
pub enum AcquireReadyResult {
    Ready(AcquireWatchRequest),
    Pending,
    Stale,
    BackendMismatch(AcquireCommitId),
}

#[derive(Debug)]
pub struct ExplicitSyncWatchRegistry {
    watches_by_token: HashMap<ReactorToken, AcquireWatch>,
    watch_by_commit: HashMap<AcquireCommitId, ReactorToken>,
    fallback_requests: HashMap<AcquireCommitId, AcquireWatchRequest>,
    fallback_schedule: FallbackSchedule,
    capability: SyncobjEventfdCapability,
    drm_file_generation: u64,
    metrics: ExplicitSyncWatchMetrics,
    recent_completed_tokens: std::collections::VecDeque<ReactorToken>,
}

impl ExplicitSyncWatchRegistry {
    pub fn new(refresh_interval_ns: u64, drm_file_generation: u64) -> Self {
        Self {
            watches_by_token: HashMap::new(),
            watch_by_commit: HashMap::new(),
            fallback_requests: HashMap::new(),
            fallback_schedule: FallbackSchedule::new(refresh_interval_ns),
            capability: SyncobjEventfdCapability::Unknown,
            drm_file_generation,
            metrics: ExplicitSyncWatchMetrics::default(),
            recent_completed_tokens: std::collections::VecDeque::new(),
        }
    }

    pub fn capability(&self) -> SyncobjEventfdCapability {
        self.capability
    }

    pub fn metrics(&self) -> ExplicitSyncWatchMetrics {
        let mut metrics = self.metrics;
        metrics.active_eventfd_watches = self.watches_by_token.len();
        metrics.active_fallback_watches = self.fallback_requests.len();
        metrics
    }

    pub fn next_fallback_deadline_ns(&self) -> Option<u64> {
        self.fallback_schedule.next_deadline_ns()
    }

    pub fn register<N: AcquirePointNotifier>(
        &mut self,
        request: AcquireWatchRequest,
        event_loop: &mut NativeEventLoop,
        now_ns: u64,
        notifier: &N,
    ) -> io::Result<AcquireRegistrationResult> {
        self.cancel_commit(
            request.commit_id,
            AcquireWatchCancelReason::Superseded,
            event_loop,
        )?;
        if notifier.point_signaled(&request.acquire)? {
            self.metrics.already_signaled = self.metrics.already_signaled.saturating_add(1);
            self.note_ready_latency(&request);
            return Ok(AcquireRegistrationResult::AlreadyReady(request));
        }
        if matches!(
            self.capability,
            SyncobjEventfdCapability::Unsupported | SyncobjEventfdCapability::BrokenOrRejected(_)
        ) {
            let id = request.commit_id;
            self.insert_fallback(request, now_ns);
            return Ok(AcquireRegistrationResult::FallbackBacked(id));
        }

        let event_fd = match create_event_fd() {
            Ok(event_fd) => event_fd,
            Err(error) => {
                self.metrics.registration_failures =
                    self.metrics.registration_failures.saturating_add(1);
                self.metrics.last_registration_errno = error.raw_os_error().unwrap_or(0);
                let id = request.commit_id;
                self.insert_fallback(request, now_ns);
                return Ok(AcquireRegistrationResult::FallbackBacked(id));
            }
        };
        if let Err(error) = notifier.register_eventfd(&request.acquire, event_fd.as_fd()) {
            self.metrics.registration_failures =
                self.metrics.registration_failures.saturating_add(1);
            self.metrics.last_registration_errno = error.raw_os_error().unwrap_or(0);
            match error.class() {
                SyncobjEventfdErrnoClass::Unsupported => {
                    self.capability = SyncobjEventfdCapability::Unsupported;
                    let id = request.commit_id;
                    self.insert_fallback(request, now_ns);
                    return Ok(AcquireRegistrationResult::FallbackBacked(id));
                }
                SyncobjEventfdErrnoClass::Failure(errno)
                    if errno != libc::EINVAL && errno != libc::EBADF =>
                {
                    self.capability = SyncobjEventfdCapability::BrokenOrRejected(errno);
                    let id = request.commit_id;
                    self.insert_fallback(request, now_ns);
                    return Ok(AcquireRegistrationResult::FallbackBacked(id));
                }
                SyncobjEventfdErrnoClass::Failure(_) => return Err(error.into_io_error()),
            }
        }
        self.capability = SyncobjEventfdCapability::Supported;
        let token = match event_loop
            .register(event_fd.as_raw_fd(), NativeEventSource::ExplicitSyncAcquire)
        {
            Ok(token) => token,
            Err(error) => {
                self.metrics.registration_failures =
                    self.metrics.registration_failures.saturating_add(1);
                self.metrics.last_registration_errno = error.raw_os_error().unwrap_or(0);
                let id = request.commit_id;
                self.insert_fallback(request, now_ns);
                return Ok(AcquireRegistrationResult::FallbackBacked(id));
            }
        };
        let commit_id = request.commit_id;
        self.watch_by_commit.insert(commit_id, token);
        self.watches_by_token.insert(
            token,
            AcquireWatch {
                token,
                event_fd,
                drm_file_generation: self.drm_file_generation,
                request,
            },
        );
        self.metrics.registrations = self.metrics.registrations.saturating_add(1);
        self.update_maximum_watches();

        // The final check closes the pending-check/ioctl/epoll-add race. If it
        // misses a later signal, level-triggered eventfd readability remains
        // queued for the next epoll wait.
        let final_request = self
            .watches_by_token
            .get(&token)
            .map(|watch| watch.request.clone())
            .expect("new acquire watch must exist");
        let final_signaled = match notifier.point_signaled(&final_request.acquire) {
            Ok(signaled) => signaled,
            Err(error) => {
                self.remove_eventfd_watch(token, event_loop)?;
                return Err(error);
            }
        };
        if final_signaled {
            let request = self
                .remove_eventfd_watch(token, event_loop)?
                .ok_or_else(|| {
                    io::Error::other("new acquire watch disappeared during final readiness check")
                })?;
            self.metrics.already_signaled = self.metrics.already_signaled.saturating_add(1);
            self.note_ready_latency(&request);
            return Ok(AcquireRegistrationResult::AlreadyReady(request));
        }
        Ok(AcquireRegistrationResult::EventfdBacked(commit_id))
    }

    pub fn handle_ready<N: AcquirePointNotifier>(
        &mut self,
        token: ReactorToken,
        event_loop: &mut NativeEventLoop,
        drm_file_generation: u64,
        notifier: &N,
    ) -> io::Result<AcquireReadyResult> {
        let Some(watch) = self.watches_by_token.get(&token) else {
            if self.recent_completed_tokens.contains(&token) {
                self.metrics.duplicate_wakeups = self.metrics.duplicate_wakeups.saturating_add(1);
            } else {
                self.metrics.stale_wakeups = self.metrics.stale_wakeups.saturating_add(1);
            }
            return Ok(AcquireReadyResult::Stale);
        };
        if watch.drm_file_generation != drm_file_generation {
            let commit_id = watch.request.commit_id;
            self.remove_eventfd_watch(token, event_loop)?;
            return Ok(AcquireReadyResult::BackendMismatch(commit_id));
        }
        match drain_event_fd(watch.event_fd.as_fd())? {
            EventFdDrain::Pending => return Ok(AcquireReadyResult::Pending),
            EventFdDrain::Ready(_) => {}
        }
        self.metrics.eventfd_wakeups = self.metrics.eventfd_wakeups.saturating_add(1);
        if !notifier.point_signaled(&watch.request.acquire)? {
            return Ok(AcquireReadyResult::Pending);
        }
        let request = self
            .remove_eventfd_watch(token, event_loop)?
            .ok_or_else(|| io::Error::other("ready acquire watch disappeared"))?;
        self.note_completed_token(token);
        self.note_ready_latency(&request);
        Ok(AcquireReadyResult::Ready(request))
    }

    pub fn retry_fallback<N: AcquirePointNotifier>(
        &mut self,
        now_ns: u64,
        notifier: &N,
    ) -> Vec<AcquireWatchRequest> {
        let due = self
            .fallback_requests
            .keys()
            .copied()
            .filter(|id| {
                self.fallback_schedule
                    .deadline_for(*id)
                    .is_some_and(|deadline| now_ns >= deadline)
            })
            .collect::<Vec<_>>();
        let mut ready = Vec::new();
        for id in due {
            let signaled = self
                .fallback_requests
                .get(&id)
                .is_some_and(|request| notifier.point_signaled(&request.acquire).unwrap_or(false));
            if signaled {
                self.fallback_schedule.remove(id);
                if let Some(request) = self.fallback_requests.remove(&id) {
                    self.note_ready_latency(&request);
                    ready.push(request);
                }
            } else {
                self.fallback_schedule.note_pending(id, now_ns);
            }
        }
        ready
    }

    pub fn cancel_commit(
        &mut self,
        commit_id: AcquireCommitId,
        reason: AcquireWatchCancelReason,
        event_loop: &mut NativeEventLoop,
    ) -> io::Result<bool> {
        let removed_eventfd = if let Some(token) = self.watch_by_commit.get(&commit_id).copied() {
            self.remove_eventfd_watch(token, event_loop)?.is_some()
        } else {
            false
        };
        let removed_fallback = self.fallback_requests.remove(&commit_id).is_some();
        self.fallback_schedule.remove(commit_id);
        if removed_eventfd || removed_fallback {
            self.metrics.cancellations = self.metrics.cancellations.saturating_add(1);
            self.metrics.cancellations_by_reason[cancellation_reason_index(reason)] =
                self.metrics.cancellations_by_reason[cancellation_reason_index(reason)]
                    .saturating_add(1);
        }
        Ok(removed_eventfd || removed_fallback)
    }

    pub fn shutdown(&mut self, event_loop: &mut NativeEventLoop) -> io::Result<()> {
        let tokens = self.watches_by_token.keys().copied().collect::<Vec<_>>();
        for token in tokens {
            self.remove_eventfd_watch(token, event_loop)?;
        }
        self.fallback_requests.clear();
        self.fallback_schedule.entries.clear();
        if !self.watches_by_token.is_empty() || !self.watch_by_commit.is_empty() {
            self.metrics.leaked_watch_assertions =
                self.metrics.leaked_watch_assertions.saturating_add(1);
            return Err(io::Error::other(
                "explicit sync watch registry did not drain during shutdown",
            ));
        }
        Ok(())
    }

    fn insert_fallback(&mut self, request: AcquireWatchRequest, now_ns: u64) {
        let commit_id = request.commit_id;
        self.fallback_requests.insert(commit_id, request);
        self.fallback_schedule.insert(commit_id, now_ns);
        self.metrics.fallback_activations = self.metrics.fallback_activations.saturating_add(1);
        self.update_maximum_watches();
    }

    fn remove_eventfd_watch(
        &mut self,
        token: ReactorToken,
        event_loop: &mut NativeEventLoop,
    ) -> io::Result<Option<AcquireWatchRequest>> {
        let Some(watch) = self.watches_by_token.get(&token) else {
            return Ok(None);
        };
        debug_assert_eq!(watch.token, token);
        event_loop.unregister(token)?;
        let watch = self
            .watches_by_token
            .remove(&token)
            .expect("acquire watch existed before epoll removal");
        self.watch_by_commit.remove(&watch.request.commit_id);
        Ok(Some(watch.request))
    }

    fn update_maximum_watches(&mut self) {
        self.metrics.maximum_simultaneous_watches = self
            .metrics
            .maximum_simultaneous_watches
            .max(self.watches_by_token.len() + self.fallback_requests.len());
    }

    fn note_completed_token(&mut self, token: ReactorToken) {
        const RECENT_TOKEN_LIMIT: usize = 256;
        if self.recent_completed_tokens.len() == RECENT_TOKEN_LIMIT {
            self.recent_completed_tokens.pop_front();
        }
        self.recent_completed_tokens.push_back(token);
    }

    fn note_ready_latency(&mut self, request: &AcquireWatchRequest) {
        self.metrics.last_commit_to_ready_ns =
            u64::try_from(request.received_at.elapsed().as_nanos()).unwrap_or(u64::MAX);
    }
}

const fn cancellation_reason_index(reason: AcquireWatchCancelReason) -> usize {
    match reason {
        AcquireWatchCancelReason::Superseded => 0,
        AcquireWatchCancelReason::SurfaceDestroyed => 1,
        AcquireWatchCancelReason::BufferDestroyed => 2,
        AcquireWatchCancelReason::SyncSurfaceDestroyed => 3,
        AcquireWatchCancelReason::TimelineDestroyed => 4,
        AcquireWatchCancelReason::ClientDisconnected => 5,
        AcquireWatchCancelReason::BackendShutdown => 6,
        AcquireWatchCancelReason::Rejected => 7,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventFdDrain {
    Pending,
    Ready(u64),
}

fn create_event_fd() -> io::Result<OwnedFd> {
    let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: eventfd returned a newly owned descriptor on success.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn drain_event_fd(event_fd: BorrowedFd<'_>) -> io::Result<EventFdDrain> {
    loop {
        let mut counter = 0u64;
        let read = unsafe {
            libc::read(
                event_fd.as_raw_fd(),
                (&mut counter as *mut u64).cast(),
                std::mem::size_of::<u64>(),
            )
        };
        if read == std::mem::size_of::<u64>() as isize {
            return Ok(EventFdDrain::Ready(counter));
        }
        if read < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() == io::ErrorKind::WouldBlock {
                return Ok(EventFdDrain::Pending);
            }
            return Err(error);
        }
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("eventfd read returned {read} bytes instead of 8"),
        ));
    }
}

impl FallbackSchedule {
    fn new(retry_interval_ns: u64) -> Self {
        Self {
            retry_interval_ns: retry_interval_ns.max(1),
            entries: HashMap::new(),
        }
    }

    fn insert(&mut self, commit_id: AcquireCommitId, now_ns: u64) {
        self.entries.insert(
            commit_id,
            FallbackEntry {
                deadline_ns: now_ns.saturating_add(self.retry_interval_ns),
            },
        );
    }

    fn remove(&mut self, commit_id: AcquireCommitId) -> bool {
        self.entries.remove(&commit_id).is_some()
    }

    fn next_deadline_ns(&self) -> Option<u64> {
        self.entries.values().map(|entry| entry.deadline_ns).min()
    }

    fn deadline_for(&self, commit_id: AcquireCommitId) -> Option<u64> {
        self.entries.get(&commit_id).map(|entry| entry.deadline_ns)
    }

    fn note_pending(&mut self, commit_id: AcquireCommitId, observed_ns: u64) {
        let Some(entry) = self.entries.get_mut(&commit_id) else {
            return;
        };
        while entry.deadline_ns <= observed_ns {
            let next = entry.deadline_ns.saturating_add(self.retry_interval_ns);
            if next == entry.deadline_ns {
                break;
            }
            entry.deadline_ns = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, time::Instant};

    use super::*;

    struct FakeNotifier {
        signaled: Cell<bool>,
        signal_during_registration: bool,
        registration_errno: Option<i32>,
    }

    impl FakeNotifier {
        fn pending() -> Self {
            Self {
                signaled: Cell::new(false),
                signal_during_registration: false,
                registration_errno: None,
            }
        }
    }

    impl AcquirePointNotifier for FakeNotifier {
        fn point_signaled(&self, _point: &ExplicitSyncPoint) -> io::Result<bool> {
            Ok(self.signaled.get())
        }

        fn register_eventfd(
            &self,
            _point: &ExplicitSyncPoint,
            _event_fd: BorrowedFd<'_>,
        ) -> Result<(), SyncobjEventfdError> {
            if let Some(errno) = self.registration_errno {
                return Err(SyncobjEventfdError::from_errno(errno));
            }
            if self.signal_during_registration {
                self.signaled.set(true);
            }
            Ok(())
        }
    }

    fn request(id: u64, surface_id: u32) -> AcquireWatchRequest {
        AcquireWatchRequest {
            commit_id: AcquireCommitId::for_tests(id),
            surface_id,
            buffer_id: id as u32 + 100,
            acquire: ExplicitSyncPoint::for_tests(id as u32, id + 10),
            received_at: Instant::now(),
        }
    }

    fn signal_counter(fd: BorrowedFd<'_>, value: u64) {
        let written = unsafe {
            libc::write(
                fd.as_raw_fd(),
                (&value as *const u64).cast(),
                std::mem::size_of::<u64>(),
            )
        };
        assert_eq!(written, std::mem::size_of::<u64>() as isize);
    }

    #[test]
    fn fallback_deadline_exists_only_while_fallback_watches_exist() {
        let mut fallback = FallbackSchedule::new(16_666_666);

        assert_eq!(fallback.next_deadline_ns(), None);
        fallback.insert(AcquireCommitId::for_tests(1), 100);
        assert_eq!(fallback.next_deadline_ns(), Some(16_666_766));
        assert!(fallback.remove(AcquireCommitId::for_tests(1)));
        assert_eq!(fallback.next_deadline_ns(), None);
    }

    #[test]
    fn fallback_deadlines_advance_absolutely_without_drift() {
        let mut fallback = FallbackSchedule::new(10);
        let id = AcquireCommitId::for_tests(1);
        fallback.insert(id, 100);

        fallback.note_pending(id, 145);

        assert_eq!(fallback.deadline_for(id), Some(150));
    }

    #[test]
    fn capability_distinguishes_supported_unsupported_and_broken() {
        assert_ne!(
            SyncobjEventfdCapability::Supported,
            SyncobjEventfdCapability::Unsupported
        );
        assert_eq!(
            SyncobjEventfdCapability::BrokenOrRejected(libc::EINVAL),
            SyncobjEventfdCapability::BrokenOrRejected(libc::EINVAL)
        );
    }

    #[test]
    fn eventfd_wakeup_completes_exact_watch_once() {
        let notifier = FakeNotifier::pending();
        let mut event_loop = NativeEventLoop::new().unwrap();
        let mut registry = ExplicitSyncWatchRegistry::new(10, 7);
        let registration = registry
            .register(request(1, 20), &mut event_loop, 100, &notifier)
            .unwrap();
        let AcquireRegistrationResult::EventfdBacked(commit_id) = registration else {
            panic!("expected eventfd-backed registration");
        };
        let token = registry.watch_by_commit[&commit_id];
        let watch = &registry.watches_by_token[&token];
        signal_counter(watch.event_fd.as_fd(), 3);
        notifier.signaled.set(true);

        let wakeup = event_loop.wait().unwrap();
        assert_eq!(wakeup.explicit_sync_acquire_tokens, vec![token]);
        let result = registry
            .handle_ready(token, &mut event_loop, 7, &notifier)
            .unwrap();
        let AcquireReadyResult::Ready(ready) = result else {
            panic!("expected exact ready result");
        };
        assert_eq!(ready.commit_id, commit_id);
        assert!(matches!(
            registry
                .handle_ready(token, &mut event_loop, 7, &notifier)
                .unwrap(),
            AcquireReadyResult::Stale
        ));
        assert_eq!(registry.metrics().active_eventfd_watches, 0);
    }

    #[test]
    fn signal_during_registration_uses_fast_path_without_retained_watch() {
        let notifier = FakeNotifier {
            signaled: Cell::new(false),
            signal_during_registration: true,
            registration_errno: None,
        };
        let mut event_loop = NativeEventLoop::new().unwrap();
        let mut registry = ExplicitSyncWatchRegistry::new(10, 7);

        let result = registry
            .register(request(1, 20), &mut event_loop, 100, &notifier)
            .unwrap();

        assert!(matches!(result, AcquireRegistrationResult::AlreadyReady(_)));
        assert_eq!(registry.metrics().active_eventfd_watches, 0);
        assert_eq!(registry.metrics().already_signaled, 1);
    }

    #[test]
    fn unsupported_ioctl_activates_only_bounded_fallback() {
        let notifier = FakeNotifier {
            signaled: Cell::new(false),
            signal_during_registration: false,
            registration_errno: Some(libc::ENOTTY),
        };
        let mut event_loop = NativeEventLoop::new().unwrap();
        let mut registry = ExplicitSyncWatchRegistry::new(10, 7);

        assert!(matches!(
            registry
                .register(request(1, 20), &mut event_loop, 100, &notifier)
                .unwrap(),
            AcquireRegistrationResult::FallbackBacked(_)
        ));
        assert_eq!(registry.capability(), SyncobjEventfdCapability::Unsupported);
        assert_eq!(registry.next_fallback_deadline_ns(), Some(110));
        assert!(registry.retry_fallback(109, &notifier).is_empty());
        notifier.signaled.set(true);
        assert_eq!(registry.retry_fallback(110, &notifier).len(), 1);
        assert_eq!(registry.next_fallback_deadline_ns(), None);
    }

    #[test]
    fn canceled_watch_turns_already_returned_token_stale() {
        let notifier = FakeNotifier::pending();
        let mut event_loop = NativeEventLoop::new().unwrap();
        let mut registry = ExplicitSyncWatchRegistry::new(10, 7);
        let AcquireRegistrationResult::EventfdBacked(id) = registry
            .register(request(1, 20), &mut event_loop, 100, &notifier)
            .unwrap()
        else {
            panic!("expected eventfd watch");
        };
        let token = registry.watch_by_commit[&id];

        assert!(
            registry
                .cancel_commit(id, AcquireWatchCancelReason::Superseded, &mut event_loop)
                .unwrap()
        );
        assert!(matches!(
            registry
                .handle_ready(token, &mut event_loop, 7, &notifier)
                .unwrap(),
            AcquireReadyResult::Stale
        ));
    }

    #[test]
    fn defensive_check_keeps_watch_when_counter_wakes_before_point_is_signaled() {
        let notifier = FakeNotifier::pending();
        let mut event_loop = NativeEventLoop::new().unwrap();
        let mut registry = ExplicitSyncWatchRegistry::new(10, 7);
        let AcquireRegistrationResult::EventfdBacked(id) = registry
            .register(request(1, 20), &mut event_loop, 100, &notifier)
            .unwrap()
        else {
            panic!("expected eventfd watch");
        };
        let token = registry.watch_by_commit[&id];
        signal_counter(registry.watches_by_token[&token].event_fd.as_fd(), 1);

        assert!(matches!(
            registry
                .handle_ready(token, &mut event_loop, 7, &notifier)
                .unwrap(),
            AcquireReadyResult::Pending
        ));
        assert_eq!(registry.metrics().active_eventfd_watches, 1);
    }

    #[test]
    fn backend_generation_mismatch_cancels_without_readiness() {
        let notifier = FakeNotifier::pending();
        let mut event_loop = NativeEventLoop::new().unwrap();
        let mut registry = ExplicitSyncWatchRegistry::new(10, 7);
        let AcquireRegistrationResult::EventfdBacked(id) = registry
            .register(request(1, 20), &mut event_loop, 100, &notifier)
            .unwrap()
        else {
            panic!("expected eventfd watch");
        };
        let token = registry.watch_by_commit[&id];

        assert!(matches!(
            registry
                .handle_ready(token, &mut event_loop, 8, &notifier)
                .unwrap(),
            AcquireReadyResult::BackendMismatch(mismatch) if mismatch == id
        ));
        assert_eq!(registry.metrics().active_eventfd_watches, 0);
    }

    #[test]
    fn shutdown_removes_all_eventfd_and_fallback_watches() {
        let notifier = FakeNotifier::pending();
        let mut event_loop = NativeEventLoop::new().unwrap();
        let mut registry = ExplicitSyncWatchRegistry::new(10, 7);
        registry
            .register(request(1, 20), &mut event_loop, 100, &notifier)
            .unwrap();
        registry.capability = SyncobjEventfdCapability::Unsupported;
        registry
            .register(request(2, 21), &mut event_loop, 100, &notifier)
            .unwrap();

        registry.shutdown(&mut event_loop).unwrap();

        let metrics = registry.metrics();
        assert_eq!(metrics.active_eventfd_watches, 0);
        assert_eq!(metrics.active_fallback_watches, 0);
        assert_eq!(registry.next_fallback_deadline_ns(), None);
        assert_eq!(metrics.leaked_watch_assertions, 0);
    }
}
