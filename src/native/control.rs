//! Native, nonblocking control socket ownership and client state machines.

use std::{
    collections::{HashMap, HashSet},
    fmt, fs, io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, MetadataExt, PermissionsExt},
        },
    },
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use crate::control::{
    ControlCodecError, ControlError, ControlErrorCode, ControlRequest, ControlResponse,
    MAX_REQUEST_BYTES, decode_request, encode_response,
};

use super::event_loop::{
    ControlReadyEvent, NativeEventLoop, NativeEventSource, ReactorToken, monotonic_now_ns,
};

pub const MAX_CONTROL_CLIENTS: usize = 32;
pub const MAX_CONTROL_OPERATIONS_PER_CYCLE: usize = 16;
pub const CONTROL_REQUEST_IDLE_TIMEOUT_NS: u64 = 10_000_000_000;
pub const CONTROL_RESPONSE_IDLE_TIMEOUT_NS: u64 = 10_000_000_000;

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
const MAX_TEMPORARY_SOCKET_ATTEMPTS: usize = 16;

#[cfg(test)]
static REMOVE_INSTANCE_DIRECTORY_AFTER_LOCK: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
#[cfg(test)]
static FORCED_TEMPORARY_COLLISIONS: OnceLock<Mutex<Option<(PathBuf, usize)>>> = OnceLock::new();

