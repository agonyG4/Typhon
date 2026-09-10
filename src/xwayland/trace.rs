use std::{
    collections::VecDeque,
    fmt::{Display, Write as _},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Mutex, OnceLock},
};

static TRACE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const MAX_TRACE_RECORDS_PER_PROCESS: u64 = 20_000;
const MAX_LIFECYCLE_RECORDS: usize = 4_096;
const MAX_LIFECYCLE_BYTES: usize = 512 * 1024;
const MAX_LIFECYCLE_LINE_BYTES: usize = 1_024;
const MAX_LIFECYCLE_FIELD_BYTES: usize = 128;
static TRACE_RECORDS_EMITTED: AtomicU64 = AtomicU64::new(0);
static LIFECYCLE_RECORDS_EMITTED: AtomicU64 = AtomicU64::new(0);
static TRACE_RECORDS_SUPPRESSED: AtomicU64 = AtomicU64::new(0);
static TRACE_ENABLED: OnceLock<bool> = OnceLock::new();
static LIFECYCLE_RECORDS: OnceLock<Mutex<LifecycleRecords>> = OnceLock::new();

#[derive(Debug, Default)]
struct LifecycleRecords {
    lines: VecDeque<String>,
    bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceCategory {
    Lifecycle,
    Stacking,
    Geometry,
    Diagnostic,
}

#[derive(Debug, Default)]
pub struct TraceFields {
    entries: Vec<(&'static str, String)>,
}

impl TraceFields {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn field(mut self, key: &'static str, value: impl Display) -> Self {
        self.entries.push((key, value.to_string()));
        self
    }

