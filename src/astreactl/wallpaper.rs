use super::client::AstreactlError;
use crate::control_snapshots::{
    AstreactlResult, WallpaperDescriptorSnapshot, WallpaperListSnapshot, WallpaperSnapshot,
};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_REQUEST_BYTES: usize = 4096;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const DEFAULT_WALLPAPER_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PaperResponse {
    ok: bool,
    #[serde(default)]
    completed: bool,
    #[serde(default)]
    request_id: Option<u64>,
    #[serde(default)]
    snapshot: Option<WallpaperSnapshot>,
    #[serde(default)]
    wallpapers: Option<Vec<WallpaperDescriptorSnapshot>>,
    #[serde(default)]
    error_code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

pub fn request(
    action: &str,
    arguments: serde_json::Value,
    timeout: Duration,
) -> Result<AstreactlResult, AstreactlError> {
    let endpoint = discover_endpoint()?;
    request_at(&endpoint, action, arguments, timeout)
}

fn request_at(
    endpoint: &Path,
    action: &str,
    arguments: serde_json::Value,
    timeout: Duration,
) -> Result<AstreactlResult, AstreactlError> {
    let body = serde_json::to_string(&arguments)
        .map_err(|_| AstreactlError::Usage("invalid wallpaper request".to_string()))?;
    let payload = format!("wallpaper {action} {body}\n").into_bytes();
    if payload.len() > MAX_REQUEST_BYTES {
        return Err(AstreactlError::Usage(
            "wallpaper request exceeds the protocol limit".to_string(),
        ));
    }
    let mut stream = UnixStream::connect(endpoint).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            AstreactlError::EndpointNotFound("Paper wallpaper endpoint is unavailable".to_string())
        } else {
            AstreactlError::Transport(error)
        }
    })?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(AstreactlError::Transport)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(1)))
        .map_err(AstreactlError::Transport)?;
    stream
        .write_all(&payload)
        .map_err(AstreactlError::Transport)?;
    let response = read_one_response(&mut stream)?;
    let decoded: PaperResponse =
        serde_json::from_slice(&response).map_err(|_| AstreactlError::MalformedResponse)?;
    if !decoded.completed {
        return Err(AstreactlError::MalformedResponse);
    }
    if !decoded.ok {
        return Err(AstreactlError::Paper {
            code: decoded
                .error_code
                .unwrap_or_else(|| "paper-request-failed".to_string()),
            message: decoded
                .message
                .unwrap_or_else(|| "Paper wallpaper request failed".to_string()),
        });
    }
    let snapshot = decoded.snapshot.ok_or(AstreactlError::MalformedResponse)?;
    let _ = decoded.request_id;
    if action == "list" {
        return Ok(AstreactlResult::WallpaperList(WallpaperListSnapshot {
            wallpapers: decoded.wallpapers.unwrap_or_default(),
            snapshot,
        }));
    }
    Ok(AstreactlResult::Wallpaper(snapshot))
}

fn read_one_response(stream: &mut UnixStream) -> Result<Vec<u8>, AstreactlError> {
    let mut response = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    loop {
        let count = stream.read(&mut chunk).map_err(|error| {
            if error.kind() == io::ErrorKind::TimedOut {
                AstreactlError::Timeout
            } else {
                AstreactlError::Transport(error)
            }
        })?;
        if count == 0 {
            return Err(AstreactlError::MalformedResponse);
        }
        if response.len() + count > MAX_RESPONSE_BYTES {
            return Err(AstreactlError::ResponseTooLarge);
        }
        response.extend_from_slice(&chunk[..count]);
        if let Some(newline) = response.iter().position(|byte| *byte == b'\n') {
            if response[newline + 1..]
                .iter()
                .any(|byte| !byte.is_ascii_whitespace())
            {
                return Err(AstreactlError::MalformedResponse);
            }
            response.truncate(newline);
            return Ok(response);
        }
    }
}

