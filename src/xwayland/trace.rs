use std::{
    collections::VecDeque,
    fmt::{Display, Write as _},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Mutex, OnceLock},
};

static TRACE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const MAX_TRACE_RECORDS_PER_PROCESS: u64 = 20_000;
const MAX_LIFECYCLE_RECORDS: usize = 4_096;
static TRACE_RECORDS_EMITTED: AtomicU64 = AtomicU64::new(0);
static LIFECYCLE_RECORDS_EMITTED: AtomicU64 = AtomicU64::new(0);
static TRACE_RECORDS_SUPPRESSED: AtomicU64 = AtomicU64::new(0);
static TRACE_ENABLED: OnceLock<bool> = OnceLock::new();
static LIFECYCLE_RECORDS: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();

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
    let line = render_line(trace_seq, monotonic_ns, event, &fields());
    if lifecycle {
        retain_lifecycle_line(line.clone());
    }
    if !trace_enabled {
        return;
    }
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
    let records = LIFECYCLE_RECORDS.get_or_init(|| Mutex::new(VecDeque::new()));
    let mut records = records
        .lock()
        .expect("XWayland trace lifecycle mutex poisoned");
    std::mem::take(&mut *records).into_iter().collect()
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

fn retain_lifecycle_line(line: String) {
    let records = LIFECYCLE_RECORDS.get_or_init(|| Mutex::new(VecDeque::new()));
    let mut records = records
        .lock()
        .expect("XWayland trace lifecycle mutex poisoned");
    if records.len() == MAX_LIFECYCLE_RECORDS {
        records.pop_front();
    }
    records.push_back(line);
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
        LIFECYCLE_RECORDS
            .get_or_init(|| Mutex::new(VecDeque::new()))
            .lock()
            .expect("trace retention mutex")
            .clear();
    }

    fn retain_lifecycle_for_test(line: String) {
        retain_lifecycle_line(line);
    }

    fn recent_lifecycle_records_for_test() -> Vec<String> {
        LIFECYCLE_RECORDS
            .get_or_init(|| Mutex::new(VecDeque::new()))
            .lock()
            .expect("trace retention mutex")
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
        ] {
            emit_category(TraceCategory::Lifecycle, event, TraceFields::new);
        }
        for _ in 0..(MAX_LIFECYCLE_RECORDS + 32) {
            emit_category(TraceCategory::Geometry, "ConfigureNotify", TraceFields::new);
        }

        let retained = take_recent_lifecycle_trace();
        assert_eq!(retained.len(), 5);
        for event in [
            "window_destroyed",
            "window_withdrawn",
            "destroy_window_processed",
            "xwm_map_notify",
            "xwayland_window_admission_failed",
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
