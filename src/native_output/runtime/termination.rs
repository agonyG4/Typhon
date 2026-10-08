use serde::Serialize;
use std::{
    env,
    error::Error,
    fmt::{self, Write as _},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_ERROR_CHAIN_BYTES: usize = 2_048;
const MAX_SUMMARY_BYTES: usize = 16_384;
const MAX_STALE_INCOMPLETE_RUN_RECORDS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessLiveness {
    Alive,
    Dead,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ExitCategory {
    Normal,
    Signal,
    Error,
    Panic,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ExitFailureKind {
    Runtime,
    Reactor,
    KmsWorker,
    Seat,
    KmsTeardown,
    Panic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RuntimePhase {
    Bootstrap,
    RuntimeCycle,
    ReactorWait,
    KmsWorker,
    SeatEvents,
    SessionRecovery,
    ShutdownRequest,
    PageflipDrain,
    StoppingChildren,
    KmsTeardown,
    EmergencyCleanup,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TerminationCause {
    pub(super) category: ExitCategory,
    pub(super) failure_kind: Option<ExitFailureKind>,
    pub(super) phase: RuntimePhase,
    pub(super) error_chain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) signal: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TerminationRunMetadata {
    pub(super) session_id: String,
    pub(super) process_id: u32,
    pub(super) boot_id: Option<String>,
    pub(super) desktop_session_id: Option<String>,
    pub(super) started_at_unix_ms: u64,
}

impl TerminationRunMetadata {
    fn current() -> Self {
        let process_id = std::process::id();
        let started_at_unix_ms = unix_time_ms();
        let boot_id = read_kernel_identifier("/proc/sys/kernel/random/boot_id");
        let session_id = read_kernel_identifier("/proc/sys/kernel/random/uuid")
            .filter(|value| {
                value
                    .chars()
                    .all(|character| character.is_ascii_hexdigit() || character == '-')
            })
            .unwrap_or_else(|| format!("{process_id}-{started_at_unix_ms}"));
        let desktop_session_id = std::env::var("XDG_SESSION_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(|value| value.chars().take(128).collect());

        Self {
            session_id,
            process_id,
            boot_id,
            desktop_session_id,
            started_at_unix_ms,
        }
    }
}

fn read_kernel_identifier(path: &str) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TerminationRecordStatus {
    Running,
    FatalCheckpoint,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(super) struct TerminationSnapshot {
    pub(super) latest_submitted_frame_id: Option<u64>,
    pub(super) latest_presented_frame_id: Option<u64>,
    pub(super) pageflip_pending: bool,
    pub(super) pending_pageflip_token: Option<u64>,
    pub(super) kms_worker_present: bool,
    pub(super) kms_worker_lifecycle: Option<&'static str>,
    pub(super) kms_worker_queue_depth: usize,
    pub(super) kms_worker_inflight: bool,
    pub(super) kms_worker_fatal_reason: Option<&'static str>,
    pub(super) seat_state: &'static str,
    pub(super) seat_session_active: bool,
    pub(super) active_gpu_release_leases: usize,
    pub(super) active_output_transactions: usize,
    pub(super) submitted_kms_ownership: usize,
    pub(super) quarantined_kms_ownership: usize,
    pub(super) kms_teardown_safety: &'static str,
}

impl Default for TerminationSnapshot {
    fn default() -> Self {
        Self {
            latest_submitted_frame_id: None,
            latest_presented_frame_id: None,
            pageflip_pending: false,
            pending_pageflip_token: None,
            kms_worker_present: false,
            kms_worker_lifecycle: None,
            kms_worker_queue_depth: 0,
            kms_worker_inflight: false,
            kms_worker_fatal_reason: None,
            seat_state: "unknown",
            seat_session_active: false,
            active_gpu_release_leases: 0,
            active_output_transactions: 0,
            submitted_kms_ownership: 0,
            quarantined_kms_ownership: 0,
            kms_teardown_safety: "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct TerminationSummary {
    pub(super) schema_version: u8,
    pub(super) status: TerminationRecordStatus,
    pub(super) metadata: TerminationRunMetadata,
    pub(super) cause: TerminationCause,
    pub(super) cause_observed_at_unix_ms: Option<u64>,
    pub(super) runtime_phase: RuntimePhase,
    pub(super) pre_cleanup_runtime_phase: Option<RuntimePhase>,
    pub(super) pre_cleanup_captured_at_unix_ms: Option<u64>,
    pub(super) pre_cleanup_snapshot: Option<TerminationSnapshot>,
    pub(super) final_kms_teardown_outcome: Option<&'static str>,
    pub(super) recorded_at_unix_ms: u64,
    pub(super) cleanup_error: Option<String>,
    pub(super) cleanup_error_count: u32,
}

#[derive(Debug)]
pub(super) struct NativeTerminationJournal {
    first_cause: Option<TerminationCause>,
    runtime_phase: RuntimePhase,
    metadata: TerminationRunMetadata,
    cause_observed_at_unix_ms: Option<u64>,
    pre_cleanup_snapshot: Option<TerminationSnapshot>,
    pre_cleanup_runtime_phase: Option<RuntimePhase>,
    pre_cleanup_captured_at_unix_ms: Option<u64>,
    cleanup_error: Option<String>,
    cleanup_error_count: u32,
}

impl Default for RuntimePhase {
    fn default() -> Self {
        Self::Bootstrap
    }
}

impl NativeTerminationJournal {
    pub(super) fn new() -> Self {
        Self {
            first_cause: None,
            runtime_phase: RuntimePhase::Bootstrap,
            metadata: TerminationRunMetadata::current(),
            cause_observed_at_unix_ms: None,
            pre_cleanup_snapshot: None,
            pre_cleanup_runtime_phase: None,
            pre_cleanup_captured_at_unix_ms: None,
            cleanup_error: None,
            cleanup_error_count: 0,
        }
    }

    pub(super) fn set_phase(&mut self, phase: RuntimePhase) {
        self.runtime_phase = phase;
    }

    pub(super) fn capture_pre_cleanup_snapshot(&mut self, snapshot: TerminationSnapshot) {
        if self.pre_cleanup_snapshot.is_some() {
            return;
        }
        self.pre_cleanup_runtime_phase = Some(self.runtime_phase);
        self.pre_cleanup_captured_at_unix_ms = Some(unix_time_ms());
        self.pre_cleanup_snapshot = Some(snapshot);
    }

    pub(super) fn persist_run_started(&self) {
        let incomplete_runs_dir = incomplete_runs_dir();
        if let Err(error) = prune_stale_incomplete_runs(
            &incomplete_runs_dir,
            self.metadata.boot_id.as_deref(),
            process_liveness,
        ) {
            let _ = writeln!(
                io::stderr().lock(),
                "oblivion-one: stale run marker retention failed: {error}"
            );
        }
        let summary = self.summary(None, TerminationRecordStatus::Running);
        let path = run_record_path(&summary.metadata.session_id);
        if let Err(error) = write_summary_file(&path, &summary) {
            let _ = writeln!(
                io::stderr().lock(),
                "oblivion-one: run-start marker write failed: {error}"
            );
        }
    }

    pub(super) fn persist_fatal_checkpoint(&self) {
        let summary = self.summary(None, TerminationRecordStatus::FatalCheckpoint);
        let path = run_record_path(&summary.metadata.session_id);
        if let Err(error) = write_summary_file(&path, &summary) {
            let _ = writeln!(
                io::stderr().lock(),
                "oblivion-one: fatal termination checkpoint write failed: {error}"
            );
        }
    }

    #[cfg(test)]
    pub(super) fn first_cause(&self) -> Option<&TerminationCause> {
        self.first_cause.as_ref()
    }

    pub(super) fn record_shortcut_shutdown(&mut self) {
        self.record_first_cause(TerminationCause {
            category: ExitCategory::Normal,
            failure_kind: None,
            phase: RuntimePhase::ShutdownRequest,
            error_chain: None,
            signal: None,
        });
    }

    pub(super) fn record_signal_shutdown(&mut self, signal: &'static str) {
        self.record_first_cause(TerminationCause {
            category: ExitCategory::Signal,
            failure_kind: None,
            phase: RuntimePhase::ShutdownRequest,
            error_chain: None,
            signal: Some(signal),
        });
    }

    pub(super) fn record_runtime_error(&mut self, error: &dyn Error) {
        if self.first_cause.is_some() {
            self.record_cleanup_error(error);
            return;
        }
        let phase = self.runtime_phase;
        let failure_kind = match phase {
            RuntimePhase::ReactorWait => ExitFailureKind::Reactor,
            RuntimePhase::KmsWorker => ExitFailureKind::KmsWorker,
            RuntimePhase::SeatEvents | RuntimePhase::SessionRecovery => ExitFailureKind::Seat,
            RuntimePhase::KmsTeardown => ExitFailureKind::KmsTeardown,
            _ => ExitFailureKind::Runtime,
        };
        self.record_first_cause(TerminationCause {
            category: ExitCategory::Error,
            failure_kind: Some(failure_kind),
            phase,
            error_chain: Some(format_error_chain(error)),
            signal: None,
        });
    }

    pub(super) fn record_panic_if_unwinding(&mut self) -> bool {
        if !std::thread::panicking() {
            return false;
        }

        if self.first_cause.is_some() {
            self.record_cleanup_error(&io::Error::other(
                "panic during native runtime cleanup; see Rust panic output for details",
            ));
            return true;
        }

        let phase = self.runtime_phase;
        self.record_first_cause(TerminationCause {
            category: ExitCategory::Panic,
            failure_kind: Some(ExitFailureKind::Panic),
            phase,
            error_chain: Some(
                "Rust panic during native runtime unwinding; see Rust panic output for details"
                    .to_string(),
            ),
            signal: None,
        });
        true
    }

    pub(super) fn record_cleanup_error(&mut self, error: &dyn Error) {
        self.cleanup_error_count = self.cleanup_error_count.saturating_add(1);
        if self.cleanup_error.is_none() {
            self.cleanup_error = Some(format_error_chain(error));
        }
    }

    fn record_first_cause(&mut self, cause: TerminationCause) {
        if self.first_cause.is_none() {
            self.first_cause = Some(cause);
            self.cause_observed_at_unix_ms = Some(unix_time_ms());
        }
    }

    pub(super) fn summary(
        &self,
        final_kms_teardown_outcome: Option<&'static str>,
        status: TerminationRecordStatus,
    ) -> TerminationSummary {
        TerminationSummary {
            schema_version: 3,
            status,
            metadata: self.metadata.clone(),
            cause: self.first_cause.clone().unwrap_or(TerminationCause {
                category: ExitCategory::Unknown,
                failure_kind: None,
                phase: self.runtime_phase,
                error_chain: None,
                signal: None,
            }),
            cause_observed_at_unix_ms: self.cause_observed_at_unix_ms,
            runtime_phase: self.runtime_phase,
            pre_cleanup_runtime_phase: self.pre_cleanup_runtime_phase,
            pre_cleanup_captured_at_unix_ms: self.pre_cleanup_captured_at_unix_ms,
            pre_cleanup_snapshot: self.pre_cleanup_snapshot,
            final_kms_teardown_outcome,
            recorded_at_unix_ms: unix_time_ms(),
            cleanup_error: self.cleanup_error.clone(),
            cleanup_error_count: self.cleanup_error_count,
        }
    }
}

pub(super) fn write_summary(
    writer: &mut impl Write,
    summary: &TerminationSummary,
) -> io::Result<()> {
    let encoded = serde_json::to_vec(summary).map_err(io::Error::other)?;
    if encoded.len() > MAX_SUMMARY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "termination summary exceeded its fixed size limit",
        ));
    }
    writer.write_all(&encoded)?;
    writer.write_all(b"\n")
}

pub(super) fn write_summary_file(path: &Path, summary: &TerminationSummary) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "summary path has no parent"))?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }

    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary)?;
        write_summary(&mut file, summary)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(super) fn emit_summary(summary: &TerminationSummary) {
    let stderr_result = write_summary(&mut io::stderr().lock(), summary);
    let run_path = run_record_path(&summary.metadata.session_id);
    let run_result = write_summary_file(&run_path, summary);
    let termination_result = write_summary_file(&termination_summary_path(), summary);
    if let Err(file_error) = run_result.as_ref() {
        let _ = writeln!(
            io::stderr().lock(),
            "oblivion-one: run record file write failed: {file_error}"
        );
    }
    if let Err(file_error) = termination_result.as_ref() {
        let _ = writeln!(
            io::stderr().lock(),
            "oblivion-one: termination summary file write failed: {file_error}"
        );
    }
    if run_result.is_ok() && termination_result.is_ok() {
        if let Err(error) = fs::remove_file(&run_path)
            && error.kind() != io::ErrorKind::NotFound
        {
            let _ = writeln!(
                io::stderr().lock(),
                "oblivion-one: completed run marker cleanup failed: {error}"
            );
        }
    }
    if let Err(stderr_error) = stderr_result {
        let _ = writeln!(
            io::stderr().lock(),
            "oblivion-one: termination summary stderr write failed: {stderr_error}"
        );
    }
}

fn termination_diagnostics_dir() -> std::path::PathBuf {
    let state_dir = env::var_os("OBLIVION_ONE_STATE_DIR")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            env::var_os("XDG_STATE_HOME")
                .filter(|value| !value.is_empty())
                .map(|value| std::path::PathBuf::from(value).join("oblivion-one"))
        })
        .unwrap_or_else(oblivion_one::default_state_dir);
    state_dir.join("diagnostics")
}

