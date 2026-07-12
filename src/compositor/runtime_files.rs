use std::{
    env,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) fn unique_runtime_file_path(prefix: &str) -> PathBuf {
    let base = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    base.join(format!("{prefix}-{}-{nonce}", std::process::id()))
}

pub(super) fn compositor_debug_surface_logging_enabled() -> bool {
    env::var_os("OBLIVION_ONE_DEBUG_SURFACES").is_some()
}

pub fn resize_debug_logging_enabled() -> bool {
    env::var_os("TYPHON_RESIZE_DEBUG").is_some_and(|value| value == "1")
}

pub fn resize_debug_log(message: impl FnOnce() -> String) {
    if resize_debug_logging_enabled() {
        eprintln!("typhon resize: {}", message());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, sync::Mutex};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn resize_debug_log_does_not_format_when_disabled() {
        let _guard = ENV_LOCK.lock().unwrap();
        // SAFETY: this test serializes access to the process environment.
        unsafe { env::remove_var("TYPHON_RESIZE_DEBUG") };

        let formatted = Cell::new(false);
        resize_debug_log(|| {
            formatted.set(true);
            "should not be formatted".to_string()
        });

        assert!(!formatted.get());
    }
}
