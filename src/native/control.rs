//! Native, nonblocking control socket ownership and client state machines.

use std::{
    collections::HashMap,
    fmt, fs, io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, MetadataExt, PermissionsExt},
        },
    },
    path::{Path, PathBuf},
};

use crate::control::{
    ControlCodecError, ControlError, ControlErrorCode, ControlRequest, ControlResponse,
    MAX_REQUEST_BYTES, decode_request, encode_response,
};

use super::event_loop::{ControlReadyEvent, NativeEventLoop, NativeEventSource, ReactorToken};

pub const MAX_CONTROL_CLIENTS: usize = 32;
pub const MAX_CONTROL_OPERATIONS_PER_CYCLE: usize = 16;

const DIRECTORY_MODE: u32 = 0o700;
const SOCKET_MODE: u32 = 0o600;
const LISTEN_BACKLOG: i32 = 32;
const READ_EVENTS: u32 =
    (libc::EPOLLIN | libc::EPOLLERR | libc::EPOLLHUP | libc::EPOLLRDHUP) as u32;
const WRITE_EVENTS: u32 =
    (libc::EPOLLOUT | libc::EPOLLERR | libc::EPOLLHUP | libc::EPOLLRDHUP) as u32;
const WAITING_EVENTS: u32 = (libc::EPOLLERR | libc::EPOLLHUP | libc::EPOLLRDHUP) as u32;
const TERMINAL_EVENTS: u32 = (libc::EPOLLERR | libc::EPOLLHUP | libc::EPOLLRDHUP) as u32;
const MAX_INSTANCE_BYTES: usize = 128;

#[derive(Debug)]
pub enum ControlServerError {
    InvalidRuntimeDirectory(String),
    InvalidInstance(String),
    UnsafePath(String),
    SocketInUse(PathBuf),
    ForeignSocket(PathBuf),
    ListenerFailure(String),
    Io(io::Error),
}

impl fmt::Display for ControlServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRuntimeDirectory(reason) => {
                write!(formatter, "invalid XDG runtime directory: {reason}")
            }
            Self::InvalidInstance(reason) => {
                write!(formatter, "invalid control instance: {reason}")
            }
            Self::UnsafePath(reason) => write!(formatter, "unsafe control socket path: {reason}"),
            Self::SocketInUse(path) => {
                write!(formatter, "control socket is in use: {}", path.display())
            }
            Self::ForeignSocket(path) => write!(
                formatter,
                "control socket is not owned by the compositor: {}",
                path.display()
            ),
            Self::ListenerFailure(reason) => {
                write!(formatter, "control listener failure: {reason}")
            }
            Self::Io(error) => write!(formatter, "control socket I/O: {error}"),
        }
    }
}

impl std::error::Error for ControlServerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ControlServerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlRuntimePaths {
    runtime_dir: PathBuf,
    socket_dir: PathBuf,
    socket_path: PathBuf,
}

