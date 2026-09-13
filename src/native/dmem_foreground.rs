use std::{
    collections::BTreeMap,
    env,
    ffi::CString,
    fmt::{self, Display, Formatter},
    fs::{self, File},
    io::{self, Write},
    os::{fd::FromRawFd, unix::ffi::OsStrExt},
    panic::{self, AssertUnwindSafe},
    path::{Component, Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmemForegroundPolicy {
    Auto,
    On,
    Off,
}

impl DmemForegroundPolicy {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "on" => Self::On,
            "off" => Self::Off,
            "auto" => Self::Auto,
            _ => Self::Auto,
        }
    }

    pub fn from_env() -> Self {
        match env::var("OBLIVION_ONE_DMEM_FOREGROUND") {
            Ok(value) => {
                let policy = Self::parse(&value);
                if !matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "auto" | "on" | "off"
                ) {
                    eprintln!(
                        "native dmem foreground: unknown OBLIVION_ONE_DMEM_FOREGROUND value={value:?}; using auto"
                    );
                }
                policy
            }
            Err(env::VarError::NotPresent) => Self::Auto,
            Err(env::VarError::NotUnicode(_)) => {
                eprintln!(
                    "native dmem foreground: invalid OBLIVION_ONE_DMEM_FOREGROUND value; using auto"
                );
                Self::Auto
            }
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DmemParseError {
    MalformedLine { line: usize },
    EmptyCapacity,
    InvalidRegion { line: usize },
    InvalidCapacity { line: usize },
    DuplicateRegion { line: usize },
}

impl Display for DmemParseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedLine { line } => write!(formatter, "malformed dmem line {line}"),
            Self::EmptyCapacity => formatter.write_str("dmem capacity set is empty"),
            Self::InvalidRegion { line } => write!(formatter, "invalid dmem region on line {line}"),
            Self::InvalidCapacity { line } => {
                write!(formatter, "invalid dmem capacity on line {line}")
            }
            Self::DuplicateRegion { line } => {
                write!(formatter, "duplicate dmem region on line {line}")
            }
        }
    }
}

