use std::io;
use std::os::fd::{AsRawFd, RawFd};

use crate::process::{ChildSupervisor, ManagedProcessId};

use super::super::{
    XwaylandGeneration,
    displayfd::{self, DisplayFdLog},
    launch::ChildFdTarget,
};
use super::{DISPLAYFD_MAX_BYTES, ServiceState, XwaylandService};

impl XwaylandService {
    pub fn handle_displayfd_ready(
        &mut self,
        generation: XwaylandGeneration,
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<()> {
        self.handle_displayfd_ready_with_flags(generation, 0, supervisor)
    }

    pub fn probe_displayfd(
        &mut self,
        generation: XwaylandGeneration,
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<()> {
        self.log_displayfd_event(
            "displayfd_probe",
            Some("immediate"),
            Some(generation),
            self.process_id_for_generation(generation),
            self.displayfd_parent_fd(generation),
            self.displayfd_child_source_fd(generation),
            self.displayfd_reactor_token(generation),
            None,
            None,
        );
        self.handle_displayfd_ready_with_flags(generation, 0, supervisor)
    }

    pub fn handle_displayfd_ready_with_flags(
        &mut self,
        generation: XwaylandGeneration,
        epoll_flags: u32,
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<()> {
        let mut bytes = [0u8; 64];
        loop {
            let read_result = match &self.state {
                ServiceState::Starting(resources) if resources.generation == generation => unsafe {
                    libc::read(
                        resources.displayfd.as_raw_fd(),
                        bytes.as_mut_ptr().cast(),
                        bytes.len(),
                    )
                },
                ServiceState::Starting(_) => {
                    self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
                    return Ok(());
                }
                _ => return Ok(()),
            };
            if read_result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                if error.kind() == io::ErrorKind::WouldBlock {
                    self.log_displayfd_event(
                        "displayfd_read",
                        Some("would_block"),
                        Some(generation),
                        self.process_id_for_generation(generation),
                        self.displayfd_parent_fd(generation),
                        self.displayfd_child_source_fd(generation),
                        self.displayfd_reactor_token(generation),
                        (epoll_flags != 0).then_some(epoll_flags),
                        Some(0),
                    );
                    return Ok(());
                }
                self.log_displayfd_event(
                    "displayfd_read",
                    Some("error"),
                    Some(generation),
                    self.process_id_for_generation(generation),
                    self.displayfd_parent_fd(generation),
                    self.displayfd_child_source_fd(generation),
                    self.displayfd_reactor_token(generation),
                    (epoll_flags != 0).then_some(epoll_flags),
                    Some(0),
                );
                return self.fail_generation(supervisor, error);
            }
            self.log_displayfd_event(
                "displayfd_read",
                Some(if read_result == 0 { "eof" } else { "payload" }),
                Some(generation),
                self.process_id_for_generation(generation),
                self.displayfd_parent_fd(generation),
                self.displayfd_child_source_fd(generation),
                self.displayfd_reactor_token(generation),
                (epoll_flags != 0).then_some(epoll_flags),
                Some(read_result.max(0) as usize),
            );
            if read_result == 0 {
                if self.displayfd_is_validated(generation) {
                    return Ok(());
                }
                return self.fail_generation(
                    supervisor,
                    io::Error::new(io::ErrorKind::UnexpectedEof, "XWayland displayfd closed"),
                );
            }
            self.handle_displayfd_bytes(generation, &bytes[..read_result as usize], supervisor)?;
            if !matches!(self.state, ServiceState::Starting(_)) {
                return Ok(());
            }
        }
    }

    pub fn handle_displayfd_bytes(
        &mut self,
        generation: XwaylandGeneration,
        bytes: &[u8],
        supervisor: &mut ChildSupervisor,
    ) -> io::Result<()> {
        let reserved_display = self.display_number();
        let mut failure = None;
        let mut validated = false;
        {
            let ServiceState::Starting(resources) = &mut self.state else {
                return Ok(());
            };
            if resources.generation != generation {
                self.metrics.stale_events = self.metrics.stale_events.saturating_add(1);
                self.metrics.unauthorized_bind_attempts =
                    self.metrics.unauthorized_bind_attempts.saturating_add(1);
                return Ok(());
            }
            resources.displayfd_readable |= !bytes.is_empty();
            if resources.display_ready && !bytes.is_empty() {
                failure = Some("XWayland displayfd emitted a duplicate payload");
            } else if resources.displayfd_bytes.len().saturating_add(bytes.len())
                > DISPLAYFD_MAX_BYTES
            {
                failure = Some("XWayland displayfd payload is oversized");
            } else {
                resources.displayfd_bytes.extend_from_slice(bytes);
                let Some(newline) = resources
                    .displayfd_bytes
                    .iter()
                    .position(|byte| *byte == b'\n')
                else {
                    return Ok(());
                };
                let payload = &resources.displayfd_bytes[..newline];
                let trailing = &resources.displayfd_bytes[newline + 1..];
                if !trailing.is_empty() {
                    failure = Some("XWayland displayfd payload contains duplicate data");
                } else if payload.is_empty() || !payload.iter().all(u8::is_ascii_digit) {
                    failure = Some("XWayland displayfd payload is malformed");
                } else {
                    let value = std::str::from_utf8(payload)
                        .ok()
                        .and_then(|value| value.parse::<u32>().ok());
                    if value == Some(0) {
                        failure = Some("XWayland displayfd reported display zero");
                    } else if value != reserved_display {
                        failure = Some("XWayland displayfd does not match lease");
                    } else {
                        resources.display_ready = true;
                        validated = true;
                    }
                }
            }
        }
        if let Some(reason) = failure {
            return self.fail_generation(
                supervisor,
                io::Error::new(io::ErrorKind::InvalidData, reason),
            );
        }
        if validated {
            self.log_displayfd_event(
                "displayfd_validated",
                None,
                Some(generation),
                self.process_id_for_generation(generation),
                self.displayfd_parent_fd(generation),
                self.displayfd_child_source_fd(generation),
                self.displayfd_reactor_token(generation),
                None,
                None,
            );
            self.maybe_mark_running();
            self.log_readiness_progress("display_number_validated");
        }
        Ok(())
    }

    pub(super) fn process_id_for_generation(
        &self,
        generation: XwaylandGeneration,
    ) -> Option<ManagedProcessId> {
        match &self.state {
            ServiceState::Starting(resources) if resources.generation == generation => {
                Some(resources.process.id)
            }
            ServiceState::RunningBase(resources) if resources.generation == generation => {
                Some(resources.process.id)
            }
            ServiceState::Running(resources) if resources.generation == generation => {
                Some(resources.process.id)
            }
            _ => None,
        }
    }

    pub(super) fn displayfd_parent_fd(&self, generation: XwaylandGeneration) -> Option<RawFd> {
        match &self.state {
            ServiceState::Starting(resources) if resources.generation == generation => {
                Some(resources.displayfd.as_raw_fd())
            }
            _ => None,
        }
    }

    pub(super) fn displayfd_child_source_fd(
        &self,
        generation: XwaylandGeneration,
    ) -> Option<RawFd> {
        match &self.state {
            ServiceState::Starting(resources) if resources.generation == generation => {
                Some(resources.displayfd_child_source_fd)
            }
            _ => None,
        }
    }

    pub(super) fn displayfd_reactor_token(&self, generation: XwaylandGeneration) -> Option<u64> {
        match &self.state {
            ServiceState::Starting(resources) if resources.generation == generation => {
                resources.displayfd_reactor_token
            }
            _ => None,
        }
    }

    pub(super) fn displayfd_is_validated(&self, generation: XwaylandGeneration) -> bool {
        matches!(
            &self.state,
            ServiceState::Starting(resources)
                if resources.generation == generation && resources.display_ready
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn log_displayfd_event(
        &self,
        event: &'static str,
        detail: Option<&'static str>,
        generation: Option<XwaylandGeneration>,
        process_id: Option<ManagedProcessId>,
        parent_read_fd: Option<RawFd>,
        child_source_fd: Option<RawFd>,
        reactor_token: Option<u64>,
        epoll_flags: Option<u32>,
        bytes_read: Option<usize>,
    ) {
        displayfd::log(DisplayFdLog {
            event,
            detail,
            generation,
            process_id,
            leased_display: self.display_number(),
            parent_read_fd,
            child_source_fd,
            child_target_fd: Some(ChildFdTarget::DisplayFd.raw_fd()),
            reactor_token,
            epoll_flags,
            inspection: parent_read_fd.and_then(|fd| displayfd::inspect(fd).ok()),
            bytes_read,
        });
    }
}
