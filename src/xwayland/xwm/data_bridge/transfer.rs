use std::{
    collections::HashMap,
    io,
    os::fd::{AsRawFd, OwnedFd, RawFd},
};

use super::BridgeGeneration;

pub const MAX_TRANSFER_CHUNK: usize = 64 * 1024;
pub const MAX_ACTIVE_TRANSFERS: usize = 64;
const MAX_INTERRUPTED_SYSCALLS_PER_PUMP: usize = 8;

pub const TRANSFER_TERMINAL_EVENTS: u32 =
    (libc::EPOLLERR | libc::EPOLLHUP | libc::EPOLLRDHUP) as u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransferId {
    pub generation: BridgeGeneration,
    pub serial: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferPumpOutcome {
    Pending,
    Completed,
    TimedOut,
    Gone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransferPhase {
    Reading,
    Writing,
}

#[derive(Debug)]
struct Transfer {
    id: TransferId,
    source: OwnedFd,
    sink: OwnedFd,
    buffer: Vec<u8>,
    offset: usize,
    phase: TransferPhase,
    deadline_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The single descriptor a transfer needs to watch at its current phase.
pub enum TransferIoInterest {
    SourceReadable,
    SinkWritable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Reactor-facing identity and readiness metadata for one transfer.
pub struct TransferInterest {
    pub id: TransferId,
    pub fd: RawFd,
    pub interest: TransferIoInterest,
    pub events: u32,
    pub deadline_ns: u64,
}

trait TransferIo {
    fn read(&mut self, fd: RawFd, buffer: &mut [u8]) -> io::Result<usize>;
    fn write(&mut self, fd: RawFd, buffer: &[u8]) -> io::Result<usize>;
}

struct SyscallIo;

impl TransferIo for SyscallIo {
    fn read(&mut self, fd: RawFd, buffer: &mut [u8]) -> io::Result<usize> {
        let result = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(result as usize)
        }
    }

    fn write(&mut self, fd: RawFd, buffer: &[u8]) -> io::Result<usize> {
        let result = unsafe { libc::write(fd, buffer.as_ptr().cast(), buffer.len()) };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(result as usize)
        }
    }
}

#[derive(Debug, Default)]
pub struct TransferManager {
    next_serial: u64,
    transfers: HashMap<TransferId, Transfer>,
}

impl TransferManager {
    pub fn start(
        &mut self,
        generation: BridgeGeneration,
        source: OwnedFd,
        sink: OwnedFd,
        deadline_ns: u64,
    ) -> io::Result<TransferId> {
        if self.transfers.len() >= MAX_ACTIVE_TRANSFERS {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "X11 selection transfer capacity is full",
            ));
        }
        set_nonblocking(&source)?;
        set_nonblocking(&sink)?;
        self.next_serial = self.next_serial.saturating_add(1).max(1);
        let id = TransferId {
            generation,
            serial: self.next_serial,
        };
        self.transfers.insert(
            id,
            Transfer {
                id,
                source,
                sink,
                buffer: Vec::with_capacity(MAX_TRANSFER_CHUNK),
                offset: 0,
                phase: TransferPhase::Reading,
                deadline_ns,
            },
        );
        Ok(id)
    }

    pub fn pump(&mut self, id: TransferId, now_ns: u64) -> io::Result<TransferPumpOutcome> {
        let mut io = SyscallIo;
        self.pump_with_io(id, now_ns, &mut io)
    }

    fn pump_with_io<I: TransferIo>(
        &mut self,
        id: TransferId,
        now_ns: u64,
        io: &mut I,
    ) -> io::Result<TransferPumpOutcome> {
        let Some(transfer) = self.transfers.get(&id) else {
            return Ok(TransferPumpOutcome::Gone);
        };
        if transfer.id != id {
            return Ok(TransferPumpOutcome::Gone);
        }
        if now_ns >= transfer.deadline_ns {
            self.transfers.remove(&id);
            return Ok(TransferPumpOutcome::TimedOut);
        }

        let result = self
            .transfers
            .get_mut(&id)
            .expect("transfer remained present after deadline check")
            .pump(io);
        match result {
            Ok(outcome) => {
                if outcome == TransferPumpOutcome::Completed {
                    self.transfers.remove(&id);
                }
                Ok(outcome)
            }
            Err(error) => {
                self.transfers.remove(&id);
                Err(error)
            }
        }
    }

    pub fn interest(&self, id: TransferId) -> Option<TransferInterest> {
        let transfer = self.transfers.get(&id)?;
        let (fd, interest) = match transfer.phase {
            TransferPhase::Reading => (
                transfer.source.as_raw_fd(),
                TransferIoInterest::SourceReadable,
            ),
            TransferPhase::Writing => (transfer.sink.as_raw_fd(), TransferIoInterest::SinkWritable),
        };
        let events = match interest {
            TransferIoInterest::SourceReadable => libc::EPOLLIN as u32,
            TransferIoInterest::SinkWritable => libc::EPOLLOUT as u32,
        } | TRANSFER_TERMINAL_EVENTS;
        Some(TransferInterest {
            id,
            fd,
            interest,
            events,
            deadline_ns: transfer.deadline_ns,
        })
    }

    pub fn next_deadline_ns(&self) -> Option<u64> {
        self.transfers
            .values()
            .map(|transfer| transfer.deadline_ns)
            .min()
    }

    pub fn expire_deadlines(&mut self, now_ns: u64) -> Vec<TransferId> {
        let expired = self
            .transfers
            .iter()
            .filter_map(|(id, transfer)| (now_ns >= transfer.deadline_ns).then_some(*id))
            .collect::<Vec<_>>();
        for id in &expired {
            self.transfers.remove(id);
        }
        expired
    }

    pub fn cancel(&mut self, id: TransferId) -> bool {
        self.transfers.remove(&id).is_some()
    }

    pub fn clear_generation(&mut self, generation: BridgeGeneration) {
        self.transfers.retain(|id, _| id.generation != generation);
    }

    pub fn len(&self) -> usize {
        self.transfers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.transfers.is_empty()
    }
}

