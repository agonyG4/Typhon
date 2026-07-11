use super::*;

pub(crate) fn native_test_fail_native_egl_gbm_enabled() -> bool {
    if !cfg!(any(test, debug_assertions)) {
        return false;
    }
    std::env::var_os("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM").is_some_and(|value| value == "1")
}

pub(crate) fn connected_connector_for_card(
    kms_device: Option<&Path>,
    sysfs_drm_root: &Path,
) -> Option<NativeConnector> {
    let card_name = kms_device
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())?;
    let mut connectors = fs::read_dir(sysfs_drm_root)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| connected_connector_from_entry(card_name, entry.path()))
        .collect::<Vec<_>>();
    connectors.sort_by(|left, right| left.name.cmp(&right.name));
    connectors.into_iter().next()
}

pub(crate) fn connected_connector_from_entry(
    card_name: &str,
    path: PathBuf,
) -> Option<NativeConnector> {
    let name = path.file_name()?.to_str()?.to_string();
    if !name.starts_with(&format!("{card_name}-")) {
        return None;
    }

    let status = read_trimmed(path.join("status"))?;
    if status != "connected" {
        return None;
    }

    let modes = fs::read_to_string(path.join("modes"))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    Some(NativeConnector {
        name,
        enabled: read_trimmed(path.join("enabled")),
        modes,
        vrr_capable: read_bool_property(path.join("vrr_capable")),
    })
}

pub(crate) fn read_bool_property(path: impl AsRef<Path>) -> Option<bool> {
    match read_trimmed(path)?.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => Some(true),
        "0" | "false" | "no" | "off" | "disabled" => Some(false),
        _ => None,
    }
}

pub(crate) fn read_trimmed(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|contents| contents.trim().to_string())
        .filter(|contents| !contents.is_empty())
}

pub(crate) fn display_optional_path(path: Option<&Path>) -> String {
    path.map(|path| path.display().to_string())
        .unwrap_or_else(|| "missing".to_string())
}
