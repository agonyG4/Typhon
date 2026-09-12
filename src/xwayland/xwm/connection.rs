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

pub(crate) const MAX_XWM_OUTPUT_BYTES: usize = 1024 * 1024;

/// The stream used by the reactor-owned XWM connection.
///
/// `DefaultStream` sets the Unix socket to `O_NONBLOCK`, but its `poll` method
/// deliberately waits forever.  This wrapper changes only that policy and
/// adds a bounded transport queue so x11rb's request machinery cannot spin on
/// a full socket or block the compositor thread.
#[derive(Debug)]
pub(crate) struct ReactorStream<S = DefaultStream> {
    inner: S,
    // Serializes every socket write with acceptance into the single output FIFO.
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
}

impl<S: Stream> ReactorStream<S> {
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
        self.flush_queued(&mut queued)
    }

    // Caller holds the output lock through draining, direct writes, and suffix acceptance.
    fn flush_queued(&self, queued: &mut VecDeque<u8>) -> io::Result<bool> {
        // The queue is at most 1 MiB and cannot grow under this guard. Each
        // iteration removes positive progress or stops immediately on EAGAIN.
        while !queued.is_empty() {
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
        }
        Ok(!queued.is_empty())
    }

    #[cfg(test)]
    fn queue(&self, bytes: &[u8]) -> io::Result<()> {
        let mut queued = self
            .queued_output
            .lock()
            .expect("XWM output mutex poisoned");
        Self::append_queued(&mut queued, bytes)
    }

    fn append_queued(queued: &mut VecDeque<u8>, bytes: &[u8]) -> io::Result<()> {
        if queued.len().saturating_add(bytes.len()) > MAX_XWM_OUTPUT_BYTES {
            return Err(io::Error::other("XWM output queue exceeded its hard bound"));
        }
        queued.extend(bytes);
        Ok(())
    }
}

