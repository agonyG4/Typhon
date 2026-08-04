use std::{
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
};

use super::client::AstreactlError;

const MAX_DISCOVERY_ENTRIES: usize = 256;
const MAX_INSTANCE_NAME_BYTES: usize = 128;

pub fn discover_socket(
    instance: Option<&str>,
    explicit: Option<&Path>,
) -> Result<PathBuf, AstreactlError> {
    if explicit.is_some() {
        return discover_socket_from(instance, explicit, Path::new("/"), None);
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
        AstreactlError::EndpointNotFound("XDG_RUNTIME_DIR is not set".to_string())
    })?;
    let display = std::env::var("WAYLAND_DISPLAY").ok();
    discover_socket_from(instance, explicit, Path::new(&runtime), display.as_deref())
}

fn discover_socket_from(
    instance: Option<&str>,
    explicit: Option<&Path>,
    runtime: &Path,
    display: Option<&str>,
) -> Result<PathBuf, AstreactlError> {
    if let Some(path) = explicit {
        if !path.is_absolute() {
            return Err(AstreactlError::Usage(
                "--socket must be an absolute path".to_string(),
            ));
        }
        validate_parent_components(path)?;
        validate_socket(path)?;
        return Ok(path.to_path_buf());
    }
    if let Some(instance) = instance {
        validate_instance(instance)?;
    }
    validate_runtime_dir(runtime)?;
    let root = runtime.join("astrea").join("typhon");
    validate_owned_directory(&runtime.join("astrea"), 0o700)?;
    validate_owned_directory(&root, 0o700)?;
    if let Some(instance) = instance {
        let directory = root.join(instance);
        validate_owned_directory(&directory, 0o700)?;
        let socket = directory.join("control.sock");
        validate_socket(&socket)?;
        return Ok(socket);
    }
    if let Some(display) = display
        && valid_instance(display)
    {
        let directory = root.join(display);
        if validate_owned_directory(&directory, 0o700).is_ok() {
            let socket = directory.join("control.sock");
            if validate_socket(&socket).is_ok() {
                return Ok(socket);
            }
        }
    }
    let entries = fs::read_dir(&root).map_err(|_| {
        AstreactlError::EndpointNotFound("no Typhon instances are running".to_string())
    })?;
    let entries = entries
        .take(MAX_DISCOVERY_ENTRIES + 1)
        .collect::<Result<Vec<_>, _>>()
        .map_err(AstreactlError::Transport)?;
    if entries.len() > MAX_DISCOVERY_ENTRIES {
        return Err(AstreactlError::EndpointNotFound(format!(
            "Typhon instance discovery entry limit exceeded (maximum {MAX_DISCOVERY_ENTRIES})"
        )));
    }
    let mut candidates = Vec::new();
    for entry in entries {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !valid_instance(name) {
            continue;
        }
        let directory = entry.path();
        if validate_owned_directory(&directory, 0o700).is_err() {
            continue;
        }
        let socket = directory.join("control.sock");
        if validate_socket(&socket).is_ok() {
            candidates.push((name.to_string(), socket));
        }
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    match candidates.len() {
        0 => Err(AstreactlError::EndpointNotFound(
            "no Typhon instances are running".to_string(),
        )),
        1 => Ok(candidates.remove(0).1),
        _ => Err(AstreactlError::EndpointNotFound(format!(
            "multiple Typhon instances are running: {}; pass --instance",
            candidates
                .iter()
                .take(32)
                .map(|candidate| candidate.0.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn validate_runtime_dir(path: &Path) -> Result<(), AstreactlError> {
    if !path.is_absolute() {
        return Err(AstreactlError::EndpointNotFound(
            "XDG_RUNTIME_DIR is not absolute".to_string(),
        ));
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        AstreactlError::EndpointNotFound("XDG_RUNTIME_DIR is unavailable".to_string())
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o022 != 0
    {
        return Err(AstreactlError::EndpointNotFound(
            "XDG_RUNTIME_DIR is not a secure directory".to_string(),
        ));
    }
    Ok(())
}

fn validate_owned_directory(path: &Path, expected_mode: u32) -> Result<(), AstreactlError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| AstreactlError::EndpointNotFound("control endpoint not found".to_string()))?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o777 != expected_mode
    {
        return Err(AstreactlError::EndpointNotFound(
            "control endpoint is in an insecure directory".to_string(),
        ));
    }
    Ok(())
}

fn validate_parent_components(path: &Path) -> Result<(), AstreactlError> {
    let mut current = path.parent();
    while let Some(parent) = current {
        let metadata = fs::symlink_metadata(parent).map_err(|_| {
            AstreactlError::EndpointNotFound("control endpoint parent not found".to_string())
        })?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
            return Err(AstreactlError::EndpointNotFound(
                "control endpoint has an unsafe parent".to_string(),
            ));
        }
        if parent == Path::new("/") {
            break;
        }
        current = parent.parent();
    }
    Ok(())
}

fn validate_socket(path: &Path) -> Result<(), AstreactlError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| AstreactlError::EndpointNotFound("control endpoint not found".to_string()))?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_socket()
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(AstreactlError::EndpointNotFound(
            "control endpoint is not a secure Unix socket".to_string(),
        ));
    }
    Ok(())
}

fn valid_instance(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_INSTANCE_NAME_BYTES
        && value != "."
        && value != ".."
        && !value.contains("..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn validate_instance(value: &str) -> Result<(), AstreactlError> {
    valid_instance(value)
        .then_some(())
        .ok_or_else(|| AstreactlError::Usage("invalid instance name".to_string()))
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference memory.
    unsafe { libc::geteuid() as u32 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::{fs::PermissionsExt, net::UnixListener},
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "astreactl-discovery-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn valid_instance_names_do_not_filter_tmp_substrings() {
        assert!(valid_instance("attempt-1"));
        assert!(valid_instance("tmp-session"));
        assert!(!valid_instance("../escape"));
    }

    #[test]
    fn explicit_socket_requires_secure_socket_metadata() {
        let directory = temp_directory();
        let socket = directory.join("control.sock");
        let _listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(discover_socket(None, Some(&socket)).unwrap(), socket);
        let link = directory.join("link.sock");
        std::os::unix::fs::symlink(&socket, &link).unwrap();
        assert!(matches!(
            discover_socket(None, Some(&link)),
            Err(AstreactlError::EndpointNotFound(_))
        ));
        drop(_listener);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn wayland_display_requires_a_secure_instance_directory() {
        let runtime = temp_directory();
        let root = runtime.join("astrea/typhon");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(runtime.join("astrea"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let instance = root.join("wayland-1");
        fs::create_dir(&instance).unwrap();
        fs::set_permissions(&instance, fs::Permissions::from_mode(0o755)).unwrap();
        let socket = instance.join("control.sock");
        let _listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();

        assert!(matches!(
            discover_socket_from(None, None, &runtime, Some("wayland-1")),
            Err(AstreactlError::EndpointNotFound(_))
        ));
        let _ = fs::remove_dir_all(runtime);
    }

    #[test]
    fn wayland_display_does_not_follow_a_symlinked_instance_directory() {
        let runtime = temp_directory();
        let root = runtime.join("astrea/typhon");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(runtime.join("astrea"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let real_instance = runtime.join("real-instance");
        fs::create_dir(&real_instance).unwrap();
        fs::set_permissions(&real_instance, fs::Permissions::from_mode(0o700)).unwrap();
        let socket = real_instance.join("control.sock");
        let _listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&real_instance, root.join("wayland-1")).unwrap();

        assert!(matches!(
            discover_socket_from(None, None, &runtime, Some("wayland-1")),
            Err(AstreactlError::EndpointNotFound(_))
        ));
        let _ = fs::remove_dir_all(runtime);
    }

    #[test]
    fn discovery_fails_closed_when_direct_entry_limit_is_exceeded() {
        let runtime = temp_directory();
        let root = runtime.join("astrea/typhon");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(runtime.join("astrea"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        for index in 0..=MAX_DISCOVERY_ENTRIES {
            fs::create_dir(root.join(format!("instance-{index}"))).unwrap();
        }

        let error = discover_socket_from(None, None, &runtime, None).unwrap_err();
        assert!(
            matches!(error, AstreactlError::EndpointNotFound(message) if message.contains("entry limit"))
        );
        let _ = fs::remove_dir_all(runtime);
    }
}
