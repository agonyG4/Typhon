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
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WakeReasons(u32);

impl WakeReasons {
    const DRM: u32 = 1 << 0;
    const WAYLAND_LISTENER: u32 = 1 << 1;
    const WAYLAND_CLIENTS: u32 = 1 << 2;
    const INPUT: u32 = 1 << 3;
    const TIMER: u32 = 1 << 4;

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
        };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeWakeup {
    pub reasons: WakeReasons,
    pub ready_sources: usize,
    pub blocked_ns: u64,
    pub timer_lateness_ns: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct Registration {
    fd: RawFd,
    source: NativeEventSource,
    active: bool,
}

#[derive(Debug)]
pub struct NativeEventLoop {
    epoll: OwnedFd,
    timer: OwnedFd,
    registrations: Vec<Registration>,
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
            events: vec![libc::epoll_event { events: 0, u64: 0 }; MAX_READY_EVENTS],
            armed_deadline_ns: None,
        };
        event_loop.register_raw(timer_fd, NativeEventSource::Timer)?;
        Ok(event_loop)
    }

    pub fn register(&mut self, fd: RawFd, source: NativeEventSource) -> io::Result<()> {
        if source == NativeEventSource::Timer {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the timer source is owned by the event loop",
            ));
        }
        self.register_raw(fd, source)
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

        for index in 0..ready {
            let event = self.events[index];
            let event_flags = event.events;
            let registration_index = usize::try_from(event.u64)
                .ok()
                .and_then(|token| token.checked_sub(1))
                .ok_or_else(|| io::Error::other("epoll returned an invalid source token"))?;
            let registration = *self
                .registrations
                .get(registration_index)
                .ok_or_else(|| io::Error::other("epoll returned an unknown source token"))?;
            if !registration.active {
                continue;
            }
            let error_events =
                libc::EPOLLERR as u32 | libc::EPOLLHUP as u32 | libc::EPOLLRDHUP as u32;
            if event_flags & error_events != 0 {
                self.disable_registration(registration_index);
                return Err(io::Error::other(format!(
                    "native event source {:?} fd {} reported readiness error 0x{:x}",
                    registration.source, registration.fd, event_flags
                )));
            }
            if event_flags & libc::EPOLLIN as u32 != 0 {
                reasons.insert(registration.source);
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
        })
    }

    fn register_raw(&mut self, fd: RawFd, source: NativeEventSource) -> io::Result<()> {
        if fd < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("cannot register invalid fd {fd}"),
            ));
        }
        let token = self.registrations.len() + 1;
        let mut event = libc::epoll_event {
            events: (libc::EPOLLIN | libc::EPOLLERR | libc::EPOLLHUP | libc::EPOLLRDHUP) as u32,
            u64: token as u64,
        };
        let result =
            unsafe { libc::epoll_ctl(self.epoll.as_raw_fd(), libc::EPOLL_CTL_ADD, fd, &mut event) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        self.registrations.push(Registration {
            fd,
            source,
            active: true,
        });
        Ok(())
    }

    fn disable_registration(&mut self, index: usize) {
        let Some(registration) = self.registrations.get_mut(index) else {
            return;
        };
        if registration.active {
            unsafe {
                libc::epoll_ctl(
                    self.epoll.as_raw_fd(),
                    libc::EPOLL_CTL_DEL,
                    registration.fd,
                    std::ptr::null_mut(),
                );
            }
            registration.active = false;
        }
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
}
