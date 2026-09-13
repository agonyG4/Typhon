use std::{
    collections::BTreeMap,
    env,
    ffi::CString,
    fmt::{self, Display, Formatter},
    fs::{self, File},
    io,
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::ffi::OsStrExt,
    },
    panic::{self, AssertUnwindSafe},
    path::{Component, Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

#[cfg(test)]
use std::collections::VecDeque;

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
    StaleProcessIdentity,
    InvalidTarget(&'static str),
    Unavailable(&'static str),
    Cleanup {
        primary: Box<Self>,
        cleanup: Box<Self>,
    },
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
            Self::StaleProcessIdentity => formatter.write_str("stale process identity"),
            Self::InvalidTarget(reason) => write!(formatter, "invalid dmem target: {reason}"),
            Self::Unavailable(reason) => write!(formatter, "dmem unavailable: {reason}"),
            Self::Cleanup { primary, cleanup } => {
                write!(formatter, "{primary}; cleanup failed: {cleanup}")
            }
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
            Self::StaleProcessIdentity => true,
            Self::Cleanup { primary, cleanup } => primary.is_retryable() || cleanup.is_retryable(),
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
    let mut touched = Vec::new();
    for (region, capacity) in capacities {
        touched.push(region);
        if let Err(error) = writer.write_region(cgroup, region, *capacity) {
            let primary = DmemError::io("write dmem.low", error);
            return match revert_low_entries(writer, cgroup, touched.iter().copied()) {
                Ok(()) => Err(primary),
                Err(cleanup) => Err(DmemError::Cleanup {
                    primary: Box::new(primary),
                    cleanup: Box::new(cleanup),
                }),
            };
        }
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
    let mut first_error = None;
    for region in regions {
        match writer.write_region(cgroup, region, 0) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(DmemError::io("revert dmem.low", error));
                }
            }
        }
    }
    first_error.map_or(Ok(()), Err)
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
    #[cfg(test)]
    test_start_times: Arc<Mutex<VecDeque<String>>>,
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
            #[cfg(test)]
            test_start_times: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    #[cfg(test)]
    fn with_start_time_sequence(self, start_times: Vec<String>) -> Self {
        Self {
            test_start_times: Arc::new(Mutex::new(start_times.into_iter().collect())),
            ..self
        }
    }

    fn read_start_time(&self, path: &Path) -> Result<u64, DmemError> {
        #[cfg(test)]
        if let Some(contents) = self
            .test_start_times
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
        {
            return parse_process_start_time(&contents);
        }
        parse_process_start_time(
            &fs::read_to_string(path).map_err(|error| DmemError::io("read process stat", error))?,
        )
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

        let start_time = self.read_start_time(&process_root.join("stat"))?;
        let path = parse_cgroup_v2_path(
            &fs::read_to_string(process_root.join("cgroup"))
                .map_err(|error| DmemError::io("read process cgroup", error))?,
        )?;
        validate_cgroup_target(&path, &self.typhon_cgroup)?;

        FilesystemDmemWriter::new(self.paths.cgroup_root.clone()).check_usable(&path)?;

        let final_start_time = self.read_start_time(&process_root.join("stat"))?;
        validate_process_identity(start_time, final_start_time)?;

        Ok(ResolvedCgroup {
            path,
            process: ProcessIdentity { pid, start_time },
        })
    }
}

fn validate_process_identity(
    initial_start_time: u64,
    final_start_time: u64,
) -> Result<(), DmemError> {
    if initial_start_time != final_start_time {
        return Err(DmemError::StaleProcessIdentity);
    }
    Ok(())
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
    Stale,
    Noop,
}

