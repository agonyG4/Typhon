use std::{
    collections::HashMap,
    io,
    os::fd::{AsRawFd, OwnedFd},
};

use super::BridgeGeneration;

pub const MAX_TRANSFER_CHUNK: usize = 64 * 1024;
const MAX_ACTIVE_TRANSFERS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransferId {
    pub generation: BridgeGeneration,
    pub serial: u64,
}

#[derive(Debug)]
struct Transfer {
    id: TransferId,
    source: OwnedFd,
    sink: OwnedFd,
    buffer: Vec<u8>,
    offset: usize,
    eof: bool,
    deadline_ns: u64,
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
                eof: false,
                deadline_ns,
            },
        );
        Ok(id)
    }

    pub fn pump(&mut self, id: TransferId, now_ns: u64) -> io::Result<bool> {
        let Some(transfer) = self.transfers.get_mut(&id) else {
            return Ok(true);
        };
        if transfer.id.generation != id.generation || now_ns >= transfer.deadline_ns {
            self.transfers.remove(&id);
            return Ok(true);
        }
        if transfer.offset == transfer.buffer.len() && !transfer.eof {
            transfer.buffer.resize(MAX_TRANSFER_CHUNK, 0);
            let read = unsafe {
                libc::read(
                    transfer.source.as_raw_fd(),
                    transfer.buffer.as_mut_ptr().cast(),
                    transfer.buffer.len(),
                )
            };
            if read == 0 {
                transfer.eof = true;
                transfer.buffer.clear();
            } else if read < 0 {
                let error = io::Error::last_os_error();
                transfer.buffer.clear();
                if error.kind() != io::ErrorKind::WouldBlock {
                    self.transfers.remove(&id);
                    return Err(error);
                }
            } else {
                transfer.buffer.truncate(read as usize);
                transfer.offset = 0;
            }
        }
        while transfer.offset < transfer.buffer.len() {
            let written = unsafe {
                libc::write(
                    transfer.sink.as_raw_fd(),
                    transfer.buffer[transfer.offset..].as_ptr().cast(),
                    transfer.buffer.len() - transfer.offset,
                )
            };
            if written < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::WouldBlock {
                    break;
                }
                self.transfers.remove(&id);
                return Err(error);
            }
            transfer.offset = transfer.offset.saturating_add(written as usize);
            if written == 0 {
                break;
            }
        }
        let finished = transfer.eof && transfer.offset == transfer.buffer.len();
        if finished {
            self.transfers.remove(&id);
        }
        Ok(finished)
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
    use std::{io::Write, num::NonZeroU64, os::unix::net::UnixStream};

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
        let _ = manager.pump(id, 0).expect("pump");
        let _ = sink_reader;
        const _: () = assert!(MAX_TRANSFER_CHUNK <= 64 * 1024);
    }
}
