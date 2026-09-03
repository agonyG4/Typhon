use std::sync::OnceLock;

static ENABLED: OnceLock<bool> = OnceLock::new();
#[allow(dead_code)]
static TIMING_TRACE_ENABLED: OnceLock<bool> = OnceLock::new();

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

#[cfg(test)]
fn log_lazy_if(enabled: bool, message: impl FnOnce() -> String) {
    if enabled {
        eprintln!("typhon pointer: {}", message());
    }
}

#[cfg(test)]
mod tests {
    use super::log_lazy_if;
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
}