pub struct ForegroundController<R, W> {
    resolver: R,
    writer: W,
    capacities: BTreeMap<String, u64>,
    current: Option<ResolvedCgroup>,
    pending_cleanup: Option<ResolvedCgroup>,
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
            pending_cleanup: None,
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
        self.reconcile_with(desired, || true)
    }

    fn reconcile_with<F>(
        &mut self,
        desired: Option<ForegroundTarget>,
        mut should_continue: F,
    ) -> Result<TransitionResult, DmemError>
    where
        F: FnMut() -> bool,
    {
        self.revert_pending_cleanup()?;

        let Some(desired) = desired else {
            if self.current.is_none() {
                return Ok(TransitionResult::Noop);
            }
            if !should_continue() {
                return Ok(TransitionResult::Stale);
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

        let resolved = resolved?;

        if !should_continue() {
            return Ok(TransitionResult::Stale);
        }

        if self.current.is_some() {
            self.revert_current()?;
        }

        if !should_continue() {
            return Ok(TransitionResult::Stale);
        }

        self.apply_target(resolved, &mut should_continue)
    }

    fn apply_target<F>(
        &mut self,
        resolved: ResolvedCgroup,
        should_continue: &mut F,
    ) -> Result<TransitionResult, DmemError>
    where
        F: FnMut() -> bool,
    {
        let mut touched = Vec::new();
        for (region, capacity) in &self.capacities {
            if !should_continue() {
                return self.finish_stale_target(resolved, touched);
            }
            touched.push(region.clone());
            if let Err(error) = self.writer.write_region(&resolved.path, region, *capacity) {
                let primary = DmemError::io("write dmem.low", error);
                return Err(self.finish_failed_target(resolved, touched, primary));
            }
        }

        if !should_continue() {
            return self.finish_stale_target(resolved, touched);
        }

        self.current = Some(resolved);
        Ok(TransitionResult::Applied)
    }

    fn finish_stale_target(
        &mut self,
        resolved: ResolvedCgroup,
        touched: Vec<String>,
    ) -> Result<TransitionResult, DmemError> {
        if touched.is_empty() {
            return Ok(TransitionResult::Stale);
        }
        self.pending_cleanup = Some(resolved.clone());
        match revert_low_entries(&mut self.writer, &resolved.path, touched.iter()) {
            Ok(()) => {
                self.pending_cleanup = None;
                Ok(TransitionResult::Stale)
            }
            Err(cleanup) => Err(cleanup),
        }
    }

    fn finish_failed_target(
        &mut self,
        resolved: ResolvedCgroup,
        touched: Vec<String>,
        primary: DmemError,
    ) -> DmemError {
        self.pending_cleanup = Some(resolved.clone());
        match revert_low_entries(&mut self.writer, &resolved.path, touched.iter()) {
            Ok(()) => {
                self.pending_cleanup = None;
                primary
            }
            Err(cleanup) => DmemError::Cleanup {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            },
        }
    }

    fn revert_pending_cleanup(&mut self) -> Result<(), DmemError> {
        let Some(pending) = self.pending_cleanup.clone() else {
            return Ok(());
        };
        match revert_low_entries(&mut self.writer, &pending.path, self.capacities.keys()) {
            Ok(()) => {
                self.pending_cleanup = None;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn revert_current(&mut self) -> Result<(), DmemError> {
        let Some(current) = self.current.clone() else {
            return Ok(());
        };
        match revert_low_entries(&mut self.writer, &current.path, self.capacities.keys()) {
            Ok(()) => {
                self.current = None;
                Ok(())
            }
            Err(error) => Err(error),
        }
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
        let record = build_low_record(region, value)?;
        let file = self.open_low(cgroup)?;
        write_single_record(file.as_raw_fd(), &record)
    }
}

trait RawWrite {
    fn write(&mut self, fd: RawFd, record: &[u8]) -> io::Result<usize>;
}

struct LibcRawWrite;

impl RawWrite for LibcRawWrite {
    fn write(&mut self, fd: RawFd, record: &[u8]) -> io::Result<usize> {
        let result = unsafe { libc::write(fd, record.as_ptr().cast(), record.len()) };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(result as usize)
        }
    }
}

fn write_single_record(fd: RawFd, record: &[u8]) -> io::Result<()> {
    let mut writer = LibcRawWrite;
    write_single_record_with(&mut writer, fd, record)
}

fn write_single_record_with<W: RawWrite>(
    writer: &mut W,
    fd: RawFd,
    record: &[u8],
) -> io::Result<()> {
    if record.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "dmem.low record must not be empty",
        ));
    }

    loop {
        match writer.write(fd, record) {
            Ok(written) if written == record.len() => return Ok(()),
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "dmem.low write transferred no bytes",
                ));
            }
            Ok(written) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!(
                        "short dmem.low keyed write: transferred {written} of {} bytes",
                        record.len()
                    ),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

fn build_low_record(region: &str, value: u64) -> io::Result<Vec<u8>> {
    if region.is_empty()
        || region.len() > 4096
        || region
            .bytes()
            .any(|byte| byte == b'\0' || byte.is_ascii_whitespace())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid dmem.low region",
        ));
    }
    Ok(format!("{region} {value}\n").into_bytes())
}

#[cfg(test)]
fn write_region_with_raw<W: RawWrite>(
    writer: &mut W,
    fd: RawFd,
    region: &str,
    value: u64,
) -> io::Result<()> {
    let record = build_low_record(region, value)?;
    write_single_record_with(writer, fd, &record)
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
        state.snapshot.last_failure = Some(bounded_string(failure_detail(failure), 256));
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
                    state.snapshot.last_failure = Some(bounded_string(failure_detail(&error), 256));
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

fn failure_detail(failure: &DmemError) -> String {
    match failure {
        DmemError::InvalidTarget("Typhon cgroup")
        | DmemError::InvalidTarget("ancestor of Typhon cgroup") => {
            "focused application shares Typhon's cgroup; application scope isolation is unavailable or was bypassed".to_owned()
        }
        _ => failure.to_string(),
    }
}

fn contains_write_failure(failure: &DmemError) -> bool {
    match failure {
        DmemError::Io { operation, .. } => {
            *operation == "write dmem.low" || *operation == "revert dmem.low"
        }
        DmemError::Cleanup { primary, cleanup } => {
            contains_write_failure(primary) || contains_write_failure(cleanup)
        }
        _ => false,
    }
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

            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                controller.reconcile_with(desired, || !generation_is_stale(&shared, generation))
            }));
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
                            TransitionResult::Stale => {
                                state.snapshot.counters.stale_desired += 1;
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
                    if contains_write_failure(&error) {
                        state.snapshot.counters.write_failures += 1;
                    } else {
                        state.snapshot.counters.resolution_failures += 1;
                    }
                    state.snapshot.last_failure = Some(bounded_string(failure_detail(&error), 256));
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
#[path = "dmem_foreground_tests.rs"]
mod tests;