impl<S: AsRawFd> AsRawFd for ReactorStream<S> {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl<S: Stream + AsRawFd> Stream for ReactorStream<S> {
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
        let mut queued = self
            .queued_output
            .lock()
            .expect("XWM output mutex poisoned");
        if self.flush_queued(&mut queued)? {
            Self::append_queued(&mut queued, buffer)?;
            return Ok(buffer.len());
        }
        match self.inner.write(buffer, fds) {
            Ok(written) if written == buffer.len() => Ok(written),
            Ok(written) => {
                if let Err(error) = Self::append_queued(&mut queued, &buffer[written..]) {
                    // Stream follows Write: an error cannot conceal accepted bytes.
                    // Leave the unaccepted suffix with x11rb for its next write.
                    return if written > 0 { Ok(written) } else { Err(error) };
                }
                Ok(buffer.len())
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                Self::append_queued(&mut queued, buffer)?;
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
    deferred_events: Mutex<VecDeque<RawEventAndSeqNumber<Vec<u8>>>>,
}

impl X11Connection {
    pub(crate) fn new(
        inner: RustConnection<ReactorStream>,
        extensions: HashMap<&'static str, ExtensionInformation>,
    ) -> Self {
        Self {
            inner,
            extensions,
            deferred_events: Mutex::new(VecDeque::new()),
        }
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

    pub(crate) fn defer_raw_event(&self, event: RawEventAndSeqNumber<Vec<u8>>) {
        self.deferred_events
            .lock()
            .expect("XWM deferred-event mutex poisoned")
            .push_back(event);
    }

    pub(crate) fn poll_new_raw_event_with_sequence(
        &self,
    ) -> Result<Option<RawEventAndSeqNumber<Vec<u8>>>, ConnectionError> {
        self.inner.poll_for_raw_event_with_sequence()
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
        if let Some(event) = self
            .deferred_events
            .lock()
            .expect("XWM deferred-event mutex poisoned")
            .pop_front()
        {
            return Ok(Some(event));
        }
        self.inner.poll_for_raw_event_with_sequence()
    }

    fn flush(&self) -> Result<(), ConnectionError> {
        self.inner.flush()
    }

    fn setup(&self) -> &Setup {
        self.inner.setup()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        os::unix::net::UnixStream,
    };

    use super::*;

    #[derive(Debug)]
    struct ScriptedStream {
        // Zero means EAGAIN; positive entries cap the next successful write.
        steps: Mutex<VecDeque<usize>>,
        received: Mutex<Vec<u8>>,
    }

    impl AsRawFd for ScriptedStream {
        fn as_raw_fd(&self) -> RawFd {
            -1
        }
    }

    impl Stream for ScriptedStream {
        fn poll(&self, _: PollMode) -> io::Result<()> {
            panic!("reactor must not call the blocking inner poll")
        }

        fn read(&self, _: &mut [u8], _: &mut Vec<RawFdContainer>) -> io::Result<usize> {
            unreachable!("output-only fixture")
        }

        fn write(&self, bytes: &[u8], _: &mut Vec<RawFdContainer>) -> io::Result<usize> {
            let limit = self.steps.lock().unwrap().pop_front().unwrap_or(usize::MAX);
            if limit == 0 {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let written = bytes.len().min(limit);
            self.received
                .lock()
                .unwrap()
                .extend_from_slice(&bytes[..written]);
            Ok(written)
        }
    }

    fn scripted_stream(steps: VecDeque<usize>) -> ReactorStream<ScriptedStream> {
        ReactorStream {
            inner: ScriptedStream {
                steps: Mutex::new(steps),
                received: Mutex::new(Vec::new()),
            },
            queued_output: Mutex::new(VecDeque::new()),
        }
    }

    #[test]
    fn short_writes_and_eagain_preserve_every_accepted_byte() {
        for offset in 0..16 {
            let stream = scripted_stream((0..128).map(|i| (i + offset) % 7).collect());
            let mut accepted = Vec::new();
            for request in 0..32 {
                let bytes = vec![b'A' + request; 1024 + usize::from(request) * 17];
                assert_eq!(stream.write(&bytes, &mut Vec::new()).unwrap(), bytes.len());
                accepted.extend(bytes);
                let received = stream.inner.received.lock().unwrap();
                assert_eq!(*received, accepted[..received.len()]);
                assert_eq!(stream.wants_writable(), received.len() < accepted.len());
            }
            for _ in 0..128 {
                if !stream.flush_pending().unwrap() {
                    break;
                }
            }
            assert_eq!(*stream.inner.received.lock().unwrap(), accepted);
            assert!(!stream.wants_writable());
        }
    }

    #[test]
    fn exact_capacity_accepts_no_extra_bytes_and_keeps_writable_interest() {
        let stream = scripted_stream(VecDeque::from([0, 0, 0]));
        let bytes = vec![b'A'; MAX_XWM_OUTPUT_BYTES];
        assert_eq!(stream.write(&bytes, &mut Vec::new()).unwrap(), bytes.len());
        assert!(stream.wants_writable());
        assert!(stream.write(b"B", &mut Vec::new()).is_err());
        assert_eq!(
            stream.queued_output.lock().unwrap().len(),
            MAX_XWM_OUTPUT_BYTES
        );
        assert!(
            stream.flush_pending().unwrap(),
            "EAGAIN retains writable interest"
        );
        assert!(stream.inner.received.lock().unwrap().is_empty());
        assert!(!stream.flush_pending().unwrap());
        assert_eq!(*stream.inner.received.lock().unwrap(), bytes);
        assert!(!stream.wants_writable());
    }

    #[test]
    fn older_queued_bytes_cannot_be_overtaken() {
        let (socket, mut peer) = UnixStream::pair().expect("socket pair");
        peer.set_nonblocking(true).expect("nonblocking peer");
        let stream = ReactorStream::from_unix_stream(socket).expect("reactor stream");
        stream.queue(&vec![b'A'; 48 * 1024]).expect("older output");
        assert_eq!(stream.write(b"B", &mut Vec::new()).unwrap(), 1);
        for _ in 0..8 {
            if !stream.flush_pending().unwrap() {
                break;
            }
        }
        let mut received = vec![0; 48 * 1024 + 1];
        peer.read_exact(&mut received).expect("all accepted bytes");
        assert_eq!(
            received.iter().position(|byte| *byte == b'B'),
            Some(48 * 1024)
        );
        assert!(received[..48 * 1024].iter().all(|byte| *byte == b'A'));
        assert!(!stream.wants_writable());
    }

    #[test]
    fn repeated_queued_bytes_are_positive_progress() {
        let (socket, mut peer) = UnixStream::pair().expect("socket pair");
        peer.set_nonblocking(true).expect("nonblocking peer");
        let stream = ReactorStream::from_unix_stream(socket).expect("reactor stream");
        let bytes = vec![b'A'; 48 * 1024];
        stream.queue(&bytes).expect("older output");
        assert!(!stream.flush_pending().expect("drain every positive write"));
        let mut received = vec![0; bytes.len()];
        peer.read_exact(&mut received).expect("all repeated bytes");
        assert_eq!(received, bytes);
        assert!(!stream.wants_writable());
    }

    #[test]
    fn oversized_short_write_reports_the_prefix_already_sent() {
        let (socket, mut peer) = UnixStream::pair().expect("socket pair");
        let capacity: libc::c_int = 4096;
        // Limit the socket below the request size to force a positive short write.
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    socket.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&capacity as *const libc::c_int).cast(),
                    std::mem::size_of_val(&capacity) as libc::socklen_t,
                )
            },
            0
        );
        peer.set_nonblocking(true).expect("nonblocking peer");
        let stream = ReactorStream::from_unix_stream(socket).expect("reactor stream");
        let bytes = vec![b'A'; 2 * MAX_XWM_OUTPUT_BYTES];
        let accepted = stream
            .write(&bytes, &mut Vec::new())
            .expect("a sent prefix must be reported, even if its suffix cannot fit");
        assert!(accepted > 0 && accepted < bytes.len());
        let mut received = vec![0; accepted];
        peer.read_exact(&mut received).expect("accepted prefix");
        assert_eq!(received, bytes[..accepted]);
        assert!(!stream.wants_writable());
    }

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
