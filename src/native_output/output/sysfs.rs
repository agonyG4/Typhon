use super::*;

pub(crate) fn native_test_fail_native_egl_gbm_enabled() -> bool {
    if !cfg!(any(test, debug_assertions)) {
        return false;
    }
    std::env::var_os("OBLIVION_ONE_TEST_FAIL_NATIVE_EGL_GBM").is_some_and(|value| value == "1")
}

#[cfg(test)]
pub(crate) fn connected_connector_for_card(
    kms_device: Option<&Path>,
    sysfs_drm_root: &Path,
) -> Option<NativeConnector> {
    connected_connectors_for_card(kms_device, sysfs_drm_root)
        .into_iter()
        .next()
}

pub(crate) fn selected_connector_for_kms_target(
    kms_device: Option<&Path>,
    sysfs_drm_root: &Path,
    target_connector_id: u32,
    target_connector_name: &str,
) -> Option<NativeConnector> {
    let connectors = connected_connectors_for_card(kms_device, sysfs_drm_root);
    connectors
        .iter()
        .find(|connector| connector.connector_id == Some(target_connector_id))
        .or_else(|| {
            connectors.iter().find(|connector| {
                connector.connector_id.is_none() && connector.name == target_connector_name
            })
        })
        .cloned()
}

fn connected_connectors_for_card(
    kms_device: Option<&Path>,
    sysfs_drm_root: &Path,
) -> Vec<NativeConnector> {
    let card_name = kms_device
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    let Some(card_name) = card_name else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(sysfs_drm_root) else {
        return Vec::new();
    };
    let mut connectors = entries
        .filter_map(Result::ok)
        .filter_map(|entry| connected_connector_from_entry(card_name, entry.path()))
        .collect::<Vec<_>>();
    connectors.sort_by(|left, right| left.name.cmp(&right.name));
    connectors
}

pub(crate) fn connected_connector_from_entry(
    card_name: &str,
    path: PathBuf,
) -> Option<NativeConnector> {
    let entry_name = path.file_name()?.to_str()?;
    let name = entry_name
        .strip_prefix(&format!("{card_name}-"))?
        .to_string();

    let status = read_trimmed(path.join("status"))?;
    if status != "connected" {
        return None;
    }

    Some(NativeConnector {
        name,
        connector_id: read_trimmed(path.join("connector_id")).and_then(|value| value.parse().ok()),
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