impl ControlRuntimePaths {
    pub fn for_runtime_dir(runtime_dir: &Path, instance: &str) -> Result<Self, ControlServerError> {
        validate_runtime_dir(runtime_dir)?;
        validate_instance(instance)?;

        let socket_dir = runtime_dir.join("astrea").join("typhon").join(instance);
        Ok(Self {
            runtime_dir: runtime_dir.to_path_buf(),
            socket_path: socket_dir.join("control.sock"),
            socket_dir,
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    fn prepare_directories(&self, owner_uid: u32) -> Result<(), ControlServerError> {
        ensure_owned_directory(&self.runtime_dir.join("astrea"), owner_uid)?;
        ensure_owned_directory(&self.runtime_dir.join("astrea").join("typhon"), owner_uid)?;
        ensure_owned_directory(&self.socket_dir, owner_uid)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

#[derive(Debug)]
pub struct NativeControlServer {
    listener: OwnedFd,
    listener_token: ReactorToken,
    clients: HashMap<ReactorToken, ControlClient>,
    paths: ControlRuntimePaths,
    owner_uid: u32,
    socket_identity: SocketIdentity,
    shut_down: bool,
}

#[derive(Debug)]
struct ControlClient {
    fd: OwnedFd,
    input: Vec<u8>,
    output: Vec<u8>,
    written: usize,
    state: ControlClientState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControlClientState {
    Reading,
    AwaitingResponse,
    Writing,
}

#[derive(Debug)]
enum ClientAction {
    None,
    Request(ControlRequest),
    Response(ControlResponse),
    Close,
}

impl NativeControlServer {
    pub fn bind(
        event_loop: &mut NativeEventLoop,
        runtime_dir: &Path,
        instance: &str,
    ) -> Result<Self, ControlServerError> {
        let paths = ControlRuntimePaths::for_runtime_dir(runtime_dir, instance)?;
        let owner_uid = effective_uid();
        paths.prepare_directories(owner_uid)?;
        remove_stale_socket(&paths, owner_uid)?;

        let listener = create_listener(&paths.socket_path)?;
        let socket_identity = socket_path_identity(&paths.socket_path)?;
        set_socket_mode(&paths.socket_path, owner_uid, socket_identity)?;
        let listener_token = match event_loop.register_with_events(
            listener.as_raw_fd(),
            NativeEventSource::ControlListener,
            READ_EVENTS,
        ) {
            Ok(token) => token,
            Err(error) => {
                remove_socket_if_identity(&paths.socket_path, socket_identity);
                return Err(error.into());
            }
        };

        eprintln!(
            "typhon control: listener_created instance={} mode=0600",
            instance
        );
        Ok(Self {
            listener,
            listener_token,
            clients: HashMap::new(),
            paths,
            owner_uid,
            socket_identity,
            shut_down: false,
        })
    }

    pub fn socket_path(&self) -> &Path {
        self.paths.socket_path()
    }

    pub fn listener_token(&self) -> ReactorToken {
        self.listener_token
    }

    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    pub fn service_events(
        &mut self,
        event_loop: &mut NativeEventLoop,
        events: &[ControlReadyEvent],
        budget: usize,
    ) -> Result<Vec<(ReactorToken, ControlRequest)>, ControlServerError> {
        let mut pending = Vec::new();
        let mut remaining = budget.min(MAX_CONTROL_OPERATIONS_PER_CYCLE);
        for event in events {
            if remaining == 0 {
                break;
            }
            remaining -= 1;

            if event.token == self.listener_token {
                if event.flags & TERMINAL_EVENTS != 0 {
                    return Err(ControlServerError::ListenerFailure(
                        "control listener reported a terminal readiness error".to_string(),
                    ));
                }
                self.accept_one(event_loop)?;
                continue;
            }

            let action = {
                let Some(client) = self.clients.get_mut(&event.token) else {
                    eprintln!("typhon control: stale_token raw={}", event.token.raw());
                    continue;
                };
                let action = client.advance(event.flags);
                if matches!(action, ClientAction::Request(_) | ClientAction::Response(_)) {
                    client.state = ControlClientState::AwaitingResponse;
                }
                action
            };
            match action {
                ClientAction::None => {}
                ClientAction::Request(request) => {
                    if !event_loop.modify(event.token, WAITING_EVENTS)? {
                        self.remove_client(event_loop, event.token);
                        continue;
                    }
                    pending.push((event.token, request));
                }
                ClientAction::Response(response) => {
                    self.queue_response(event_loop, event.token, response)?;
                }
                ClientAction::Close => {
                    self.remove_client(event_loop, event.token);
                }
            }
        }
        Ok(pending)
    }

    pub fn queue_response(
        &mut self,
        event_loop: &mut NativeEventLoop,
        token: ReactorToken,
        response: ControlResponse,
    ) -> Result<(), ControlServerError> {
        let encoded = match encode_response(&response) {
            Ok(encoded) => encoded,
            Err(_) => encode_response(&ControlResponse::failure(
                response.id,
                ControlError::new(
                    ControlErrorCode::Internal,
                    "control response exceeded the protocol limit",
                ),
            ))
            .map_err(|_| {
                ControlServerError::ListenerFailure(
                    "failed to encode bounded error response".to_string(),
                )
            })?,
        };
        let Some(client) = self.clients.get_mut(&token) else {
            return Ok(());
        };
        if client.state != ControlClientState::AwaitingResponse {
            return Ok(());
        }
        client.output = encoded;
        client.written = 0;
        client.state = ControlClientState::Writing;
        if !event_loop.modify(token, WRITE_EVENTS)? {
            self.remove_client(event_loop, token);
        }
        Ok(())
    }

    pub fn shutdown(&mut self, event_loop: &mut NativeEventLoop) -> Result<(), ControlServerError> {
        if self.shut_down {
            return Ok(());
        }
        let tokens = self.clients.keys().copied().collect::<Vec<_>>();
        for token in tokens {
            let _ = event_loop.unregister(token);
        }
        self.clients.clear();
        let _ = event_loop.unregister(self.listener_token);
        self.shut_down = true;
        remove_socket_if_identity(&self.paths.socket_path, self.socket_identity);
        eprintln!("typhon control: shutdown_cleanup clients=0");
        Ok(())
    }

    fn accept_one(&mut self, event_loop: &mut NativeEventLoop) -> Result<(), ControlServerError> {
        let fd = loop {
            let fd = unsafe {
                // SAFETY: `self.listener` is a live listening socket owned by
                // this server, and the flags request nonblocking CLOEXEC I/O.
                libc::accept4(
                    self.listener.as_raw_fd(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                )
            };
            if fd >= 0 {
                break fd;
            }
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::EAGAIN) => return Ok(()),
                _ => {
                    return Err(ControlServerError::ListenerFailure(error.to_string()));
                }
            }
        };
        let client_fd = unsafe {
            // SAFETY: `accept4` returned a new owned descriptor.
            OwnedFd::from_raw_fd(fd)
        };
        let peer_uid = peer_uid(client_fd.as_raw_fd());
        if peer_uid != Some(self.owner_uid) {
            eprintln!("typhon control: rejected_unauthorized");
            send_best_effort_unauthorized(client_fd.as_raw_fd());
            return Ok(());
        }
        if self.clients.len() >= MAX_CONTROL_CLIENTS {
            eprintln!(
                "typhon control: rejected_capacity clients={}",
                self.clients.len()
            );
            return Ok(());
        }
        let token = event_loop.register_with_events(
            client_fd.as_raw_fd(),
            NativeEventSource::ControlClient,
            READ_EVENTS,
        )?;
        self.clients.insert(
            token,
            ControlClient {
                fd: client_fd,
                input: Vec::with_capacity(MAX_REQUEST_BYTES),
                output: Vec::new(),
                written: 0,
                state: ControlClientState::Reading,
            },
        );
        eprintln!("typhon control: accepted clients={}", self.clients.len());
        Ok(())
    }

    fn remove_client(&mut self, event_loop: &mut NativeEventLoop, token: ReactorToken) {
        let _ = event_loop.unregister(token);
        if self.clients.remove(&token).is_some() {
            eprintln!("typhon control: client_cleanup");
        }
    }
}

impl Drop for NativeControlServer {
    fn drop(&mut self) {
        if !self.shut_down {
            remove_socket_if_identity(&self.paths.socket_path, self.socket_identity);
        }
    }
}

impl ControlClient {
    fn advance(&mut self, flags: u32) -> ClientAction {
        if flags & TERMINAL_EVENTS != 0 {
            return ClientAction::Close;
        }
        match self.state {
            ControlClientState::Reading if flags & libc::EPOLLIN as u32 != 0 => self.read_once(),
            ControlClientState::Writing if flags & libc::EPOLLOUT as u32 != 0 => self.write_once(),
            _ => ClientAction::None,
        }
    }

    fn read_once(&mut self) -> ClientAction {
        let remaining = MAX_REQUEST_BYTES.saturating_sub(self.input.len());
        let read_len = remaining.saturating_add(1).min(4096);
        if read_len == 0 {
            return request_too_large_response();
        }
        let mut buffer = [0u8; 4096];
        let read_len = read_len.min(buffer.len());
        let read = unsafe {
            // SAFETY: `self.fd` is a live nonblocking client socket and the
            // buffer is valid for the requested bounded length.
            libc::read(self.fd.as_raw_fd(), buffer.as_mut_ptr().cast(), read_len)
        };
        if read > 0 {
            self.input.extend_from_slice(&buffer[..read as usize]);
            if let Some(newline) = self.input.iter().position(|byte| *byte == b'\n') {
                let request = decode_request(&self.input[..=newline]);
                return match request {
                    Ok(request) => ClientAction::Request(request),
                    Err(error) => ClientAction::Response(response_for_codec_error(error)),
                };
            }
            if self.input.len() > MAX_REQUEST_BYTES {
                return request_too_large_response();
            }
            return ClientAction::None;
        }
        if read == 0 {
            return ClientAction::Close;
        }
        match io::Error::last_os_error().raw_os_error() {
            Some(libc::EAGAIN | libc::EINTR) => ClientAction::None,
            Some(libc::EPIPE | libc::ECONNRESET) => ClientAction::Close,
            _ => ClientAction::Close,
        }
    }

    fn write_once(&mut self) -> ClientAction {
        let remaining = &self.output[self.written..];
        if remaining.is_empty() {
            return ClientAction::Close;
        }
        let written = unsafe {
            // SAFETY: `self.fd` is a live nonblocking client socket and the
            // slice points to initialized response bytes owned by this client.
            libc::write(
                self.fd.as_raw_fd(),
                remaining.as_ptr().cast(),
                remaining.len(),
            )
        };
        if written > 0 {
            self.written += written as usize;
            if self.written == self.output.len() {
                return ClientAction::Close;
            }
            return ClientAction::None;
        }
        match io::Error::last_os_error().raw_os_error() {
            Some(libc::EAGAIN | libc::EINTR) => ClientAction::None,
            Some(libc::EPIPE | libc::ECONNRESET) => ClientAction::Close,
            _ => ClientAction::Close,
        }
    }
}

fn request_too_large_response() -> ClientAction {
    ClientAction::Response(ControlResponse::failure(
        0,
        ControlError::new(
            ControlErrorCode::RequestTooLarge,
            "control request exceeds 64 KiB",
        ),
    ))
}

fn response_for_codec_error(error: ControlCodecError) -> ControlResponse {
    let (code, message) = match error {
        ControlCodecError::RequestTooLarge => (
            ControlErrorCode::RequestTooLarge,
            "control request exceeds 64 KiB".to_string(),
        ),
        ControlCodecError::MalformedJson => (
            ControlErrorCode::MalformedJson,
            "control request is not valid JSON".to_string(),
        ),
        ControlCodecError::InvalidRequest => (
            ControlErrorCode::InvalidRequest,
            "control request is invalid".to_string(),
        ),
        ControlCodecError::UnsupportedVersion(version) => (
            ControlErrorCode::UnsupportedVersion,
            format!("unsupported control protocol version {version}"),
        ),
        ControlCodecError::ResponseTooLarge | ControlCodecError::InvalidResponse => (
            ControlErrorCode::Internal,
            "control request could not be processed".to_string(),
        ),
    };
    ControlResponse::failure(0, ControlError::new(code, message))
}

fn send_best_effort_unauthorized(fd: RawFd) {
    let response = ControlResponse::failure(
        0,
        ControlError::new(
            ControlErrorCode::Unauthorized,
            "control peer is unauthorized",
        ),
    );
    let Ok(bytes) = encode_response(&response) else {
        return;
    };
    let _ = unsafe {
        // SAFETY: `fd` is the accepted descriptor owned by the caller for the
        // duration of this best-effort nonblocking write.
        libc::write(fd, bytes.as_ptr().cast(), bytes.len())
    };
}

fn validate_runtime_dir(runtime_dir: &Path) -> Result<(), ControlServerError> {
    if !runtime_dir.is_absolute() {
        return Err(ControlServerError::InvalidRuntimeDirectory(
            "path is not absolute".to_string(),
        ));
    }
    let metadata = fs::symlink_metadata(runtime_dir).map_err(|error| {
        ControlServerError::InvalidRuntimeDirectory(format!("{runtime_dir:?}: {error}"))
    })?;
    let mode = metadata.mode() & 0o777;
    if !metadata.file_type().is_dir() {
        return Err(ControlServerError::InvalidRuntimeDirectory(
            "path is not a directory".to_string(),
        ));
    }
    if metadata.uid() != effective_uid() {
        return Err(ControlServerError::InvalidRuntimeDirectory(
            "directory is not owned by the compositor user".to_string(),
        ));
    }
    if mode & 0o022 != 0 {
        return Err(ControlServerError::InvalidRuntimeDirectory(
            "directory is group- or world-writable".to_string(),
        ));
    }
    Ok(())
}

fn validate_instance(instance: &str) -> Result<(), ControlServerError> {
    if instance.is_empty() || instance.len() > MAX_INSTANCE_BYTES {
        return Err(ControlServerError::InvalidInstance(
            "instance length is outside the supported range".to_string(),
        ));
    }
    if instance == ".." || instance.contains("..") {
        return Err(ControlServerError::InvalidInstance(
            "instance contains traversal syntax".to_string(),
        ));
    }
    if !instance
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(ControlServerError::InvalidInstance(
            "instance contains a disallowed character".to_string(),
        ));
    }
    Ok(())
}

fn ensure_owned_directory(path: &Path, owner_uid: u32) -> Result<(), ControlServerError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => verify_owned_directory(path, &metadata, owner_uid),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|error| {
                ControlServerError::UnsafePath(format!("create {}: {error}", path.display()))
            })?;
            fs::set_permissions(path, fs::Permissions::from_mode(DIRECTORY_MODE)).map_err(
                |error| {
                    ControlServerError::UnsafePath(format!("chmod {}: {error}", path.display()))
                },
            )?;
            let metadata = fs::symlink_metadata(path).map_err(ControlServerError::Io)?;
            verify_owned_directory(path, &metadata, owner_uid)
        }
        Err(error) => Err(ControlServerError::UnsafePath(format!(
            "inspect {}: {error}",
            path.display()
        ))),
    }
}

