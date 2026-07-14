use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicU64, Ordering},
    mpsc::{SyncSender, sync_channel},
};
use std::thread;

pub(crate) fn client_pacing_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("TYPHON_FRAME_PACING_TRACE")
            .ok()
            .is_some_and(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on" | "debug" | "trace"
                )
            })
    })
}

pub(crate) fn client_pacing_now_ns() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return 0;
    }
    u64::try_from(time.tv_sec)
        .unwrap_or(0)
        .saturating_mul(1_000_000_000)
        .saturating_add(u64::try_from(time.tv_nsec).unwrap_or(0))
}

pub(crate) fn client_pacing_log(event: &str, fields: &[(&str, String)]) {
    if client_pacing_enabled() {
        client_trace_sink().send(client_pacing_line(event, client_pacing_now_ns(), fields));
    }
}

pub(crate) fn commit_debug_log(line: String) {
    if commit_debug_enabled() {
        commit_trace_sink().send(line);
    }
}

pub(crate) fn client_pacing_trace_dropped_entries() -> u64 {
    if client_pacing_enabled() {
        client_trace_sink().dropped_entries()
    } else {
        0
    }
}

pub(crate) fn commit_debug_trace_dropped_entries() -> u64 {
    if commit_debug_enabled() {
        commit_trace_sink().dropped_entries()
    } else {
        0
    }
}

fn commit_debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| env_enabled("TYPHON_COMMIT_DEBUG"))
}

fn env_enabled(name: &str) -> bool {
    std::env::var(name).ok().is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on" | "debug" | "trace"
        )
    })
}

const TRACE_QUEUE_CAPACITY: usize = 2_048;

#[derive(Debug)]
struct AsyncTraceSink {
    sender: SyncSender<String>,
    dropped: Arc<AtomicU64>,
}

impl AsyncTraceSink {
    fn new(thread_name: &'static str) -> Self {
        let (sender, receiver) = sync_channel(TRACE_QUEUE_CAPACITY);
        let _ = thread::Builder::new()
            .name(thread_name.to_string())
            .spawn(move || {
                while let Ok(line) = receiver.recv() {
                    println!("{line}");
                }
            });
        Self {
            sender,
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }

    fn send(&self, line: String) {
        if self.sender.try_send(line).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn dropped_entries(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

fn client_trace_sink() -> &'static AsyncTraceSink {
    static SINK: OnceLock<AsyncTraceSink> = OnceLock::new();
    SINK.get_or_init(|| AsyncTraceSink::new("typhon-client-trace"))
}

fn commit_trace_sink() -> &'static AsyncTraceSink {
    static SINK: OnceLock<AsyncTraceSink> = OnceLock::new();
    SINK.get_or_init(|| AsyncTraceSink::new("typhon-commit-trace"))
}

pub(crate) fn client_pacing_line(event: &str, event_ns: u64, fields: &[(&str, String)]) -> String {
    let mut line = format!("typhon pacing: event={event} event_ns={event_ns}");
    for (key, value) in fields {
        line.push(' ');
        line.push_str(key);
        line.push('=');
        line.push_str(&pacing_value(value));
    }
    line
}

fn pacing_value(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./:@+=".contains(c))
    {
        return value.to_string();
    }
    format!("\"{}\"", value.replace('\\', "\\\\").replace('\"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_pacing_line_has_stable_prefix_event_and_timestamp() {
        assert_eq!(
            client_pacing_line(
                "surface_commit",
                42,
                &[("surface", "7".to_string()), ("damage", "true".to_string())],
            ),
            "typhon pacing: event=surface_commit event_ns=42 surface=7 damage=true"
        );
    }
}