impl Transfer {
    fn pump<I: TransferIo>(&mut self, io: &mut I) -> io::Result<TransferPumpOutcome> {
        self.assert_invariants();
        if self.phase == TransferPhase::Reading {
            self.buffer.resize(MAX_TRANSFER_CHUNK, 0);
            let mut interrupted = 0;
            loop {
                match io.read(self.source.as_raw_fd(), &mut self.buffer) {
                    Ok(0) => {
                        self.transition_to_reading();
                        return Ok(TransferPumpOutcome::Completed);
                    }
                    Ok(read) if read <= MAX_TRANSFER_CHUNK => {
                        self.buffer.truncate(read);
                        self.offset = 0;
                        self.phase = TransferPhase::Writing;
                        self.assert_invariants();
                        break;
                    }
                    Ok(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "transfer source returned more than its bounded chunk",
                        ));
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                        interrupted += 1;
                        if interrupted >= MAX_INTERRUPTED_SYSCALLS_PER_PUMP {
                            self.transition_to_reading();
                            return Ok(TransferPumpOutcome::Pending);
                        }
                        continue;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        self.transition_to_reading();
                        return Ok(TransferPumpOutcome::Pending);
                    }
                    Err(error) => return Err(error),
                }
            }
        }

        debug_assert_eq!(self.phase, TransferPhase::Writing);
        let mut interrupted = 0;
        while self.offset < self.buffer.len() {
            let write_result = {
                let remaining = &self.buffer[self.offset..];
                io.write(self.sink.as_raw_fd(), remaining)
            };
            match write_result {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "transfer sink accepted zero bytes",
                    ));
                }
                Ok(written) if written <= self.buffer.len() - self.offset => {
                    self.offset += written;
                    if self.offset == self.buffer.len() {
                        self.transition_to_reading();
                        return Ok(TransferPumpOutcome::Pending);
                    }
                }
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "transfer sink reported more bytes than requested",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                    interrupted += 1;
                    if interrupted >= MAX_INTERRUPTED_SYSCALLS_PER_PUMP {
                        self.assert_invariants();
                        return Ok(TransferPumpOutcome::Pending);
                    }
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.assert_invariants();
                    return Ok(TransferPumpOutcome::Pending);
                }
                Err(error) => return Err(error),
            }
        }

        unreachable!("writing phase always normalizes after draining its buffer")
    }

    fn transition_to_reading(&mut self) {
        self.buffer.clear();
        self.offset = 0;
        self.phase = TransferPhase::Reading;
        self.assert_invariants();
    }

    fn assert_invariants(&self) {
        debug_assert!(self.buffer.capacity() <= MAX_TRANSFER_CHUNK);
        debug_assert!(self.offset <= self.buffer.len());
        match self.phase {
            TransferPhase::Reading => {
                debug_assert!(self.buffer.is_empty());
                debug_assert_eq!(self.offset, 0);
            }
            TransferPhase::Writing => {
                debug_assert!(!self.buffer.is_empty());
                debug_assert!(self.offset < self.buffer.len());
            }
        }
    }
}

