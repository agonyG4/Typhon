use serde::de::Error as _;
use std::{
    error::Error,
    fmt,
    io::{self, Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};

use crate::control::{
    ControlCodecError, ControlError, ControlRequest, ControlResponse, MAX_RESPONSE_BYTES,
    decode_response, encode_request,
};
use crate::control_snapshots::{
    ActiveWindowSnapshot, AstreactlResult, CursorSnapshot, DoctorSnapshot, OutputListSnapshot,
    PerformanceSnapshot, StatusSnapshot, VersionSnapshot, WindowListSnapshot,
};
use crate::cursor_theme::CursorConfiguration;

#[derive(Debug)]
pub enum AstreactlError {
    Usage(String),
    EndpointNotFound(String),
    Transport(io::Error),
    Timeout,
    ResponseTooLarge,
    MalformedResponse,
    ProtocolMismatch,
    ResponseIdMismatch { expected: u64, actual: u64 },
    Server(ControlError),
}

impl fmt::Display for AstreactlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) => write!(formatter, "{message}"),
            Self::EndpointNotFound(message) => write!(formatter, "{message}"),
            Self::Transport(error) => write!(formatter, "transport failed: {error}"),
            Self::Timeout => write!(formatter, "control request timed out"),
            Self::ResponseTooLarge => write!(formatter, "control response exceeds 1 MiB"),
            Self::MalformedResponse => write!(formatter, "malformed control response"),
            Self::ProtocolMismatch => write!(formatter, "incompatible control response"),
            Self::ResponseIdMismatch { expected, actual } => write!(
                formatter,
                "control response id mismatch: expected {expected}, got {actual}"
            ),
            Self::Server(error) => write!(
                formatter,
                "{}: {}",
                error.code.as_str(),
                error.detail.as_deref().unwrap_or(&error.message)
            ),
        }
    }
}

impl Error for AstreactlError {}

fn decode_command_result(
    command: &str,
    response: ControlResponse,
) -> Result<AstreactlResult, AstreactlError> {
    if !response.ok {
        return Err(AstreactlError::Server(response.error.unwrap_or_else(
            || ControlError::new(crate::control::ControlErrorCode::Internal, "command failed"),
        )));
    }
    let value = response.result.ok_or(AstreactlError::MalformedResponse)?;
    let decoded = match command {
        "version" => serde_json::from_value::<VersionSnapshot>(value).map(AstreactlResult::Version),
        "status" => serde_json::from_value::<StatusSnapshot>(value).map(AstreactlResult::Status),
        "performance" => serde_json::from_value::<PerformanceSnapshot>(value)
            .map(|snapshot| AstreactlResult::Performance(Box::new(snapshot))),
        "doctor" => serde_json::from_value::<DoctorSnapshot>(value).map(AstreactlResult::Doctor),
        "outputs" => {
            serde_json::from_value::<OutputListSnapshot>(value).map(AstreactlResult::Outputs)
        }
        "windows" => {
            serde_json::from_value::<WindowListSnapshot>(value).map(AstreactlResult::Windows)
        }
        "active-window" => {
            serde_json::from_value::<ActiveWindowSnapshot>(value).map(AstreactlResult::ActiveWindow)
        }
        "cursor.get" | "cursor.set-theme" | "cursor.set-size" | "cursor.set" | "cursor.reload" => {
            serde_json::from_value::<CursorSnapshot>(value)
                .and_then(|snapshot| {
                    CursorConfiguration::new(&snapshot.desired_theme, snapshot.desired_size_px)
                        .map_err(|_| serde_json::Error::custom("invalid desired cursor state"))?;
                    CursorConfiguration::new(&snapshot.active_theme, snapshot.active_size_px)
                        .map_err(|_| serde_json::Error::custom("invalid active cursor state"))?;
                    Ok(snapshot)
                })
                .map(AstreactlResult::Cursor)
        }
        _ => return Err(AstreactlError::Usage("unknown control command".to_string())),
    };
    decoded.map_err(|_| AstreactlError::MalformedResponse)
}

pub fn request(
    path: &Path,
    command: &str,
    timeout: Duration,
) -> Result<AstreactlResult, AstreactlError> {
    request_with_args(path, command, serde_json::json!({}), timeout)
}

