use std::{
    fs, io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, MetadataExt, PermissionsExt},
        },
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{
    system_action::{AstreaSystemAction, AstreaSystemActionCapabilities},
    system_action_protocol::{
        PACKET_SIZE, PROTOCOL_MAJOR, PROTOCOL_MINOR, SystemActionMessage, decode, encode,
    },
};
use oblivion_one::native::{
    control::ControlRuntimePaths,
    event_loop::{NativeEventLoop, NativeEventSource, ReactorToken},
};

pub(crate) const MAX_SYSTEM_ACTION_INBOUND_PACKETS_PER_CYCLE: usize = 16;
pub(crate) const MAX_SYSTEM_ACTION_ACCEPTS_PER_CYCLE: usize = 4;
pub(crate) const MAX_SYSTEM_ACTION_OUTBOUND_PACKETS_PER_CYCLE: usize = 16;
pub(crate) const SYSTEM_ACTION_OUTBOUND_CAPACITY: usize = 32;
const SYSTEM_ACTION_HANDSHAKE_TIMEOUT_NS: u64 = 2_000_000_000;
const SYSTEM_ACTION_REJECT_MAJOR_MISMATCH: u16 = 1;
const SYSTEM_ACTION_REJECT_BUSY: u16 = 2;
const SYSTEM_ACTION_REJECT_PROTOCOL: u16 = 3;
const PEER_EVENTS: u32 =
    (libc::EPOLLIN | libc::EPOLLERR | libc::EPOLLHUP | libc::EPOLLRDHUP) as u32;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SystemActionTransportTelemetry {
    pub(crate) listener_accepts: u64,
    pub(crate) peer_rejections: u64,
    pub(crate) handshake_successes: u64,
    pub(crate) handshake_failures: u64,
    pub(crate) peer_disconnects: u64,
    pub(crate) capability_updates: u64,
    pub(crate) actions_sent: u64,
    pub(crate) action_occurrences_sent: u64,
    pub(crate) action_records_coalesced: u64,
    pub(crate) send_would_block: u64,
    pub(crate) queue_overflows: u64,
    pub(crate) protocol_errors: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SystemActionTransportSnapshot {
    pub(crate) peer_connected: bool,
    pub(crate) protocol_ready: bool,
    pub(crate) outbound_queue_depth: usize,
    pub(crate) telemetry: SystemActionTransportTelemetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActionRecord {
    action: AstreaSystemAction,
    occurrences: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueuePush {
    Queued,
    Coalesced,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FixedActionQueue {
    entries: [Option<ActionRecord>; SYSTEM_ACTION_OUTBOUND_CAPACITY],
    len: usize,
}

impl Default for FixedActionQueue {
    fn default() -> Self {
        Self {
            entries: [None; SYSTEM_ACTION_OUTBOUND_CAPACITY],
            len: 0,
        }
    }
}

impl FixedActionQueue {
    fn push(&mut self, action: AstreaSystemAction) -> QueuePush {
        if action.is_coalescible_step()
            && let Some(Some(tail)) = self
                .len
                .checked_sub(1)
                .and_then(|i| self.entries.get_mut(i))
            && tail.action == action
            && let Some(count) = tail.occurrences.checked_add(1)
        {
            tail.occurrences = count;
            return QueuePush::Coalesced;
        }
        if self.len == self.entries.len() {
            return QueuePush::Full;
        }
        self.entries[self.len] = Some(ActionRecord {
            action,
            occurrences: 1,
        });
        self.len += 1;
        QueuePush::Queued
    }

    fn front(&self) -> Option<ActionRecord> {
        self.entries.first().copied().flatten()
    }

    fn pop(&mut self) -> Option<ActionRecord> {
        if self.len == 0 {
            return None;
        }
        let first = self.entries[0].take();
        self.entries.copy_within(1..self.len, 0);
        self.len -= 1;
        self.entries[self.len] = None;
        first
    }

    fn retain_capabilities(&mut self, capabilities: AstreaSystemActionCapabilities) {
        let mut retained = 0;
        for index in 0..self.len {
            if let Some(record) = self.entries[index]
                && capabilities.contains(record.action)
            {
                self.entries[retained] = Some(record);
                retained += 1;
            }
        }
        self.entries[retained..self.len].fill(None);
        self.len = retained;
    }

    #[cfg(test)]
    fn clear(&mut self) {
        self.entries[..self.len].fill(None);
        self.len = 0;
    }

    const fn len(&self) -> usize {
        self.len
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeerPhase {
    AwaitingHello,
    SendingWelcome,
    Ready,
}

#[derive(Debug)]
struct SystemActionPeer {
    fd: OwnedFd,
    token: ReactorToken,
    phase: PeerPhase,
    handshake_deadline_ns: u64,
    negotiated_minor: u8,
    pending_capabilities: Option<AstreaSystemActionCapabilities>,
    welcome: Option<[u8; PACKET_SIZE]>,
    outbound: FixedActionQueue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SystemActionSubmitResult {
    Sent,
    Queued,
    Unavailable,
    Disconnected,
}

#[derive(Debug)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

#[derive(Debug)]
struct SocketPathGuard {
    path: PathBuf,
    identity: SocketIdentity,
}

impl Drop for SocketPathGuard {
    fn drop(&mut self) {
        remove_socket_if_identity(&self.path, &self.identity);
    }
}

#[derive(Debug)]
pub(crate) struct NativeSystemActionTransport {
    listener: Option<OwnedFd>,
    listener_token: Option<ReactorToken>,
    peer: Option<SystemActionPeer>,
    socket_path: Option<SocketPathGuard>,
    pending_capabilities: Option<AstreaSystemActionCapabilities>,
    next_sequence: u64,
    telemetry: SystemActionTransportTelemetry,
}

impl NativeSystemActionTransport {
    pub(crate) fn unavailable() -> Self {
        Self {
            listener: None,
            listener_token: None,
            peer: None,
            socket_path: None,
            pending_capabilities: None,
            next_sequence: 1,
            telemetry: SystemActionTransportTelemetry::default(),
        }
    }

    pub(crate) fn bind(
        event_loop: &mut NativeEventLoop,
        runtime_dir: &Path,
        instance: &str,
    ) -> io::Result<Self> {
        let paths = ControlRuntimePaths::for_runtime_dir(runtime_dir, instance)
            .map_err(io::Error::other)?;
        let owner_uid = effective_uid();
        let socket_path = paths
            .prepare_system_action_endpoint()
            .map_err(io::Error::other)?;
        // Validate the final endpoint too: the temporary bind name is shorter
        // and could otherwise fit even when the public socket path does not.
        let _ = unix_socket_address(&socket_path)?;
        remove_stale_socket(&socket_path, owner_uid)?;
        let (listener, identity) = create_listener(&socket_path, owner_uid)?;
        let token = match event_loop.register(
            listener.as_raw_fd(),
            NativeEventSource::SystemActionListener,
        ) {
            Ok(token) => token,
            Err(error) => {
                remove_socket_if_identity(&socket_path, &identity);
                return Err(error);
            }
        };
        Ok(Self {
            listener: Some(listener),
            listener_token: Some(token),
            peer: None,
            socket_path: Some(SocketPathGuard {
                path: socket_path,
                identity,
            }),
            pending_capabilities: None,
            next_sequence: 1,
            telemetry: SystemActionTransportTelemetry::default(),
        })
    }

    pub(crate) fn take_capability_update(&mut self) -> Option<AstreaSystemActionCapabilities> {
        self.pending_capabilities.take()
    }

    pub(crate) fn next_deadline_ns(&self) -> Option<u64> {
        self.peer
            .as_ref()
            .and_then(|peer| (peer.phase != PeerPhase::Ready).then_some(peer.handshake_deadline_ns))
    }

    pub(crate) fn snapshot(&self) -> SystemActionTransportSnapshot {
        SystemActionTransportSnapshot {
            peer_connected: self.peer.is_some(),
            protocol_ready: self
                .peer
                .as_ref()
                .is_some_and(|peer| peer.phase == PeerPhase::Ready),
            outbound_queue_depth: self.peer.as_ref().map_or(0, |peer| peer.outbound.len()),
            telemetry: self.telemetry,
        }
    }

    pub(crate) fn service(
        &mut self,
        event_loop: &mut NativeEventLoop,
        events: &oblivion_one::native::event_loop::SystemActionReadyEvents,
        now_ns: u64,
    ) {
        if self.peer.as_ref().is_some_and(|peer| {
            peer.phase != PeerPhase::Ready && peer.handshake_deadline_ns <= now_ns
        }) {
            self.disconnect_peer(event_loop, true);
        }

        let listener_ready = self.listener_token.is_some_and(|token| {
            events.iter().any(|event| {
                event.token == token
                    && event.flags & (libc::EPOLLERR | libc::EPOLLHUP | libc::EPOLLRDHUP) as u32
                        == 0
            })
        });
        let listener_terminal = self.listener_token.is_some_and(|token| {
            events.iter().any(|event| {
                event.token == token
                    && event.flags & (libc::EPOLLERR | libc::EPOLLHUP | libc::EPOLLRDHUP) as u32
                        != 0
            })
        });
        if listener_terminal {
            self.disable_listener(event_loop);
        } else if listener_ready {
            self.accept_ready_peers(event_loop, now_ns);
        }

        let Some(peer_token) = self.peer.as_ref().map(|peer| peer.token) else {
            return;
        };
        let Some(event) = events
            .iter()
            .find(|event| event.token == peer_token)
            .copied()
        else {
            return;
        };
        if event.flags & (libc::EPOLLERR | libc::EPOLLHUP | libc::EPOLLRDHUP) as u32 != 0 {
            self.disconnect_peer(event_loop, true);
            return;
        }
        if event.flags & libc::EPOLLIN as u32 != 0 && !self.receive_packets(event_loop) {
            return;
        }
        if event.flags & libc::EPOLLOUT as u32 != 0 {
            self.flush_outbound(event_loop);
        }
        let _ = self.update_peer_interest(event_loop);
    }

    pub(crate) fn submit(
        &mut self,
        event_loop: &mut NativeEventLoop,
        action: AstreaSystemAction,
    ) -> SystemActionSubmitResult {
        let Some(peer) = self.peer.as_mut() else {
            return SystemActionSubmitResult::Unavailable;
        };
        if peer.phase != PeerPhase::Ready {
            return SystemActionSubmitResult::Unavailable;
        }
        if peer.outbound.len() > 0 {
            return match peer.outbound.push(action) {
                QueuePush::Queued => {
                    if self.update_peer_interest(event_loop) {
                        SystemActionSubmitResult::Queued
                    } else {
                        SystemActionSubmitResult::Disconnected
                    }
                }
                QueuePush::Coalesced => {
                    self.telemetry.action_records_coalesced =
                        self.telemetry.action_records_coalesced.saturating_add(1);
                    SystemActionSubmitResult::Queued
                }
                QueuePush::Full => {
                    self.telemetry.queue_overflows =
                        self.telemetry.queue_overflows.saturating_add(1);
                    self.disconnect_peer(event_loop, true);
                    SystemActionSubmitResult::Disconnected
                }
            };
        }
        let Some(sequence_after_send) = self.next_sequence.checked_add(1) else {
            self.disconnect_peer(event_loop, true);
            return SystemActionSubmitResult::Disconnected;
        };
        let packet = encode(SystemActionMessage::Action {
            sequence: self.next_sequence,
            action,
            occurrences: 1,
        });
        self.next_sequence = sequence_after_send;
        match send_packet(peer.fd.as_raw_fd(), &packet) {
            Ok(()) => {
                self.note_action_sent(1);
                SystemActionSubmitResult::Sent
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                self.telemetry.send_would_block = self.telemetry.send_would_block.saturating_add(1);
                let push = peer.outbound.push(action);
                if push == QueuePush::Full {
                    self.telemetry.queue_overflows =
                        self.telemetry.queue_overflows.saturating_add(1);
                    self.disconnect_peer(event_loop, true);
                    return SystemActionSubmitResult::Disconnected;
                }
                if push == QueuePush::Coalesced {
                    self.telemetry.action_records_coalesced =
                        self.telemetry.action_records_coalesced.saturating_add(1);
                }
                if self.update_peer_interest(event_loop) {
                    SystemActionSubmitResult::Queued
                } else {
                    SystemActionSubmitResult::Disconnected
                }
            }
            Err(_) => {
                self.disconnect_peer(event_loop, true);
                SystemActionSubmitResult::Disconnected
            }
        }
    }

    pub(crate) fn shutdown(&mut self, event_loop: &mut NativeEventLoop) {
        self.pending_capabilities = Some(AstreaSystemActionCapabilities::EMPTY);
        self.disconnect_peer(event_loop, false);
        self.disable_listener(event_loop);
        self.socket_path.take();
    }

    fn accept_ready_peers(&mut self, event_loop: &mut NativeEventLoop, now_ns: u64) {
        for _ in 0..MAX_SYSTEM_ACTION_ACCEPTS_PER_CYCLE {
            let Some(listener) = self.listener.as_ref() else {
                return;
            };
            let fd = unsafe {
                libc::accept4(
                    listener.as_raw_fd(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                )
            };
            if fd < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::Interrupted
                {
                    if error.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return;
                }
                self.disable_listener(event_loop);
                return;
            }
            self.telemetry.listener_accepts = self.telemetry.listener_accepts.saturating_add(1);
            let peer_fd = unsafe { OwnedFd::from_raw_fd(fd) };
            if !peer_uid(fd).is_some_and(|uid| peer_uid_matches(uid, effective_uid())) {
                self.telemetry.peer_rejections = self.telemetry.peer_rejections.saturating_add(1);
                continue;
            }
            if self.peer.is_some() {
                self.telemetry.peer_rejections = self.telemetry.peer_rejections.saturating_add(1);
                let reject = encode(SystemActionMessage::Reject {
                    reason: SYSTEM_ACTION_REJECT_BUSY,
                });
                let _ = send_packet(fd, &reject);
                continue;
            }
            let token = match event_loop.register(fd, NativeEventSource::SystemActionPeer) {
                Ok(token) => token,
                Err(_) => {
                    self.telemetry.peer_rejections =
                        self.telemetry.peer_rejections.saturating_add(1);
                    continue;
                }
            };
            self.peer = Some(SystemActionPeer {
                fd: peer_fd,
                token,
                phase: PeerPhase::AwaitingHello,
                handshake_deadline_ns: now_ns.saturating_add(SYSTEM_ACTION_HANDSHAKE_TIMEOUT_NS),
                negotiated_minor: 0,
                pending_capabilities: None,
                welcome: None,
                outbound: FixedActionQueue::default(),
            });
            let _ = self.update_peer_interest(event_loop);
        }
    }

    fn receive_packets(&mut self, event_loop: &mut NativeEventLoop) -> bool {
        for _ in 0..MAX_SYSTEM_ACTION_INBOUND_PACKETS_PER_CYCLE {
            let Some(peer) = self.peer.as_ref() else {
                return false;
            };
            let mut packet = [0_u8; PACKET_SIZE + 1];
            let received = unsafe {
                libc::recv(
                    peer.fd.as_raw_fd(),
                    packet.as_mut_ptr().cast(),
                    packet.len(),
                    libc::MSG_DONTWAIT | libc::MSG_TRUNC,
                )
            };
            if received < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::WouldBlock {
                    return true;
                }
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                self.disconnect_peer(event_loop, true);
                return false;
            }
            if received == 0 {
                self.disconnect_peer(event_loop, true);
                return false;
            }
            if received as usize != PACKET_SIZE {
                self.protocol_error(event_loop);
                return false;
            }
            let message = match decode(&packet[..PACKET_SIZE]) {
                Ok(message) => message,
                Err(_) => {
                    self.protocol_error(event_loop);
                    return false;
                }
            };
            if !self.handle_message(event_loop, message) {
                return false;
            }
        }
        // The reactor is level-triggered. If the bounded receive budget is
        // exhausted while packets remain, EPOLLIN remains asserted and wakes
        // the next cycle without an auxiliary timer or continuation.
        true
    }

    fn handle_message(
        &mut self,
        event_loop: &mut NativeEventLoop,
        message: SystemActionMessage,
    ) -> bool {
        let phase = match self.peer.as_ref() {
            Some(peer) => peer.phase,
            None => return false,
        };
        match (phase, message) {
            (
                PeerPhase::AwaitingHello,
                SystemActionMessage::Hello {
                    major,
                    minor,
                    capabilities,
                },
            ) => {
                if major != PROTOCOL_MAJOR {
                    self.best_effort_reject(event_loop, SYSTEM_ACTION_REJECT_MAJOR_MISMATCH);
                    return false;
                }
                let accepted =
                    AstreaSystemActionCapabilities::from_wire_bits_truncate(capabilities);
                let negotiated_minor = minor.min(PROTOCOL_MINOR);
                let welcome = encode(SystemActionMessage::Welcome {
                    major: PROTOCOL_MAJOR,
                    minor: negotiated_minor,
                    capabilities: accepted.wire_bits(),
                });
                if let Some(peer) = self.peer.as_mut() {
                    peer.pending_capabilities = Some(accepted);
                    peer.negotiated_minor = negotiated_minor;
                    peer.welcome = Some(welcome);
                    peer.phase = PeerPhase::SendingWelcome;
                }
                match self.send_welcome() {
                    Ok(true) => {
                        self.activate_peer();
                        self.update_peer_interest(event_loop)
                    }
                    Ok(false) => self.update_peer_interest(event_loop),
                    Err(_) => {
                        self.disconnect_peer(event_loop, true);
                        false
                    }
                }
            }
            (PeerPhase::Ready, SystemActionMessage::CapabilitiesChanged { capabilities }) => {
                if let Some(peer) = self.peer.as_ref()
                    && peer.negotiated_minor < PROTOCOL_MINOR
                {
                    self.protocol_error(event_loop);
                    return false;
                }
                let capabilities =
                    AstreaSystemActionCapabilities::from_wire_bits_truncate(capabilities);
                if let Some(peer) = self.peer.as_mut() {
                    peer.outbound.retain_capabilities(capabilities);
                }
                self.publish_capability_update(capabilities);
                self.update_peer_interest(event_loop)
            }
            _ => {
                self.protocol_error(event_loop);
                false
            }
        }
    }

    fn send_welcome(&mut self) -> io::Result<bool> {
        let Some(peer) = self.peer.as_mut() else {
            return Ok(false);
        };
        let Some(packet) = peer.welcome else {
            return Ok(false);
        };
        match send_packet(peer.fd.as_raw_fd(), &packet) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                self.telemetry.send_would_block = self.telemetry.send_would_block.saturating_add(1);
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    fn activate_peer(&mut self) {
        let Some(peer) = self.peer.as_mut() else {
            return;
        };
        peer.phase = PeerPhase::Ready;
        peer.welcome = None;
        let capabilities = peer.pending_capabilities.take().unwrap_or_default();
        self.telemetry.handshake_successes = self.telemetry.handshake_successes.saturating_add(1);
        self.publish_capability_update(capabilities);
    }

    fn flush_outbound(&mut self, event_loop: &mut NativeEventLoop) {
        if self
            .peer
            .as_ref()
            .is_some_and(|peer| peer.phase == PeerPhase::SendingWelcome)
        {
            match self.send_welcome() {
                Ok(true) => self.activate_peer(),
                Ok(false) => return,
                Err(_) => {
                    self.disconnect_peer(event_loop, true);
                    return;
                }
            }
        }
        for _ in 0..MAX_SYSTEM_ACTION_OUTBOUND_PACKETS_PER_CYCLE {
            let Some((fd, record)) = self.peer.as_ref().and_then(|peer| {
                peer.outbound
                    .front()
                    .map(|record| (peer.fd.as_raw_fd(), record))
            }) else {
                break;
            };
            let Some(sequence_after_send) = self.next_sequence.checked_add(1) else {
                self.disconnect_peer(event_loop, true);
                return;
            };
            let packet = encode(SystemActionMessage::Action {
                sequence: self.next_sequence,
                action: record.action,
                occurrences: record.occurrences,
            });
            match send_packet(fd, &packet) {
                Ok(()) => {
                    self.next_sequence = sequence_after_send;
                    if let Some(peer) = self.peer.as_mut() {
                        peer.outbound.pop();
                    }
                    self.note_action_sent(record.occurrences);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.telemetry.send_would_block =
                        self.telemetry.send_would_block.saturating_add(1);
                    break;
                }
                Err(_) => {
                    self.disconnect_peer(event_loop, true);
                    return;
                }
            }
        }
        let _ = self.update_peer_interest(event_loop);
    }

    fn note_action_sent(&mut self, occurrences: u32) {
        self.telemetry.actions_sent = self.telemetry.actions_sent.saturating_add(1);
        self.telemetry.action_occurrences_sent = self
            .telemetry
            .action_occurrences_sent
            .saturating_add(u64::from(occurrences));
    }

    fn update_peer_interest(&mut self, event_loop: &mut NativeEventLoop) -> bool {
        let Some((token, interests)) = self.peer.as_ref().map(|peer| {
            let mut interests = PEER_EVENTS;
            if peer.phase == PeerPhase::SendingWelcome || peer.outbound.len() > 0 {
                interests |= libc::EPOLLOUT as u32;
            }
            (peer.token, interests)
        }) else {
            return true;
        };
        match event_loop.modify(token, interests) {
            Ok(true) => true,
            Ok(false) | Err(_) => {
                self.disconnect_peer(event_loop, true);
                false
            }
        }
    }

    fn publish_capability_update(&mut self, capabilities: AstreaSystemActionCapabilities) {
        self.pending_capabilities = Some(capabilities);
        self.telemetry.capability_updates = self.telemetry.capability_updates.saturating_add(1);
    }

    fn best_effort_reject(&mut self, event_loop: &mut NativeEventLoop, reason: u16) {
        self.telemetry.peer_rejections = self.telemetry.peer_rejections.saturating_add(1);
        let packet = encode(SystemActionMessage::Reject { reason });
        if let Some(peer) = self.peer.as_ref() {
            let _ = send_packet(peer.fd.as_raw_fd(), &packet);
        }
        self.disconnect_peer(event_loop, false);
    }

    fn protocol_error(&mut self, event_loop: &mut NativeEventLoop) {
        self.telemetry.protocol_errors = self.telemetry.protocol_errors.saturating_add(1);
        self.best_effort_reject(event_loop, SYSTEM_ACTION_REJECT_PROTOCOL);
    }

    fn disconnect_peer(&mut self, event_loop: &mut NativeEventLoop, count_disconnect: bool) {
        let Some(peer) = self.peer.take() else {
            return;
        };
        let _ = event_loop.unregister(peer.token);
        if count_disconnect {
            self.telemetry.peer_disconnects = self.telemetry.peer_disconnects.saturating_add(1);
        }
        if peer.phase != PeerPhase::Ready {
            self.telemetry.handshake_failures = self.telemetry.handshake_failures.saturating_add(1);
        } else {
            self.publish_capability_update(AstreaSystemActionCapabilities::EMPTY);
        }
    }

    fn disable_listener(&mut self, event_loop: &mut NativeEventLoop) {
        if let Some(token) = self.listener_token.take() {
            let _ = event_loop.unregister(token);
        }
        self.listener.take();
        self.socket_path.take();
    }
}

fn send_packet(fd: RawFd, packet: &[u8; PACKET_SIZE]) -> io::Result<()> {
    loop {
        let sent = unsafe {
            libc::send(
                fd,
                packet.as_ptr().cast(),
                packet.len(),
                libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
            )
        };
        if sent == packet.len() as isize {
            return Ok(());
        }
        if sent < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        return Err(io::Error::other("short SOCK_SEQPACKET system-action send"));
    }
}

fn peer_uid(fd: RawFd) -> Option<u32> {
    let mut credentials = unsafe { std::mem::zeroed::<libc::ucred>() };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    (result == 0 && length as usize == std::mem::size_of::<libc::ucred>())
        .then_some(credentials.uid)
}

fn effective_uid() -> u32 {
    unsafe { libc::geteuid() }
}

fn peer_uid_matches(peer: u32, owner: u32) -> bool {
    peer == owner
}

fn create_listener(path: &Path, owner_uid: u32) -> io::Result<(OwnedFd, SocketIdentity)> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let (temporary_path, listener, identity) = loop {
        let temp_path = path.with_file_name(format!(
            ".sa-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let (address, address_len) = unix_socket_address(&temp_path)?;
        let raw_fd = unsafe {
            libc::socket(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                0,
            )
        };
        if raw_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let listener = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        let bound = unsafe {
            libc::bind(
                listener.as_raw_fd(),
                (&address as *const libc::sockaddr_un).cast(),
                address_len,
            )
        };
        if bound < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EADDRINUSE) {
                continue;
            }
            return Err(error);
        }
        let identity = match socket_identity(&temp_path) {
            Ok(identity) => identity,
            Err(error) => {
                remove_owned_socket(&temp_path, owner_uid);
                return Err(error);
            }
        };
        if let Err(error) = fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o600)) {
            remove_socket_if_identity(&temp_path, &identity);
            return Err(error);
        }
        if let Err(error) = verify_socket_identity(&temp_path, owner_uid, &identity) {
            remove_socket_if_identity(&temp_path, &identity);
            return Err(error);
        }
        let result = unsafe { libc::listen(listener.as_raw_fd(), 16) };
        if result < 0 {
            remove_socket_if_identity(&temp_path, &identity);
            return Err(io::Error::last_os_error());
        }
        break (temp_path, listener, identity);
    };
    if let Err(error) = rename_noreplace(&temporary_path, path) {
        remove_socket_if_identity(&temporary_path, &identity);
        return Err(error);
    }
    if let Err(error) = verify_socket_identity(path, owner_uid, &identity) {
        remove_socket_if_identity(path, &identity);
        return Err(error);
    }
    Ok((listener, identity))
}

fn remove_stale_socket(path: &Path, owner_uid: u32) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_socket()
        || metadata.uid() != owner_uid
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "system-action endpoint path is not an owned socket",
        ));
    }
    let identity = SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    if socket_is_live(path)? {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "system-action endpoint already has a listener",
        ));
    }
    let revalidated = fs::symlink_metadata(path)?;
    if !revalidated.file_type().is_socket()
        || revalidated.uid() != owner_uid
        || revalidated.dev() != identity.device
        || revalidated.ino() != identity.inode
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "system-action endpoint changed during stale cleanup",
        ));
    }
    remove_socket_if_identity(path, &identity);
    Ok(())
}

