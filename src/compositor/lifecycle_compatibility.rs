use std::collections::VecDeque;
use std::ffi::OsStr;

use wayland_server::backend::ClientId;

use super::protocol_error_trace::protocol_error_timestamp_ns;

const LIFECYCLE_COMPATIBILITY_TRACE_CAPACITY: usize = 64;
const LIFECYCLE_COMPATIBILITY_LIVE_TRACE_LIMIT: usize = 64;
const LIFECYCLE_COMPATIBILITY_TRACE_ENV: &str = "TYPHON_WAYLAND_COMPAT_TRACE";
const CLIPBOARD_TRANSFER_TRACE_CAPACITY: usize = 256;
const CLIPBOARD_TRANSFER_LIVE_TRACE_LIMIT: usize = 256;
const CLIPBOARD_TRANSFER_TRACE_ENV: &str = "TYPHON_CLIPBOARD_TRACE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifecycleCompatibilityViolation {
    SurfaceDestroyedWithLiveRole,
    XdgSurfaceDestroyedWithLiveRole,
    ActiveClipboardSourceReused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifecycleCompatibilityAction {
    CanonicalSurfaceTeardown,
    CanonicalXdgRoleTeardown,
    KeepActiveClipboardSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LifecycleCompatibilityRecord {
    pub(crate) timestamp_ns: u64,
    pub(crate) client_id: ClientId,
    pub(crate) object_id: u32,
    pub(crate) interface: &'static str,
    pub(crate) surface_id: Option<u32>,
    pub(crate) violation: LifecycleCompatibilityViolation,
    pub(crate) action: LifecycleCompatibilityAction,
}

#[derive(Debug)]
pub(crate) struct LifecycleCompatibilityTrace {
    records: VecDeque<LifecycleCompatibilityRecord>,
    live_trace_enabled: bool,
    live_records_emitted: usize,
    live_trace_limit_notice_emitted: bool,
    clipboard_records: VecDeque<ClipboardTransferTraceRecord>,
    clipboard_live_trace_enabled: bool,
    clipboard_live_records_emitted: usize,
    clipboard_live_trace_limit_notice_emitted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClipboardTransferTraceRecord {
    timestamp_ns: u64,
    details: String,
}

impl LifecycleCompatibilityTrace {
    pub(crate) fn clipboard_trace_enabled(&self) -> bool {
        self.clipboard_live_trace_enabled
    }

    pub(crate) fn record(
        &mut self,
        client_id: ClientId,
        object_id: u32,
        interface: &'static str,
        surface_id: Option<u32>,
        violation: LifecycleCompatibilityViolation,
        action: LifecycleCompatibilityAction,
    ) {
        let record = LifecycleCompatibilityRecord {
            timestamp_ns: protocol_error_timestamp_ns(),
            client_id,
            object_id,
            interface,
            surface_id,
            violation,
            action,
        };
        if self.records.len() == LIFECYCLE_COMPATIBILITY_TRACE_CAPACITY {
            self.records.pop_front();
        }
        self.records.push_back(record.clone());

        if !self.live_trace_enabled {
            return;
        }
        if self.live_records_emitted < LIFECYCLE_COMPATIBILITY_LIVE_TRACE_LIMIT {
            Self::emit_record(&record);
            self.live_records_emitted += 1;
        } else if !self.live_trace_limit_notice_emitted {
            eprintln!(
                "typhon_lifecycle_compatibility_notice live_trace_limit_reached={} remaining_records=shutdown_ring",
                LIFECYCLE_COMPATIBILITY_LIVE_TRACE_LIMIT,
            );
            self.live_trace_limit_notice_emitted = true;
        }
    }

    pub(crate) fn dump(&self) {
        for record in &self.records {
            Self::emit_record(record);
        }
        for record in &self.clipboard_records {
            Self::emit_clipboard_record(record);
        }
    }

    pub(crate) fn record_clipboard_trace(&mut self, build_details: impl FnOnce() -> String) {
        if !self.clipboard_live_trace_enabled {
            return;
        }

        let record = ClipboardTransferTraceRecord {
            timestamp_ns: protocol_error_timestamp_ns(),
            details: build_details(),
        };
        if self.clipboard_records.len() == CLIPBOARD_TRANSFER_TRACE_CAPACITY {
            self.clipboard_records.pop_front();
        }
        self.clipboard_records.push_back(record.clone());

        if self.clipboard_live_records_emitted < CLIPBOARD_TRANSFER_LIVE_TRACE_LIMIT {
            Self::emit_clipboard_record(&record);
            self.clipboard_live_records_emitted += 1;
        } else if !self.clipboard_live_trace_limit_notice_emitted {
            eprintln!(
                "typhon_clipboard_transfer_notice clipboard_trace_limit_reached={} remaining_records=shutdown_ring",
                CLIPBOARD_TRANSFER_LIVE_TRACE_LIMIT,
            );
            self.clipboard_live_trace_limit_notice_emitted = true;
        }
    }

    fn emit_record(record: &LifecycleCompatibilityRecord) {
        eprintln!(
            "typhon_lifecycle_compatibility timestamp_ns={} client={:?} object_id={} interface={} surface_id={:?} violation={:?} action={:?}",
            record.timestamp_ns,
            record.client_id,
            record.object_id,
            record.interface,
            record.surface_id,
            record.violation,
            record.action,
        );
    }

    fn emit_clipboard_record(record: &ClipboardTransferTraceRecord) {
        eprintln!(
            "typhon_clipboard_transfer timestamp_ns={} {}",
            record.timestamp_ns, record.details
        );
    }
}

impl Default for LifecycleCompatibilityTrace {
    fn default() -> Self {
        Self {
            records: VecDeque::with_capacity(LIFECYCLE_COMPATIBILITY_TRACE_CAPACITY),
            live_trace_enabled: std::env::var_os(LIFECYCLE_COMPATIBILITY_TRACE_ENV)
                .is_some_and(|value| value == OsStr::new("1")),
            live_records_emitted: 0,
            live_trace_limit_notice_emitted: false,
            clipboard_records: VecDeque::with_capacity(CLIPBOARD_TRANSFER_TRACE_CAPACITY),
            clipboard_live_trace_enabled: std::env::var_os(CLIPBOARD_TRANSFER_TRACE_ENV)
                .is_some_and(|value| value == OsStr::new("1")),
            clipboard_live_records_emitted: 0,
            clipboard_live_trace_limit_notice_emitted: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::net::UnixStream,
        process::{Command, Output},
        sync::Arc,
    };

    use super::*;

    const TRACE_PROBE_ENV: &str = "TYPHON_WAYLAND_COMPAT_TRACE_TEST_PROBE";
    const TRACE_PROBE_TEST: &str =
        "compositor::lifecycle_compatibility::tests::lifecycle_trace_probe";
    const CLIPBOARD_TRACE_PROBE_ENV: &str = "TYPHON_CLIPBOARD_TRACE_TEST_PROBE";
    const CLIPBOARD_TRACE_PROBE_TEST: &str =
        "compositor::lifecycle_compatibility::tests::clipboard_trace_probe";

    fn test_client_id() -> ClientId {
        let (stream, _peer) = UnixStream::pair().expect("test client socket");
        let display =
            wayland_server::Display::<super::super::CompositorState>::new().expect("test display");
        display
            .handle()
            .insert_client(stream, Arc::new(()))
            .expect("test client")
            .id()
    }

    fn run_probe(gate_value: Option<&str>) -> Output {
        let mut command = Command::new(std::env::current_exe().expect("test binary path"));
        command
            .arg("--exact")
            .arg(TRACE_PROBE_TEST)
            .arg("--nocapture")
            .env(TRACE_PROBE_ENV, "1")
            .env_remove("TYPHON_WAYLAND_COMPAT_TRACE");
        if let Some(value) = gate_value {
            command.env("TYPHON_WAYLAND_COMPAT_TRACE", value);
        }
        command.output().expect("run lifecycle trace probe")
    }

    fn run_clipboard_probe(gate_value: Option<&str>) -> Output {
        let mut command = Command::new(std::env::current_exe().expect("test binary path"));
        command
            .arg("--exact")
            .arg(CLIPBOARD_TRACE_PROBE_TEST)
            .arg("--nocapture")
            .env(CLIPBOARD_TRACE_PROBE_ENV, "1")
            .env_remove("TYPHON_CLIPBOARD_TRACE")
            .env_remove("TYPHON_WAYLAND_COMPAT_TRACE");
        if let Some(value) = gate_value {
            command.env("TYPHON_CLIPBOARD_TRACE", value);
        }
        command.output().expect("run clipboard trace probe")
    }

    #[test]
    fn lifecycle_trace_probe() {
        if std::env::var_os(TRACE_PROBE_ENV).as_deref() != Some(std::ffi::OsStr::new("1")) {
            return;
        }
        let client_id = test_client_id();
        let mut trace = LifecycleCompatibilityTrace::default();
        for index in 0..70 {
            trace.record(
                client_id.clone(),
                100 + index,
                "wl_surface",
                Some(200 + index),
                LifecycleCompatibilityViolation::SurfaceDestroyedWithLiveRole,
                LifecycleCompatibilityAction::CanonicalSurfaceTeardown,
            );
        }
        assert_eq!(trace.records.len(), LIFECYCLE_COMPATIBILITY_TRACE_CAPACITY);
        assert_eq!(
            trace.records.front().map(|record| record.object_id),
            Some(106)
        );
        assert_eq!(
            trace.records.back().map(|record| record.object_id),
            Some(169)
        );
        eprintln!("lifecycle_trace_dump_begin");
        trace.dump();
        eprintln!("lifecycle_trace_dump_end");
        eprintln!("lifecycle_trace_probe_complete");
    }

    #[test]
    fn clipboard_trace_probe() {
        if std::env::var_os(CLIPBOARD_TRACE_PROBE_ENV).as_deref() != Some(std::ffi::OsStr::new("1"))
        {
            return;
        }
        let mut trace = LifecycleCompatibilityTrace::default();
        for index in 0..300 {
            trace.record_clipboard_trace(|| format!("probe_id={index}"));
        }
        eprintln!("clipboard_trace_dump_begin");
        trace.dump();
        eprintln!("clipboard_trace_dump_end");
        eprintln!("clipboard_trace_probe_complete");
    }

    #[test]
    fn live_trace_requires_exact_opt_in_and_is_bounded() {
        for (gate_value, expected_records) in [
            (None, 0),
            (Some("0"), 0),
            (Some("true"), 0),
            (Some("01"), 0),
            (Some(" 1"), 0),
            (Some("1"), LIFECYCLE_COMPATIBILITY_TRACE_CAPACITY),
        ] {
            let output = run_probe(gate_value);
            assert!(output.status.success(), "probe failed: {output:?}");
            let stderr = String::from_utf8(output.stderr).expect("UTF-8 trace output");
            let (live, rest) = stderr
                .split_once("lifecycle_trace_dump_begin\n")
                .expect("live trace phase marker");
            let (dump, _) = rest
                .split_once("lifecycle_trace_dump_end\n")
                .expect("shutdown dump phase marker");
            let live_record_count = live
                .lines()
                .filter(|line| line.starts_with("typhon_lifecycle_compatibility "))
                .count();
            let dump_record_count = dump
                .lines()
                .filter(|line| line.starts_with("typhon_lifecycle_compatibility "))
                .count();
            assert_eq!(
                live_record_count, expected_records,
                "gate value {gate_value:?}"
            );
            assert_eq!(dump_record_count, LIFECYCLE_COMPATIBILITY_TRACE_CAPACITY);
            assert_eq!(
                stderr
                    .lines()
                    .filter(|line| line.contains("live_trace_limit_reached=64"))
                    .count(),
                usize::from(gate_value == Some("1")),
                "gate value {gate_value:?}"
            );
            if gate_value == Some("1") {
                assert!(live.contains("timestamp_ns="));
                assert!(live.contains("client="));
                assert!(live.contains("object_id=100"));
                assert!(live.contains("interface=wl_surface"));
                assert!(live.contains("surface_id=Some(200)"));
                assert!(live.contains("violation=SurfaceDestroyedWithLiveRole"));
                assert!(live.contains("action=CanonicalSurfaceTeardown"));
            }
        }
    }

    #[test]
    fn clipboard_trace_requires_exact_opt_in_and_is_bounded() {
        for (gate_value, expected_records) in [
            (None, 0),
            (Some("0"), 0),
            (Some("true"), 0),
            (Some("01"), 0),
            (Some(" 1"), 0),
            (Some("1"), 256),
        ] {
            let output = run_clipboard_probe(gate_value);
            assert!(output.status.success(), "probe failed: {output:?}");
            let stderr = String::from_utf8(output.stderr).expect("UTF-8 trace output");
            let (live, rest) = stderr
                .split_once("clipboard_trace_dump_begin\n")
                .expect("live trace phase marker");
            let (dump, _) = rest
                .split_once("clipboard_trace_dump_end\n")
                .expect("shutdown dump phase marker");
            let live_record_count = live
                .lines()
                .filter(|line| line.starts_with("typhon_clipboard_transfer "))
                .count();
            let dump_record_count = dump
                .lines()
                .filter(|line| line.starts_with("typhon_clipboard_transfer "))
                .count();
            assert_eq!(live_record_count, expected_records, "gate {gate_value:?}");
            assert_eq!(dump_record_count, expected_records, "gate {gate_value:?}");
            assert_eq!(
                stderr
                    .lines()
                    .filter(|line| line.contains("clipboard_trace_limit_reached=256"))
                    .count(),
                usize::from(gate_value == Some("1")),
                "gate {gate_value:?}"
            );
            if gate_value == Some("1") {
                assert!(live.contains("probe_id=0"));
                assert!(live.contains("probe_id=255"));
                assert!(dump.contains("probe_id=44"));
                assert!(dump.contains("probe_id=299"));
            }
        }
    }
}
