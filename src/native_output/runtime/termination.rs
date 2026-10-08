use serde::Serialize;
use std::{
    env,
    error::Error,
    fmt::{self, Write as _},
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::Path,
};

const MAX_ERROR_CHAIN_BYTES: usize = 2_048;
const MAX_SUMMARY_BYTES: usize = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ExitCategory {
    Normal,
    Error,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub(super) cause: TerminationCause,
    pub(super) runtime_phase: RuntimePhase,
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
    pub(super) cleanup_error: Option<String>,
    pub(super) cleanup_error_count: u32,
}

#[derive(Debug, Default)]
pub(super) struct NativeTerminationJournal {
    first_cause: Option<TerminationCause>,
    runtime_phase: RuntimePhase,
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
        Self::default()
    }

    pub(super) fn set_phase(&mut self, phase: RuntimePhase) {
        self.runtime_phase = phase;
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
        });
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
        }
    }

    pub(super) fn summary(&self, snapshot: TerminationSnapshot) -> TerminationSummary {
        TerminationSummary {
            schema_version: 1,
            cause: self.first_cause.clone().unwrap_or(TerminationCause {
                category: ExitCategory::Unknown,
                failure_kind: None,
                phase: self.runtime_phase,
                error_chain: None,
            }),
            runtime_phase: self.runtime_phase,
            latest_submitted_frame_id: snapshot.latest_submitted_frame_id,
            latest_presented_frame_id: snapshot.latest_presented_frame_id,
            pageflip_pending: snapshot.pageflip_pending,
            pending_pageflip_token: snapshot.pending_pageflip_token,
            kms_worker_present: snapshot.kms_worker_present,
            kms_worker_lifecycle: snapshot.kms_worker_lifecycle,
            kms_worker_queue_depth: snapshot.kms_worker_queue_depth,
            kms_worker_inflight: snapshot.kms_worker_inflight,
            kms_worker_fatal_reason: snapshot.kms_worker_fatal_reason,
            seat_state: snapshot.seat_state,
            seat_session_active: snapshot.seat_session_active,
            active_gpu_release_leases: snapshot.active_gpu_release_leases,
            active_output_transactions: snapshot.active_output_transactions,
            submitted_kms_ownership: snapshot.submitted_kms_ownership,
            quarantined_kms_ownership: snapshot.quarantined_kms_ownership,
            kms_teardown_safety: snapshot.kms_teardown_safety,
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
    let path = termination_summary_path();
    let file_result = write_summary_file(&path, summary);
    if let Err(file_error) = file_result {
        let _ = writeln!(
            io::stderr().lock(),
            "oblivion-one: termination summary file write failed: {file_error}"
        );
    }
    if let Err(stderr_error) = stderr_result {
        let _ = writeln!(
            io::stderr().lock(),
            "oblivion-one: termination summary stderr write failed: {stderr_error}"
        );
    }
}

fn termination_summary_path() -> std::path::PathBuf {
    let state_dir = env::var_os("OBLIVION_ONE_STATE_DIR")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            env::var_os("XDG_STATE_HOME")
                .filter(|value| !value.is_empty())
                .map(|value| std::path::PathBuf::from(value).join("oblivion-one"))
        })
        .unwrap_or_else(oblivion_one::default_state_dir);
    state_dir.join("diagnostics").join("last-termination.json")
}

fn format_error_chain(error: &dyn Error) -> String {
    let mut output = BoundedString::new(MAX_ERROR_CHAIN_BYTES.saturating_sub(3));
    let mut current = Some(error);
    let mut depth = 0;
    while let Some(cause) = current {
        if depth > 0 {
            let _ = output.write_str(": ");
        }
        let _ = write!(output, "{cause}");
        depth += 1;
        let source = cause.source();
        if output.truncated || depth == 8 {
            current = source;
            break;
        }
        current = source;
    }
    if output.truncated {
        output.value.push_str("...");
    } else if depth == 8 && current.is_some() {
        let _ = output.write_str(": [source chain truncated]");
    }
    output.value
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
        ExitCategory, ExitFailureKind, NativeTerminationJournal, RuntimePhase, TerminationSnapshot,
        write_summary, write_summary_file,
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
        let summary = journal.summary(TerminationSnapshot::default());

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
        let summary = journal.summary(TerminationSnapshot::default());

        write_summary_file(&path, &summary).expect("write summary file");
        let bytes = fs::read(&path).expect("read summary file");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");
        assert_eq!(json["cause"]["category"], "normal");
        fs::remove_dir_all(directory).expect("remove test output directory");
    }

    #[test]
    fn summary_file_failure_preserves_cause_and_returns_sink_error() {
        let blocker = test_output_path("termination-summary-blocker");
        fs::write(&blocker, b"not a directory").expect("create deterministic blocker file");
        let path = blocker.join("last-termination.json");
        let mut journal = NativeTerminationJournal::new();
        journal.record_shortcut_shutdown();
        let summary = journal.summary(TerminationSnapshot::default());

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
            kms_teardown_safety: "safe_disable",
            ..TerminationSnapshot::default()
        };

        let summary = journal.summary(snapshot);
        assert_eq!(summary.latest_submitted_frame_id, Some(41));
        assert_eq!(summary.latest_presented_frame_id, Some(40));
        assert!(summary.pageflip_pending);
        assert_eq!(summary.kms_teardown_safety, "safe_disable");

        let mut encoded = Vec::new();
        write_summary(&mut encoded, &summary).expect("serialize termination summary");
        let json: serde_json::Value = serde_json::from_slice(&encoded).expect("valid JSON");
        assert_eq!(json["latest_submitted_frame_id"], 41);
        assert_eq!(json["latest_presented_frame_id"], 40);
        assert_eq!(json["pageflip_pending"], true);
        assert_eq!(json["kms_worker_present"], true);
        assert_eq!(json["kms_worker_lifecycle"], "quiescing");
        assert_eq!(json["kms_teardown_safety"], "safe_disable");
    }

    #[test]
    fn cleanup_failure_is_secondary_and_unproven_kms_ownership_is_reported() {
        let mut journal = NativeTerminationJournal::new();
        journal.record_shortcut_shutdown();
        journal.set_phase(RuntimePhase::KmsTeardown);
        journal.record_runtime_error(&io::Error::other("target destroy failed"));

        let summary = journal.summary(TerminationSnapshot {
            kms_teardown_safety: "unproven",
            submitted_kms_ownership: 2,
            active_gpu_release_leases: 3,
            ..TerminationSnapshot::default()
        });

        assert_eq!(summary.cause.category, ExitCategory::Normal);
        assert_eq!(summary.kms_teardown_safety, "unproven");
        assert_eq!(summary.submitted_kms_ownership, 2);
        assert_eq!(summary.active_gpu_release_leases, 3);
        assert_eq!(summary.cleanup_error_count, 1);
        assert!(
            summary
                .cleanup_error
                .as_deref()
                .is_some_and(|error| error.contains("target destroy failed"))
        );
    }
}