impl std::error::Error for DmemParseError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DmemError {
    Io {
        operation: &'static str,
        kind: io::ErrorKind,
        message: String,
    },
    Parse(DmemParseError),
    InvalidCgroupPath(&'static str),
    MissingCgroupV2Entry,
    InvalidProcessMetadata(&'static str),
    InvalidTarget(&'static str),
    Unavailable(&'static str),
}

impl DmemError {
    fn io(operation: &'static str, error: io::Error) -> Self {
        Self::Io {
            operation,
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}

impl Display for DmemError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation, message, ..
            } => write!(formatter, "{operation}: {message}"),
            Self::Parse(error) => Display::fmt(error, formatter),
            Self::InvalidCgroupPath(reason) => write!(formatter, "invalid cgroup path: {reason}"),
            Self::MissingCgroupV2Entry => formatter.write_str("process has no cgroup v2 entry"),
            Self::InvalidProcessMetadata(reason) => {
                write!(formatter, "invalid process metadata: {reason}")
            }
            Self::InvalidTarget(reason) => write!(formatter, "invalid dmem target: {reason}"),
            Self::Unavailable(reason) => write!(formatter, "dmem unavailable: {reason}"),
        }
    }
}

impl std::error::Error for DmemError {}

impl From<DmemParseError> for DmemError {
    fn from(error: DmemParseError) -> Self {
        Self::Parse(error)
    }
}

impl DmemError {
    fn is_retryable(&self) -> bool {
        match self {
            Self::Io {
                operation, kind, ..
            } => {
                matches!(
                    kind,
                    io::ErrorKind::PermissionDenied
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) || (*operation == "open dmem.low" && *kind == io::ErrorKind::NotFound)
            }
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RelativeCgroupPath(PathBuf);

impl RelativeCgroupPath {
    pub fn parse(path: &str) -> Result<Self, DmemError> {
        if path.is_empty() || path.contains('\0') || !path.starts_with('/') {
            return Err(DmemError::InvalidCgroupPath(
                "path must be absolute and NUL-free",
            ));
        }
        if path
            .split('/')
            .any(|component| matches!(component, "." | ".."))
        {
            return Err(DmemError::InvalidCgroupPath(
                "current- or parent-directory component",
            ));
        }

        let mut relative = PathBuf::new();
        for component in Path::new(path).components() {
            match component {
                Component::RootDir => {}
                Component::Normal(component) => relative.push(component),
                Component::CurDir => {
                    return Err(DmemError::InvalidCgroupPath("current-directory component"));
                }
                Component::ParentDir => {
                    return Err(DmemError::InvalidCgroupPath("parent-directory component"));
                }
                Component::Prefix(_) => {
                    return Err(DmemError::InvalidCgroupPath("path prefix"));
                }
            }
        }

        Ok(Self(relative))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    fn is_ancestor_of(&self, other: &Self) -> bool {
        other.as_path().starts_with(self.as_path())
    }
}

pub fn parse_cgroup_v2_path(contents: &str) -> Result<RelativeCgroupPath, DmemError> {
    let mut unified_path = None;

    for line in contents.lines() {
        let mut fields = line.splitn(3, ':');
        let hierarchy = fields
            .next()
            .ok_or(DmemError::InvalidProcessMetadata("malformed cgroup line"))?;
        let controllers = fields
            .next()
            .ok_or(DmemError::InvalidProcessMetadata("malformed cgroup line"))?;
        let path = fields
            .next()
            .ok_or(DmemError::InvalidProcessMetadata("malformed cgroup line"))?;

        if hierarchy == "0" && controllers.is_empty() {
            if path.contains("(deleted)") {
                return Err(DmemError::InvalidCgroupPath("deleted cgroup"));
            }
            if unified_path.is_some() {
                return Err(DmemError::InvalidProcessMetadata(
                    "duplicate cgroup v2 entry",
                ));
            }
            unified_path = Some(RelativeCgroupPath::parse(path)?);
        }
    }

    unified_path.ok_or(DmemError::MissingCgroupV2Entry)
}

pub fn parse_process_uid(contents: &str) -> Result<u32, DmemError> {
    let line = contents
        .lines()
        .find(|line| line.starts_with("Uid:"))
        .ok_or(DmemError::InvalidProcessMetadata("missing Uid field"))?;
    let values = line[4..].split_whitespace().collect::<Vec<_>>();
    if values.len() != 4 {
        return Err(DmemError::InvalidProcessMetadata("malformed Uid field"));
    }

    let uids = values
        .iter()
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|_| DmemError::InvalidProcessMetadata("invalid uid"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if uids.iter().any(|uid| *uid != uids[0]) {
        return Err(DmemError::InvalidProcessMetadata(
            "process uid identities differ",
        ));
    }
    Ok(uids[0])
}

pub fn parse_process_start_time(contents: &str) -> Result<u64, DmemError> {
    let close_paren = contents
        .rfind(')')
        .ok_or(DmemError::InvalidProcessMetadata("malformed stat command"))?;
    let start_time = contents[close_paren + 1..]
        .split_whitespace()
        .nth(19)
        .ok_or(DmemError::InvalidProcessMetadata("missing start time"))?;

    start_time
        .parse::<u64>()
        .map_err(|_| DmemError::InvalidProcessMetadata("invalid start time"))
}

pub fn validate_cgroup_target(
    target: &RelativeCgroupPath,
    typhon_cgroup: &RelativeCgroupPath,
) -> Result<(), DmemError> {
    if target.as_path().as_os_str().is_empty() {
        return Err(DmemError::InvalidTarget("cgroup root"));
    }
    if target == typhon_cgroup {
        return Err(DmemError::InvalidTarget("Typhon cgroup"));
    }
    if target.is_ancestor_of(typhon_cgroup) {
        return Err(DmemError::InvalidTarget("ancestor of Typhon cgroup"));
    }

    Ok(())
}

pub trait DmemLowWriter {
    fn write_region(
        &mut self,
        cgroup: &RelativeCgroupPath,
        region: &str,
        value: u64,
    ) -> io::Result<()>;
}

pub fn apply_low_entries<W: DmemLowWriter>(
    writer: &mut W,
    cgroup: &RelativeCgroupPath,
    capacities: &BTreeMap<String, u64>,
) -> Result<(), DmemError> {
    for (region, capacity) in capacities {
        writer
            .write_region(cgroup, region, *capacity)
            .map_err(|error| DmemError::io("write dmem.low", error))?;
    }
    Ok(())
}

pub fn revert_low_entries<'a, W, I>(
    writer: &mut W,
    cgroup: &RelativeCgroupPath,
    regions: I,
) -> Result<(), DmemError>
where
    W: DmemLowWriter,
    I: IntoIterator<Item = &'a String>,
{
    for region in regions {
        writer
            .write_region(cgroup, region, 0)
            .map_err(|error| DmemError::io("revert dmem.low", error))?;
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DmemPaths {
    pub proc_root: PathBuf,
    pub cgroup_root: PathBuf,
}

impl DmemPaths {
    pub fn production() -> Self {
        Self {
            proc_root: PathBuf::from("/proc"),
            cgroup_root: PathBuf::from("/sys/fs/cgroup"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_time: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedCgroup {
    pub path: RelativeCgroupPath,
    pub process: ProcessIdentity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForegroundTarget {
    pub window_id: crate::core::WindowId,
    pub pid: u32,
}

#[derive(Clone, Debug)]
pub struct ProcCgroupResolver {
    paths: DmemPaths,
    session_uid: u32,
    typhon_cgroup: RelativeCgroupPath,
}

pub trait DmemTargetResolver {
    fn resolve_pid(&self, pid: u32) -> Result<ResolvedCgroup, DmemError>;
}

impl ProcCgroupResolver {
    pub fn new(paths: DmemPaths, session_uid: u32, typhon_cgroup: RelativeCgroupPath) -> Self {
        Self {
            paths,
            session_uid,
            typhon_cgroup,
        }
    }

    fn resolve_pid_impl(&self, pid: u32) -> Result<ResolvedCgroup, DmemError> {
        if pid == 0 {
            return Err(DmemError::InvalidTarget("zero pid"));
        }

        let process_root = self.paths.proc_root.join(pid.to_string());
        let status = fs::read_to_string(process_root.join("status"))
            .map_err(|error| DmemError::io("read process status", error))?;
        if parse_process_uid(&status)? != self.session_uid {
            return Err(DmemError::InvalidTarget("different uid"));
        }

        let start_time = parse_process_start_time(
            &fs::read_to_string(process_root.join("stat"))
                .map_err(|error| DmemError::io("read process stat", error))?,
        )?;
        let path = parse_cgroup_v2_path(
            &fs::read_to_string(process_root.join("cgroup"))
                .map_err(|error| DmemError::io("read process cgroup", error))?,
        )?;
        validate_cgroup_target(&path, &self.typhon_cgroup)?;

        FilesystemDmemWriter::new(self.paths.cgroup_root.clone()).check_usable(&path)?;

        Ok(ResolvedCgroup {
            path,
            process: ProcessIdentity { pid, start_time },
        })
    }
}

impl DmemTargetResolver for ProcCgroupResolver {
    fn resolve_pid(&self, pid: u32) -> Result<ResolvedCgroup, DmemError> {
        self.resolve_pid_impl(pid)
    }
}

impl ProcCgroupResolver {
    pub fn resolve_pid(&self, pid: u32) -> Result<ResolvedCgroup, DmemError> {
        self.resolve_pid_impl(pid)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionResult {
    Applied,
    Reverted,
    SameCgroup,
    Noop,
}

pub struct ForegroundController<R, W> {
    resolver: R,
    writer: W,
    capacities: BTreeMap<String, u64>,
    current: Option<ResolvedCgroup>,
}

impl<R, W> ForegroundController<R, W>
where
    R: DmemTargetResolver,
    W: DmemLowWriter,
{
    pub fn new(resolver: R, writer: W, capacities: BTreeMap<String, u64>) -> Self {
        Self {
            resolver,
            writer,
            capacities,
            current: None,
        }
    }

    pub fn current_path(&self) -> Option<&Path> {
        self.current.as_ref().map(|current| current.path.as_path())
    }

    pub fn writer(&self) -> &W {
        &self.writer
    }

    pub fn writer_mut(&mut self) -> &mut W {
        &mut self.writer
    }

    fn total_capacity(&self) -> Option<u64> {
        self.capacities
            .values()
            .try_fold(0_u64, |total, value| total.checked_add(*value))
    }

    pub fn reconcile(
        &mut self,
        desired: Option<ForegroundTarget>,
    ) -> Result<TransitionResult, DmemError> {
        let Some(desired) = desired else {
            if self.current.is_none() {
                return Ok(TransitionResult::Noop);
            }
            self.revert_current()?;
            return Ok(TransitionResult::Reverted);
        };

        let resolved = self.resolver.resolve_pid(desired.pid);
        if let Ok(resolved) = &resolved
            && self
                .current
                .as_ref()
                .is_some_and(|current| current.path == resolved.path)
        {
            self.current = Some(resolved.clone());
            return Ok(TransitionResult::SameCgroup);
        }

        if self.current.is_some() {
            self.revert_current()?;
        }

        let resolved = resolved?;
        if let Err(error) = apply_low_entries(&mut self.writer, &resolved.path, &self.capacities) {
            let _ = revert_low_entries(&mut self.writer, &resolved.path, self.capacities.keys());
            return Err(error);
        }
        self.current = Some(resolved);
        Ok(TransitionResult::Applied)
    }

    fn revert_current(&mut self) -> Result<(), DmemError> {
        let Some(current) = self.current.as_ref() else {
            return Ok(());
        };

        let mut first_error = None;
        for region in self.capacities.keys() {
            if let Err(error) = self.writer.write_region(&current.path, region, 0)
                && error.kind() != io::ErrorKind::NotFound
                && first_error.is_none()
            {
                first_error = Some(DmemError::io("revert dmem.low", error));
            }
        }

        if let Some(error) = first_error {
            return Err(error);
        }

        self.current = None;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct FilesystemDmemWriter {
    cgroup_root: PathBuf,
}

impl FilesystemDmemWriter {
    pub fn new(cgroup_root: PathBuf) -> Self {
        Self { cgroup_root }
    }

    fn low_path(&self, cgroup: &RelativeCgroupPath) -> PathBuf {
        self.cgroup_root.join(cgroup.as_path()).join("dmem.low")
    }

    fn open_low(&self, cgroup: &RelativeCgroupPath) -> io::Result<File> {
        let path = self.low_path(cgroup);
        let path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in dmem.low path"))?;
        let flags = libc::O_WRONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
        let fd = unsafe { libc::open(path.as_ptr(), flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(unsafe { File::from_raw_fd(fd) })
    }

    pub fn check_usable(&self, cgroup: &RelativeCgroupPath) -> Result<(), DmemError> {
        self.open_low(cgroup)
            .map(|_| ())
            .map_err(|error| DmemError::io("open dmem.low", error))
    }
}

impl DmemLowWriter for FilesystemDmemWriter {
    fn write_region(
        &mut self,
        cgroup: &RelativeCgroupPath,
        region: &str,
        value: u64,
    ) -> io::Result<()> {
        let mut file = self.open_low(cgroup)?;
        writeln!(file, "{region} {value}")
    }
}

const RETRY_DELAYS: [Duration; 5] = [
    Duration::from_millis(10),
    Duration::from_millis(25),
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(200),
];

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DmemForegroundCounters {
    pub transitions_requested: u64,
    pub transitions_applied: u64,
    pub coalesced_desired: u64,
    pub stale_desired: u64,
    pub same_cgroup_noops: u64,
    pub retries: u64,
    pub resolution_failures: u64,
    pub write_failures: u64,
    pub successful_reverts: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DmemForegroundSnapshot {
    pub policy: DmemForegroundPolicy,
    pub kernel_capacity_available: bool,
    pub worker_available: bool,
    pub region_count: usize,
    pub desired_window_id: Option<u64>,
    pub desired_pid: Option<u32>,
    pub protected_cgroup: Option<String>,
    pub total_protected_capacity: Option<u64>,
    pub degraded: bool,
    pub last_failure: Option<String>,
    pub counters: DmemForegroundCounters,
}

impl DmemForegroundSnapshot {
    pub fn doctor_severity(&self) -> crate::control_snapshots::DoctorSeverity {
        if self.policy == DmemForegroundPolicy::Off {
            return crate::control_snapshots::DoctorSeverity::Ok;
        }
        if self.degraded || (self.kernel_capacity_available && !self.worker_available) {
            return crate::control_snapshots::DoctorSeverity::Warning;
        }
        crate::control_snapshots::DoctorSeverity::Ok
    }

    pub fn detail(&self) -> String {
        let desired = match (self.desired_window_id, self.desired_pid) {
            (Some(window_id), Some(pid)) => format!("window={window_id} pid={pid}"),
            (Some(window_id), None) => format!("window={window_id} pid=unknown"),
            _ => "none".to_owned(),
        };
        let protected = self.protected_cgroup.as_deref().unwrap_or("none");
        let total = self
            .total_protected_capacity
            .map_or_else(|| "none".to_owned(), |capacity| capacity.to_string());
        let failure = self.last_failure.as_deref().unwrap_or("none");
        format!(
            "policy={} state={} kernel_capacity={} worker={} regions={} desired={} protected={} total_bytes={} last_failure={failure}",
            self.policy.as_str(),
            self.state_name(),
            self.kernel_capacity_available,
            self.worker_available,
            self.region_count,
            desired,
            protected,
            total,
        )
    }

    fn state_name(&self) -> &'static str {
        if self.policy == DmemForegroundPolicy::Off {
            "off"
        } else if !self.kernel_capacity_available {
            "unsupported"
        } else if !self.worker_available {
            "unavailable"
        } else if self.degraded {
            "degraded"
        } else if self.protected_cgroup.is_some() {
            "active"
        } else {
            "idle"
        }
    }
}

struct DmemWorkerState {
    desired: Option<ForegroundTarget>,
    generation: u64,
    processed_generation: u64,
    stopping: bool,
    snapshot: DmemForegroundSnapshot,
}

struct DmemShared {
    state: Mutex<DmemWorkerState>,
    wake: Condvar,
}

impl DmemShared {
    fn lock(&self) -> std::sync::MutexGuard<'_, DmemWorkerState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn mark_failure(&self, failure: &DmemError) {
        let mut state = self.lock();
        state.snapshot.degraded = true;
        state.snapshot.last_failure = Some(bounded_string(failure.to_string(), 256));
    }
}

pub struct DmemForeground {
    shared: Arc<DmemShared>,
    last_submitted: Option<ForegroundTarget>,
    worker: Option<JoinHandle<()>>,
}

impl DmemForeground {
    pub fn start(
        policy: DmemForegroundPolicy,
        paths: DmemPaths,
        session_uid: u32,
        typhon_pid: u32,
    ) -> Self {
        let mut snapshot = DmemForegroundSnapshot {
            policy,
            kernel_capacity_available: false,
            worker_available: false,
            region_count: 0,
            desired_window_id: None,
            desired_pid: None,
            protected_cgroup: None,
            total_protected_capacity: None,
            degraded: policy == DmemForegroundPolicy::On,
            last_failure: None,
            counters: DmemForegroundCounters::default(),
        };
        let shared = Arc::new(DmemShared {
            state: Mutex::new(DmemWorkerState {
                desired: None,
                generation: 0,
                processed_generation: 0,
                stopping: false,
                snapshot,
            }),
            wake: Condvar::new(),
        });

        if policy == DmemForegroundPolicy::Off {
            return Self {
                shared,
                last_submitted: None,
                worker: None,
            };
        }

        let capacities = match discover_capacities(&paths) {
            Ok(capacities) => capacities,
            Err(error) => {
                {
                    let mut state = shared.lock();
                    state.snapshot.last_failure = Some(bounded_string(error.to_string(), 256));
                    if !matches!(error, DmemError::Unavailable(_)) {
                        state.snapshot.kernel_capacity_available = true;
                    }
                }
                return Self {
                    shared,
                    last_submitted: None,
                    worker: None,
                };
            }
        };

        snapshot = shared.lock().snapshot.clone();
        snapshot.kernel_capacity_available = true;
        snapshot.region_count = capacities.len();
        snapshot.total_protected_capacity = None;
        {
            let mut state = shared.lock();
            state.snapshot = snapshot;
        }

        let own_cgroup =
            match fs::read_to_string(paths.proc_root.join(typhon_pid.to_string()).join("cgroup"))
                .map_err(|error| DmemError::io("read Typhon cgroup", error))
                .and_then(|contents| parse_cgroup_v2_path(&contents))
            {
                Ok(path) => path,
                Err(error) => {
                    shared.mark_failure(&error);
                    return Self {
                        shared,
                        last_submitted: None,
                        worker: None,
                    };
                }
            };

        let resolver = ProcCgroupResolver::new(paths.clone(), session_uid, own_cgroup);
        let writer = FilesystemDmemWriter::new(paths.cgroup_root);
        let controller = ForegroundController::new(resolver, writer, capacities);
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("typhon-dmem-foreground".to_owned())
            .spawn(move || worker_loop(worker_shared, controller));

        match worker {
            Ok(worker) => {
                {
                    let mut state = shared.lock();
                    state.snapshot.worker_available = true;
                    state.snapshot.degraded = false;
                    state.snapshot.last_failure = None;
                }
                Self {
                    shared,
                    last_submitted: None,
                    worker: Some(worker),
                }
            }
            Err(error) => {
                let error = DmemError::io("start dmem worker", error);
                shared.mark_failure(&error);
                Self {
                    shared,
                    last_submitted: None,
                    worker: None,
                }
            }
        }
    }

    pub fn submit(&mut self, desired: Option<ForegroundTarget>) {
        if self.last_submitted == desired {
            return;
        }
        self.last_submitted = desired;

        let mut state = self.shared.lock();
        if state.stopping {
            return;
        }
        if state.generation != state.processed_generation {
            state.snapshot.counters.coalesced_desired += 1;
        }
        state.desired = desired;
        state.generation = state.generation.wrapping_add(1);
        state.snapshot.desired_window_id = desired.map(|target| target.window_id.get());
        state.snapshot.desired_pid = desired.map(|target| target.pid);
        state.snapshot.counters.transitions_requested += 1;
        self.shared.wake.notify_one();
    }

    pub fn snapshot(&self) -> DmemForegroundSnapshot {
        self.shared.lock().snapshot.clone()
    }

    pub fn shutdown(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };

        {
            let mut state = self.shared.lock();
            state.stopping = true;
            state.desired = None;
            state.generation = state.generation.wrapping_add(1);
            state.snapshot.desired_window_id = None;
            state.snapshot.desired_pid = None;
            self.last_submitted = None;
            self.shared.wake.notify_one();
        }

        if worker.join().is_err() {
            let error = DmemError::Unavailable("dmem worker panicked during shutdown");
            self.shared.mark_failure(&error);
        }
    }
}

impl Drop for DmemForeground {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn discover_capacities(paths: &DmemPaths) -> Result<BTreeMap<String, u64>, DmemError> {
    let controllers = match fs::read_to_string(paths.cgroup_root.join("cgroup.controllers")) {
        Ok(controllers) => controllers,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(DmemError::Unavailable("cgroup v2 is not mounted"));
        }
        Err(error) => return Err(DmemError::io("read cgroup.controllers", error)),
    };
    if !controllers
        .split_whitespace()
        .any(|controller| controller == "dmem")
    {
        return Err(DmemError::Unavailable("dmem controller is not available"));
    }

    let capacity = match fs::read_to_string(paths.cgroup_root.join("dmem.capacity")) {
        Ok(capacity) => capacity,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(DmemError::Unavailable("dmem.capacity is not available"));
        }
        Err(error) => return Err(DmemError::io("read dmem.capacity", error)),
    };
    match parse_capacity(&capacity) {
        Ok(capacities) => Ok(capacities),
        Err(DmemParseError::EmptyCapacity) => {
            Err(DmemError::Unavailable("dmem.capacity has no regions"))
        }
        Err(error) => Err(error.into()),
    }
}

fn bounded_string(value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }
    let mut truncated = value;
    let mut end = limit.saturating_sub(3);
    while end > 0 && !truncated.is_char_boundary(end) {
        end -= 1;
    }
    truncated.truncate(end);
    truncated.push_str("...");
    truncated
}

fn worker_loop<R, W>(shared: Arc<DmemShared>, mut controller: ForegroundController<R, W>)
where
    R: DmemTargetResolver,
    W: DmemLowWriter,
{
    loop {
        let (desired, generation, stopping) = {
            let mut state = shared.lock();
            while !state.stopping && state.generation == state.processed_generation {
                state = match shared.wake.wait(state) {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
            }
            (state.desired, state.generation, state.stopping)
        };

        if stopping {
            let result = panic::catch_unwind(AssertUnwindSafe(|| controller.reconcile(None)));
            match result {
                Ok(Ok(TransitionResult::Reverted)) | Ok(Ok(TransitionResult::Noop)) => {
                    let mut state = shared.lock();
                    state.snapshot.counters.successful_reverts += 1;
                    update_protected_snapshot(&mut state.snapshot, &controller);
                }
                Ok(Err(error)) => shared.mark_failure(&error),
                Err(_) => shared.mark_failure(&DmemError::Unavailable("dmem revert panicked")),
                _ => {}
            }
            let mut state = shared.lock();
            state.snapshot.worker_available = false;
            return;
        }

        let mut attempt = 0;
        loop {
            if generation_is_stale(&shared, generation) {
                let mut state = shared.lock();
                state.snapshot.counters.stale_desired += 1;
                break;
            }

            let result = panic::catch_unwind(AssertUnwindSafe(|| controller.reconcile(desired)));
            let result = match result {
                Ok(result) => result,
                Err(_) => {
                    let _ = panic::catch_unwind(AssertUnwindSafe(|| controller.reconcile(None)));
                    shared.mark_failure(&DmemError::Unavailable("dmem worker panicked"));
                    let mut state = shared.lock();
                    state.snapshot.worker_available = false;
                    return;
                }
            };

            match result {
                Ok(transition) => {
                    let mut state = shared.lock();
                    if state.generation != generation {
                        state.snapshot.counters.stale_desired += 1;
                    } else {
                        state.processed_generation = generation;
                        state.snapshot.degraded = false;
                        state.snapshot.last_failure = None;
                        match transition {
                            TransitionResult::Applied => {
                                state.snapshot.counters.transitions_applied += 1;
                            }
                            TransitionResult::SameCgroup => {
                                state.snapshot.counters.same_cgroup_noops += 1;
                            }
                            TransitionResult::Reverted => {
                                state.snapshot.counters.successful_reverts += 1;
                            }
                            TransitionResult::Noop => {}
                        }
                    }
                    update_protected_snapshot(&mut state.snapshot, &controller);
                    break;
                }
                Err(error) if error.is_retryable() && attempt < RETRY_DELAYS.len() => {
                    {
                        let mut state = shared.lock();
                        state.snapshot.counters.retries += 1;
                    }
                    if !wait_for_retry(&shared, generation, RETRY_DELAYS[attempt]) {
                        let mut state = shared.lock();
                        state.snapshot.counters.stale_desired += 1;
                        break;
                    }
                    attempt += 1;
                }
                Err(error) => {
                    let mut state = shared.lock();
                    if matches!(
                        error,
                        DmemError::Io {
                            operation: "write dmem.low",
                            ..
                        }
                    ) {
                        state.snapshot.counters.write_failures += 1;
                    } else {
                        state.snapshot.counters.resolution_failures += 1;
                    }
                    state.snapshot.last_failure = Some(bounded_string(error.to_string(), 256));
                    state.snapshot.degraded = true;
                    if state.generation == generation {
                        state.processed_generation = generation;
                    } else {
                        state.snapshot.counters.stale_desired += 1;
                    }
                    update_protected_snapshot(&mut state.snapshot, &controller);
                    break;
                }
            }
        }
    }
}

fn generation_is_stale(shared: &DmemShared, generation: u64) -> bool {
    let state = shared.lock();
    state.stopping || state.generation != generation
}

fn wait_for_retry(shared: &DmemShared, generation: u64, delay: Duration) -> bool {
    let state = shared.lock();
    if state.stopping || state.generation != generation {
        return false;
    }
    let state = match shared.wake.wait_timeout(state, delay) {
        Ok((state, _)) => state,
        Err(poisoned) => poisoned.into_inner().0,
    };
    !state.stopping && state.generation == generation
}

fn update_protected_snapshot<R, W>(
    snapshot: &mut DmemForegroundSnapshot,
    controller: &ForegroundController<R, W>,
) where
    R: DmemTargetResolver,
    W: DmemLowWriter,
{
    snapshot.protected_cgroup = controller
        .current_path()
        .map(|path| bounded_string(path.display().to_string(), 160));
    if snapshot.protected_cgroup.is_some() {
        snapshot.total_protected_capacity = controller.total_capacity();
    } else {
        snapshot.total_protected_capacity = None;
    }
}

pub fn parse_capacity(contents: &str) -> Result<BTreeMap<String, u64>, DmemParseError> {
    let mut capacities = BTreeMap::new();

    for (line_index, line) in contents.lines().enumerate() {
        let line_number = line_index + 1;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let mut fields = line.split_whitespace();
        let region = fields
            .next()
            .ok_or(DmemParseError::MalformedLine { line: line_number })?;
        let capacity = fields
            .next()
            .ok_or(DmemParseError::MalformedLine { line: line_number })?;

        if fields.next().is_some() {
            return Err(DmemParseError::MalformedLine { line: line_number });
        }
        if region.is_empty() || region.contains('\0') {
            return Err(DmemParseError::InvalidRegion { line: line_number });
        }

        let capacity = capacity
            .parse::<u64>()
            .map_err(|_| DmemParseError::InvalidCapacity { line: line_number })?;
        if capacities.insert(region.to_owned(), capacity).is_some() {
            return Err(DmemParseError::DuplicateRegion { line: line_number });
        }
    }

    if capacities.is_empty() {
        return Err(DmemParseError::EmptyCapacity);
    }

    Ok(capacities)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn capacity_parser_accepts_multiple_regions_in_sorted_order() {
        let capacity = parse_capacity("region-b 67108864\nregion-a 8589934592\n")
            .expect("capacity should parse");

        assert_eq!(
            capacity,
            [
                ("region-a".to_owned(), 8_589_934_592),
                ("region-b".to_owned(), 67_108_864),
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn capacity_parser_rejects_malformed_duplicate_and_overflow_entries() {
        for input in [
            "region-a",
            "region-a 1 extra",
            "region-a nope",
            "region-a 18446744073709551616",
            "region-a 1\nregion-a 2",
        ] {
            assert!(
                parse_capacity(input).is_err(),
                "input should fail: {input:?}"
            );
        }

        assert!(parse_capacity("\n  \n").is_err());
    }

    #[test]
    fn policy_parser_defaults_and_normalizes_unknown_values_to_auto() {
        assert_eq!(
            DmemForegroundPolicy::parse("auto"),
            DmemForegroundPolicy::Auto
        );
        assert_eq!(
            DmemForegroundPolicy::parse(" ON "),
            DmemForegroundPolicy::On
        );
        assert_eq!(
            DmemForegroundPolicy::parse("off"),
            DmemForegroundPolicy::Off
        );
        assert_eq!(
            DmemForegroundPolicy::parse("unexpected"),
            DmemForegroundPolicy::Auto
        );
        assert_eq!(DmemForegroundPolicy::parse(""), DmemForegroundPolicy::Auto);
    }

    #[test]
    fn cgroup_parser_accepts_unified_entry_and_rejects_unsafe_paths() {
        assert_eq!(
            parse_cgroup_v2_path("5:cpu:/legacy\n0::/user.slice/app.slice/foo.scope\n")
                .expect("unified cgroup should parse")
                .as_path(),
            Path::new("user.slice/app.slice/foo.scope")
        );

        for input in [
            "5:cpu:/legacy\n",
            "0::/user.slice/../other.scope\n",
            "0::/user.slice/./foo.scope\n",
            "0::/user.slice/foo.scope (deleted)\n",
            "0:/wrong-field-count\n",
        ] {
            assert!(
                parse_cgroup_v2_path(input).is_err(),
                "input should fail: {input:?}"
            );
        }
    }

    #[test]
    fn process_metadata_parsers_require_valid_uid_and_start_time() {
        assert_eq!(
            parse_process_uid("Name:\tapp\nUid:\t1000\t1000\t1000\t1000\n")
                .expect("uid should parse"),
            1000
        );
        assert!(parse_process_uid("Name:\tapp\n").is_err());
        assert!(parse_process_uid("Uid:\t1000\tnope\n").is_err());

        let stat = "123 (app process) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21";
        assert_eq!(
            parse_process_start_time(stat).expect("start time should parse"),
            19
        );
        assert!(parse_process_start_time("123 (app) S").is_err());
    }

    #[test]
    fn target_validation_rejects_root_own_cgroup_and_unsafe_ancestors() {
        let own = RelativeCgroupPath::parse("/user.slice/user-1000.slice/session.scope")
            .expect("own path should parse");

        assert!(
            validate_cgroup_target(
                &RelativeCgroupPath::parse("/user.slice/app.scope").expect("target should parse"),
                &own,
            )
            .is_ok()
        );
        for target in [
            "/",
            "/user.slice/user-1000.slice/session.scope",
            "/user.slice",
            "/user.slice/user-1000.slice",
        ] {
            assert!(
                validate_cgroup_target(&RelativeCgroupPath::parse(target).expect("path"), &own)
                    .is_err(),
                "target should fail: {target}"
            );
        }
    }

    #[derive(Default)]
    struct RecordingWriter {
        writes: Vec<(String, String, u64)>,
        missing: bool,
    }

    impl DmemLowWriter for RecordingWriter {
        fn write_region(
            &mut self,
            cgroup: &RelativeCgroupPath,
            region: &str,
            value: u64,
        ) -> std::io::Result<()> {
            if self.missing {
                return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
            }
            self.writes.push((
                cgroup.as_path().display().to_string(),
                region.to_owned(),
                value,
            ));
            Ok(())
        }
    }

    #[test]
    fn low_entries_are_written_as_distinct_region_value_operations() {
        let mut writer = RecordingWriter::default();
        let cgroup = RelativeCgroupPath::parse("/user.slice/app.scope").expect("path");
        let capacities = parse_capacity("region-b 2\nregion-a 1\n").expect("capacity");

        apply_low_entries(&mut writer, &cgroup, &capacities).expect("apply should succeed");
        assert_eq!(
            writer.writes,
            vec![
                ("user.slice/app.scope".to_owned(), "region-a".to_owned(), 1),
                ("user.slice/app.scope".to_owned(), "region-b".to_owned(), 2),
            ]
        );

        writer.writes.clear();
        revert_low_entries(&mut writer, &cgroup, capacities.keys()).expect("revert should succeed");
        assert_eq!(
            writer.writes,
            vec![
                ("user.slice/app.scope".to_owned(), "region-a".to_owned(), 0),
                ("user.slice/app.scope".to_owned(), "region-b".to_owned(), 0),
            ]
        );
    }

    fn resolver_fixture(name: &str, pid: u32, uid: u32, cgroup: &str) -> DmemPaths {
        let root =
            std::env::temp_dir().join(format!("typhon-dmem-{}-{name}-{}", std::process::id(), pid));
        let proc_dir = root.join("proc").join(pid.to_string());
        let cgroup_dir = root.join("cgroup").join(cgroup.trim_start_matches('/'));
        std::fs::create_dir_all(&proc_dir).expect("proc fixture");
        std::fs::create_dir_all(&cgroup_dir).expect("cgroup fixture");
        std::fs::write(
            proc_dir.join("status"),
            format!("Name:\tfixture\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"),
        )
        .expect("status fixture");
        std::fs::write(
            proc_dir.join("stat"),
            "123 (fixture) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21",
        )
        .expect("stat fixture");
        std::fs::write(proc_dir.join("cgroup"), format!("0::{cgroup}\n"))
            .expect("cgroup membership fixture");
        std::fs::write(cgroup_dir.join("dmem.low"), "").expect("dmem.low fixture");
        DmemPaths {
            proc_root: root.join("proc"),
            cgroup_root: root.join("cgroup"),
        }
    }

    #[test]
    fn resolver_accepts_same_uid_and_rejects_different_uid() {
        let accepted_paths = resolver_fixture("same-uid", 42, 1000, "/user.slice/app.scope");
        let own = RelativeCgroupPath::parse("/user.slice/typhon.scope").expect("own path");
        let resolver = ProcCgroupResolver::new(accepted_paths.clone(), 1000, own.clone());
        assert!(resolver.resolve_pid(42).is_ok());
        std::fs::remove_dir_all(accepted_paths.proc_root.parent().expect("fixture root"))
            .expect("cleanup");

        let rejected_paths = resolver_fixture("different-uid", 43, 1001, "/user.slice/app.scope");
        let resolver = ProcCgroupResolver::new(rejected_paths.clone(), 1000, own);
        assert!(matches!(
            resolver.resolve_pid(43),
            Err(DmemError::InvalidTarget("different uid"))
        ));
        std::fs::remove_dir_all(rejected_paths.proc_root.parent().expect("fixture root"))
            .expect("cleanup");
    }

    #[test]
    fn resolver_rejects_missing_or_unusable_target_cgroup() {
        let paths = resolver_fixture("missing-low", 44, 1000, "/user.slice/app.scope");
        std::fs::remove_file(paths.cgroup_root.join("user.slice/app.scope/dmem.low"))
            .expect("remove dmem.low");
        let own = RelativeCgroupPath::parse("/user.slice/typhon.scope").expect("own path");
        let resolver = ProcCgroupResolver::new(paths.clone(), 1000, own);
        assert!(resolver.resolve_pid(44).is_err());
        assert!(resolver.resolve_pid(45).is_err());
        std::fs::remove_dir_all(paths.proc_root.parent().expect("fixture root")).expect("cleanup");
    }

    #[derive(Clone)]
    struct StubResolver {
        results: BTreeMap<u32, Result<ResolvedCgroup, DmemError>>,
    }

    impl DmemTargetResolver for StubResolver {
        fn resolve_pid(&self, pid: u32) -> Result<ResolvedCgroup, DmemError> {
            self.results
                .get(&pid)
                .cloned()
                .unwrap_or(Err(DmemError::InvalidTarget("unknown test pid")))
        }
    }

    impl RecordingWriter {
        fn path(path: &str, pid: u32) -> ResolvedCgroup {
            ResolvedCgroup {
                path: RelativeCgroupPath::parse(path).expect("path"),
                process: ProcessIdentity { pid, start_time: 1 },
            }
        }
    }

    fn controller_fixture(
        resolver: StubResolver,
    ) -> ForegroundController<StubResolver, RecordingWriter> {
        ForegroundController::new(
            resolver,
            RecordingWriter::default(),
            parse_capacity("region-a 10\nregion-b 20").expect("capacity"),
        )
    }

    #[test]
    fn controller_applies_and_reverts_all_regions_for_a_target() {
        let resolved = RecordingWriter::path("/user.slice/app-a.scope", 42);
        let resolver = StubResolver {
            results: [(42, Ok(resolved.clone()))].into_iter().collect(),
        };
        let mut controller = controller_fixture(resolver);
        let target = ForegroundTarget {
            window_id: crate::core::WindowId::from_raw(1).expect("window id"),
            pid: 42,
        };

        assert_eq!(
            controller.reconcile(Some(target)).expect("apply"),
            TransitionResult::Applied
        );
        assert_eq!(controller.current_path(), Some(resolved.path.as_path()));

        assert_eq!(
            controller.reconcile(None).expect("revert"),
            TransitionResult::Reverted
        );
        assert_eq!(controller.current_path(), None);
        assert_eq!(
            controller.writer().writes,
            vec![
                (
                    "user.slice/app-a.scope".to_owned(),
                    "region-a".to_owned(),
                    10
                ),
                (
                    "user.slice/app-a.scope".to_owned(),
                    "region-b".to_owned(),
                    20
                ),
                (
                    "user.slice/app-a.scope".to_owned(),
                    "region-a".to_owned(),
                    0
                ),
                (
                    "user.slice/app-a.scope".to_owned(),
                    "region-b".to_owned(),
                    0
                ),
            ]
        );
    }

    #[test]
    fn controller_reverts_old_target_before_applying_a_different_target() {
        let first = RecordingWriter::path("/user.slice/app-a.scope", 42);
        let second = RecordingWriter::path("/user.slice/app-b.scope", 43);
        let resolver = StubResolver {
            results: [(42, Ok(first.clone())), (43, Ok(second.clone()))]
                .into_iter()
                .collect(),
        };
        let mut controller = controller_fixture(resolver);
        let window_a = ForegroundTarget {
            window_id: crate::core::WindowId::from_raw(1).expect("window id"),
            pid: 42,
        };
        let window_b = ForegroundTarget {
            window_id: crate::core::WindowId::from_raw(2).expect("window id"),
            pid: 43,
        };

        controller.reconcile(Some(window_a)).expect("first apply");
        assert_eq!(
            controller.reconcile(Some(window_b)).expect("second apply"),
            TransitionResult::Applied
        );
        assert_eq!(
            controller.writer().writes,
            vec![
                (
                    "user.slice/app-a.scope".to_owned(),
                    "region-a".to_owned(),
                    10
                ),
                (
                    "user.slice/app-a.scope".to_owned(),
                    "region-b".to_owned(),
                    20
                ),
                (
                    "user.slice/app-a.scope".to_owned(),
                    "region-a".to_owned(),
                    0
                ),
                (
                    "user.slice/app-a.scope".to_owned(),
                    "region-b".to_owned(),
                    0
                ),
                (
                    "user.slice/app-b.scope".to_owned(),
                    "region-a".to_owned(),
                    10
                ),
                (
                    "user.slice/app-b.scope".to_owned(),
                    "region-b".to_owned(),
                    20
                ),
            ]
        );
    }

    #[test]
    fn controller_skips_writes_for_same_cgroup_and_cleans_up_failed_new_targets() {
        let first = RecordingWriter::path("/user.slice/app.scope", 42);
        let resolver = StubResolver {
            results: [
                (42, Ok(first.clone())),
                (43, Ok(RecordingWriter::path("/user.slice/app.scope", 43))),
                (44, Err(DmemError::InvalidTarget("new target unavailable"))),
            ]
            .into_iter()
            .collect(),
        };
        let mut controller = controller_fixture(resolver);
        let target_a = ForegroundTarget {
            window_id: crate::core::WindowId::from_raw(1).expect("window id"),
            pid: 42,
        };
        let target_same_cgroup = ForegroundTarget {
            window_id: crate::core::WindowId::from_raw(2).expect("window id"),
            pid: 43,
        };
        let target_failed = ForegroundTarget {
            window_id: crate::core::WindowId::from_raw(3).expect("window id"),
            pid: 44,
        };

        controller.reconcile(Some(target_a)).expect("first apply");
        assert_eq!(
            controller
                .reconcile(Some(target_same_cgroup))
                .expect("same cgroup"),
            TransitionResult::SameCgroup
        );
        assert_eq!(controller.writer().writes.len(), 2);
        assert!(controller.reconcile(Some(target_failed)).is_err());
        assert_eq!(controller.current_path(), None);
        assert_eq!(controller.writer().writes.len(), 4);
    }

    #[test]
    fn controller_treats_disappearing_old_cgroup_as_a_successful_revert() {
        let first = RecordingWriter::path("/user.slice/app.scope", 42);
        let resolver = StubResolver {
            results: [(42, Ok(first.clone()))].into_iter().collect(),
        };
        let mut controller = controller_fixture(resolver);
        let target = ForegroundTarget {
            window_id: crate::core::WindowId::from_raw(1).expect("window id"),
            pid: 42,
        };
        controller.reconcile(Some(target)).expect("apply");
        controller.writer_mut().missing = true;

        assert_eq!(
            controller
                .reconcile(None)
                .expect("disappearing cgroup is okay"),
            TransitionResult::Reverted
        );
        assert_eq!(controller.current_path(), None);
    }

    fn worker_fixture(name: &str, include_a_low: bool) -> (DmemPaths, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("typhon-dmem-worker-{}-{name}", std::process::id()));
        let proc_root = root.join("proc");
        let cgroup_root = root.join("cgroup");
        std::fs::create_dir_all(&proc_root).expect("proc root");
        std::fs::create_dir_all(&cgroup_root).expect("cgroup root");
        std::fs::write(cgroup_root.join("cgroup.controllers"), "cpu dmem memory\n")
            .expect("controllers");
        std::fs::write(
            cgroup_root.join("dmem.capacity"),
            "region-a 10\nregion-b 20\n",
        )
        .expect("capacity");

        for (pid, cgroup) in [
            (1, "/typhon.scope"),
            (42, "/app-a.scope"),
            (43, "/app-b.scope"),
        ] {
            let proc_dir = proc_root.join(pid.to_string());
            let cgroup_dir = cgroup_root.join(cgroup.trim_start_matches('/'));
            std::fs::create_dir_all(&proc_dir).expect("process directory");
            std::fs::create_dir_all(&cgroup_dir).expect("cgroup directory");
            std::fs::write(
                proc_dir.join("status"),
                "Name:\tfixture\nUid:\t1000\t1000\t1000\t1000\n",
            )
            .expect("status");
            std::fs::write(
                proc_dir.join("stat"),
                "123 (fixture) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21",
            )
            .expect("stat");
            std::fs::write(proc_dir.join("cgroup"), format!("0::{cgroup}\n"))
                .expect("cgroup membership");
            if cgroup != "/app-a.scope" || include_a_low {
                std::fs::write(cgroup_dir.join("dmem.low"), "").expect("dmem.low");
            }
        }

        (
            DmemPaths {
                proc_root,
                cgroup_root,
            },
            root,
        )
    }

    fn wait_until<F>(mut condition: F)
    where
        F: FnMut() -> bool,
    {
        for _ in 0..500 {
            if condition() {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("condition did not become true");
    }

    #[test]
    fn worker_reconciles_latest_focus_and_reverts_on_shutdown() {
        let (paths, root) = worker_fixture("focus", true);
        let mut foreground = DmemForeground::start(DmemForegroundPolicy::Auto, paths, 1000, 1);
        let window_a = ForegroundTarget {
            window_id: crate::core::WindowId::from_raw(1).expect("window id"),
            pid: 42,
        };
        let window_b = ForegroundTarget {
            window_id: crate::core::WindowId::from_raw(2).expect("window id"),
            pid: 43,
        };

        foreground.submit(Some(window_a));
        wait_until(|| foreground.snapshot().protected_cgroup.as_deref() == Some("app-a.scope"));
        foreground.submit(Some(window_b));
        wait_until(|| foreground.snapshot().protected_cgroup.as_deref() == Some("app-b.scope"));
        foreground.submit(None);
        wait_until(|| foreground.snapshot().protected_cgroup.is_none());
        assert!(foreground.snapshot().counters.transitions_applied >= 2);
        foreground.shutdown();
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn worker_accepts_pid_metadata_appearing_after_focus_and_skips_off_policy() {
        let (paths, root) = worker_fixture("late-pid", true);
        let mut foreground = DmemForeground::start(DmemForegroundPolicy::Auto, paths, 1000, 1);
        let window = crate::core::WindowId::from_raw(7).expect("window id");
        foreground.submit(None);
        foreground.submit(Some(ForegroundTarget {
            window_id: window,
            pid: 42,
        }));
        wait_until(|| foreground.snapshot().protected_cgroup.is_some());
        foreground.shutdown();
        std::fs::remove_dir_all(root).expect("cleanup");

        let (paths, root) = worker_fixture("off", true);
        let foreground = DmemForeground::start(DmemForegroundPolicy::Off, paths, 1000, 1);
        assert!(!foreground.snapshot().worker_available);
        drop(foreground);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn retrying_target_is_superseded_by_newer_focus() {
        let (paths, root) = worker_fixture("retry", false);
        let mut foreground =
            DmemForeground::start(DmemForegroundPolicy::Auto, paths.clone(), 1000, 1);
        foreground.submit(Some(ForegroundTarget {
            window_id: crate::core::WindowId::from_raw(1).expect("window id"),
            pid: 42,
        }));
        wait_until(|| foreground.snapshot().counters.retries > 0);
        foreground.submit(Some(ForegroundTarget {
            window_id: crate::core::WindowId::from_raw(2).expect("window id"),
            pid: 43,
        }));
        wait_until(|| foreground.snapshot().protected_cgroup.as_deref() == Some("app-b.scope"));
        assert!(foreground.snapshot().counters.stale_desired > 0);
        foreground.shutdown();
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn doctor_severity_follows_policy_and_runtime_health() {
        let base = DmemForegroundSnapshot {
            policy: DmemForegroundPolicy::Auto,
            kernel_capacity_available: false,
            worker_available: false,
            region_count: 0,
            desired_window_id: None,
            desired_pid: None,
            protected_cgroup: None,
            total_protected_capacity: None,
            degraded: false,
            last_failure: None,
            counters: DmemForegroundCounters::default(),
        };
        assert_eq!(
            base.doctor_severity(),
            crate::control_snapshots::DoctorSeverity::Ok
        );

        let mut forced = base.clone();
        forced.policy = DmemForegroundPolicy::On;
        forced.degraded = true;
        assert_eq!(
            forced.doctor_severity(),
            crate::control_snapshots::DoctorSeverity::Warning
        );

        let mut healthy = base;
        healthy.kernel_capacity_available = true;
        healthy.worker_available = true;
        healthy.protected_cgroup = Some("app.scope".to_owned());
        assert_eq!(
            healthy.doctor_severity(),
            crate::control_snapshots::DoctorSeverity::Ok
        );
    }

    #[test]
    fn unsupported_capability_is_nonfatal_in_auto_and_degraded_in_on() {
        let (paths, root) = worker_fixture("unsupported", true);
        std::fs::remove_file(paths.cgroup_root.join("cgroup.controllers"))
            .expect("remove controllers");
        let auto = DmemForeground::start(DmemForegroundPolicy::Auto, paths.clone(), 1000, 1);
        assert_eq!(
            auto.snapshot().doctor_severity(),
            crate::control_snapshots::DoctorSeverity::Ok
        );
        assert!(!auto.snapshot().worker_available);
        drop(auto);
        std::fs::remove_dir_all(root).expect("cleanup");

        let (paths, root) = worker_fixture("forced-unsupported", true);
        std::fs::remove_file(paths.cgroup_root.join("cgroup.controllers"))
            .expect("remove controllers");
        let forced = DmemForeground::start(DmemForegroundPolicy::On, paths, 1000, 1);
        assert_eq!(
            forced.snapshot().doctor_severity(),
            crate::control_snapshots::DoctorSeverity::Warning
        );
        drop(forced);
        std::fs::remove_dir_all(root).expect("cleanup");
    }
}
