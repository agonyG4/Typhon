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
