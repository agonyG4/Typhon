use std::{
    ffi::CString,
    fs, io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
};

use oblivion_one::native::event_loop::{ControllerDeviceReadyEvent, ReactorToken};

use super::{
    device::{
        ControllerBatchStats, ControllerDevice, ControllerDeviceId, ControllerIdAllocator,
        TransportFingerprint,
    },
    policy::ControllerPolicy,
    semantic::{ControllerActionMask, ControllerSemanticFrame},
};

/// Target admission budget. An admitted synchronized batch is indivisible and may exceed it.
pub(crate) const MAX_RAW_EVENTS_PER_CYCLE: usize = 384;
pub(crate) const MAX_CONTROLLER_DEVICES: usize = 32;
pub(super) const MAX_DISCOVERY_CANDIDATES: usize = 128;
const MONITOR_READ_BYTES: usize = 4096;
const MONITOR_READS_PER_CYCLE: usize = 4;

const INOTIFY_MASK: u32 = libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_ATTRIB
    | libc::IN_CLOSE_WRITE;
const TERMINAL_EVENTS: u32 =
    libc::EPOLLERR as u32 | libc::EPOLLHUP as u32 | libc::EPOLLRDHUP as u32;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ControllerTelemetry {
    pub(crate) device_opens: u64,
    pub(crate) device_closes: u64,
    pub(crate) raw_events: u64,
    pub(crate) logical_frames: u64,
    pub(crate) activity_transitions: u64,
    pub(crate) semantic_frames: u64,
    pub(crate) semantic_transitions: u64,
    pub(crate) drain_budget_exhaustions: u64,
    pub(crate) backlog_continuations: u64,
    pub(crate) read_failures: u64,
    pub(crate) hotplug_failures: u64,
}