fn verify_owned_directory(
    path: &Path,
    metadata: &fs::Metadata,
    owner_uid: u32,
) -> Result<(), ControlServerError> {
    if !metadata.file_type().is_dir() {
        return Err(ControlServerError::UnsafePath(format!(
            "{} is not a directory",
            path.display()
        )));
    }
    if metadata.uid() != owner_uid {
        return Err(ControlServerError::UnsafePath(format!(
            "{} has a foreign owner",
            path.display()
        )));
    }
    if metadata.mode() & 0o777 != DIRECTORY_MODE {
        return Err(ControlServerError::UnsafePath(format!(
            "{} does not have mode 0700",
            path.display()
        )));
    }
    Ok(())
}

fn remove_stale_socket(
    paths: &ControlRuntimePaths,
    owner_uid: u32,
) -> Result<(), ControlServerError> {
    let metadata = match fs::symlink_metadata(paths.socket_path()) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(ControlServerError::UnsafePath(error.to_string())),
    };
    if !metadata.file_type().is_socket() {
        return Err(ControlServerError::UnsafePath(format!(
            "{} is not a socket",
            paths.socket_path().display()
        )));
    }
    if metadata.uid() != owner_uid {
        return Err(ControlServerError::ForeignSocket(
            paths.socket_path().to_path_buf(),
        ));
    }
    match probe_existing_socket(paths.socket_path())? {
        SocketProbe::Live => Err(ControlServerError::SocketInUse(
            paths.socket_path().to_path_buf(),
        )),
        SocketProbe::Dead => {
            let revalidated = fs::symlink_metadata(paths.socket_path())?;
            if !revalidated.file_type().is_socket() || revalidated.uid() != owner_uid {
                return Err(ControlServerError::UnsafePath(
                    "socket changed during stale cleanup".to_string(),
                ));
            }
            fs::remove_file(paths.socket_path())?;
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SocketProbe {
    Live,
    Dead,
}

fn probe_existing_socket(path: &Path) -> Result<SocketProbe, ControlServerError> {
    let (address, address_len) = unix_socket_address(path)?;
    let fd = unsafe {
        // SAFETY: the arguments describe a local stream socket with bounded
        // nonblocking and close-on-exec flags.
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let fd = unsafe {
        // SAFETY: `socket` returned a new owned descriptor.
        OwnedFd::from_raw_fd(fd)
    };
    let result = unsafe {
        // SAFETY: `address` is initialized and `address_len` matches its
        // pathname payload.
        libc::connect(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            address_len,
        )
    };
    if result == 0 {
        return Ok(SocketProbe::Live);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ECONNREFUSED | libc::ENOENT) => Ok(SocketProbe::Dead),
        Some(libc::EINPROGRESS | libc::EALREADY) => {
            let mut socket_error = 0i32;
            let mut length = std::mem::size_of::<i32>() as libc::socklen_t;
            let result = unsafe {
                // SAFETY: `socket_error` and `length` are valid writable
                // storage for the SO_ERROR integer returned by the kernel.
                libc::getsockopt(
                    fd.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_ERROR,
                    (&mut socket_error as *mut i32).cast(),
                    &mut length,
                )
            };
            if result < 0 {
                return Err(io::Error::last_os_error().into());
            }
            match socket_error {
                0 => Ok(SocketProbe::Live),
                libc::ECONNREFUSED | libc::ENOENT => Ok(SocketProbe::Dead),
                _ => Err(ControlServerError::SocketInUse(path.to_path_buf())),
            }
        }
        _ => Err(ControlServerError::SocketInUse(path.to_path_buf())),
    }
}

fn create_listener(path: &Path) -> Result<OwnedFd, ControlServerError> {
    let (address, address_len) = unix_socket_address(path)?;
    let fd = unsafe {
        // SAFETY: the arguments describe the required local nonblocking stream
        // socket and close-on-exec is set atomically at creation.
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let listener = unsafe {
        // SAFETY: `socket` returned a new owned descriptor.
        OwnedFd::from_raw_fd(fd)
    };
    let result = unsafe {
        // SAFETY: `address` is initialized and names only the validated
        // Astrea socket path.
        libc::bind(
            listener.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            address_len,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let result = unsafe {
        // SAFETY: `listener` is the socket just successfully bound above.
        libc::listen(listener.as_raw_fd(), LISTEN_BACKLOG)
    };
    if result < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(listener)
}

fn set_socket_mode(
    path: &Path,
    owner_uid: u32,
    identity: SocketIdentity,
) -> Result<(), ControlServerError> {
    fs::set_permissions(path, fs::Permissions::from_mode(SOCKET_MODE))?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != owner_uid
        || metadata.mode() & 0o777 != SOCKET_MODE
        || metadata.dev() != identity.device
        || metadata.ino() != identity.inode
    {
        return Err(ControlServerError::UnsafePath(format!(
            "bound control socket changed during setup type={} uid={} mode={:o} dev={} expected_dev={} ino={} expected_ino={}",
            metadata.file_type().is_socket(),
            metadata.uid(),
            metadata.mode() & 0o777,
            metadata.dev(),
            identity.device,
            metadata.ino(),
            identity.inode,
        )));
    }
    Ok(())
}

fn remove_socket_if_identity(path: &Path, identity: SocketIdentity) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_socket()
        && metadata.dev() == identity.device
        && metadata.ino() == identity.inode
    {
        let _ = fs::remove_file(path);
    }
}

fn socket_path_identity(path: &Path) -> Result<SocketIdentity, ControlServerError> {
    let metadata = fs::symlink_metadata(path)?;
    Ok(SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn unix_socket_address(
    path: &Path,
) -> Result<(libc::sockaddr_un, libc::socklen_t), ControlServerError> {
    let bytes = path_bytes(path)?;
    let mut address = unsafe {
        // SAFETY: zeroed storage is valid before assigning the family and
        // pathname bytes in the sockaddr_un structure.
        std::mem::zeroed::<libc::sockaddr_un>()
    };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    if bytes.len() >= address.sun_path.len() {
        return Err(ControlServerError::UnsafePath(
            "control socket path exceeds the Unix socket limit".to_string(),
        ));
    }
    unsafe {
        // SAFETY: the destination is the zeroed sun_path array and the source
        // length was bounded against that array above.
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr().cast::<libc::c_char>(),
            address.sun_path.as_mut_ptr(),
            bytes.len(),
        );
    }
    let address_len =
        (std::mem::size_of_val(&address.sun_family) + bytes.len() + 1) as libc::socklen_t;
    Ok((address, address_len))
}

fn path_bytes(path: &Path) -> Result<&[u8], ControlServerError> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.contains(&0) {
        return Err(ControlServerError::UnsafePath(
            "control socket path contains NUL".to_string(),
        ));
    }
    Ok(bytes)
}

fn peer_uid(fd: RawFd) -> Option<u32> {
    let mut credentials = unsafe {
        // SAFETY: zeroed storage is valid for the Linux ucred output struct.
        std::mem::zeroed::<libc::ucred>()
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        // SAFETY: `fd` is the accepted socket and the credential buffers are
        // valid writable storage of the sizes supplied to getsockopt.
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    (result == 0).then_some(credentials.uid)
}

fn effective_uid() -> u32 {
    unsafe {
        // SAFETY: geteuid has no preconditions and does not dereference a
        // caller-provided pointer.
        libc::geteuid()
    }
}

#[cfg(test)]
pub(crate) fn peer_uid_matches(peer_uid: u32, owner_uid: u32) -> bool {
    peer_uid == owner_uid
}