fn run_record_path(session_id: &str) -> std::path::PathBuf {
    run_record_path_in(&termination_diagnostics_dir(), session_id)
}

fn incomplete_runs_dir() -> std::path::PathBuf {
    termination_diagnostics_dir().join("incomplete-runs")
}

fn run_record_path_in(diagnostics_dir: &Path, session_id: &str) -> std::path::PathBuf {
    diagnostics_dir
        .join("incomplete-runs")
        .join(format!("{session_id}.json"))
}

fn process_liveness(process_id: u32) -> ProcessLiveness {
    match fs::metadata(format!("/proc/{process_id}")) {
        Ok(_) => ProcessLiveness::Alive,
        Err(error) if error.kind() == io::ErrorKind::NotFound => ProcessLiveness::Dead,
        Err(_) => ProcessLiveness::Unknown,
    }
}

fn prune_stale_incomplete_runs(
    directory: &Path,
    current_boot_id: Option<&str>,
    mut process_state: impl FnMut(u32) -> ProcessLiveness,
) -> io::Result<usize> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut stale = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.path().extension().is_some_and(|ext| ext == "json")
        {
            let path = entry.path();
            let metadata = entry.metadata()?;
            if metadata.len() > MAX_SUMMARY_BYTES as u64 {
                continue;
            }
            let contents = match fs::read(&path) {
                Ok(contents) => contents,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            let record: serde_json::Value = match serde_json::from_slice(&contents) {
                Ok(record) => record,
                Err(_) => continue,
            };
            if !matches!(
                record.get("status").and_then(serde_json::Value::as_str),
                Some("running" | "fatal_checkpoint")
            ) {
                continue;
            }
            let Some(record_metadata) = record.get("metadata") else {
                continue;
            };
            let Some(process_id) = record_metadata
                .get("process_id")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
            else {
                continue;
            };
            let record_boot_id = record_metadata
                .get("boot_id")
                .and_then(serde_json::Value::as_str);
            let boot_changed = current_boot_id
                .zip(record_boot_id)
                .is_some_and(|(current, recorded)| current != recorded);
            if !boot_changed && process_state(process_id) != ProcessLiveness::Dead {
                continue;
            }
            let recorded_start = record_metadata
                .get("started_at_unix_ms")
                .and_then(serde_json::Value::as_u64)
                .or_else(|| {
                    metadata
                        .modified()
                        .ok()?
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
                })
                .unwrap_or(0);
            stale.push((recorded_start, path));
        }
    }

    stale.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let remove_count = stale.len().saturating_sub(MAX_STALE_INCOMPLETE_RUN_RECORDS);
    for (_, path) in stale.into_iter().take(remove_count) {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(remove_count)
}

