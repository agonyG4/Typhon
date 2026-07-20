use std::{
    collections::{HashMap, VecDeque},
    io,
    io::IoSlice,
    os::fd::{AsRawFd, RawFd},
    os::unix::net::UnixStream,
    sync::Mutex,
};

use x11rb::{
    connection::{
        BufWithFds, Connection, DiscardMode, ReplyOrError, RequestConnection, RequestKind,
    },
    cookie::{Cookie, CookieWithFds, VoidCookie},
    errors::{ConnectionError, ParseError, ReplyOrIdError},
    protocol::{Event, xproto::Setup},
    rust_connection::{DefaultStream, PollMode, RustConnection, Stream},
    utils::RawFdContainer,
    x11_utils::{ExtensionInformation, TryParse, TryParseFd, X11Error},
};
use x11rb_protocol::{RawEventAndSeqNumber, SequenceNumber};

use super::{
    Xwm, XwmStartupError, atoms::XwmAtoms, capabilities::XwmCapabilities, window::X11WindowRegistry,
};
use crate::xwayland::XwaylandGeneration;

pub(crate) const MAX_XWM_OUTPUT_BYTES: usize = 1024 * 1024;

/// The stream used by the reactor-owned XWM connection.
///
/// `DefaultStream` sets the Unix socket to `O_NONBLOCK`, but its `poll` method
/// deliberately waits forever.  This wrapper changes only that policy and
/// adds a bounded transport queue so x11rb's request machinery cannot spin on
/// a full socket or block the compositor thread.
#[derive(Debug)]
pub(crate) struct ReactorStream {
    inner: DefaultStream,
    queued_output: Mutex<VecDeque<u8>>,
}

impl ReactorStream {
    pub(crate) fn from_unix_stream(stream: UnixStream) -> io::Result<Self> {
        let (inner, _) = DefaultStream::from_unix_stream(stream)?;
        let flags = unsafe { libc::fcntl(inner.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if flags & libc::O_NONBLOCK == 0 {
            return Err(io::Error::other("XWM socket is not nonblocking"));
        }
        Ok(Self {
            inner,
            queued_output: Mutex::new(VecDeque::new()),
        })
    }

    pub(crate) fn wants_writable(&self) -> bool {
        !self
            .queued_output
            .lock()
            .expect("XWM output mutex poisoned")
            .is_empty()
    }

    pub(crate) fn flush_pending(&self) -> io::Result<bool> {
        let mut queued = self
            .queued_output
            .lock()
            .expect("XWM output mutex poisoned");
        while let Some(byte) = queued.front().copied() {
            let mut scratch = [0u8; 16 * 1024];
            let count = scratch.len().min(queued.len());
            for (slot, value) in scratch[..count].iter_mut().zip(queued.iter().take(count)) {
                *slot = *value;
            }
            match self.inner.write(&scratch[..count], &mut Vec::new()) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "XWM output closed",
                    ));
                }
                Ok(written) => {
                    for _ in 0..written {
                        queued.pop_front();
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
            if queued.front().copied() == Some(byte) && count != 0 {
                // The write path must make progress when it reported bytes.
                break;
            }
        }
        Ok(!queued.is_empty())
    }

    fn queue(&self, bytes: &[u8]) -> io::Result<()> {
        let mut queued = self
            .queued_output
            .lock()
            .expect("XWM output mutex poisoned");
        if queued.len().saturating_add(bytes.len()) > MAX_XWM_OUTPUT_BYTES {
            return Err(io::Error::other("XWM output queue exceeded its hard bound"));
        }
        queued.extend(bytes);
        Ok(())
    }
}

impl AsRawFd for ReactorStream {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl Stream for ReactorStream {
    fn poll(&self, mode: PollMode) -> io::Result<()> {
        let _ = self.flush_pending()?;

        // A bounded in-process queue is writable until it reaches its limit.
        // This lets x11rb accept a request without retrying forever while the
        // reactor waits for the next EPOLLOUT notification.
        if mode.writable()
            && self
                .queued_output
                .lock()
                .expect("XWM output mutex poisoned")
                .len()
                < MAX_XWM_OUTPUT_BYTES
        {
            return Ok(());
        }

        let mut events = 0;
        if mode.readable() {
            events |= libc::POLLIN;
        }
        if mode.writable() {
            events |= libc::POLLOUT;
        }
        let mut pollfd = libc::pollfd {
            fd: self.as_raw_fd(),
            events,
            revents: 0,
        };
        // SAFETY: `pollfd` is initialized and points to one valid descriptor.
        let result = unsafe { libc::poll(&mut pollfd, 1, 0) };
        if result > 0 {
            Ok(())
        } else if result == 0 {
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "XWM socket is not ready",
            ))
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn read(&self, buffer: &mut [u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.inner.read(buffer, fds)
    }

    fn write(&self, buffer: &[u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        if !fds.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "XWM transport does not support ancillary descriptors",
            ));
        }
        let _ = self.flush_pending()?;
        match self.inner.write(buffer, fds) {
            Ok(written) if written == buffer.len() => Ok(written),
            Ok(written) => {
                self.queue(&buffer[written..])?;
                Ok(buffer.len())
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                self.queue(buffer)?;
                Ok(buffer.len())
            }
            Err(error) => Err(error),
        }
    }