impl ControllerTelemetry {
    pub(super) fn record_batch(&mut self, batch: ControllerBatchStats) {
        self.logical_frames = self
            .logical_frames
            .saturating_add(batch.logical_frames as u64);
        self.activity_transitions = self
            .activity_transitions
            .saturating_add(batch.activity_transitions as u64);
        self.semantic_frames = self
            .semantic_frames
            .saturating_add(batch.semantic_frames as u64);
        self.semantic_transitions = self
            .semantic_transitions
            .saturating_add(batch.semantic_transitions as u64);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ControllerDrainOutcome {
    pub(crate) meaningful_activity: bool,
    pub(crate) backlog_pending: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ControllerEventBudget {
    remaining: usize,
    processed: usize,
}

impl Default for ControllerEventBudget {
    fn default() -> Self {
        Self {
            remaining: MAX_RAW_EVENTS_PER_CYCLE,
            processed: 0,
        }
    }
}

impl ControllerEventBudget {
    pub(super) const fn can_admit_batch(self) -> bool {
        self.remaining > 0
    }

    #[cfg(test)]
    pub(super) fn remaining(self) -> usize {
        self.remaining
    }

    /// Accounts a fully consumed synchronized batch and closes admission at the target.
    pub(super) fn account_batch(&mut self, count: usize) -> bool {
        self.processed = self.processed.saturating_add(count);
        self.remaining = self.remaining.saturating_sub(count);
        self.remaining == 0
    }

    #[cfg(test)]
    pub(super) fn processed(self) -> usize {
        self.processed
    }
}

/// Admits at most one batch and accounts it only after the drain closure completes.
pub(super) fn drain_admitted_batch(
    budget: &mut ControllerEventBudget,
    drain: impl FnOnce() -> io::Result<ControllerBatchStats>,
) -> io::Result<Option<(ControllerBatchStats, bool)>> {
    if !budget.can_admit_batch() {
        return Ok(None);
    }

    let batch = drain()?;
    let exhausted = budget.account_batch(batch.raw_events);
    Ok(Some((batch, exhausted)))
}

#[derive(Debug)]
struct CandidatePath {
    path: PathBuf,
    fingerprint: TransportFingerprint,
}

#[derive(Debug)]
pub(crate) struct ControllerManager {
    policy: ControllerPolicy,
    input_root: PathBuf,
    udev_data_root: PathBuf,
    monitor: Option<OwnedFd>,
    devices: Vec<ControllerDevice>,
    ids: ControllerIdAllocator,
    telemetry: ControllerTelemetry,
    suspended: bool,
    ready_work: Vec<ControllerDeviceId>,
    pending_ready: Vec<ControllerDeviceId>,
    failed_devices: Vec<ControllerDeviceId>,
    last_serviced: Option<ControllerDeviceId>,
}

impl ControllerManager {
    pub(crate) fn new(policy: ControllerPolicy) -> Self {
        Self::with_paths(
            policy,
            PathBuf::from("/dev/input"),
            PathBuf::from("/run/udev/data"),
        )
    }

    pub(super) fn with_paths(
        policy: ControllerPolicy,
        input_root: PathBuf,
        udev_data_root: PathBuf,
    ) -> Self {
        let mut manager = Self {
            policy,
            input_root,
            udev_data_root,
            monitor: None,
            devices: Vec::with_capacity(MAX_CONTROLLER_DEVICES),
            ids: ControllerIdAllocator::default(),
            telemetry: ControllerTelemetry::default(),
            suspended: false,
            ready_work: Vec::with_capacity(MAX_CONTROLLER_DEVICES),
            pending_ready: Vec::with_capacity(MAX_CONTROLLER_DEVICES),
            failed_devices: Vec::with_capacity(MAX_CONTROLLER_DEVICES),
            last_serviced: None,
        };
        if policy == ControllerPolicy::Observe {
            manager.start_observing();
        }
        manager
    }

    pub(crate) fn policy(&self) -> ControllerPolicy {
        self.policy
    }

    pub(crate) fn telemetry(&self) -> ControllerTelemetry {
        self.telemetry
    }

    pub(crate) fn connected_count(&self) -> usize {
        self.devices.len()
    }

    pub(crate) fn monitor_fd(&self) -> Option<RawFd> {
        self.monitor.as_ref().map(AsRawFd::as_raw_fd)
    }

    pub(crate) fn device_ids(&self) -> impl Iterator<Item = ControllerDeviceId> + '_ {
        self.devices.iter().map(ControllerDevice::id)
    }

    pub(crate) fn device_fd(&self, id: ControllerDeviceId) -> Option<RawFd> {
        self.devices
            .iter()
            .find(|device| device.id() == id)
            .map(ControllerDevice::raw_fd)
    }

    #[allow(dead_code)]
    pub(crate) fn semantic_state(&self, id: ControllerDeviceId) -> Option<ControllerActionMask> {
        self.devices
            .iter()
            .find(|device| device.id() == id)
            .map(ControllerDevice::semantic_state)
    }

    pub(crate) fn service_monitor(&mut self) {
        if self.policy != ControllerPolicy::Observe || self.suspended {
            return;
        }
        let Some(monitor_fd) = self.monitor.as_ref().map(AsRawFd::as_raw_fd) else {
            return;
        };
        let mut changed = false;
        let mut retire_monitor = false;
        let mut buffer = [0u8; MONITOR_READ_BYTES];
        for _ in 0..MONITOR_READS_PER_CYCLE {
            let result =
                unsafe { libc::read(monitor_fd, buffer.as_mut_ptr().cast(), buffer.len()) };
            if result > 0 {
                changed = true;
                continue;
            }
            if result == 0 {
                self.telemetry.hotplug_failures = self.telemetry.hotplug_failures.saturating_add(1);
                retire_monitor = true;
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock
                || error.kind() == io::ErrorKind::Interrupted
            {
                break;
            }
            self.telemetry.hotplug_failures = self.telemetry.hotplug_failures.saturating_add(1);
            retire_monitor = true;
            break;
        }
        if retire_monitor {
            self.monitor = None;
        }
        if changed {
            self.rescan();
        }
    }

    pub(crate) fn drain_ready(
        &mut self,
        readiness: &[ControllerDeviceReadyEvent],
        mut readiness_is_current: impl FnMut(ControllerDeviceId, ReactorToken) -> bool,
        mut semantic_sink: impl FnMut(ControllerDeviceId, ControllerSemanticFrame),
    ) -> ControllerDrainOutcome {
        if self.suspended || self.policy != ControllerPolicy::Observe {
            return ControllerDrainOutcome::default();
        }

        self.ready_work.clear();
        self.ready_work.append(&mut self.pending_ready);
        for ready in readiness {
            let Some(id) = ControllerDeviceId::from_raw(ready.device_id) else {
                continue;
            };
            if !readiness_is_current(id, ready.token)
                || !self.contains_id(id)
                || self.ready_work.contains(&id)
            {
                continue;
            }
            self.ready_work.push(id);
        }
        rotate_ready_work_after_last_serviced(&mut self.ready_work, self.last_serviced);

        self.failed_devices.clear();
        let mut budget = ControllerEventBudget::default();
        let work_count = self.ready_work.len();
        let mut outcome = ControllerDrainOutcome::default();

        for work_index in 0..work_count {
            let id = self.ready_work[work_index];
            let ready_flags = readiness
                .iter()
                .find(|ready| ready.device_id == id.get())
                .map_or(0, |ready| ready.flags);
            if ready_flags & TERMINAL_EVENTS != 0 {
                self.failed_devices.push(id);
                continue;
            }
            let Some(device_index) = self.device_index(id) else {
                continue;
            };
            match drain_admitted_batch(&mut budget, || {
                self.devices[device_index].drain_synchronized_batch(&mut semantic_sink)
            }) {
                Ok(Some((batch, exhausted))) => {
                    self.record_batch(batch, &mut outcome);
                    self.telemetry.raw_events = self
                        .telemetry
                        .raw_events
                        .saturating_add(batch.raw_events as u64);
                    self.last_serviced = Some(id);
                    if exhausted {
                        self.telemetry.drain_budget_exhaustions =
                            self.telemetry.drain_budget_exhaustions.saturating_add(1);
                        // Preserve this device and its peers for the next reactor turn. The next
                        // turn rotates after `last_serviced`, so a noisy device cannot stay first.
                        self.defer_ready_work_from(work_index);
                        break;
                    }
                }
                Ok(None) => {
                    self.defer_ready_work_from(work_index);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => {
                    self.telemetry.read_failures = self.telemetry.read_failures.saturating_add(1);
                    self.failed_devices.push(id);
                }
            }
        }

        for index in (0..self.failed_devices.len()).rev() {
            self.remove_device(self.failed_devices[index]);
        }
        self.ready_work.clear();
        outcome.backlog_pending = !self.pending_ready.is_empty();
        outcome
    }

    pub(crate) fn record_backlog_continuation(&mut self) {
        self.telemetry.backlog_continuations =
            self.telemetry.backlog_continuations.saturating_add(1);
    }

    pub(crate) fn suspend(&mut self) {
        if self.suspended {
            return;
        }
        self.suspended = true;
        self.monitor = None;
        self.pending_ready.clear();
        self.ready_work.clear();
        self.failed_devices.clear();
        self.last_serviced = None;
        for device in &mut self.devices {
            device.clear_session_state();
        }
        let removed = self.devices.len() as u64;
        self.devices.clear();
        self.telemetry.device_closes = self.telemetry.device_closes.saturating_add(removed);
    }

    pub(crate) fn resume(&mut self) {
        if !self.suspended || self.policy != ControllerPolicy::Observe {
            return;
        }
        self.suspended = false;
        self.start_observing();
    }

    pub(crate) fn retire_device(&mut self, id: ControllerDeviceId) -> bool {
        self.remove_device(id)
    }

    pub(crate) fn disable_monitor(&mut self) {
        if self.monitor.take().is_some() {
            self.telemetry.hotplug_failures = self.telemetry.hotplug_failures.saturating_add(1);
        }
    }

    pub(crate) fn record_registration_failure(&mut self, device: bool) {
        if device {
            self.telemetry.read_failures = self.telemetry.read_failures.saturating_add(1);
        } else {
            self.telemetry.hotplug_failures = self.telemetry.hotplug_failures.saturating_add(1);
        }
    }

    fn start_observing(&mut self) {
        match open_input_monitor(&self.input_root) {
            Ok(monitor) => self.monitor = Some(monitor),
            Err(_) => {
                self.telemetry.hotplug_failures = self.telemetry.hotplug_failures.saturating_add(1);
                self.monitor = None;
            }
        }
        self.rescan();
    }

    fn rescan(&mut self) {
        let entries = match fs::read_dir(&self.input_root) {
            Ok(entries) => entries,
            Err(_) => {
                self.telemetry.hotplug_failures = self.telemetry.hotplug_failures.saturating_add(1);
                return;
            }
        };
        let entry_paths = entries.filter_map(Result::ok).map(|entry| entry.path());
        let mut candidates = Vec::with_capacity(MAX_DISCOVERY_CANDIDATES);
        for path in event_candidate_paths(entry_paths) {
            match TransportFingerprint::read(&path) {
                Ok(fingerprint) => candidates.push(CandidatePath { path, fingerprint }),
                Err(_) => {
                    self.telemetry.hotplug_failures =
                        self.telemetry.hotplug_failures.saturating_add(1);
                }
            }
        }
        candidates.sort_by(|left, right| left.path.cmp(&right.path));

        let mut index = 0;
        while index < self.devices.len() {
            let device = &self.devices[index];
            let still_present = candidates.iter().any(|candidate| {
                candidate.path == device.path() && candidate.fingerprint == device.fingerprint()
            });
            if still_present {
                index += 1;
            } else {
                let id = device.id();
                self.remove_device(id);
            }
        }

        for candidate in candidates {
            if self.devices.iter().any(|device| {
                device.path() == candidate.path && device.fingerprint() == candidate.fingerprint
            }) {
                continue;
            }
            if self.devices.len() >= MAX_CONTROLLER_DEVICES {
                self.telemetry.hotplug_failures = self.telemetry.hotplug_failures.saturating_add(1);
                break;
            }
            match ControllerDevice::open(&candidate.path, &self.udev_data_root, &mut self.ids) {
                Ok(Some(device)) => {
                    self.devices.push(device);
                    self.telemetry.device_opens = self.telemetry.device_opens.saturating_add(1);
                }
                Ok(None) => {}
                Err(_) => {
                    self.telemetry.read_failures = self.telemetry.read_failures.saturating_add(1);
                }
            }
        }
    }

    pub(crate) fn contains_id(&self, id: ControllerDeviceId) -> bool {
        self.devices.iter().any(|device| device.id() == id)
    }

    fn device_index(&self, id: ControllerDeviceId) -> Option<usize> {
        self.devices.iter().position(|device| device.id() == id)
    }

    fn push_pending_if_known(&mut self, id: ControllerDeviceId) {
        if self.contains_id(id) && !self.pending_ready.contains(&id) {
            self.pending_ready.push(id);
        }
    }

    fn defer_ready_work_from(&mut self, first_index: usize) {
        for pending_index in first_index..self.ready_work.len() {
            let id = self.ready_work[pending_index];
            self.push_pending_if_known(id);
        }
    }

    fn record_batch(&mut self, batch: ControllerBatchStats, outcome: &mut ControllerDrainOutcome) {
        self.telemetry.record_batch(batch);
        outcome.meaningful_activity |= batch.activity_transitions > 0;
    }

    fn remove_device(&mut self, id: ControllerDeviceId) -> bool {
        let Some(index) = self.device_index(id) else {
            self.pending_ready.retain(|pending| *pending != id);
            return false;
        };
        self.devices[index].clear_session_state();
        self.devices.remove(index);
        self.pending_ready.retain(|pending| *pending != id);
        self.telemetry.device_closes = self.telemetry.device_closes.saturating_add(1);
        true
    }
}

pub(super) fn rotate_ready_work_after_last_serviced(
    ready_work: &mut [ControllerDeviceId],
    last_serviced: Option<ControllerDeviceId>,
) {
    if let Some(last) = last_serviced
        && let Some(index) = ready_work.iter().position(|id| *id == last)
    {
        let next = (index + 1) % ready_work.len().max(1);
        ready_work.rotate_left(next);
    }
}

fn is_event_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("event"))
}

pub(super) fn event_candidate_paths(
    paths: impl Iterator<Item = PathBuf>,
) -> impl Iterator<Item = PathBuf> {
    paths
        .filter(|path| is_event_path(path))
        .take(MAX_DISCOVERY_CANDIDATES)
}

fn open_input_monitor(input_root: &Path) -> io::Result<OwnedFd> {
    let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let path = CString::new(input_root.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "input path contains NUL"))?;
    let watch = unsafe { libc::inotify_add_watch(fd.as_raw_fd(), path.as_ptr(), INOTIFY_MASK) };
    if watch < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}