pub fn request_with_args(
    path: &Path,
    command: &str,
    args: serde_json::Value,
    timeout: Duration,
) -> Result<AstreactlResult, AstreactlError> {
    let request = ControlRequest::new(1, command, args)
        .map_err(|_| AstreactlError::Usage("invalid control request".to_string()))?;
    let encoded = encode_request(&request).map_err(|_| {
        AstreactlError::Usage("control request exceeds the protocol limit".to_string())
    })?;
    let deadline = Instant::now() + timeout;
    let mut stream = connect(path, deadline)?;
    stream
        .set_nonblocking(true)
        .map_err(AstreactlError::Transport)?;
    write_all_until(&mut stream, &encoded, deadline)?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .map_err(AstreactlError::Transport)?;
    let response = read_one_response(&mut stream, deadline)?;
    if response.id != request.id {
        return Err(AstreactlError::ResponseIdMismatch {
            expected: request.id,
            actual: response.id,
        });
    }
    decode_command_result(command, response)
}

fn connect(path: &Path, deadline: Instant) -> Result<UnixStream, AstreactlError> {
    let path = std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str());
    if path.contains(&0) || path.len() >= 108 {
        return Err(AstreactlError::Transport(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Unix socket path is too long",
        )));
    }
    // SAFETY: socket arguments are constant and the returned descriptor is owned below.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(AstreactlError::Transport(io::Error::last_os_error()));
    }
    let result = (|| {
        // SAFETY: fd is the socket created above and F_SETFL only changes its flags.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(AstreactlError::Transport(io::Error::last_os_error()));
        }
        // SAFETY: fd is the socket created above and F_SETFL only changes its flags.
        let set_flags = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
        if set_flags < 0 {
            return Err(AstreactlError::Transport(io::Error::last_os_error()));
        }
        // SAFETY: zeroed sockaddr_un is valid, and all assigned bytes stay within sun_path.
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (index, byte) in path.iter().enumerate() {
            address.sun_path[index] = *byte as libc::c_char;
        }
        // SAFETY: address and its length describe the initialized Unix socket path.
        let connect_result = unsafe {
            libc::connect(
                fd,
                (&address as *const libc::sockaddr_un).cast(),
                (std::mem::size_of_val(&address.sun_family) + path.len() + 1) as libc::socklen_t,
            )
        };
        if connect_result == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(AstreactlError::Transport(error));
        }
        wait_for(fd, libc::POLLOUT, deadline)?;
        let mut socket_error = 0;
        let mut length = std::mem::size_of_val(&socket_error) as libc::socklen_t;
        // SAFETY: socket_error and length are valid writable storage for getsockopt.
        let status = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut socket_error as *mut i32).cast(),
                &mut length,
            )
        };
        if status < 0 {
            return Err(AstreactlError::Transport(io::Error::last_os_error()));
        }
        if socket_error != 0 {
            return Err(AstreactlError::Transport(io::Error::from_raw_os_error(
                socket_error,
            )));
        }
        Ok(())
    })();
    if let Err(error) = result {
        // SAFETY: fd is valid and still owned by this function.
        unsafe { libc::close(fd) };
        return Err(error);
    }
    // SAFETY: successful connect transfers sole descriptor ownership to UnixStream.
    Ok(unsafe { UnixStream::from_raw_fd(fd) })
}

fn write_all_until(
    stream: &mut UnixStream,
    bytes: &[u8],
    deadline: Instant,
) -> Result<(), AstreactlError> {
    let mut written = 0;
    while written < bytes.len() {
        match stream.write(&bytes[written..]) {
            Ok(0) => return Err(AstreactlError::Transport(io::ErrorKind::WriteZero.into())),
            Ok(count) => written += count,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                wait_for(stream.as_raw_fd(), libc::POLLOUT, deadline)?;
            }
            Err(error) => return Err(AstreactlError::Transport(error)),
        }
    }
    Ok(())
}