fn set_nonblocking(fd: &OwnedFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        io::{self, Read, Write},
        num::NonZeroU64,
        os::unix::net::UnixStream,
    };

    use super::*;

    #[test]
    fn transfer_uses_nonblocking_bounded_chunks() {
        let (mut source_writer, source_reader) = UnixStream::pair().expect("source");
        let (sink_reader, sink_writer) = UnixStream::pair().expect("sink");
        source_writer.write_all(b"clipboard").expect("source write");
        let generation = BridgeGeneration::new(NonZeroU64::new(1).expect("nonzero"));
        let mut manager = TransferManager::default();
        let id = manager
            .start(
                generation,
                source_reader.into(),
                sink_writer.into(),
                u64::MAX,
            )
            .expect("transfer");
        let interest = manager.interest(id).expect("initial interest");
        assert_eq!(interest.interest, TransferIoInterest::SourceReadable);
        assert_eq!(
            interest.events & TRANSFER_TERMINAL_EVENTS,
            TRANSFER_TERMINAL_EVENTS
        );
        assert_ne!(interest.events & libc::EPOLLIN as u32, 0);
        assert_eq!(interest.deadline_ns, u64::MAX);
        assert!(interest.fd >= 0);
        let _ = manager.pump(id, 0).expect("pump");
        let _ = sink_reader;
        const _: () = assert!(MAX_TRANSFER_CHUNK <= 64 * 1024);
    }

    fn read_available(stream: &mut UnixStream) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; MAX_TRANSFER_CHUNK];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => bytes.extend_from_slice(&buffer[..read]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("read transfer sink: {error}"),
            }
        }
        bytes
    }

    #[test]
    fn eof_after_drained_payload_completes_transfer() {
        let (mut source_writer, source_reader) = UnixStream::pair().expect("source");
        let (mut sink_reader, sink_writer) = UnixStream::pair().expect("sink");
        sink_reader.set_nonblocking(true).expect("nonblocking sink");
        source_writer.write_all(b"abc").expect("source write");
        let generation = BridgeGeneration::new(NonZeroU64::new(2).expect("nonzero"));
        let mut manager = TransferManager::default();
        let id = manager
            .start(
                generation,
                source_reader.into(),
                sink_writer.into(),
                u64::MAX,
            )
            .expect("transfer");

        assert_eq!(
            manager.pump(id, 0).expect("payload pump"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(read_available(&mut sink_reader), b"abc");
        drop(source_writer);

        assert_eq!(
            manager.pump(id, 1).expect("EOF pump"),
            TransferPumpOutcome::Completed
        );
        assert_eq!(manager.len(), 0);
    }

    #[test]
    fn source_eagain_after_drained_chunk_allows_later_data() {
        let (mut source_writer, source_reader) = UnixStream::pair().expect("source");
        let (mut sink_reader, sink_writer) = UnixStream::pair().expect("sink");
        sink_reader.set_nonblocking(true).expect("nonblocking sink");
        source_writer.write_all(b"abc").expect("source write");
        let generation = BridgeGeneration::new(NonZeroU64::new(3).expect("nonzero"));
        let mut manager = TransferManager::default();
        let id = manager
            .start(
                generation,
                source_reader.into(),
                sink_writer.into(),
                u64::MAX,
            )
            .expect("transfer");

        assert_eq!(
            manager.pump(id, 0).expect("first pump"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(read_available(&mut sink_reader), b"abc");
        assert_eq!(
            manager.pump(id, 1).expect("EAGAIN pump"),
            TransferPumpOutcome::Pending
        );
        source_writer.write_all(b"def").expect("later source write");

        assert_eq!(
            manager.pump(id, 2).expect("later data pump"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(read_available(&mut sink_reader), b"def");
    }

    #[derive(Debug)]
    enum ReadOutcome {
        Data(Vec<u8>),
        Eof,
        WouldBlock,
        Interrupted,
        Error(io::ErrorKind),
    }

    #[derive(Debug)]
    enum WriteOutcome {
        Count(usize),
        WouldBlock,
        Interrupted,
        Zero,
        Error(io::ErrorKind),
    }

    #[derive(Debug, Default)]
    struct ScriptedIo {
        reads: VecDeque<ReadOutcome>,
        writes: VecDeque<WriteOutcome>,
        received: Vec<u8>,
    }

    impl TransferIo for ScriptedIo {
        fn read(&mut self, _fd: RawFd, buffer: &mut [u8]) -> io::Result<usize> {
            match self.reads.pop_front().unwrap_or(ReadOutcome::WouldBlock) {
                ReadOutcome::Data(data) => {
                    if data.len() > buffer.len() {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "scripted read exceeds transfer buffer",
                        ));
                    }
                    buffer[..data.len()].copy_from_slice(&data);
                    Ok(data.len())
                }
                ReadOutcome::Eof => Ok(0),
                ReadOutcome::WouldBlock => Err(io::ErrorKind::WouldBlock.into()),
                ReadOutcome::Interrupted => Err(io::ErrorKind::Interrupted.into()),
                ReadOutcome::Error(kind) => Err(kind.into()),
            }
        }

        fn write(&mut self, _fd: RawFd, buffer: &[u8]) -> io::Result<usize> {
            match self.writes.pop_front().unwrap_or(WriteOutcome::WouldBlock) {
                WriteOutcome::Count(count) if count <= buffer.len() => {
                    self.received.extend_from_slice(&buffer[..count]);
                    Ok(count)
                }
                WriteOutcome::Count(_) => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "scripted write exceeds requested bytes",
                )),
                WriteOutcome::WouldBlock => Err(io::ErrorKind::WouldBlock.into()),
                WriteOutcome::Interrupted => Err(io::ErrorKind::Interrupted.into()),
                WriteOutcome::Zero => Ok(0),
                WriteOutcome::Error(kind) => Err(kind.into()),
            }
        }
    }

    fn scripted_transfer(deadline_ns: u64) -> (TransferManager, TransferId) {
        let (_source_peer, source) = UnixStream::pair().expect("source");
        let (sink, _sink_peer) = UnixStream::pair().expect("sink");
        let generation = BridgeGeneration::new(NonZeroU64::new(10).expect("nonzero"));
        let mut manager = TransferManager::default();
        let id = manager
            .start(generation, source.into(), sink.into(), deadline_ns)
            .expect("transfer");
        (manager, id)
    }

    #[test]
    fn partial_sink_write_resumes_from_exact_offset() {
        let (mut manager, id) = scripted_transfer(u64::MAX);
        let mut io = ScriptedIo {
            reads: VecDeque::from([ReadOutcome::Data(b"ABCDEFGH".to_vec()), ReadOutcome::Eof]),
            writes: VecDeque::from([
                WriteOutcome::Count(2),
                WriteOutcome::WouldBlock,
                WriteOutcome::Count(3),
                WriteOutcome::Count(1),
                WriteOutcome::Count(2),
            ]),
            received: Vec::new(),
        };

        assert_eq!(
            manager.pump_with_io(id, 0, &mut io).expect("partial pump"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(io.received, b"AB");
        assert_eq!(io.reads.len(), 1, "one source chunk per pump");
        assert_eq!(
            manager.interest(id).expect("writing interest").interest,
            TransferIoInterest::SinkWritable
        );

        assert_eq!(
            manager.pump_with_io(id, 1, &mut io).expect("resume pump"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(io.received, b"ABCDEFGH");
        assert_eq!(io.reads.len(), 1, "draining does not read a second chunk");
        assert_eq!(
            manager.interest(id).expect("reading interest").interest,
            TransferIoInterest::SourceReadable
        );
        assert_eq!(
            manager.pump_with_io(id, 2, &mut io).expect("EOF pump"),
            TransferPumpOutcome::Completed
        );
        assert!(manager.is_empty());
    }

    #[test]
    fn sink_eagain_preserves_buffer_and_offset() {
        let (mut manager, id) = scripted_transfer(u64::MAX);
        let mut io = ScriptedIo {
            reads: VecDeque::from([ReadOutcome::Data(b"abcdef".to_vec())]),
            writes: VecDeque::from([
                WriteOutcome::Count(2),
                WriteOutcome::WouldBlock,
                WriteOutcome::Count(4),
            ]),
            received: Vec::new(),
        };

        assert_eq!(
            manager
                .pump_with_io(id, 0, &mut io)
                .expect("initial partial write"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(io.received, b"ab");
        assert_eq!(
            manager.interest(id).expect("sink interest").interest,
            TransferIoInterest::SinkWritable
        );

        assert_eq!(
            manager
                .pump_with_io(id, 1, &mut io)
                .expect("resume after EAGAIN"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(io.received, b"abcdef");
        assert_eq!(
            manager.interest(id).expect("source interest").interest,
            TransferIoInterest::SourceReadable
        );
    }

    #[test]
    fn arbitrary_partial_write_schedule_conserves_repeated_bytes() {
        let (mut manager, id) = scripted_transfer(u64::MAX);
        let mut io = ScriptedIo {
            reads: VecDeque::from([ReadOutcome::Data(vec![b'x'; 10]), ReadOutcome::Eof]),
            writes: VecDeque::from([
                WriteOutcome::Count(1),
                WriteOutcome::Count(2),
                WriteOutcome::WouldBlock,
                WriteOutcome::Count(3),
                WriteOutcome::Count(1),
                WriteOutcome::Count(3),
            ]),
            received: Vec::new(),
        };

        assert_eq!(
            manager
                .pump_with_io(id, 0, &mut io)
                .expect("first partial schedule"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(io.received, vec![b'x'; 3]);
        assert_eq!(
            manager
                .pump_with_io(id, 1, &mut io)
                .expect("second partial schedule"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(io.received, vec![b'x'; 10]);
        assert_eq!(
            manager.pump_with_io(id, 2, &mut io).expect("scheduled EOF"),
            TransferPumpOutcome::Completed
        );
        assert_eq!(io.received, vec![b'x'; 10]);
    }

    #[test]
    fn eof_while_sink_is_backpressured_drains_before_completion() {
        let (mut manager, id) = scripted_transfer(u64::MAX);
        let mut io = ScriptedIo {
            reads: VecDeque::from([ReadOutcome::Data(b"payload".to_vec()), ReadOutcome::Eof]),
            writes: VecDeque::from([
                WriteOutcome::Count(2),
                WriteOutcome::WouldBlock,
                WriteOutcome::Count(5),
            ]),
            received: Vec::new(),
        };

        assert_eq!(
            manager
                .pump_with_io(id, 0, &mut io)
                .expect("backpressured pump"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(io.received, b"pa");
        assert_eq!(manager.len(), 1);
        assert_eq!(
            manager.interest(id).expect("sink interest").interest,
            TransferIoInterest::SinkWritable
        );

        assert_eq!(
            manager.pump_with_io(id, 1, &mut io).expect("drain pump"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(io.received, b"payload");
        assert_eq!(
            manager.interest(id).expect("source interest").interest,
            TransferIoInterest::SourceReadable
        );
        assert_eq!(manager.len(), 1);

        assert_eq!(
            manager.pump_with_io(id, 2, &mut io).expect("EOF pump"),
            TransferPumpOutcome::Completed
        );
        assert!(manager.is_empty());
    }

    #[test]
    fn source_eagain_preserves_reading_interest() {
        let (mut manager, id) = scripted_transfer(u64::MAX);
        let mut io = ScriptedIo {
            reads: VecDeque::from([
                ReadOutcome::WouldBlock,
                ReadOutcome::Data(b"later".to_vec()),
            ]),
            writes: VecDeque::from([WriteOutcome::Count(5)]),
            received: Vec::new(),
        };

        assert_eq!(
            manager.pump_with_io(id, 0, &mut io).expect("source EAGAIN"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(manager.len(), 1);
        assert_eq!(
            manager.interest(id).expect("source interest").interest,
            TransferIoInterest::SourceReadable
        );
        assert_eq!(
            manager
                .pump_with_io(id, 1, &mut io)
                .expect("later source data"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(io.received, b"later");
    }

    #[test]
    fn readiness_tracks_reading_and_writing_phase() {
        let (mut manager, id) = scripted_transfer(u64::MAX);
        assert_eq!(
            manager.interest(id).expect("initial interest").interest,
            TransferIoInterest::SourceReadable
        );

        let mut io = ScriptedIo {
            reads: VecDeque::from([
                ReadOutcome::Data(b"abc".to_vec()),
                ReadOutcome::WouldBlock,
                ReadOutcome::Eof,
            ]),
            writes: VecDeque::from([
                WriteOutcome::Count(1),
                WriteOutcome::WouldBlock,
                WriteOutcome::Count(2),
            ]),
            received: Vec::new(),
        };
        assert_eq!(
            manager
                .pump_with_io(id, 0, &mut io)
                .expect("partial sink write"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(
            manager.interest(id).expect("sink interest").interest,
            TransferIoInterest::SinkWritable
        );
        assert_eq!(
            manager.interest(id).expect("sink interest").events & TRANSFER_TERMINAL_EVENTS,
            TRANSFER_TERMINAL_EVENTS
        );
        assert_ne!(
            manager.interest(id).expect("sink interest").events & libc::EPOLLOUT as u32,
            0
        );

        assert_eq!(
            manager.pump_with_io(id, 1, &mut io).expect("drain sink"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(
            manager.interest(id).expect("source interest").interest,
            TransferIoInterest::SourceReadable
        );

        assert_eq!(
            manager.pump_with_io(id, 2, &mut io).expect("source EAGAIN"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(
            manager.interest(id).expect("source interest").interest,
            TransferIoInterest::SourceReadable
        );

        assert_eq!(
            manager.pump_with_io(id, 3, &mut io).expect("EOF"),
            TransferPumpOutcome::Completed
        );
        assert!(manager.interest(id).is_none());
        assert_eq!(io.received, b"abc");
    }

    #[test]
    fn interrupted_read_and_write_are_retried_without_loss() {
        let (mut manager, id) = scripted_transfer(u64::MAX);
        let mut io = ScriptedIo {
            reads: VecDeque::from([ReadOutcome::Interrupted, ReadOutcome::Data(b"abc".to_vec())]),
            writes: VecDeque::from([WriteOutcome::Interrupted, WriteOutcome::Count(3)]),
            received: Vec::new(),
        };

        assert_eq!(
            manager
                .pump_with_io(id, 0, &mut io)
                .expect("interrupted I/O"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(io.received, b"abc");
        assert_eq!(
            manager.interest(id).expect("source interest").interest,
            TransferIoInterest::SourceReadable
        );
        assert_eq!(manager.len(), 1);
    }

    #[test]
    fn source_eintr_does_not_abort_transfer() {
        let (mut manager, id) = scripted_transfer(u64::MAX);
        let mut io = ScriptedIo {
            reads: VecDeque::from([ReadOutcome::Interrupted, ReadOutcome::Data(b"abc".to_vec())]),
            writes: VecDeque::from([WriteOutcome::Count(3)]),
            received: Vec::new(),
        };

        assert_eq!(
            manager.pump_with_io(id, 0, &mut io).expect("source EINTR"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(io.received, b"abc");
        assert_eq!(manager.len(), 1);
    }

    #[test]
    fn sink_eintr_does_not_abort_transfer() {
        let (mut manager, id) = scripted_transfer(u64::MAX);
        let mut io = ScriptedIo {
            reads: VecDeque::from([ReadOutcome::Data(b"abc".to_vec())]),
            writes: VecDeque::from([WriteOutcome::Interrupted, WriteOutcome::Count(3)]),
            received: Vec::new(),
        };

        assert_eq!(
            manager.pump_with_io(id, 0, &mut io).expect("sink EINTR"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(io.received, b"abc");
        assert_eq!(manager.len(), 1);
    }

    #[test]
    fn repeated_eintr_is_bounded_without_losing_state() {
        let (mut manager, id) = scripted_transfer(u64::MAX);
        let mut io = ScriptedIo {
            reads: std::iter::repeat_with(|| ReadOutcome::Interrupted)
                .take(32)
                .collect(),
            writes: VecDeque::new(),
            received: Vec::new(),
        };

        assert_eq!(
            manager
                .pump_with_io(id, 0, &mut io)
                .expect("bounded source EINTR"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(io.reads.len(), 24);
        assert_eq!(
            manager.interest(id).expect("source interest").interest,
            TransferIoInterest::SourceReadable
        );

        io.reads.push_front(ReadOutcome::Data(b"abc".to_vec()));
        io.writes.push_back(WriteOutcome::Count(3));
        assert_eq!(
            manager
                .pump_with_io(id, 1, &mut io)
                .expect("data after bounded EINTR"),
            TransferPumpOutcome::Pending
        );
        assert_eq!(io.received, b"abc");
    }

    #[test]
    fn zero_byte_write_is_terminal_and_releases_slot() {
        let (mut manager, id) = scripted_transfer(u64::MAX);
        let mut io = ScriptedIo {
            reads: VecDeque::from([ReadOutcome::Data(b"abc".to_vec())]),
            writes: VecDeque::from([WriteOutcome::Zero]),
            received: Vec::new(),
        };

        let error = manager
            .pump_with_io(id, 0, &mut io)
            .expect_err("zero write must fail");
        assert_eq!(error.kind(), io::ErrorKind::WriteZero);
        assert!(manager.is_empty());
    }

    #[test]
    fn source_and_sink_errors_release_transfers_immediately() {
        let (mut manager, source_error_id) = scripted_transfer(u64::MAX);
        let mut source_error = ScriptedIo {
            reads: VecDeque::from([ReadOutcome::Error(io::ErrorKind::BrokenPipe)]),
            ..ScriptedIo::default()
        };
        assert_eq!(
            manager
                .pump_with_io(source_error_id, 0, &mut source_error)
                .expect_err("source error")
                .kind(),
            io::ErrorKind::BrokenPipe
        );
        assert!(manager.is_empty());

        let (mut manager, sink_error_id) = scripted_transfer(u64::MAX);
        let mut sink_error = ScriptedIo {
            reads: VecDeque::from([ReadOutcome::Data(b"abc".to_vec())]),
            writes: VecDeque::from([WriteOutcome::Error(io::ErrorKind::BrokenPipe)]),
            ..ScriptedIo::default()
        };
        assert_eq!(
            manager
                .pump_with_io(sink_error_id, 0, &mut sink_error)
                .expect_err("sink error")
                .kind(),
            io::ErrorKind::BrokenPipe
        );
        assert!(manager.is_empty());
    }

    #[test]
    fn deadline_expires_without_fd_readiness() {
        let (mut manager, id) = scripted_transfer(100);
        assert_eq!(manager.next_deadline_ns(), Some(100));
        assert!(manager.expire_deadlines(99).is_empty());
        assert_eq!(manager.len(), 1);
        assert_eq!(manager.expire_deadlines(100), vec![id]);
        assert_eq!(manager.next_deadline_ns(), None);
        assert!(manager.is_empty());

        let (mut manager, id) = scripted_transfer(100);
        let mut io = ScriptedIo {
            reads: VecDeque::from([ReadOutcome::Data(b"must not read".to_vec())]),
            ..ScriptedIo::default()
        };
        assert_eq!(
            manager
                .pump_with_io(id, 100, &mut io)
                .expect("deadline outcome"),
            TransferPumpOutcome::TimedOut
        );
        assert_eq!(io.reads.len(), 1, "expired transfer attempted I/O");
        assert!(manager.is_empty());
    }

    #[test]
    fn cancellation_is_exact_and_stale_ids_are_noops() {
        let (mut manager, old_id) = scripted_transfer(u64::MAX);
        assert!(manager.cancel(old_id));
        assert!(!manager.cancel(old_id));
        let (_source_peer, source) = UnixStream::pair().expect("replacement source");
        let (sink, _sink_peer) = UnixStream::pair().expect("replacement sink");
        let new_id = manager
            .start(old_id.generation, source.into(), sink.into(), u64::MAX)
            .expect("replacement transfer");
        let mut io = ScriptedIo::default();
        assert_eq!(
            manager
                .pump_with_io(old_id, 0, &mut io)
                .expect("stale event"),
            TransferPumpOutcome::Gone
        );
        assert!(io.reads.is_empty());
        assert_eq!(manager.len(), 1);
        assert_eq!(
            manager.interest(new_id).expect("replacement interest").id,
            new_id
        );
    }

    #[test]
    fn generation_cleanup_is_selective_and_capacity_is_reusable() {
        let first_generation = BridgeGeneration::new(NonZeroU64::new(11).expect("nonzero"));
        let second_generation = BridgeGeneration::new(NonZeroU64::new(12).expect("nonzero"));
        let mut manager = TransferManager::default();
        let first_id = {
            let (_source_peer, source) = UnixStream::pair().expect("first source");
            let (sink, _sink_peer) = UnixStream::pair().expect("first sink");
            manager
                .start(first_generation, source.into(), sink.into(), u64::MAX)
                .expect("first transfer")
        };
        let second_id = {
            let (_source_peer, source) = UnixStream::pair().expect("second source");
            let (sink, _sink_peer) = UnixStream::pair().expect("second sink");
            manager
                .start(second_generation, source.into(), sink.into(), u64::MAX)
                .expect("second transfer")
        };

        manager.clear_generation(first_generation);
        assert_eq!(manager.len(), 1);
        assert!(manager.interest(first_id).is_none());
        assert_eq!(
            manager.interest(second_id).expect("new generation").id,
            second_id
        );

        let (_source_peer, source) = UnixStream::pair().expect("replacement source");
        let (sink, _sink_peer) = UnixStream::pair().expect("replacement sink");
        manager
            .start(first_generation, source.into(), sink.into(), u64::MAX)
            .expect("reusable generation slot");
        assert_eq!(manager.len(), 2);
    }

    fn fill_capacity(
        manager: &mut TransferManager,
        generation: BridgeGeneration,
    ) -> Vec<TransferId> {
        (0..MAX_ACTIVE_TRANSFERS)
            .map(|_| {
                let (_source_peer, source) = UnixStream::pair().expect("capacity source");
                let (sink, _sink_peer) = UnixStream::pair().expect("capacity sink");
                manager
                    .start(generation, source.into(), sink.into(), u64::MAX)
                    .expect("capacity transfer")
            })
            .collect()
    }

    fn assert_capacity_is_full(manager: &mut TransferManager, generation: BridgeGeneration) {
        let (_source_peer, source) = UnixStream::pair().expect("full source");
        let (sink, _sink_peer) = UnixStream::pair().expect("full sink");
        let error = manager
            .start(generation, source.into(), sink.into(), u64::MAX)
            .expect_err("capacity must be bounded");
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    }

    fn assert_slot_reusable(manager: &mut TransferManager, generation: BridgeGeneration) {
        let (_source_peer, source) = UnixStream::pair().expect("replacement source");
        let (sink, _sink_peer) = UnixStream::pair().expect("replacement sink");
        manager
            .start(generation, source.into(), sink.into(), u64::MAX)
            .expect("freed transfer slot");
    }

    #[test]
    fn capacity_recovers_after_cancel_deadline_generation_clear_eof_and_io_error() {
        let generation = BridgeGeneration::new(NonZeroU64::new(20).expect("nonzero"));

        let mut manager = TransferManager::default();
        let ids = fill_capacity(&mut manager, generation);
        assert_capacity_is_full(&mut manager, generation);
        assert!(manager.cancel(ids[0]));
        assert_slot_reusable(&mut manager, generation);

        let mut manager = TransferManager::default();
        let _ = fill_capacity(&mut manager, generation);
        manager.expire_deadlines(u64::MAX);
        assert_slot_reusable(&mut manager, generation);

        let mut manager = TransferManager::default();
        let _ = fill_capacity(&mut manager, generation);
        manager.clear_generation(generation);
        assert_slot_reusable(&mut manager, generation);

        let mut manager = TransferManager::default();
        let ids = fill_capacity(&mut manager, generation);
        assert_eq!(
            manager.pump(ids[0], 0).expect("EOF completion"),
            TransferPumpOutcome::Completed
        );
        assert_slot_reusable(&mut manager, generation);

        let mut manager = TransferManager::default();
        let ids = fill_capacity(&mut manager, generation);
        let mut io = ScriptedIo {
            reads: VecDeque::from([ReadOutcome::Error(io::ErrorKind::BrokenPipe)]),
            ..ScriptedIo::default()
        };
        assert!(manager.pump_with_io(ids[0], 0, &mut io).is_err());
        assert_slot_reusable(&mut manager, generation);
    }

    fn transfer_payload_exact(payload: Vec<u8>) -> Vec<u8> {
        let (mut source_writer, source_reader) = UnixStream::pair().expect("source");
        let (mut sink_reader, sink_writer) = UnixStream::pair().expect("sink");
        sink_reader.set_nonblocking(true).expect("nonblocking sink");
        source_writer.write_all(&payload).expect("payload write");
        drop(source_writer);
        let generation = BridgeGeneration::new(NonZeroU64::new(30).expect("nonzero"));
        let mut manager = TransferManager::default();
        let id = manager
            .start(
                generation,
                source_reader.into(),
                sink_writer.into(),
                u64::MAX,
            )
            .expect("transfer");
        let mut received = Vec::new();
        for now_ns in 0..16 {
            if manager.is_empty() {
                break;
            }
            manager.pump(id, now_ns).expect("payload pump");
            received.extend(read_available(&mut sink_reader));
        }
        assert!(manager.is_empty(), "payload transfer did not complete");
        received
    }

    #[test]
    fn payload_sizes_are_byte_exact_across_chunk_boundaries() {
        for payload in [
            Vec::new(),
            vec![b'a'],
            vec![b'b'; MAX_TRANSFER_CHUNK - 1],
            vec![b'c'; MAX_TRANSFER_CHUNK],
            vec![b'd'; MAX_TRANSFER_CHUNK + 1],
            vec![b'e'; MAX_TRANSFER_CHUNK * 2 + 17],
        ] {
            assert_eq!(transfer_payload_exact(payload.clone()), payload);
        }
    }

    #[test]
    fn large_slow_transfer_preserves_every_byte_and_bounded_buffer() {
        let chunks = [
            vec![b'a'; MAX_TRANSFER_CHUNK],
            vec![b'b'; MAX_TRANSFER_CHUNK],
            vec![b'c'; MAX_TRANSFER_CHUNK],
            vec![b'd'; MAX_TRANSFER_CHUNK],
            vec![b'e'; MAX_TRANSFER_CHUNK],
            vec![b'f'; 17],
        ];
        let payload = chunks.concat();
        let mut writes = VecDeque::new();
        for chunk in &chunks {
            writes.push_back(WriteOutcome::Count(1));
            writes.push_back(WriteOutcome::WouldBlock);
            writes.push_back(WriteOutcome::Count(chunk.len() - 1));
        }
        let (mut manager, id) = scripted_transfer(u64::MAX);
        let mut io = ScriptedIo {
            reads: chunks
                .iter()
                .cloned()
                .map(ReadOutcome::Data)
                .chain([ReadOutcome::Eof])
                .collect(),
            writes,
            received: Vec::new(),
        };

        for now_ns in 0..32 {
            if manager.is_empty() {
                break;
            }
            let outcome = manager
                .pump_with_io(id, now_ns, &mut io)
                .expect("large slow transfer");
            if let Some(transfer) = manager.transfers.get(&id) {
                assert!(transfer.buffer.capacity() <= MAX_TRANSFER_CHUNK);
                assert!(transfer.buffer.len() <= MAX_TRANSFER_CHUNK);
            }
            if outcome == TransferPumpOutcome::Completed {
                break;
            }
        }

        assert_eq!(io.received, payload);
        assert!(manager.is_empty());
    }
}