fn socket_is_live(path: &Path) -> io::Result<bool> {
    let (address, address_len) = unix_socket_address(path)?;
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let result = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            address_len,
        )
    };
    if result == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ECONNREFUSED | libc::ENOENT) => Ok(false),
        Some(libc::EINPROGRESS | libc::EAGAIN | libc::EALREADY) => Ok(true),
        _ => Err(error),
    }
}

fn socket_identity(path: &Path) -> io::Result<SocketIdentity> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bound path is not a socket",
        ));
    }
    Ok(SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn verify_socket_identity(
    path: &Path,
    owner_uid: u32,
    identity: &SocketIdentity,
) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != owner_uid
        || metadata.mode() & 0o777 != 0o600
        || metadata.dev() != identity.device
        || metadata.ino() != identity.inode
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "system-action socket identity verification failed",
        ));
    }
    Ok(())
}

fn remove_socket_if_identity(path: &Path, identity: &SocketIdentity) {
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

fn remove_owned_socket(path: &Path, owner_uid: u32) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_socket() && metadata.uid() == owner_uid {
        let _ = fs::remove_file(path);
    }
}

fn unix_socket_address(path: &Path) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    let bytes = path.as_os_str().as_bytes();
    let mut address = unsafe { std::mem::zeroed::<libc::sockaddr_un>() };
    if bytes.is_empty() || bytes.len() + 1 > address.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "system-action socket path exceeds Unix-domain path limit",
        ));
    }
    if bytes.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "system-action socket path contains NUL",
        ));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (destination, source) in address.sun_path.iter_mut().zip(bytes.iter().copied()) {
        *destination = source as libc::c_char;
    }
    let length = (std::mem::size_of_val(&address.sun_family) + bytes.len() + 1) as libc::socklen_t;
    Ok((address, length))
}

fn rename_noreplace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::ffi::CString;
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source path contains NUL"))?;
    let destination = CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "destination path contains NUL")
    })?;
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[path = "system_action_transport_tests.rs"]
mod tests;