fn read_one_response(
    stream: &mut UnixStream,
    deadline: Instant,
) -> Result<ControlResponse, AstreactlError> {
    let mut response = Vec::with_capacity(MAX_RESPONSE_BYTES.min(4096));
    let mut chunk = [0_u8; 4096];
    loop {
        wait_for(stream.as_raw_fd(), libc::POLLIN, deadline)?;
        match stream.read(&mut chunk) {
            Ok(0) => return Err(AstreactlError::MalformedResponse),
            Ok(count) => {
                if let Some(response) = response_chunk(&mut response, &chunk[..count])? {
                    return Ok(response);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            Err(error) => return Err(AstreactlError::Transport(error)),
        }
    }
}

fn response_chunk(
    response: &mut Vec<u8>,
    chunk: &[u8],
) -> Result<Option<ControlResponse>, AstreactlError> {
    let available = MAX_RESPONSE_BYTES.saturating_sub(response.len());
    if chunk.len() > available {
        return Err(AstreactlError::ResponseTooLarge);
    }
    response.extend_from_slice(chunk);
    if let Some(newline) = response.iter().position(|byte| *byte == b'\n') {
        if response[newline + 1..]
            .iter()
            .any(|byte| !byte.is_ascii_whitespace())
        {
            return Err(AstreactlError::MalformedResponse);
        }
        return decode_framed_response(&response[..=newline]).map(Some);
    }
    Ok(None)
}

fn decode_framed_response(bytes: &[u8]) -> Result<ControlResponse, AstreactlError> {
    decode_response(bytes).map_err(|error| match error {
        ControlCodecError::ResponseTooLarge => AstreactlError::ResponseTooLarge,
        ControlCodecError::UnsupportedVersion(_) => AstreactlError::ProtocolMismatch,
        ControlCodecError::MalformedJson | ControlCodecError::InvalidResponse => {
            AstreactlError::MalformedResponse
        }
        _ => AstreactlError::ProtocolMismatch,
    })
}

fn wait_for(fd: i32, events: i16, deadline: Instant) -> Result<(), AstreactlError> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(AstreactlError::Timeout);
        }
        let timeout_ms = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        let mut pollfd = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        // SAFETY: pollfd points to one initialized descriptor record.
        let result = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(AstreactlError::Transport(error));
        }
        if result == 0 {
            return Err(AstreactlError::Timeout);
        }
        if pollfd.revents & libc::POLLNVAL != 0 {
            return Err(AstreactlError::Transport(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "control socket closed",
            )));
        }
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_snapshots::AstreactlResult;
    use std::{
        io::{Read, Write},
        os::unix::net::UnixListener,
        sync::mpsc,
        thread,
    };

    const VALID_STATUS_RESPONSE: &[u8] = br#"{"protocol":"astrea.control","version":1,"id":1,"ok":true,"result":{"instance":"test","waylandDisplay":"wayland-1","uptimeMs":1,"sessionState":"active","shutdownState":"running","outputCount":1,"mappedWindowCount":0,"minimizedWindowCount":0,"activeWindow":null,"xwayland":{"configured":false,"state":"disabled","generation":null},"control":{"endpointActive":true,"clientCount":0,"accepted":1}}}