#[derive(Debug)]
pub enum ControlServerError {
    InvalidRuntimeDirectory(String),
    InvalidInstance(String),
    UnsafePath(String),
    SocketInUse(PathBuf),
    InstanceLocked(PathBuf),
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
            Self::InstanceLocked(path) => {
                write!(
                    formatter,
                    "control instance is already locked: {}",
                    path.display()
                )
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

    pub fn lock_path(&self) -> PathBuf {
        self.socket_dir.join("control.lock")
    }

    #[cfg(test)]
    pub(crate) fn socket_dir_for_test(&self) -> &Path {
        &self.socket_dir
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
struct BoundSocketGuard {
    path: PathBuf,
    identity: Option<SocketIdentity>,
    armed: bool,
}

impl BoundSocketGuard {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            identity: None,
            armed: true,
        }
    }

    fn set_identity(&mut self, identity: SocketIdentity) {
        self.identity = Some(identity);
    }

    fn set_path(&mut self, path: &Path) {
        self.path = path.to_path_buf();
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for BoundSocketGuard {
    fn drop(&mut self) {
        if self.armed
            && let Some(identity) = self.identity
        {
            remove_socket_if_identity(&self.path, identity);
        }
    }
}

#[derive(Debug)]
struct CreatedListener {
    fd: OwnedFd,
    identity: SocketIdentity,
    cleanup: BoundSocketGuard,
}

#[derive(Debug)]
pub struct NativeControlServer {
    listener: Option<OwnedFd>,
    _instance_lock: InstanceLock,
    listener_token: ReactorToken,
    clients: HashMap<ReactorToken, ControlClient>,
    paths: ControlRuntimePaths,
    owner_uid: u32,
    socket_identity: SocketIdentity,
    counters: ControlServerCounters,
    shut_down: bool,
}

#[derive(Debug)]
struct InstanceLock {
    _fd: OwnedFd,
    path: PathBuf,
}

static HELD_INSTANCE_LOCKS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

impl Drop for InstanceLock {
    fn drop(&mut self) {
        if let Some(locks) = HELD_INSTANCE_LOCKS.get()
            && let Ok(mut locks) = locks.lock()
        {
            locks.remove(&self.path);
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ControlServerCounters {
    pub accepted: u64,
    pub unauthorized: u64,
    pub capacity_rejected: u64,
    pub registration_rejected: u64,
    pub malformed: u64,
    pub oversized: u64,
    pub request_timeouts: u64,
    pub response_timeouts: u64,
    pub stale_tokens: u64,
    pub client_io_failures: u64,
}

#[derive(Debug)]
struct ControlClient {
    fd: OwnedFd,
    input: Vec<u8>,
    output: Vec<u8>,
    written: usize,
    state: ControlClientState,
    peer_write_closed: bool,
    last_progress_ns: u64,
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
        let instance_lock = acquire_instance_lock(&paths.lock_path(), owner_uid)?;
        #[cfg(test)]
        if remove_instance_directory_after_lock_requested(&paths.socket_dir) {
            fs::remove_dir_all(&paths.socket_dir)?;
        }
        remove_stale_socket(&paths, owner_uid)?;

        let mut temporary_attempts = 0;
        let CreatedListener {
            fd: listener,
            identity: socket_identity,
            cleanup: mut bound_socket,
        } = loop {
            match create_listener(&paths, owner_uid) {
                Ok(listener) => break listener,
                Err(ControlServerError::SocketInUse(path)) if path == *paths.socket_path() => {
                    remove_stale_socket(&paths, owner_uid)?;
                }
                Err(ControlServerError::Io(error))
                    if error.raw_os_error() == Some(libc::EADDRINUSE) =>
                {
                    temporary_attempts += 1;
                    if temporary_attempts >= MAX_TEMPORARY_SOCKET_ATTEMPTS {
                        return Err(ControlServerError::ListenerFailure(
                            "temporary control socket collision limit exhausted".to_string(),
                        ));
                    }
                }
                Err(ControlServerError::Io(error))
                    if error.raw_os_error() == Some(libc::ENOENT) =>
                {
                    return Err(ControlServerError::ListenerFailure(format!(
                        "control instance directory disappeared during listener setup: {error}"
                    )));
                }
                Err(error) => return Err(error),
            }
        };
        let listener_token = match event_loop.register_with_events(
            listener.as_raw_fd(),
            NativeEventSource::ControlListener,
            READ_EVENTS,
        ) {
            Ok(token) => token,
            Err(error) => {
                remove_socket_if_identity(&paths.socket_path, socket_identity);
                return Err(ControlServerError::ListenerFailure(format!(
                    "listener registration: {error}"
                )));
            }
        };

        eprintln!(
            "typhon control: listener_created instance={} mode=0600",
            instance
        );
        bound_socket.disarm();
        Ok(Self {
            listener: Some(listener),
            _instance_lock: instance_lock,
            listener_token,
            clients: HashMap::new(),
            paths,
            owner_uid,
            socket_identity,
            counters: ControlServerCounters::default(),
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

    pub fn counters(&self) -> ControlServerCounters {
        self.counters
    }

    #[cfg(test)]
    pub(crate) fn captured_socket_identity(&self) -> (u64, u64) {
        (self.socket_identity.device, self.socket_identity.inode)
    }

    pub fn next_deadline_ns(&self) -> Option<u64> {
        self.clients
            .values()
            .filter_map(ControlClient::next_deadline_ns)
            .min()
    }

    pub fn expire_idle_clients(
        &mut self,
        event_loop: &mut NativeEventLoop,
        now_ns: u64,
        budget: usize,
    ) {
        let expired = self
            .clients
            .iter()
            .filter(|(_, client)| {
                client
                    .next_deadline_ns()
                    .is_some_and(|deadline| deadline <= now_ns)
            })
            .map(|(token, client)| (*token, client.state))
            .take(budget.min(MAX_CONTROL_OPERATIONS_PER_CYCLE))
            .collect::<Vec<_>>();
        for (token, state) in expired {
            match state {
                ControlClientState::Reading => {
                    self.counters.request_timeouts =
                        self.counters.request_timeouts.saturating_add(1)
                }
                ControlClientState::Writing => {
                    self.counters.response_timeouts =
                        self.counters.response_timeouts.saturating_add(1)
                }
                ControlClientState::AwaitingResponse => {
                    self.counters.response_timeouts =
                        self.counters.response_timeouts.saturating_add(1)
                }
            }
            self.remove_client(event_loop, token);
        }
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
                    self.counters.stale_tokens = self.counters.stale_tokens.saturating_add(1);
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
                    if !event_loop
                        .modify(event.token, WAITING_EVENTS)
                        .unwrap_or(false)
                    {
                        self.counters.client_io_failures =
                            self.counters.client_io_failures.saturating_add(1);
                        self.remove_client(event_loop, event.token);
                        continue;
                    }
                    pending.push((event.token, request));
                }
                ClientAction::Response(response) => {
                    if let Some(error) = response.error.as_ref() {
                        match error.code {
                            ControlErrorCode::MalformedJson => {
                                self.counters.malformed = self.counters.malformed.saturating_add(1)
                            }
                            ControlErrorCode::RequestTooLarge => {
                                self.counters.oversized = self.counters.oversized.saturating_add(1)
                            }
                            _ => {}
                        }
                    }
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
            Err(error) => {
                if matches!(error, ControlCodecError::ResponseTooLarge) {
                    self.counters.oversized = self.counters.oversized.saturating_add(1);
                }
                encode_response(&ControlResponse::failure(
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
                })?
            }
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
        client.last_progress_ns = monotonic_now_ns().unwrap_or(client.last_progress_ns);
        if !event_loop.modify(token, WRITE_EVENTS).unwrap_or(false) {
            self.counters.client_io_failures = self.counters.client_io_failures.saturating_add(1);
            self.remove_client(event_loop, token);
        }
        Ok(())
    }

    pub fn shutdown(&mut self, event_loop: &mut NativeEventLoop) -> Result<(), ControlServerError> {
        if self.shut_down {
            return Ok(());
        }
        let tokens = self.clients.keys().copied().collect::<Vec<_>>();
        let mut first_error = None;
        for token in tokens {
            if let Err(error) = event_loop.unregister(token) {
                first_error.get_or_insert(error);
            }
        }
        self.clients.clear();
        if let Err(error) = event_loop.unregister(self.listener_token) {
            first_error.get_or_insert(error);
        }
        self.listener.take();
        self.shut_down = true;
        remove_socket_if_identity(&self.paths.socket_path, self.socket_identity);
        first_error.map_or(Ok(()), |error| Err(error.into()))
    }

    fn accept_one(&mut self, event_loop: &mut NativeEventLoop) -> Result<(), ControlServerError> {
        let fd = loop {
            let fd = unsafe {
                // SAFETY: `self.listener` is a live listening socket owned by
                // this server, and the flags request nonblocking CLOEXEC I/O.
                libc::accept4(
                    self.listener.as_ref().expect("live listener").as_raw_fd(),
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
                Some(libc::EBADF | libc::EINVAL | libc::ENOTSOCK) => {
                    return Err(ControlServerError::ListenerFailure(error.to_string()));
                }
                Some(
                    libc::ECONNABORTED
                    | libc::EPROTO
                    | libc::EMFILE
                    | libc::ENFILE
                    | libc::ENOBUFS
                    | libc::ENOMEM,
                ) => return Ok(()),
                _ => return Ok(()),
            }
        };
        let client_fd = unsafe {
            // SAFETY: `accept4` returned a new owned descriptor.
            OwnedFd::from_raw_fd(fd)
        };
        let peer_uid = peer_uid(client_fd.as_raw_fd());
        if peer_uid != Some(self.owner_uid) {
            self.counters.unauthorized = self.counters.unauthorized.saturating_add(1);
            send_best_effort_unauthorized(client_fd.as_raw_fd());
            return Ok(());
        }
        if self.clients.len() >= MAX_CONTROL_CLIENTS {
            self.counters.capacity_rejected = self.counters.capacity_rejected.saturating_add(1);
            return Ok(());
        }
        let token = match event_loop.register_with_events(
            client_fd.as_raw_fd(),
            NativeEventSource::ControlClient,
            READ_EVENTS,
        ) {
            Ok(token) => token,
            Err(_) => {
                self.counters.registration_rejected =
                    self.counters.registration_rejected.saturating_add(1);
                return Ok(());
            }
        };
        let now_ns = monotonic_now_ns().unwrap_or(0);
        self.clients.insert(
            token,
            ControlClient {
                fd: client_fd,
                input: Vec::with_capacity(MAX_REQUEST_BYTES),
                output: Vec::new(),
                written: 0,
                state: ControlClientState::Reading,
                peer_write_closed: false,
                last_progress_ns: now_ns,
            },
        );
        self.counters.accepted = self.counters.accepted.saturating_add(1);
        Ok(())
    }

    fn remove_client(&mut self, event_loop: &mut NativeEventLoop, token: ReactorToken) {
        let _ = event_loop.unregister(token);
        if self.clients.remove(&token).is_some() {}
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
    fn next_deadline_ns(&self) -> Option<u64> {
        let timeout = match self.state {
            ControlClientState::Reading => CONTROL_REQUEST_IDLE_TIMEOUT_NS,
            ControlClientState::AwaitingResponse | ControlClientState::Writing => {
                CONTROL_RESPONSE_IDLE_TIMEOUT_NS
            }
        };
        Some(self.last_progress_ns.saturating_add(timeout))
    }

    fn advance(&mut self, flags: u32) -> ClientAction {
        let terminal = flags & TERMINAL_EVENTS != 0;
        if flags & libc::EPOLLERR as u32 != 0 {
            let mut socket_error = 0i32;
            let mut length = std::mem::size_of::<i32>() as libc::socklen_t;
            let _ = unsafe {
                // SAFETY: this is a live client socket and both output values
                // point to valid writable storage for SO_ERROR.
                libc::getsockopt(
                    self.fd.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_ERROR,
                    (&mut socket_error as *mut i32).cast(),
                    &mut length,
                )
            };
        }
        match self.state {
            ControlClientState::Reading if flags & libc::EPOLLIN as u32 != 0 || terminal => {
                self.read_once(terminal)
            }
            ControlClientState::Writing if flags & libc::EPOLLOUT as u32 != 0 || terminal => {
                self.write_once()
            }
            _ => ClientAction::None,
        }
    }

    fn read_once(&mut self, terminal: bool) -> ClientAction {
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
            self.last_progress_ns = monotonic_now_ns().unwrap_or(self.last_progress_ns);
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
            self.peer_write_closed = true;
            return if self.input.is_empty() {
                ClientAction::Close
            } else {
                ClientAction::Response(ControlResponse::failure(
                    0,
                    ControlError::new(
                        ControlErrorCode::InvalidRequest,
                        "control request ended before a newline",
                    ),
                ))
            };
        }
        match io::Error::last_os_error().raw_os_error() {
            Some(libc::EAGAIN | libc::EINTR) => ClientAction::None,
            Some(libc::EPIPE | libc::ECONNRESET) => ClientAction::Close,
            _ if terminal && !self.input.is_empty() => {
                ClientAction::Response(ControlResponse::failure(
                    0,
                    ControlError::new(
                        ControlErrorCode::InvalidRequest,
                        "control request could not be read completely",
                    ),
                ))
            }
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
            libc::send(
                self.fd.as_raw_fd(),
                remaining.as_ptr().cast(),
                remaining.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        if written > 0 {
            self.written += written as usize;
            self.last_progress_ns = monotonic_now_ns().unwrap_or(self.last_progress_ns);
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
        libc::send(fd, bytes.as_ptr().cast(), bytes.len(), libc::MSG_NOSIGNAL)
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
    if instance == "." || instance == ".." || instance.contains("..") {
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
            match fs::create_dir(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let metadata = fs::symlink_metadata(path).map_err(|error| {
                        ControlServerError::UnsafePath(format!(
                            "inspect {}: {error}",
                            path.display()
                        ))
                    })?;
                    return verify_owned_directory(path, &metadata, owner_uid);
                }
                Err(error) => {
                    return Err(ControlServerError::UnsafePath(format!(
                        "create {}: {error}",
                        path.display()
                    )));
                }
            }
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
    let identity = SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    match probe_existing_socket(paths.socket_path())? {
        SocketProbe::Live => Err(ControlServerError::SocketInUse(
            paths.socket_path().to_path_buf(),
        )),
        SocketProbe::Dead => {
            let revalidated = fs::symlink_metadata(paths.socket_path())?;
            if !revalidated.file_type().is_socket()
                || revalidated.uid() != owner_uid
                || revalidated.dev() != identity.device
                || revalidated.ino() != identity.inode
            {
                return Err(ControlServerError::UnsafePath(
                    "socket changed during stale cleanup".to_string(),
                ));
            }
            remove_socket_if_identity(paths.socket_path(), identity);
            if paths.socket_path().exists() {
                return Err(ControlServerError::UnsafePath(
                    "stale socket changed during removal".to_string(),
                ));
            }
            Ok(())
        }
    }
}

fn acquire_instance_lock(path: &Path, owner_uid: u32) -> Result<InstanceLock, ControlServerError> {
    let bytes = path_bytes(path)?;
    let path_c = std::ffi::CString::new(bytes)
        .map_err(|_| ControlServerError::UnsafePath("lock path contains NUL".to_string()))?;
    let fd = unsafe {
        // SAFETY: `path_c` is a validated NUL-terminated path and the flags
        // request a new non-following close-on-exec descriptor.
        libc::open(
            path_c.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        let error = io::Error::last_os_error();
        if matches!(error.raw_os_error(), Some(libc::ELOOP)) {
            return Err(ControlServerError::UnsafePath(format!(
                "lock path is a symlink: {}",
                path.display()
            )));
        }
        return Err(error.into());
    }
    let lock = unsafe {
        // SAFETY: `open` returned a new owned descriptor.
        OwnedFd::from_raw_fd(fd)
    };
    let mut metadata = unsafe { std::mem::zeroed::<libc::stat>() };
    let result = unsafe {
        // SAFETY: `lock` is live and `metadata` is valid writable stat storage.
        libc::fstat(lock.as_raw_fd(), &mut metadata)
    };
    if result < 0 {
        return Err(io::Error::last_os_error().into());
    }
    if metadata.st_uid != owner_uid
        || metadata.st_mode & libc::S_IFMT != libc::S_IFREG
        || metadata.st_mode & 0o777 != 0o600
    {
        return Err(ControlServerError::UnsafePath(format!(
            "unsafe instance lock: {}",
            path.display()
        )));
    }
    let result = unsafe {
        // SAFETY: `lock` is a valid regular-file descriptor and flock only
        // changes its kernel lock state.
        libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)
    };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Err(ControlServerError::InstanceLocked(path.to_path_buf()));
        }
        return Err(error.into());
    }
    let held_locks = HELD_INSTANCE_LOCKS.get_or_init(|| Mutex::new(HashSet::new()));
    let mut held_locks = held_locks.lock().map_err(|_| {
        ControlServerError::UnsafePath("instance lock registry poisoned".to_string())
    })?;
    if !held_locks.insert(path.to_path_buf()) {
        return Err(ControlServerError::InstanceLocked(path.to_path_buf()));
    }
    drop(held_locks);
    Ok(InstanceLock {
        _fd: lock,
        path: path.to_path_buf(),
    })
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

fn create_listener(
    paths: &ControlRuntimePaths,
    owner_uid: u32,
) -> Result<CreatedListener, ControlServerError> {
    #[cfg(test)]
    if forced_temporary_collision_requested(&paths.socket_dir) {
        return Err(io::Error::from_raw_os_error(libc::EADDRINUSE).into());
    }
    let temporary_path = temporary_socket_path(&paths.socket_dir)?;
    let (address, address_len) = unix_socket_address(&temporary_path)?;
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
        // SAFETY: `address` is initialized and names only a unique pathname
        // inside the already validated private instance directory.
        libc::bind(
            listener.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            address_len,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let mut bound_socket = BoundSocketGuard::new(&temporary_path);
    let identity = socket_path_identity(&temporary_path)?;
    bound_socket.set_identity(identity);
    set_socket_mode(&temporary_path, owner_uid, identity)?;
    let result = unsafe {
        // SAFETY: `listener` is the socket just successfully bound above.
        libc::listen(listener.as_raw_fd(), LISTEN_BACKLOG)
    };
    if result < 0 {
        return Err(io::Error::last_os_error().into());
    }

    match rename_noreplace(&temporary_path, paths.socket_path()) {
        Ok(()) => {}
        Err(error) if error.raw_os_error() == Some(libc::EEXIST) => {
            return Err(ControlServerError::SocketInUse(
                paths.socket_path().to_path_buf(),
            ));
        }
        Err(error) => return Err(error.into()),
    }
    if let Err(error) = verify_socket_identity(paths.socket_path(), owner_uid, identity, true) {
        remove_socket_if_identity(paths.socket_path(), identity);
        return Err(error);
    }
    bound_socket.set_path(paths.socket_path());
    Ok(CreatedListener {
        fd: listener,
        identity,
        cleanup: bound_socket,
    })
}

fn set_socket_mode(
    path: &Path,
    owner_uid: u32,
    identity: SocketIdentity,
) -> Result<(), ControlServerError> {
    fs::set_permissions(path, fs::Permissions::from_mode(SOCKET_MODE))?;
    verify_socket_identity(path, owner_uid, identity, true)
}

fn verify_socket_identity(
    path: &Path,
    owner_uid: u32,
    identity: SocketIdentity,
    require_mode: bool,
) -> Result<(), ControlServerError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != owner_uid
        || (require_mode && metadata.mode() & 0o777 != SOCKET_MODE)
        || metadata.dev() != identity.device
        || metadata.ino() != identity.inode
    {
        return Err(ControlServerError::UnsafePath(format!(
            "control socket identity verification failed type={} uid={} mode={:o} dev={} expected_dev={} ino={} expected_ino={}",
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
        let Some(parent) = path.parent() else {
            return;
        };
        for _ in 0..16 {
            let Ok(quarantine) = temporary_socket_path(parent) else {
                return;
            };
            match rename_noreplace(path, &quarantine) {
                Ok(()) => {
                    if socket_path_identity(&quarantine).is_ok_and(|moved| moved == identity) {
                        let _ = fs::remove_file(&quarantine);
                    } else {
                        let _ = rename_noreplace(&quarantine, path);
                    }
                    return;
                }
                Err(error) if error.raw_os_error() == Some(libc::EEXIST) => continue,
                Err(_) => return,
            }
        }
    }
}

fn socket_path_identity(path: &Path) -> Result<SocketIdentity, ControlServerError> {
    let metadata = fs::symlink_metadata(path)?;
    Ok(SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn temporary_socket_path(socket_dir: &Path) -> Result<PathBuf, ControlServerError> {
    // Keep the basename short enough for the Unix pathname limit even when
    // the validated instance name is near the maximum supported length.
    let mut nonce = [0u8; 8];
    let result = unsafe {
        // SAFETY: `nonce` is valid writable storage for exactly its length.
        libc::getrandom(nonce.as_mut_ptr().cast(), nonce.len(), 0)
    };
    if result != nonce.len() as isize {
        return Err(io::Error::last_os_error().into());
    }
    let suffix = nonce
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let preferred = socket_dir.join(format!("control.sock.tmp-{suffix}"));
    let max_path = unsafe {
        // SAFETY: zeroed storage is used only to query the fixed pathname
        // array length for this platform's Unix socket address.
        std::mem::zeroed::<libc::sockaddr_un>().sun_path.len()
    };
    if preferred.as_os_str().as_bytes().len() < max_path {
        Ok(preferred)
    } else {
        Ok(socket_dir.join(format!("t-{suffix}")))
    }
}

#[cfg(test)]
pub(crate) fn temporary_socket_path_for_test(
    socket_dir: &Path,
) -> Result<PathBuf, ControlServerError> {
    temporary_socket_path(socket_dir)
}

#[cfg(test)]
pub(crate) fn remove_instance_directory_after_lock_for_test(socket_dir: &Path) {
    *REMOVE_INSTANCE_DIRECTORY_AFTER_LOCK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = Some(socket_dir.to_path_buf());
}

#[cfg(test)]
pub(crate) fn force_temporary_collisions_for_test(socket_dir: &Path, attempts: usize) {
    *FORCED_TEMPORARY_COLLISIONS
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = Some((socket_dir.to_path_buf(), attempts));
}

#[cfg(test)]
fn remove_instance_directory_after_lock_requested(socket_dir: &Path) -> bool {
    let mut request = REMOVE_INSTANCE_DIRECTORY_AFTER_LOCK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap();
    if request
        .as_ref()
        .is_some_and(|requested| requested == socket_dir)
    {
        request.take();
        true
    } else {
        false
    }
}

#[cfg(test)]
fn forced_temporary_collision_requested(socket_dir: &Path) -> bool {
    let mut request = FORCED_TEMPORARY_COLLISIONS
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap();
    let Some((requested, remaining)) = request.as_mut() else {
        return false;
    };
    if requested != socket_dir || *remaining == 0 {
        return false;
    }
    *remaining -= 1;
    true
}

fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    let from = std::ffi::CString::new(from.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "temporary socket contains NUL")
    })?;
    let to = std::ffi::CString::new(to.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "socket path contains NUL"))?;
    let result = unsafe {
        // SAFETY: both paths are validated NUL-free absolute paths and the
        // syscall only changes the directory entries named by those paths.
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
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
