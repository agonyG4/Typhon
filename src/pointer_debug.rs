use std::sync::{OnceLock, atomic::AtomicU64};

static ENABLED: OnceLock<bool> = OnceLock::new();
#[allow(dead_code)]
static TIMING_TRACE_ENABLED: OnceLock<bool> = OnceLock::new();
static CURSOR_PRESENTATION_TRACE_ENABLED: OnceLock<bool> = OnceLock::new();
static CURSOR_PRESENTATION_TRACE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn enabled() -> bool {
    *ENABLED.get_or_init(|| std::env::var_os("TYPHON_POINTER_DEBUG").is_some())
}

#[allow(dead_code)]
pub(crate) fn timing_trace_enabled() -> bool {
    *TIMING_TRACE_ENABLED.get_or_init(|| std::env::var_os("TYPHON_POINTER_TIMING_TRACE").is_some())
}

pub(crate) fn log(message: impl AsRef<str>) {
    if enabled() {
        eprintln!("typhon pointer: {}", message.as_ref());
    }
}

pub(crate) fn log_lazy(message: impl FnOnce() -> String) {
    if enabled() {
        log(message());
    }
}

pub(crate) fn cursor_presentation_trace_enabled() -> bool {
    *CURSOR_PRESENTATION_TRACE_ENABLED.get_or_init(|| {
        std::env::var_os("TYPHON_CURSOR_PRESENTATION_TRACE").is_some_and(|value| value == "1")
    })
}

pub(crate) fn cursor_presentation_log_lazy(message: impl FnOnce() -> String) {
    if cursor_presentation_trace_enabled() {
        let sequence = CURSOR_PRESENTATION_TRACE_SEQUENCE
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        eprintln!(
            "typhon cursor presentation: sequence={} {}",
            sequence,
            message()
        );
    }
}

#[cfg(test)]
fn log_lazy_if(enabled: bool, message: impl FnOnce() -> String) {
    if enabled {
        eprintln!("typhon pointer: {}", message());
    }
}

#[cfg(test)]
fn cursor_presentation_log_lazy_if(enabled: bool, message: impl FnOnce() -> String) {
    if enabled {
        eprintln!("typhon cursor presentation: {}", message());
    }
}

#[cfg(test)]
mod tests {
    use super::{cursor_presentation_log_lazy_if, log_lazy_if};
    use std::cell::Cell;

    #[test]
    fn disabled_lazy_logging_does_not_evaluate_formatter() {
        let evaluated = Cell::new(false);
        log_lazy_if(false, || {
            evaluated.set(true);
            String::from("unused")
        });
        assert!(!evaluated.get());
    }

    #[test]
    fn disabled_cursor_presentation_logging_does_not_evaluate_formatter() {
        let evaluated = Cell::new(false);
        cursor_presentation_log_lazy_if(false, || {
            evaluated.set(true);
            String::from("unused")
        });
        assert!(!evaluated.get());
    }
}