fn termination_summary_path() -> std::path::PathBuf {
    termination_diagnostics_dir().join("last-termination.json")
}

pub(crate) fn format_error_chain(error: &dyn Error) -> String {
    let mut output = BoundedString::new(MAX_ERROR_CHAIN_BYTES.saturating_sub(3));
    let mut current = Some(error);
    let mut depth = 0;
    while let Some(cause) = current {
        if depth > 0 {
            let _ = output.write_str(": ");
        }
        let _ = write!(output, "{cause}");
        let source = cause.source();
        depth += 1;
        if output.truncated || depth == 8 {
            current = source;
            break;
        }
        current = source;
    }
    if output.truncated {
        output.value.push_str("...");
    } else if depth == 8 && current.is_some() {
        append_bounded_suffix(&mut output.value, ": [source chain truncated]");
    }
    output.value
}

fn append_bounded_suffix(value: &mut String, suffix: &str) {
    if value.len().saturating_add(suffix.len()) <= MAX_ERROR_CHAIN_BYTES {
        value.push_str(suffix);
        return;
    }

    let prefix_limit = MAX_ERROR_CHAIN_BYTES.saturating_sub(suffix.len());
    let mut end = prefix_limit.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value.push_str(suffix);
}

struct BoundedString {
    value: String,
    limit: usize,
    truncated: bool,
}