    fn write_vectored(
        &self,
        buffers: &[IoSlice<'_>],
        fds: &mut Vec<RawFdContainer>,
    ) -> io::Result<usize> {
        let total = buffers.iter().map(|buffer| buffer.len()).sum::<usize>();
        let mut flattened = Vec::with_capacity(total);
        for buffer in buffers {
            flattened.extend_from_slice(buffer);
        }
        self.write(&flattened, fds)
    }
}

/// A small adapter that keeps x11rb's generated request/reply types while
/// supplying extension information discovered by the incremental handshake.
#[derive(Debug)]
pub(crate) struct X11Connection {
    inner: RustConnection<ReactorStream>,
    extensions: HashMap<&'static str, ExtensionInformation>,
}

impl X11Connection {
    pub(crate) fn new(
        inner: RustConnection<ReactorStream>,
        extensions: HashMap<&'static str, ExtensionInformation>,
    ) -> Self {
        Self { inner, extensions }
    }

    pub(crate) fn stream(&self) -> &ReactorStream {
        self.inner.stream()
    }

    pub(crate) fn setup(&self) -> &Setup {
        self.inner.setup()
    }

    pub(crate) fn set_extensions(
        &mut self,
        extensions: HashMap<&'static str, ExtensionInformation>,
    ) {
        self.extensions = extensions;
    }
}

impl RequestConnection for X11Connection {
    type Buf = Vec<u8>;

    fn send_request_with_reply<R>(
        &self,
        bufs: &[IoSlice<'_>],
        fds: Vec<RawFdContainer>,
    ) -> Result<Cookie<'_, Self, R>, ConnectionError>
    where
        R: TryParse,
    {
        let cookie = self.inner.send_request_with_reply::<R>(bufs, fds)?;
        let sequence = cookie.sequence_number();
        std::mem::forget(cookie);
        Ok(Cookie::new(self, sequence))
    }

    fn send_request_with_reply_with_fds<R>(
        &self,
        bufs: &[IoSlice<'_>],
        fds: Vec<RawFdContainer>,
    ) -> Result<CookieWithFds<'_, Self, R>, ConnectionError>
    where
        R: TryParseFd,
    {
        let cookie = self
            .inner
            .send_request_with_reply_with_fds::<R>(bufs, fds)?;
        let sequence = cookie.sequence_number();
        std::mem::forget(cookie);
        Ok(CookieWithFds::new(self, sequence))
    }

    fn send_request_without_reply(
        &self,
        bufs: &[IoSlice<'_>],
        fds: Vec<RawFdContainer>,
    ) -> Result<VoidCookie<'_, Self>, ConnectionError> {
        let cookie = self.inner.send_request_without_reply(bufs, fds)?;
        let sequence = cookie.sequence_number();
        std::mem::forget(cookie);
        Ok(VoidCookie::new(self, sequence))
    }

    fn discard_reply(&self, sequence: SequenceNumber, kind: RequestKind, mode: DiscardMode) {
        self.inner.discard_reply(sequence, kind, mode);
    }

    fn prefetch_extension_information(
        &self,
        _extension_name: &'static str,
    ) -> Result<(), ConnectionError> {
        Ok(())
    }

    fn extension_information(
        &self,
        extension_name: &'static str,
    ) -> Result<Option<ExtensionInformation>, ConnectionError> {
        Ok(self.extensions.get(extension_name).copied())
    }

    fn wait_for_reply_or_raw_error(
        &self,
        sequence: SequenceNumber,
    ) -> Result<ReplyOrError<Self::Buf>, ConnectionError> {
        self.inner.wait_for_reply_or_raw_error(sequence)
    }

    fn wait_for_reply(
        &self,
        sequence: SequenceNumber,
    ) -> Result<Option<Self::Buf>, ConnectionError> {
        self.inner.wait_for_reply(sequence)
    }

    fn wait_for_reply_with_fds_raw(
        &self,
        sequence: SequenceNumber,
    ) -> Result<ReplyOrError<BufWithFds<Self::Buf>, Self::Buf>, ConnectionError> {
        self.inner.wait_for_reply_with_fds_raw(sequence)
    }

    fn check_for_raw_error(
        &self,
        sequence: SequenceNumber,
    ) -> Result<Option<Self::Buf>, ConnectionError> {
        self.inner.check_for_raw_error(sequence)
    }

    fn prefetch_maximum_request_bytes(&self) {
        self.inner.prefetch_maximum_request_bytes();
    }

    fn maximum_request_bytes(&self) -> usize {
        self.inner.maximum_request_bytes()
    }

    fn parse_error(&self, error: &[u8]) -> Result<X11Error, ParseError> {
        self.inner.parse_error(error)
    }

    fn parse_event(&self, event: &[u8]) -> Result<Event, ParseError> {
        self.inner.parse_event(event)
    }
}

impl Connection for X11Connection {
    fn generate_id(&self) -> Result<u32, ReplyOrIdError> {
        self.inner.generate_id()
    }

    fn wait_for_raw_event_with_sequence(
        &self,
    ) -> Result<RawEventAndSeqNumber<Self::Buf>, ConnectionError> {
        self.inner.wait_for_raw_event_with_sequence()
    }

    fn poll_for_raw_event_with_sequence(
        &self,
    ) -> Result<Option<RawEventAndSeqNumber<Self::Buf>>, ConnectionError> {
        self.inner.poll_for_raw_event_with_sequence()
    }

    fn flush(&self) -> Result<(), ConnectionError> {
        self.inner.flush()
    }

    fn setup(&self) -> &Setup {
        self.inner.setup()
    }
}

pub(crate) fn connect(
    generation: XwaylandGeneration,
    stream: UnixStream,
) -> Result<Xwm, XwmStartupError> {
    let stream = ReactorStream::from_unix_stream(stream)
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    let raw_fd = stream.as_raw_fd();
    let connection =
        RustConnection::connect_to_stream(stream, 0).map_err(XwmStartupError::Connection)?;
    let screen_number = 0;
    let root = connection
        .setup()
        .roots
        .get(screen_number)
        .ok_or(XwmStartupError::InvalidScreen)?
        .root;
    // This compatibility path is retained for tests and callers that already
    // have a completed handshake. Managed startup uses `startup::XwmStartup`.
    let capabilities = XwmCapabilities::discover(&connection)?;
    let atoms = XwmAtoms::intern(&connection)?;
    let supporting_wm_check = super::startup::setup_root(&connection, root, &atoms, &capabilities)?;
    connection
        .flush()
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    let connection = X11Connection::new(connection, HashMap::new());
    Ok(Xwm {
        generation,
        connection,
        adoption: Default::default(),
        screen_number,
        root,
        atoms,
        capabilities,
        windows: X11WindowRegistry::default(),
        outgoing_events: Default::default(),
        association: super::association::SurfaceAssociationJoin::default(),
        resize_sync: super::resize_sync::ResizeSyncTracker::default(),
        sync_alarms: Default::default(),
        sync_handles_by_counter: Default::default(),
        next_resize_counter_values: Default::default(),
        shapes: Default::default(),
        data_bridge: Default::default(),
        randr: super::startup::default_randr_snapshot(),
        pending_properties: Default::default(),
        deferred_properties: Default::default(),
        property_metrics: Default::default(),
        buffer_ready_surfaces: Default::default(),
        supporting_wm_check,
        raw_fd,
    })
}

#[cfg(test)]
mod tests {
    use std::{io::Write, os::unix::net::UnixStream};

    use super::*;

    #[test]
    fn reactor_stream_is_nonblocking_before_any_x11_request() {
        let (stream, _peer) = UnixStream::pair().expect("socket pair");
        let stream = ReactorStream::from_unix_stream(stream).expect("nonblocking stream");
        let flags = unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_GETFL) };
        assert_ne!(flags & libc::O_NONBLOCK, 0);
        assert!(matches!(
            Stream::poll(&stream, PollMode::Readable),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock
        ));
    }

