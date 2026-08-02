use std::{
    collections::VecDeque,
    fmt::{Display, Write as _},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Mutex, OnceLock},
};

static TRACE_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const MAX_TRACE_RECORDS_PER_PROCESS: u64 = 20_000;
const MAX_LIFECYCLE_RECORDS: usize = 4_096;
static TRACE_RECORDS_SUPPRESSED: AtomicU64 = AtomicU64::new(0);
static TRACE_ENABLED: OnceLock<bool> = OnceLock::new();
static LIFECYCLE_RECORDS: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();

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
    if !enabled() {
        return;
    }
    let trace_seq = TRACE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let monotonic_ns = crate::native::event_loop::monotonic_now_ns().unwrap_or_default();
    let line = render_line(trace_seq, monotonic_ns, event, &fields());
    if is_lifecycle_event(event) {
        retain_lifecycle_line(line.clone());
        eprintln!("{line}");
        return;
    }
    if trace_seq > MAX_TRACE_RECORDS_PER_PROCESS {
        TRACE_RECORDS_SUPPRESSED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    eprintln!("{line}");
}

pub fn suppressed_records() -> u64 {
    TRACE_RECORDS_SUPPRESSED.load(Ordering::Relaxed)
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

fn is_lifecycle_event(event: &str) -> bool {
    matches!(
        event,
        "CreateNotify"
            | "MapRequest"
            | "MapNotify"
            | "UnmapNotify"
            | "DestroyNotify"
            | "ConfigureNotify"
            | "WindowReady"
            | "WindowDestroyed"
            | "WindowWithdrawn"
            | "OverrideRedirectStackSnapshot"
    ) || event.contains("association")
        || event.contains("lifecycle")
        || event.contains("popup")
        || event.contains("cleanup")
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
}
