//! Linux readiness waiting for the native compositor runtime.

use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
};

const MAX_READY_EVENTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeEventSource {
    Drm,
    WaylandListener,
    WaylandClients,
    Input(u16),
    Timer,
    ExplicitSyncAcquire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReactorToken(u64);

impl ReactorToken {
    fn new(slot_index: usize, generation: u32) -> io::Result<Self> {
        let slot = u32::try_from(slot_index)
            .ok()
            .and_then(|slot| slot.checked_add(1))
            .ok_or_else(|| io::Error::other("native reactor token slots exhausted"))?;
        if generation == 0 {
            return Err(io::Error::other(
                "native reactor token generation must be nonzero",
            ));
        }
        Ok(Self((u64::from(generation) << 32) | u64::from(slot)))
    }

    fn decode(self) -> Option<(usize, u32)> {
        let slot = self.0 as u32;
        let generation = (self.0 >> 32) as u32;
        if slot == 0 || generation == 0 {
            return None;
        }
        Some(((slot - 1) as usize, generation))
    }

    const fn raw(self) -> u64 {
        self.0
    }

    const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WakeReasons(u32);

impl WakeReasons {
    const DRM: u32 = 1 << 0;
    const WAYLAND_LISTENER: u32 = 1 << 1;
    const WAYLAND_CLIENTS: u32 = 1 << 2;
    const INPUT: u32 = 1 << 3;
    const TIMER: u32 = 1 << 4;
    const EXPLICIT_SYNC_ACQUIRE: u32 = 1 << 5;

    pub const fn drm(self) -> bool {
        self.0 & Self::DRM != 0
    }

    pub const fn wayland_listener(self) -> bool {
        self.0 & Self::WAYLAND_LISTENER != 0
    }

    pub const fn wayland_clients(self) -> bool {
        self.0 & Self::WAYLAND_CLIENTS != 0
    }

    pub const fn input(self) -> bool {
        self.0 & Self::INPUT != 0
    }

    pub const fn timer(self) -> bool {
        self.0 & Self::TIMER != 0
    }

    pub const fn explicit_sync_acquire(self) -> bool {
        self.0 & Self::EXPLICIT_SYNC_ACQUIRE != 0
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    fn insert(&mut self, source: NativeEventSource) {
        self.0 |= match source {
            NativeEventSource::Drm => Self::DRM,
            NativeEventSource::WaylandListener => Self::WAYLAND_LISTENER,
            NativeEventSource::WaylandClients => Self::WAYLAND_CLIENTS,
            NativeEventSource::Input(_) => Self::INPUT,
            NativeEventSource::Timer => Self::TIMER,
            NativeEventSource::ExplicitSyncAcquire => Self::EXPLICIT_SYNC_ACQUIRE,
        };
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeWakeup {
    pub reasons: WakeReasons,
    pub ready_sources: usize,
    pub blocked_ns: u64,
    pub timer_lateness_ns: Option<u64>,
    pub explicit_sync_acquire_tokens: Vec<ReactorToken>,
}

#[derive(Debug, Clone, Copy)]
struct Registration {
    fd: RawFd,
    source: NativeEventSource,
}

#[derive(Debug)]
struct RegistrationSlot {
    generation: u32,
    registration: Option<Registration>,
}

#[derive(Debug)]
pub struct NativeEventLoop {
    epoll: OwnedFd,
    timer: OwnedFd,
    registrations: Vec<RegistrationSlot>,
    free_registration_slots: Vec<usize>,
    events: Vec<libc::epoll_event>,
    armed_deadline_ns: Option<u64>,
}

impl NativeEventLoop {
    pub fn new() -> io::Result<Self> {
        let epoll_fd = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
        if epoll_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let timer_fd = unsafe {
            libc::timerfd_create(
                libc::CLOCK_MONOTONIC,
                libc::TFD_CLOEXEC | libc::TFD_NONBLOCK,
            )
        };
        if timer_fd < 0 {
            let error = io::Error::last_os_error();
            unsafe { libc::close(epoll_fd) };
            return Err(error);
        }

        let mut event_loop = Self {
            epoll: unsafe { OwnedFd::from_raw_fd(epoll_fd) },
            timer: unsafe { OwnedFd::from_raw_fd(timer_fd) },
            registrations: Vec::new(),
            free_registration_slots: Vec::new(),
            events: vec![libc::epoll_event { events: 0, u64: 0 }; MAX_READY_EVENTS],
            armed_deadline_ns: None,
        };
        event_loop.register_raw(timer_fd, NativeEventSource::Timer)?;
        Ok(event_loop)
    }

    pub fn register(&mut self, fd: RawFd, source: NativeEventSource) -> io::Result<ReactorToken> {
        if source == NativeEventSource::Timer {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the timer source is owned by the event loop",
            ));
        }
        self.register_raw(fd, source)
    }

    pub fn unregister(&mut self, token: ReactorToken) -> io::Result<bool> {
        let Some((slot_index, generation)) = token.decode() else {
            return Ok(false);
        };
        let Some(slot) = self.registrations.get(slot_index) else {
            return Ok(false);
        };
        if slot.generation != generation {
            return Ok(false);
        }
        let Some(registration) = slot.registration else {
            return Ok(false);
        };
        let result = unsafe {
            libc::epoll_ctl(
                self.epoll.as_raw_fd(),
                libc::EPOLL_CTL_DEL,
                registration.fd,
                std::ptr::null_mut(),
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        let slot = &mut self.registrations[slot_index];
        slot.registration = None;
        if slot.generation != u32::MAX {
            slot.generation += 1;
            self.free_registration_slots.push(slot_index);
        }
        Ok(true)
    }

    #[cfg(test)]
    fn source_for_token(&self, token: ReactorToken) -> Option<NativeEventSource> {
        let (slot_index, generation) = token.decode()?;
        let slot = self.registrations.get(slot_index)?;
        (slot.generation == generation)
            .then_some(slot.registration?)
            .map(|registration| registration.source)
    }

    pub fn arm_deadline(&mut self, deadline_ns: Option<u64>) -> io::Result<()> {
        let value = deadline_ns.unwrap_or(0);
        let timer_spec = libc::itimerspec {
            it_interval: libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            it_value: libc::timespec {
                tv_sec: (value / 1_000_000_000) as libc::time_t,
                tv_nsec: (value % 1_000_000_000) as libc::c_long,
            },
        };
        let result = unsafe {
            libc::timerfd_settime(
                self.timer.as_raw_fd(),
                libc::TFD_TIMER_ABSTIME,
                &timer_spec,
                std::ptr::null_mut(),
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        self.armed_deadline_ns = deadline_ns;
        Ok(())
    }

    pub fn wait(&mut self) -> io::Result<NativeWakeup> {
        let wait_started_ns = monotonic_now_ns()?;
        let ready = retry_interrupted(|| {
            let result = unsafe {
                libc::epoll_wait(
                    self.epoll.as_raw_fd(),
                    self.events.as_mut_ptr(),
                    self.events.len() as libc::c_int,
                    -1,
                )
            };
            if result < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(result as usize)
            }
        })?;
        let observed_ns = monotonic_now_ns()?;
        let mut reasons = WakeReasons::default();
        let mut explicit_sync_acquire_tokens = Vec::new();

        for index in 0..ready {
            let event = self.events[index];
            let event_flags = event.events;
            let token = ReactorToken::from_raw(event.u64);
            let Some((registration_index, generation)) = token.decode() else {
                continue;
            };
            let Some(slot) = self.registrations.get(registration_index) else {
                continue;
            };
            if slot.generation != generation {
                continue;
            }
            let Some(registration) = slot.registration else {
                continue;
            };
            let error_events =
                libc::EPOLLERR as u32 | libc::EPOLLHUP as u32 | libc::EPOLLRDHUP as u32;
            if event_flags & error_events != 0 {
                let _ = self.unregister(token);
                return Err(io::Error::other(format!(
                    "native event source {:?} fd {} reported readiness error 0x{:x}",
                    registration.source, registration.fd, event_flags
                )));
            }
            if event_flags & libc::EPOLLIN as u32 != 0 {
                reasons.insert(registration.source);
                if registration.source == NativeEventSource::ExplicitSyncAcquire {
                    explicit_sync_acquire_tokens.push(token);
                }
            }
        }

        if reasons.timer() {
            self.drain_timer()?;
        }
        let timer_lateness_ns = reasons
            .timer()
            .then(|| {
                self.armed_deadline_ns
                    .map(|deadline| observed_ns.saturating_sub(deadline))
            })
            .flatten();
        Ok(NativeWakeup {
            reasons,
            ready_sources: ready,
            blocked_ns: observed_ns.saturating_sub(wait_started_ns),
            timer_lateness_ns,
            explicit_sync_acquire_tokens,
        })
    }

    fn register_raw(&mut self, fd: RawFd, source: NativeEventSource) -> io::Result<ReactorToken> {
        if fd < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("cannot register invalid fd {fd}"),
            ));
        }
        let (slot_index, generation, reusing_slot) =
            if let Some(slot_index) = self.free_registration_slots.last().copied() {
                (slot_index, self.registrations[slot_index].generation, true)
            } else {
                (self.registrations.len(), 1, false)
            };
        let token = ReactorToken::new(slot_index, generation)?;
        let mut event = libc::epoll_event {
            events: (libc::EPOLLIN | libc::EPOLLERR | libc::EPOLLHUP | libc::EPOLLRDHUP) as u32,
            u64: token.raw(),
        };
        let result =
            unsafe { libc::epoll_ctl(self.epoll.as_raw_fd(), libc::EPOLL_CTL_ADD, fd, &mut event) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        let registration = Registration { fd, source };
        if reusing_slot {
            self.free_registration_slots.pop();
            self.registrations[slot_index].registration = Some(registration);
        } else {
            self.registrations.push(RegistrationSlot {
                generation,
                registration: Some(registration),
            });
        }
        Ok(token)
    }

    fn drain_timer(&self) -> io::Result<()> {
        let mut expirations = 0u64;
        let read = unsafe {
            libc::read(
                self.timer.as_raw_fd(),
                (&mut expirations as *mut u64).cast(),
                std::mem::size_of::<u64>(),
            )
        };
        if read < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::WouldBlock {
                return Err(error);
            }
        }
        Ok(())
    }
}

impl Drop for NativeEventLoop {
    fn drop(&mut self) {
        for registration in self
            .registrations
            .iter()
            .filter_map(|slot| slot.registration)
        {
            unsafe {
                libc::epoll_ctl(
                    self.epoll.as_raw_fd(),
                    libc::EPOLL_CTL_DEL,
                    registration.fd,
                    std::ptr::null_mut(),
                );
            }
        }
    }
}

pub fn monotonic_now_ns() -> io::Result<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((time.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(time.tv_nsec as u64))
}

fn retry_interrupted<T>(mut operation: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    loop {
        match operation() {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsRawFd;

    use crate::native::scheduler::{NativeFrameScheduler, SchedulerDecision};

    use super::*;

    fn event_fd() -> OwnedFd {
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        assert!(fd >= 0);
        unsafe { OwnedFd::from_raw_fd(fd) }
    }

    fn signal(fd: RawFd) {
        let value = 1u64;
        let written = unsafe {
            libc::write(
                fd,
                (&value as *const u64).cast(),
                std::mem::size_of::<u64>(),
            )
        };
        assert_eq!(written as usize, std::mem::size_of::<u64>());
    }

    #[test]
    fn input_readiness_wakes_before_future_refresh_deadline() {
        let input = event_fd();
        let mut event_loop = NativeEventLoop::new().unwrap();
        event_loop
            .register(input.as_raw_fd(), NativeEventSource::Input(0))
            .unwrap();
        event_loop
            .arm_deadline(Some(monotonic_now_ns().unwrap() + 1_000_000_000))
            .unwrap();

        signal(input.as_raw_fd());
        let wakeup = event_loop.wait().unwrap();

        assert!(wakeup.reasons.input());
        assert!(!wakeup.reasons.timer());
    }

    #[test]
    fn listener_readiness_requests_client_acceptance() {
        let listener = event_fd();
        let mut event_loop = NativeEventLoop::new().unwrap();
        event_loop
            .register(listener.as_raw_fd(), NativeEventSource::WaylandListener)
            .unwrap();

        signal(listener.as_raw_fd());

        assert!(event_loop.wait().unwrap().reasons.wayland_listener());
    }

    #[test]
    fn client_readiness_requests_wayland_dispatch() {
        let clients = event_fd();
        let mut event_loop = NativeEventLoop::new().unwrap();
        event_loop
            .register(clients.as_raw_fd(), NativeEventSource::WaylandClients)
            .unwrap();

        signal(clients.as_raw_fd());

        assert!(event_loop.wait().unwrap().reasons.wayland_clients());
    }

    #[test]
    fn client_readiness_wakes_while_page_flip_is_pending() {
        let clients = event_fd();
        let mut event_loop = NativeEventLoop::new().unwrap();
        event_loop
            .register(clients.as_raw_fd(), NativeEventSource::WaylandClients)
            .unwrap();
        let mut scheduler = NativeFrameScheduler::new(60, 0);
        scheduler.queue_visual_work();
        scheduler.note_async_submission(41, 1).unwrap();

        signal(clients.as_raw_fd());
        let wakeup = event_loop.wait().unwrap();

        assert!(wakeup.reasons.wayland_clients());
        assert_eq!(scheduler.decision(2), SchedulerDecision::WaitForPageFlip);
    }

    #[test]
    fn drm_completion_is_observed_before_next_render_decision() {
        let drm = event_fd();
        let mut event_loop = NativeEventLoop::new().unwrap();
        event_loop
            .register(drm.as_raw_fd(), NativeEventSource::Drm)
            .unwrap();
        let mut scheduler = NativeFrameScheduler::new(60, 0);
        scheduler.queue_visual_work();
        scheduler.note_async_submission(41, 1).unwrap();
        scheduler.queue_visual_work();

        signal(drm.as_raw_fd());
        assert!(event_loop.wait().unwrap().reasons.drm());
        assert_eq!(
            scheduler.note_page_flip_completion(41, 2),
            crate::native::scheduler::PageFlipCompletionResult::Completed { submitted_at_ns: 1 }
        );

        assert_eq!(scheduler.decision(2), SchedulerDecision::Render);
    }

    #[test]
    fn simultaneous_sources_are_returned_in_one_wakeup() {
        let drm = event_fd();
        let input = event_fd();
        let clients = event_fd();
        let mut event_loop = NativeEventLoop::new().unwrap();
        event_loop
            .register(drm.as_raw_fd(), NativeEventSource::Drm)
            .unwrap();
        event_loop
            .register(input.as_raw_fd(), NativeEventSource::Input(0))
            .unwrap();
        event_loop
            .register(clients.as_raw_fd(), NativeEventSource::WaylandClients)
            .unwrap();

        signal(drm.as_raw_fd());
        signal(input.as_raw_fd());
        signal(clients.as_raw_fd());
        let wakeup = event_loop.wait().unwrap();

        assert!(wakeup.reasons.drm());
        assert!(wakeup.reasons.input());
        assert!(wakeup.reasons.wayland_clients());
        assert_eq!(wakeup.ready_sources, 3);
    }

    #[test]
    fn interrupted_wait_operation_is_retried() {
        let mut calls = 0;
        let result = retry_interrupted(|| {
            calls += 1;
            if calls == 1 {
                Err(io::Error::from(io::ErrorKind::Interrupted))
            } else {
                Ok(7)
            }
        });

        assert_eq!(result.unwrap(), 7);
        assert_eq!(calls, 2);
    }

    #[test]
    fn hup_source_is_disabled_and_returns_error() {
        let mut pipe = [0; 2];
        assert_eq!(
            unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) },
            0
        );
        let read = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
        let write = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
        let mut event_loop = NativeEventLoop::new().unwrap();
        event_loop
            .register(read.as_raw_fd(), NativeEventSource::Input(0))
            .unwrap();

        drop(write);
        let error = event_loop.wait().unwrap_err();

        assert!(error.to_string().contains("readiness error"));
    }

    #[test]
    fn absolute_timer_reports_lateness() {
        let mut event_loop = NativeEventLoop::new().unwrap();
        event_loop
            .arm_deadline(Some(monotonic_now_ns().unwrap() + 1_000_000))
            .unwrap();

        let wakeup = event_loop.wait().unwrap();

        assert!(wakeup.reasons.timer());
        assert!(wakeup.timer_lateness_ns.is_some());
    }

    #[test]
    fn explicit_sync_readiness_returns_its_registration_token() {
        let acquire = event_fd();
        let mut event_loop = NativeEventLoop::new().unwrap();
        let token = event_loop
            .register(acquire.as_raw_fd(), NativeEventSource::ExplicitSyncAcquire)
            .unwrap();

        signal(acquire.as_raw_fd());
        let wakeup = event_loop.wait().unwrap();

        assert!(wakeup.reasons.explicit_sync_acquire());
        assert_eq!(wakeup.explicit_sync_acquire_tokens, vec![token]);
    }

    #[test]
    fn removed_registration_token_is_stale() {
        let acquire = event_fd();
        let mut event_loop = NativeEventLoop::new().unwrap();
        let token = event_loop
            .register(acquire.as_raw_fd(), NativeEventSource::ExplicitSyncAcquire)
            .unwrap();

        assert!(event_loop.unregister(token).unwrap());
        assert!(!event_loop.unregister(token).unwrap());
        assert_eq!(event_loop.source_for_token(token), None);
    }

    #[test]
    fn reused_registration_slot_changes_generation() {
        let first = event_fd();
        let second = event_fd();
        let mut event_loop = NativeEventLoop::new().unwrap();
        let first_token = event_loop
            .register(first.as_raw_fd(), NativeEventSource::ExplicitSyncAcquire)
            .unwrap();
        event_loop.unregister(first_token).unwrap();

        let second_token = event_loop
            .register(second.as_raw_fd(), NativeEventSource::ExplicitSyncAcquire)
            .unwrap();

        assert_ne!(first_token, second_token);
        assert_eq!(event_loop.source_for_token(first_token), None);
        assert_eq!(
            event_loop.source_for_token(second_token),
            Some(NativeEventSource::ExplicitSyncAcquire)
        );
    }

    #[test]
    fn numeric_fd_reuse_does_not_reuse_registration_identity() {
        let first = event_fd();
        let reused_fd_number = first.as_raw_fd();
        let mut event_loop = NativeEventLoop::new().unwrap();
        let first_token = event_loop
            .register(reused_fd_number, NativeEventSource::ExplicitSyncAcquire)
            .unwrap();
        event_loop.unregister(first_token).unwrap();
        drop(first);

        let replacement = event_fd();
        let replacement_token = event_loop
            .register(
                replacement.as_raw_fd(),
                NativeEventSource::ExplicitSyncAcquire,
            )
            .unwrap();

        assert_ne!(first_token, replacement_token);
        assert_eq!(event_loop.source_for_token(first_token), None);
    }
}