    #[test]
    fn pure_epollout_flushes_managed_output() {
        let (stream, _peer) = UnixStream::pair().expect("socket pair");
        let stream = ReactorStream::from_unix_stream(stream).expect("reactor stream");
        stream.queue(b"pending XWM output").expect("queue output");
        assert!(stream.wants_writable());
        assert!(!stream.flush_pending().expect("flush output"));
        assert!(!stream.wants_writable());
    }

    #[test]
    fn writable_interest_is_removed_after_drain() {
        let (stream, _peer) = UnixStream::pair().expect("socket pair");
        let stream = ReactorStream::from_unix_stream(stream).expect("reactor stream");
        stream.queue(b"one bounded request").expect("queue output");
        assert!(stream.wants_writable());
        stream.flush_pending().expect("drain output");
        assert!(!stream.wants_writable());
    }

    #[test]
    fn epollout_without_pending_output_does_not_spin() {
        let (stream, _peer) = UnixStream::pair().expect("socket pair");
        let stream = ReactorStream::from_unix_stream(stream).expect("reactor stream");
        assert!(!stream.flush_pending().expect("empty flush"));
        assert!(!stream.wants_writable());
        assert!(matches!(Stream::poll(&stream, PollMode::Writable), Ok(())));
    }

    #[test]
    fn readable_hup_drains_before_failure() {
        let (stream, mut peer) = UnixStream::pair().expect("socket pair");
        let stream = ReactorStream::from_unix_stream(stream).expect("reactor stream");
        peer.write_all(b"final X11 event").expect("event");
        let mut bytes = [0u8; 32];
        let read = stream
            .read(&mut bytes, &mut Vec::new())
            .expect("drain event");
        assert_eq!(&bytes[..read], b"final X11 event");
    }
}