impl BoundedString {
    fn new(limit: usize) -> Self {
        Self {
            value: String::with_capacity(limit),
            limit,
            truncated: false,
        }
    }
}

impl fmt::Write for BoundedString {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let remaining = self.limit.saturating_sub(self.value.len());
        if value.len() <= remaining {
            self.value.push_str(value);
            return Ok(());
        }
        let mut end = remaining.min(value.len());
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        self.value.push_str(&value[..end]);
        self.truncated = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ExitCategory, ExitFailureKind, MAX_STALE_INCOMPLETE_RUN_RECORDS, NativeTerminationJournal,
        ProcessLiveness, RuntimePhase, TerminationRecordStatus, TerminationSnapshot,
        prune_stale_incomplete_runs, write_summary, write_summary_file,
    };
    use std::{error::Error, fmt, fs, io, path::PathBuf};

    fn test_output_path(name: &str) -> PathBuf {
        std::env::current_exe()
            .expect("test executable path")
            .parent()
            .expect("test executable directory")
            .join(format!("{name}-{}", std::process::id()))
    }

    #[derive(Debug)]
    struct OuterError(io::Error);

    impl fmt::Display for OuterError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "outer: {}", self.0)
        }
    }

    impl Error for OuterError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.0)
        }
    }

    #[derive(Debug)]
    struct ChainedError {
        index: usize,
        source: Option<Box<ChainedError>>,
    }

    impl fmt::Display for ChainedError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "error-{}", self.index)
        }
    }

    impl Error for ChainedError {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.source
                .as_deref()
                .map(|source| source as &(dyn Error + 'static))
        }
    }

    fn chained_error(count: usize) -> ChainedError {
        assert!(count > 0);
        let mut error = ChainedError {
            index: count - 1,
            source: None,
        };
        for index in (0..count - 1).rev() {
            error = ChainedError {
                index,
                source: Some(Box::new(error)),
            };
        }
        error
    }

    #[test]
    fn normal_exit_has_a_typed_shortcut_cause() {
        let mut journal = NativeTerminationJournal::new();
        journal.record_shortcut_shutdown();

        let cause = journal.first_cause().expect("normal exit cause");
        assert_eq!(cause.category, ExitCategory::Normal);
        assert_eq!(cause.phase, RuntimePhase::ShutdownRequest);
        assert_eq!(cause.failure_kind, None);
        assert!(cause.error_chain.is_none());
    }

    #[test]
    fn runtime_error_records_phase_and_full_source_chain() {
        let mut journal = NativeTerminationJournal::new();
        journal.set_phase(RuntimePhase::ReactorWait);
        let error = OuterError(io::Error::other("inner reactor failure"));

        journal.record_runtime_error(&error);

        let cause = journal.first_cause().expect("runtime error cause");
        assert_eq!(cause.category, ExitCategory::Error);
        assert_eq!(cause.failure_kind, Some(ExitFailureKind::Reactor));
        assert_eq!(cause.phase, RuntimePhase::ReactorWait);
        let chain = cause.error_chain.as_deref().expect("captured error chain");
        assert!(chain.contains("outer: inner reactor failure"));
        assert!(chain.contains("inner reactor failure"));
    }

    #[test]
    fn panic_during_native_unwind_is_recorded_before_cleanup() {
        let mut journal = NativeTerminationJournal::new();
        journal.set_phase(RuntimePhase::ReactorWait);

        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            struct RecordPanicOnDrop<'a>(&'a mut NativeTerminationJournal);

            impl Drop for RecordPanicOnDrop<'_> {
                fn drop(&mut self) {
                    self.0.record_panic_if_unwinding();
                }
            }

            let _record_panic = RecordPanicOnDrop(&mut journal);
            panic!("simulated native runtime panic");
        }));

        assert!(unwind.is_err());
        let cause = journal.first_cause().expect("panic is the first cause");
        assert_eq!(cause.category, ExitCategory::Panic);
        assert_eq!(cause.failure_kind, Some(ExitFailureKind::Panic));
        assert_eq!(cause.phase, RuntimePhase::ReactorWait);
        assert!(
            cause
                .error_chain
                .as_deref()
                .is_some_and(|chain| chain.contains("Rust panic"))
        );
    }

    #[test]
    fn panic_during_cleanup_does_not_replace_the_first_runtime_error() {
        let mut journal = NativeTerminationJournal::new();
        journal.set_phase(RuntimePhase::ReactorWait);
        journal.record_runtime_error(&io::Error::other("first reactor failure"));

        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            struct RecordPanicOnDrop<'a>(&'a mut NativeTerminationJournal);

            impl Drop for RecordPanicOnDrop<'_> {
                fn drop(&mut self) {
                    self.0.record_panic_if_unwinding();
                }
            }

            let _record_panic = RecordPanicOnDrop(&mut journal);
            panic!("simulated cleanup panic");
        }));

        assert!(unwind.is_err());
        let cause = journal.first_cause().expect("first runtime error remains");
        assert_eq!(cause.category, ExitCategory::Error);
        assert!(
            cause
                .error_chain
                .as_deref()
                .is_some_and(|chain| chain.contains("first reactor failure"))
        );
        assert!(
            journal
                .summary(None, TerminationRecordStatus::FatalCheckpoint)
                .cleanup_error
                .is_some_and(|error| error.contains("panic during native runtime cleanup"))
        );
    }

    #[test]
    fn error_chain_marks_truncation_only_when_a_ninth_cause_exists() {
        let exactly_eight = chained_error(8);
        let formatted = super::format_error_chain(&exactly_eight);
        assert!(formatted.contains("error-7"));
        assert!(!formatted.contains("source chain truncated"));

        let nine = chained_error(9);
        let formatted = super::format_error_chain(&nine);
        assert!(formatted.contains("error-7"));
        assert!(formatted.contains("source chain truncated"));
        assert!(!formatted.contains("error-8"));
    }

    #[test]
    fn kms_teardown_failure_keeps_its_phase_and_failure_category() {
        let mut journal = NativeTerminationJournal::new();
        journal.set_phase(RuntimePhase::KmsTeardown);
        journal.record_runtime_error(&io::Error::other("restore could not prove safety"));

        let cause = journal.first_cause().expect("KMS teardown cause");
        assert_eq!(cause.category, ExitCategory::Error);
        assert_eq!(cause.failure_kind, Some(ExitFailureKind::KmsTeardown));
        assert_eq!(cause.phase, RuntimePhase::KmsTeardown);
        assert!(
            cause
                .error_chain
                .as_deref()
                .is_some_and(|chain| chain.contains("restore could not prove safety"))
        );
    }

    #[test]
    fn first_actual_cause_survives_later_cleanup_errors() {
        let mut journal = NativeTerminationJournal::new();
        journal.record_shortcut_shutdown();
        journal.set_phase(RuntimePhase::KmsTeardown);
        journal.record_runtime_error(&io::Error::other("KMS cleanup failed"));

        let cause = journal.first_cause().expect("first cause remains recorded");
        assert_eq!(cause.category, ExitCategory::Normal);
        assert_eq!(cause.phase, RuntimePhase::ShutdownRequest);
        assert!(cause.error_chain.is_none());
    }

    #[test]
    fn repeated_shutdown_requests_preserve_the_original_reason() {
        let mut journal = NativeTerminationJournal::new();
        journal.record_shortcut_shutdown();
        let first = journal.first_cause().cloned().expect("first request");
        journal.record_shortcut_shutdown();

        assert_eq!(journal.first_cause(), Some(&first));
    }

    #[test]
    fn signal_shutdown_records_signal_and_preserves_first_cause() {
        let mut journal = NativeTerminationJournal::new();

        journal.record_signal_shutdown("SIGTERM");
        journal.record_signal_shutdown("SIGINT");
        journal.record_shortcut_shutdown();

        let cause = journal.first_cause().expect("first signal cause");
        assert_eq!(cause.category, ExitCategory::Signal);
        assert_eq!(cause.signal, Some("SIGTERM"));
    }

    #[test]
    fn diagnostic_write_failure_is_returned_without_losing_the_cause() {
        struct FailingWriter;
        impl io::Write for FailingWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("diagnostic sink unavailable"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let mut journal = NativeTerminationJournal::new();
        journal.record_shortcut_shutdown();
        journal.capture_pre_cleanup_snapshot(TerminationSnapshot::default());
        let summary = journal.summary(None, TerminationRecordStatus::Completed);

        let error = write_summary(&mut FailingWriter, &summary)
            .expect_err("failed sink must be visible to the fallback reporter");
        assert!(error.to_string().contains("diagnostic sink unavailable"));
        assert_eq!(
            journal.first_cause().map(|cause| cause.category),
            Some(ExitCategory::Normal)
        );
    }

    #[test]
    fn summary_file_is_written_atomically_with_valid_json() {
        let directory = test_output_path("termination-summary-test");
        let path = directory.join("last-termination.json");
        let mut journal = NativeTerminationJournal::new();
        journal.record_shortcut_shutdown();
        journal.capture_pre_cleanup_snapshot(TerminationSnapshot::default());
        let summary = journal.summary(None, TerminationRecordStatus::Completed);

        write_summary_file(&path, &summary).expect("write summary file");
        let bytes = fs::read(&path).expect("read summary file");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");
        assert_eq!(json["schema_version"], 3);
        assert_eq!(json["cause"]["category"], "normal");
        fs::remove_dir_all(directory).expect("remove test output directory");
    }

    #[test]
    fn signal_shutdown_serializes_the_signal_cause() {
        let mut journal = NativeTerminationJournal::new();
        journal.record_signal_shutdown("SIGTERM");
        let summary = journal.summary(None, TerminationRecordStatus::Completed);

        let json = serde_json::to_value(summary).expect("serialize termination summary");
        assert_eq!(json["cause"]["category"], "signal");
        assert_eq!(json["cause"]["signal"], "SIGTERM");
    }

    #[test]
    fn summary_file_failure_preserves_cause_and_returns_sink_error() {
        let blocker = test_output_path("termination-summary-blocker");
        fs::write(&blocker, b"not a directory").expect("create deterministic blocker file");
        let path = blocker.join("last-termination.json");
        let mut journal = NativeTerminationJournal::new();
        journal.record_shortcut_shutdown();
        journal.capture_pre_cleanup_snapshot(TerminationSnapshot::default());
        let summary = journal.summary(None, TerminationRecordStatus::Completed);

        let error = write_summary_file(&path, &summary).expect_err("path parent is a regular file");
        assert_eq!(
            journal.first_cause().map(|cause| cause.category),
            Some(ExitCategory::Normal)
        );
        assert!(!error.to_string().is_empty());
        fs::remove_file(blocker).expect("remove blocker file");
    }

    #[test]
    fn summary_keeps_kms_cleanup_and_frame_obligations_structured() {
        let mut journal = NativeTerminationJournal::new();
        journal.record_shortcut_shutdown();
        let snapshot = TerminationSnapshot {
            latest_submitted_frame_id: Some(41),
            latest_presented_frame_id: Some(40),
            pageflip_pending: true,
            kms_worker_present: true,
            kms_worker_lifecycle: Some("quiescing"),
            kms_teardown_safety: "unknown",
            ..TerminationSnapshot::default()
        };

        journal.capture_pre_cleanup_snapshot(snapshot);
        let summary = journal.summary(Some("safe_disable"), TerminationRecordStatus::Completed);
        let pre_cleanup = summary
            .pre_cleanup_snapshot
            .expect("pre-cleanup snapshot is captured");
        assert_eq!(pre_cleanup.latest_submitted_frame_id, Some(41));
        assert_eq!(pre_cleanup.latest_presented_frame_id, Some(40));
        assert!(pre_cleanup.pageflip_pending);
        assert_eq!(pre_cleanup.kms_teardown_safety, "unknown");
        assert_eq!(summary.final_kms_teardown_outcome, Some("safe_disable"));

        let mut encoded = Vec::new();
        write_summary(&mut encoded, &summary).expect("serialize termination summary");
        let json: serde_json::Value = serde_json::from_slice(&encoded).expect("valid JSON");
        assert_eq!(
            json["pre_cleanup_snapshot"]["latest_submitted_frame_id"],
            41
        );
        assert_eq!(
            json["pre_cleanup_snapshot"]["latest_presented_frame_id"],
            40
        );
        assert_eq!(json["pre_cleanup_snapshot"]["pageflip_pending"], true);
        assert_eq!(json["pre_cleanup_snapshot"]["kms_worker_present"], true);
        assert_eq!(
            json["pre_cleanup_snapshot"]["kms_worker_lifecycle"],
            "quiescing"
        );
        assert_eq!(
            json["pre_cleanup_snapshot"]["kms_teardown_safety"],
            "unknown"
        );
        assert_eq!(json["final_kms_teardown_outcome"], "safe_disable");
    }

    #[test]
    fn cleanup_failure_is_secondary_and_unproven_kms_ownership_is_reported() {
        let mut journal = NativeTerminationJournal::new();
        journal.record_shortcut_shutdown();
        journal.set_phase(RuntimePhase::KmsTeardown);
        journal.record_runtime_error(&io::Error::other("target destroy failed"));

        journal.capture_pre_cleanup_snapshot(TerminationSnapshot {
            kms_teardown_safety: "unknown",
            submitted_kms_ownership: 2,
            active_gpu_release_leases: 3,
            ..TerminationSnapshot::default()
        });
        let summary = journal.summary(Some("unproven"), TerminationRecordStatus::Completed);

        assert_eq!(summary.cause.category, ExitCategory::Normal);
        assert_eq!(
            summary
                .pre_cleanup_snapshot
                .as_ref()
                .map(|snapshot| snapshot.kms_teardown_safety),
            Some("unknown")
        );
        assert_eq!(summary.final_kms_teardown_outcome, Some("unproven"));
        assert_eq!(
            summary
                .pre_cleanup_snapshot
                .as_ref()
                .unwrap()
                .submitted_kms_ownership,
            2
        );
        assert_eq!(
            summary
                .pre_cleanup_snapshot
                .as_ref()
                .unwrap()
                .active_gpu_release_leases,
            3
        );
        assert_eq!(summary.cleanup_error_count, 1);
        assert!(
            summary
                .cleanup_error
                .as_deref()
                .is_some_and(|error| error.contains("target destroy failed"))
        );
    }

    #[test]
    fn run_metadata_and_checkpoints_keep_one_stable_session_identity() {
        let mut journal = NativeTerminationJournal::new();
        journal.set_phase(RuntimePhase::ReactorWait);
        let started = journal.summary(None, TerminationRecordStatus::Running);
        journal.record_runtime_error(&io::Error::other("reactor failed"));
        journal.capture_pre_cleanup_snapshot(TerminationSnapshot {
            latest_submitted_frame_id: Some(19),
            ..TerminationSnapshot::default()
        });

        let checkpoint = journal.summary(None, TerminationRecordStatus::FatalCheckpoint);
        let completed = journal.summary(Some("safe_disable"), TerminationRecordStatus::Completed);

        assert!(!started.metadata.session_id.is_empty());
        assert_eq!(started.metadata.process_id, std::process::id());
        assert!(started.metadata.started_at_unix_ms > 0);
        assert_eq!(checkpoint.metadata, started.metadata);
        assert_eq!(completed.metadata, started.metadata);
        assert!(
            checkpoint.cause_observed_at_unix_ms.unwrap() >= started.metadata.started_at_unix_ms
        );
        assert_eq!(checkpoint.status, TerminationRecordStatus::FatalCheckpoint);
        assert_eq!(checkpoint.final_kms_teardown_outcome, None);
        assert_eq!(
            checkpoint
                .pre_cleanup_snapshot
                .unwrap()
                .latest_submitted_frame_id,
            Some(19)
        );
        assert_eq!(completed.status, TerminationRecordStatus::Completed);
        assert_eq!(completed.final_kms_teardown_outcome, Some("safe_disable"));
        assert!(completed.recorded_at_unix_ms >= checkpoint.recorded_at_unix_ms);
    }

    #[test]
    fn current_run_marker_identifies_a_new_run_without_replacing_last_termination() {
        let directory = test_output_path("termination-run-marker-test");
        let previous_termination = directory.join("last-termination.json");
        let mut previous_journal = NativeTerminationJournal::new();
        previous_journal.record_shortcut_shutdown();
        previous_journal.capture_pre_cleanup_snapshot(TerminationSnapshot::default());
        let previous = previous_journal.summary(None, TerminationRecordStatus::Completed);
        write_summary_file(&previous_termination, &previous).expect("write previous record");

        let current_journal = NativeTerminationJournal::new();
        let current = current_journal.summary(None, TerminationRecordStatus::Running);
        let current_run = super::run_record_path_in(&directory, &current.metadata.session_id);
        write_summary_file(&current_run, &current).expect("write current run marker");

        let previous_json: serde_json::Value =
            serde_json::from_slice(&fs::read(&previous_termination).expect("read previous record"))
                .expect("valid previous JSON");
        let current_json: serde_json::Value =
            serde_json::from_slice(&fs::read(&current_run).expect("read current marker"))
                .expect("valid current JSON");
        assert_ne!(
            previous_json["metadata"]["session_id"],
            current_json["metadata"]["session_id"]
        );
        assert_eq!(previous_json["status"], "completed");
        assert_eq!(current_json["status"], "running");
        fs::remove_dir_all(directory).expect("remove test output directory");
    }

    #[test]
    fn stale_incomplete_run_retention_keeps_recent_evidence_and_uncertain_records() {
        let directory = test_output_path("termination-incomplete-retention-test");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("create incomplete run directory");

        for index in 0..40 {
            let marker = serde_json::json!({
                "status": "fatal_checkpoint",
                "metadata": {
                    "session_id": format!("stale-{index:02}"),
                    "process_id": 20_000 + index,
                    "boot_id": if index == 0 { "current-boot" } else { "previous-boot" },
                    "started_at_unix_ms": index,
                },
            });
            fs::write(
                directory.join(format!("stale-{index:02}.json")),
                serde_json::to_vec(&marker).expect("serialize stale marker"),
            )
            .expect("write stale marker");
        }

        for (name, pid, boot_id) in [
            ("live", 30_001, "current-boot"),
            ("uncertain", 30_002, "current-boot"),
        ] {
            let marker = serde_json::json!({
                "status": "running",
                "metadata": {
                    "session_id": name,
                    "process_id": pid,
                    "boot_id": boot_id,
                    "started_at_unix_ms": 100,
                },
            });
            fs::write(
                directory.join(format!("{name}.json")),
                serde_json::to_vec(&marker).expect("serialize live marker"),
            )
            .expect("write live marker");
        }
        fs::write(directory.join("unreadable-evidence.json"), b"not json")
            .expect("write uncertain marker");

        prune_stale_incomplete_runs(&directory, Some("current-boot"), |pid| match pid {
            20_000 => ProcessLiveness::Dead,
            20_001..=20_039 | 30_001 => ProcessLiveness::Alive,
            30_002 => ProcessLiveness::Unknown,
            _ => ProcessLiveness::Dead,
        })
        .expect("prune stale markers");

        let retained_stale = (0..40)
            .filter(|index| directory.join(format!("stale-{index:02}.json")).exists())
            .collect::<Vec<_>>();
        assert_eq!(
            retained_stale.len(),
            MAX_STALE_INCOMPLETE_RUN_RECORDS,
            "only the most recent stale markers should remain"
        );
        assert!(!directory.join("stale-00.json").exists());
        assert!(!directory.join("stale-07.json").exists());
        assert!(directory.join("stale-08.json").exists());
        assert!(directory.join("stale-39.json").exists());
        assert!(directory.join("live.json").exists());
        assert!(directory.join("uncertain.json").exists());
        assert!(directory.join("unreadable-evidence.json").exists());

        fs::remove_dir_all(directory).expect("remove incomplete run directory");
    }
}