fn discover_endpoint() -> Result<PathBuf, AstreactlError> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
        AstreactlError::EndpointNotFound("XDG_RUNTIME_DIR is not set".to_string())
    })?;
    let runtime = PathBuf::from(runtime);
    if !runtime.is_absolute() {
        return Err(AstreactlError::EndpointNotFound(
            "XDG_RUNTIME_DIR is not absolute".to_string(),
        ));
    }
    validate_directory(&runtime, None, "XDG_RUNTIME_DIR")?;
    let shell_dir = runtime.join("astrea-shell");
    validate_directory(&shell_dir, Some(0o700), "Paper runtime directory")?;
    let endpoint = shell_dir.join("wallpaper.sock");
    if endpoint.as_os_str().len() >= 108 {
        return Err(AstreactlError::Transport(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Paper socket path is too long",
        )));
    }
    let metadata = fs::symlink_metadata(&endpoint).map_err(|_| {
        AstreactlError::EndpointNotFound("Paper wallpaper endpoint is unavailable".to_string())
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_socket()
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(AstreactlError::EndpointNotFound(
            "Paper wallpaper endpoint is insecure".to_string(),
        ));
    }
    Ok(endpoint)
}

fn validate_directory(
    path: &Path,
    expected_mode: Option<u32>,
    label: &str,
) -> Result<(), AstreactlError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| AstreactlError::EndpointNotFound(format!("{label} is unavailable")))?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || metadata.uid() != effective_uid()
        || expected_mode.is_some_and(|mode| metadata.mode() & 0o777 != mode)
        || (expected_mode.is_none() && metadata.mode() & 0o022 != 0)
    {
        return Err(AstreactlError::EndpointNotFound(format!(
            "{label} is insecure"
        )));
    }
    Ok(())
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and returns the current process uid.
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::thread;

    #[test]
    fn default_wallpaper_timeout_includes_operation_margin() {
        assert!(DEFAULT_WALLPAPER_TIMEOUT >= Duration::from_secs(6));
    }

    #[test]
    fn paper_list_response_decodes_stable_catalog_entries() {
        let root = std::env::temp_dir().join(format!(
            "astrea-paper-cli-list-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let endpoint = root.join("wallpaper.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; MAX_REQUEST_BYTES];
            let _ = stream.read(&mut request).unwrap();
            let response = serde_json::json!({
                "ok": true,
                "completed": true,
                "requestId": 0,
                "wallpapers": [{
                    "kind": "image",
                    "sourceKind": "system-resource",
                    "origin": "system",
                    "fit": "cover",
                    "scope": "global",
                    "logicalId": "astrea://wallpaper/default",
                    "source": "/tmp/default.png",
                    "displayName": "Astrea Default"
                }],
                "snapshot": {
                    "configured": null,
                    "factoryDefault": {
                        "kind": "image",
                        "sourceKind": "system-resource",
                        "origin": "system",
                        "fit": "cover",
                        "scope": "global",
                        "logicalId": "astrea://wallpaper/default",
                        "source": "/tmp/default.png"
                    },
                    "effective": {
                        "kind": "image",
                        "sourceKind": "system-resource",
                        "origin": "system",
                        "fit": "cover",
                        "scope": "global",
                        "logicalId": "astrea://wallpaper/default",
                        "source": "/tmp/default.png"
                    },
                    "state": "ready",
                    "fallback": "none",
                    "generation": 1,
                    "errorCode": "",
                    "lastError": ""
                }
            });
            stream
                .write_all(serde_json::to_string(&response).unwrap().as_bytes())
                .unwrap();
            stream.write_all(b"\n").unwrap();
        });
        let result = request_at(
            &endpoint,
            "list",
            serde_json::json!({}),
            Duration::from_secs(1),
        )
        .unwrap();
        match result {
            AstreactlResult::WallpaperList(list) => {
                assert_eq!(list.wallpapers.len(), 1);
                assert_eq!(list.wallpapers[0].logical_id, "astrea://wallpaper/default");
            }
            other => panic!("unexpected result: {other:?}"),
        }
        server.join().unwrap();
        let _ = fs::remove_file(endpoint);
        let _ = fs::remove_dir(root);
    }

    #[test]
    fn paper_request_waits_past_transport_for_final_completion() {
        let root = std::env::temp_dir().join(format!(
            "astrea-paper-cli-delay-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let endpoint = root.join("wallpaper.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; MAX_REQUEST_BYTES];
            let _ = stream.read(&mut request).unwrap();
            thread::sleep(Duration::from_millis(1500));
            stream
                .write_all(
                    br#"{"ok":true,"completed":true,"requestId":1,"snapshot":{"configured":null,"factoryDefault":{"kind":"image","sourceKind":"system-resource","fit":"cover","scope":"global","logicalId":"astrea://wallpaper/default","source":"/tmp/default.png"},"effective":{"kind":"image","sourceKind":"system-resource","fit":"cover","scope":"global","logicalId":"astrea://wallpaper/default","source":"/tmp/default.png"},"state":"ready","fallback":"none","generation":1,"errorCode":"","lastError":""}}
"#,
                )
                .unwrap();
        });
        let result = request_at(
            &endpoint,
            "set",
            serde_json::json!({"source":"/tmp/selected.png","fit":"contain"}),
            DEFAULT_WALLPAPER_TIMEOUT,
        )
        .unwrap();
        assert!(matches!(result, AstreactlResult::Wallpaper(_)));
        server.join().unwrap();
        let _ = fs::remove_file(endpoint);
        let _ = fs::remove_dir(root);
    }

    #[test]
    fn paper_request_preserves_unicode_and_spaces_as_json() {
        let root = std::env::temp_dir().join(format!(
            "astrea-paper-cli-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let endpoint = root.join("wallpaper.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; MAX_REQUEST_BYTES];
            let count = stream.read(&mut request).unwrap();
            let line = String::from_utf8_lossy(&request[..count]);
            assert!(line.contains("snow & café.png"));
            stream
                .write_all(
                    br#"{"ok":true,"completed":true,"requestId":1,"snapshot":{"configured":null,"factoryDefault":{"kind":"image","sourceKind":"system-resource","fit":"cover","scope":"global","logicalId":"astrea://wallpaper/default","source":"/tmp/default.png"},"effective":{"kind":"image","sourceKind":"system-resource","fit":"cover","scope":"global","logicalId":"astrea://wallpaper/default","source":"/tmp/default.png"},"state":"ready","fallback":"none","generation":1,"errorCode":"","lastError":""}}
"#,
                )
                .unwrap();
        });
        let result = request_at(
            &endpoint,
            "set",
            serde_json::json!({"source":"/tmp/snow & café.png","fit":"cover"}),
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(matches!(result, AstreactlResult::Wallpaper(_)));
        server.join().unwrap();
        let _ = fs::remove_file(endpoint);
        let _ = fs::remove_dir(root);
    }

    #[test]
    fn paper_request_requires_final_completion_and_preserves_error_code() {
        let root = std::env::temp_dir().join(format!(
            "astrea-paper-cli-error-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let endpoint = root.join("wallpaper.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; MAX_REQUEST_BYTES];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(
                    br#"{"ok":false,"completed":true,"requestId":2,"errorCode":"source-missing","message":"source vanished","snapshot":{"configured":null,"factoryDefault":{"kind":"image","sourceKind":"system-resource","fit":"cover","scope":"global","logicalId":"astrea://wallpaper/default","source":"/tmp/default.png"},"effective":{"kind":"image","sourceKind":"system-resource","fit":"cover","scope":"global","logicalId":"astrea://wallpaper/default","source":"/tmp/default.png"},"state":"fallback","fallback":"source-missing","generation":1,"errorCode":"source-missing","lastError":"source vanished"}}
"#,
                )
                .unwrap();
        });
        let result = request_at(
            &endpoint,
            "set",
            serde_json::json!({"source":"/tmp/gone.png","fit":"cover"}),
            Duration::from_secs(1),
        );
        assert!(matches!(
            result,
            Err(AstreactlError::Paper { code, message })
                if code == "source-missing" && message == "source vanished"
        ));
        server.join().unwrap();
        let _ = fs::remove_file(endpoint);
        let _ = fs::remove_dir(root);
    }

    #[test]
    fn paper_request_rejects_loading_acknowledgements() {
        let root = std::env::temp_dir().join(format!(
            "ap-load-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let endpoint = root.join("wallpaper.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; MAX_REQUEST_BYTES];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(
                    br#"{"ok":true,"completed":false,"accepted":true}
"#,
                )
                .unwrap();
        });
        let result = request_at(
            &endpoint,
            "set",
            serde_json::json!({"source":"/tmp/loading.png","fit":"cover"}),
            Duration::from_secs(1),
        );
        assert!(matches!(result, Err(AstreactlError::MalformedResponse)));
        server.join().unwrap();
        let _ = fs::remove_file(endpoint);
        let _ = fs::remove_dir(root);
    }
}