"#;

    fn socket() -> (std::path::PathBuf, UnixListener) {
        let path = std::env::temp_dir().join(format!(
            "astreactl-client-test-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        (path.clone(), UnixListener::bind(path).unwrap())
    }

    #[test]
    fn command_specific_result_decoding_rejects_empty_null_and_wrong_shapes() {
        let status = serde_json::json!({
            "instance": "test",
            "waylandDisplay": "wayland-1",
            "uptimeMs": 1,
            "sessionState": "active",
            "shutdownState": "running",
            "outputCount": 1,
            "mappedWindowCount": 0,
            "minimizedWindowCount": 0,
            "activeWindow": null,
            "xwayland": {"configured": false, "state": "disabled", "generation": null},
            "control": {"endpointActive": true, "clientCount": 0, "accepted": 1}
        });
        let valid = ControlResponse::success(1, status.clone());
        assert!(matches!(
            decode_command_result("status", valid),
            Ok(AstreactlResult::Status(_))
        ));
        for result in [
            serde_json::json!({}),
            serde_json::Value::Null,
            status["xwayland"].clone(),
        ] {
            let response = ControlResponse::success(1, result);
            assert!(matches!(
                decode_command_result("status", response),
                Err(AstreactlError::MalformedResponse)
            ));
        }
    }

    #[test]
    fn response_boundary_accepts_exact_frame_and_rejects_same_chunk_overflow() {
        let prefix = br#"{"protocol":"astrea.control","version":1,"id":1,"ok":true,"result":""#;
        let mut exact = Vec::with_capacity(MAX_RESPONSE_BYTES);
        exact.extend_from_slice(prefix);
        exact.resize(MAX_RESPONSE_BYTES - 3, b'a');
        exact.extend_from_slice(b"\"}\n");
        let mut retained = Vec::new();
        assert!(response_chunk(&mut retained, &exact).unwrap().is_some());

        let mut overflow = exact;
        overflow.push(b'x');
        let mut retained = Vec::new();
        assert!(matches!(
            response_chunk(&mut retained, &overflow),
            Err(AstreactlError::ResponseTooLarge)
        ));
    }

    #[test]
    fn accepts_a_complete_newline_frame_without_waiting_for_eof() {
        let (path, listener) = socket();
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 512];
            let _ = stream.read(&mut request).unwrap();
            stream.write_all(VALID_STATUS_RESPONSE).unwrap();
            let _ = release_rx.recv();
        });
        let response = request(&path, "status", Duration::from_secs(1)).unwrap();
        assert!(matches!(response, AstreactlResult::Status(_)));
        release_tx.send(()).unwrap();
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_a_response_id_mismatch() {
        let (path, listener) = socket();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 512];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(
                    br#"{"protocol":"astrea.control","version":1,"id":2,"ok":true,"result":{} }
"#,
                )
                .unwrap();
        });
        assert!(matches!(
            request(&path, "status", Duration::from_secs(1)),
            Err(AstreactlError::ResponseIdMismatch { .. })
        ));
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_a_response_without_a_newline() {
        let (path, listener) = socket();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 512];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(
                    br#"{"protocol":"astrea.control","version":1,"id":1,"ok":true,"result":{}}"#,
                )
                .unwrap();
        });
        assert!(matches!(
            request(&path, "status", Duration::from_secs(1)),
            Err(AstreactlError::MalformedResponse)
        ));
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_an_endless_response_at_the_bound() {
        let (path, listener) = socket();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 512];
            let _ = stream.read(&mut request).unwrap();
            let bytes = vec![b'x'; MAX_RESPONSE_BYTES + 1];
            let _ = stream.write_all(&bytes);
        });
        assert!(matches!(
            request(&path, "status", Duration::from_secs(2)),
            Err(AstreactlError::ResponseTooLarge)
        ));
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_non_whitespace_after_the_first_frame() {
        let (path, listener) = socket();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 512];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(
                    br#"{"protocol":"astrea.control","version":1,"id":1,"ok":true,"result":{}}
{}"#,
                )
                .unwrap();
        });
        assert!(matches!(
            request(&path, "status", Duration::from_secs(1)),
            Err(AstreactlError::MalformedResponse)
        ));
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_an_unsupported_response_version() {
        let (path, listener) = socket();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 512];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(
                    br#"{"protocol":"astrea.control","version":2,"id":1,"ok":true,"result":{} }
"#,
                )
                .unwrap();
        });
        assert!(matches!(
            request(&path, "status", Duration::from_secs(1)),
            Err(AstreactlError::ProtocolMismatch)
        ));
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn read_timeout_is_a_typed_total_deadline() {
        let (path, listener) = socket();
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 512];
            let _ = stream.read(&mut request).unwrap();
            let _ = release_rx.recv();
        });
        let result = request(&path, "status", Duration::from_millis(20));
        release_tx.send(()).unwrap();
        assert!(matches!(result, Err(AstreactlError::Timeout)));
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn slow_drip_cannot_extend_the_total_deadline() {
        let (path, listener) = socket();
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 512];
            let _ = stream.read(&mut request).unwrap();
            let _ = stream.write_all(b"{");
            let _ = release_rx.recv();
        });
        let result = request(&path, "status", Duration::from_millis(30));
        release_tx.send(()).unwrap();
        assert!(matches!(result, Err(AstreactlError::Timeout)));
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }
}
