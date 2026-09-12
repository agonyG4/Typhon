//! Standalone audit probes. Includes production source; does not patch Typhon.
//! Compile in the existing target directory; commands are in the audit report.
#![allow(dead_code)]

mod connection {
    include!("../../src/xwayland/xwm/connection.rs");

    #[test]
    fn audit_queued_x11_bytes_must_precede_new_request() {
        use std::io::Read;
        let (socket, mut peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        let stream = ReactorStream::from_unix_stream(socket).unwrap();
        let older = vec![b'A'; 48 * 1024];
        // A valid state following a previous short socket write.
        stream.queue(&older).unwrap();
        Stream::write(&stream, b"B", &mut Vec::new()).unwrap();
        for _ in 0..8 {
            if !stream.flush_pending().unwrap() {
                break;
            }
        }
        let mut received = vec![0; older.len() + 1];
        peer.read_exact(&mut received).unwrap();
        let first_new_byte = received.iter().position(|byte| *byte == b'B').unwrap();
        eprintln!(
            "first new request byte at {first_new_byte}; expected {}",
            older.len()
        );
        assert_eq!(
            first_new_byte,
            older.len(),
            "new request overtook queued X11 bytes"
        );
    }
}

mod bridge {
    // Only the generation key is supplied locally. Transfer I/O and state are
    // the unmodified production implementation included below.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct BridgeGeneration(std::num::NonZeroU64);
    impl BridgeGeneration {
        pub const fn new(value: std::num::NonZeroU64) -> Self {
            Self(value)
        }
    }
    pub mod transfer {
        include!("../../src/xwayland/xwm/data_bridge/transfer.rs");

        #[test]
        fn audit_eof_after_data_must_complete_transfer() {
            use std::{io::Write, os::unix::net::UnixStream};
            let (mut producer, source) = UnixStream::pair().unwrap();
            let (_consumer, sink) = UnixStream::pair().unwrap();
            let generation = BridgeGeneration::new(std::num::NonZeroU64::new(1).unwrap());
            let mut manager = TransferManager::default();
            let id = manager
                .start(generation, source.into(), sink.into(), 100)
                .unwrap();
            producer.write_all(b"abc").unwrap();
            assert!(!manager.pump(id, 1).unwrap());
            drop(producer);
            assert!(
                manager.pump(id, 2).unwrap(),
                "EOF leaves offset beyond empty buffer"
            );
            assert_eq!(manager.len(), 0);
        }

        #[test]
        fn audit_eagain_after_data_must_allow_later_data() {
            use std::{
                io::{Read, Write},
                os::unix::net::UnixStream,
            };
            let (mut producer, source) = UnixStream::pair().unwrap();
            let (mut consumer, sink) = UnixStream::pair().unwrap();
            consumer.set_nonblocking(true).unwrap();
            let generation = BridgeGeneration::new(std::num::NonZeroU64::new(1).unwrap());
            let mut manager = TransferManager::default();
            let id = manager
                .start(generation, source.into(), sink.into(), 100)
                .unwrap();
            producer.write_all(b"abc").unwrap();
            manager.pump(id, 1).unwrap();
            let mut received = [0; 3];
            consumer.read_exact(&mut received).unwrap();
            manager.pump(id, 2).unwrap(); // source EAGAIN
            producer.write_all(b"def").unwrap();
            manager.pump(id, 3).unwrap();
            assert_eq!(
                consumer.read(&mut received).unwrap(),
                3,
                "later chunk was stranded"
            );
            assert_eq!(&received, b"def");
        }
    }
}