    pub fn optional<T: Display>(self, key: &'static str, value: Option<T>) -> Self {
        match value {
            Some(value) => self.field(key, value),
            None => self,
        }
    }
}

pub fn enabled() -> bool {
    *TRACE_ENABLED
        .get_or_init(|| std::env::var_os("TYPHON_XWAYLAND_TRACE").is_some_and(|value| value == "1"))
}

pub fn emit<F>(event: &'static str, fields: F)
where
    F: FnOnce() -> TraceFields,
{
    emit_category(TraceCategory::Diagnostic, event, fields);
}

pub fn emit_category<F>(category: TraceCategory, event: &'static str, fields: F)
where
    F: FnOnce() -> TraceFields,
{
    let trace_enabled = enabled();
    if !trace_enabled && category != TraceCategory::Lifecycle {
        return;
    }
    let trace_seq = TRACE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let lifecycle = category == TraceCategory::Lifecycle;
    let output_index = if lifecycle {
        LIFECYCLE_RECORDS_EMITTED.fetch_add(1, Ordering::Relaxed)
    } else {
        TRACE_RECORDS_EMITTED.fetch_add(1, Ordering::Relaxed)
    };
    let monotonic_ns = crate::native::event_loop::monotonic_now_ns().unwrap_or_default();
    let fields = fields();
    if lifecycle {
        retain_lifecycle_line(&render_lifecycle_line(
            trace_seq,
            monotonic_ns,
            event,
            &fields,
        ));
    }
    if !trace_enabled {
        return;
    }
    let line = render_line(trace_seq, monotonic_ns, event, &fields);
    if !trace_output_allowed(lifecycle, output_index) {
        TRACE_RECORDS_SUPPRESSED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    eprintln!("{line}");
}

pub fn suppressed_records() -> u64 {
    TRACE_RECORDS_SUPPRESSED.load(Ordering::Relaxed)
}

pub fn take_recent_lifecycle_trace() -> Vec<String> {
    let records = LIFECYCLE_RECORDS.get_or_init(|| Mutex::new(LifecycleRecords::default()));
    let mut records = records
        .lock()
        .expect("XWayland trace lifecycle mutex poisoned");
    let lines = std::mem::take(&mut records.lines);
    records.bytes = 0;
    lines.into_iter().collect()
}

#[cfg(test)]
fn lifecycle_retained_bytes_for_test() -> usize {
    LIFECYCLE_RECORDS
        .get_or_init(|| Mutex::new(LifecycleRecords::default()))
        .lock()
        .expect("XWayland trace lifecycle mutex poisoned")
        .bytes
}

pub fn render_line(trace_seq: u64, monotonic_ns: u64, event: &str, fields: &TraceFields) -> String {
    let mut line = format!(
        "oblivion-one xwayland: trace_seq={trace_seq} monotonic_ns={monotonic_ns} x_event_type={event}"
    );
    for (key, value) in &fields.entries {
        let _ = write!(line, " {key}={}", encode_value(value));
    }
    line
}

fn encode_value(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
    {
        return value.to_owned();
    }
    let mut encoded = String::with_capacity(value.len().saturating_add(2));
    encoded.push('"');
    for character in value.chars() {
        match character {
            '\\' => encoded.push_str("\\\\"),
            '"' => encoded.push_str("\\\""),
            '\n' => encoded.push_str("\\n"),
            '\r' => encoded.push_str("\\r"),
            '\t' => encoded.push_str("\\t"),
            character => encoded.push(character),
        }
    }
    encoded.push('"');
    encoded
}

fn trace_output_allowed(lifecycle: bool, output_index: u64) -> bool {
    let limit = if lifecycle {
        MAX_LIFECYCLE_RECORDS as u64
    } else {
        MAX_TRACE_RECORDS_PER_PROCESS
    };
    output_index < limit
}

fn render_lifecycle_line(
    trace_seq: u64,
    monotonic_ns: u64,
    event: &str,
    fields: &TraceFields,
) -> String {
    let mut line = format!(
        "oblivion-one xwayland: trace_seq={trace_seq} monotonic_ns={monotonic_ns} x_event_type={event}"
    );
    for (key, value) in &fields.entries {
        let value = truncate_text(value, MAX_LIFECYCLE_FIELD_BYTES);
        let _ = write!(line, " {key}={}", encode_value(&value));
    }
    line
}

fn retain_lifecycle_line(line: &str) {
    let line = truncate_text(line, MAX_LIFECYCLE_LINE_BYTES);
    let line_bytes = line.len();
    if line_bytes > MAX_LIFECYCLE_BYTES {
        return;
    }
    let records = LIFECYCLE_RECORDS.get_or_init(|| Mutex::new(LifecycleRecords::default()));
    let mut records = records
        .lock()
        .expect("XWayland trace lifecycle mutex poisoned");
    while (records.lines.len() >= MAX_LIFECYCLE_RECORDS
        || records.bytes.saturating_add(line_bytes) > MAX_LIFECYCLE_BYTES)
        && !records.lines.is_empty()
    {
        if let Some(old) = records.lines.pop_front() {
            records.bytes = records.bytes.saturating_sub(old.len());
        }
    }
    records.bytes = records.bytes.saturating_add(line_bytes);
    records.lines.push_back(line);
}

fn truncate_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    const SUFFIX: &str = "…";
    let end_limit = max_bytes.saturating_sub(SUFFIX.len());
    let end = value
        .char_indices()
        .take_while(|(index, _)| *index <= end_limit)
        .map(|(index, _)| index)
        .last()
        .unwrap_or(0);
    let mut truncated = String::with_capacity(end.saturating_add(SUFFIX.len()));
    truncated.push_str(&value[..end]);
    truncated.push_str(SUFFIX);
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("trace test mutex poisoned")
    }

    fn reset_retention_for_test() {
        let mut records = LIFECYCLE_RECORDS
            .get_or_init(|| Mutex::new(LifecycleRecords::default()))
            .lock()
            .expect("trace retention mutex");
        records.lines.clear();
        records.bytes = 0;
    }

    fn retain_lifecycle_for_test(line: String) {
        retain_lifecycle_line(&line);
    }

    fn recent_lifecycle_records_for_test() -> Vec<String> {
        LIFECYCLE_RECORDS
            .get_or_init(|| Mutex::new(LifecycleRecords::default()))
            .lock()
            .expect("trace retention mutex")
            .lines
            .iter()
            .cloned()
            .collect()
    }

    #[test]
    fn trace_line_has_stable_order_and_escapes_values() {
        let fields = TraceFields::new()
            .field("source", "x11")
            .field("xid", 42)
            .field("detail", "menu popup");

        assert_eq!(
            render_line(7, 11, "MapNotify", &fields),
            r#"oblivion-one xwayland: trace_seq=7 monotonic_ns=11 x_event_type=MapNotify source=x11 xid=42 detail="menu popup""#
        );
    }

    #[test]
    fn lifecycle_retention_keeps_newest_records_after_high_frequency_noise() {
        let _guard = test_lock();
        reset_retention_for_test();
        for index in 0..(MAX_LIFECYCLE_RECORDS + 32) {
            retain_lifecycle_for_test(format!("popup-{index}"));
        }
        let retained = recent_lifecycle_records_for_test();
        assert_eq!(retained.len(), MAX_LIFECYCLE_RECORDS);
        assert_eq!(retained.first().map(String::as_str), Some("popup-32"));
        assert_eq!(
            retained.last().map(String::as_str),
            Some(format!("popup-{}", MAX_LIFECYCLE_RECORDS + 31).as_str())
        );
    }

    #[test]
    fn trace_output_budgets_are_independent_and_lifecycle_output_is_bounded() {
        assert!(trace_output_allowed(
            false,
            MAX_TRACE_RECORDS_PER_PROCESS - 1
        ));
        assert!(!trace_output_allowed(false, MAX_TRACE_RECORDS_PER_PROCESS));
        assert!(trace_output_allowed(true, MAX_LIFECYCLE_RECORDS as u64 - 1));
        assert!(!trace_output_allowed(true, MAX_LIFECYCLE_RECORDS as u64));
    }

    #[test]
    fn lifecycle_retention_is_bounded_for_pathological_text_properties() {
        let _guard = test_lock();
        reset_retention_for_test();
        let pathological = "x".repeat(64 * 1024);
        let fields = TraceFields::new()
            .field("app_id", pathological.clone())
            .field("title", pathological.clone());
        let full_line = render_line(1, 2, "window_state", &fields);
        let retained_line = render_lifecycle_line(1, 2, "window_state", &fields);
        assert!(full_line.len() > MAX_LIFECYCLE_LINE_BYTES);
        assert!(retained_line.len() <= MAX_LIFECYCLE_LINE_BYTES);
        assert!(retained_line.contains("app_id="));
        assert!(retained_line.contains("title="));
        let record_count = MAX_LIFECYCLE_BYTES / MAX_LIFECYCLE_LINE_BYTES + 64;
        for index in 0..record_count {
            retain_lifecycle_for_test(format!(
                "window-{index} app_id={pathological} title={pathological}"
            ));
        }

        let retained = recent_lifecycle_records_for_test();
        assert!(
            retained
                .iter()
                .all(|line| line.len() <= MAX_LIFECYCLE_LINE_BYTES)
        );
        assert!(lifecycle_retained_bytes_for_test() <= MAX_LIFECYCLE_BYTES);
        assert!(retained.len() <= MAX_LIFECYCLE_RECORDS);
        assert!(!retained.iter().any(|line| line.contains("window-0")));
        assert!(
            retained
                .last()
                .is_some_and(|line| line.contains(&format!("window-{}", record_count - 1)))
        );

        let dumped = take_recent_lifecycle_trace();
        assert_eq!(dumped, retained);
        assert!(take_recent_lifecycle_trace().is_empty());
        assert_eq!(lifecycle_retained_bytes_for_test(), 0);
    }

    #[test]
    fn lifecycle_trace_dump_returns_newest_bounded_records_once() {
        let _guard = test_lock();
        reset_retention_for_test();
        for index in 0..(MAX_LIFECYCLE_RECORDS + 32) {
            retain_lifecycle_for_test(format!("popup-{index}"));
        }

        let ordinary_before = TRACE_RECORDS_EMITTED.load(Ordering::Relaxed);
        let lifecycle_before = LIFECYCLE_RECORDS_EMITTED.load(Ordering::Relaxed);
        let dumped = take_recent_lifecycle_trace();
        assert_eq!(dumped.len(), MAX_LIFECYCLE_RECORDS);
        assert_eq!(dumped.first().map(String::as_str), Some("popup-32"));
        assert_eq!(
            dumped.last().map(String::as_str),
            Some(format!("popup-{}", MAX_LIFECYCLE_RECORDS + 31).as_str())
        );
        assert!(take_recent_lifecycle_trace().is_empty());
        assert_eq!(
            TRACE_RECORDS_EMITTED.load(Ordering::Relaxed),
            ordinary_before
        );
        assert_eq!(
            LIFECYCLE_RECORDS_EMITTED.load(Ordering::Relaxed),
            lifecycle_before
        );
    }

    #[test]
    fn production_lifecycle_names_survive_geometry_noise() {
        let _guard = test_lock();
        reset_retention_for_test();

        for event in [
            "window_destroyed",
            "window_withdrawn",
            "destroy_window_processed",
            "xwm_map_notify",
            "xwayland_window_admission_failed",
            "window_ready_emitted",
            "xwayland_surface_attached",
        ] {
            emit_category(TraceCategory::Lifecycle, event, TraceFields::new);
        }
        for _ in 0..(MAX_LIFECYCLE_RECORDS + 32) {
            emit_category(TraceCategory::Geometry, "ConfigureNotify", TraceFields::new);
        }

        let retained = take_recent_lifecycle_trace();
        assert_eq!(retained.len(), 7);
        for event in [
            "window_destroyed",
            "window_withdrawn",
            "destroy_window_processed",
            "xwm_map_notify",
            "xwayland_window_admission_failed",
            "window_ready_emitted",
            "xwayland_surface_attached",
        ] {
            assert!(
                retained
                    .iter()
                    .any(|line| line.contains(&format!("x_event_type={event}"))),
                "missing lifecycle event {event}"
            );
        }
    }
}
