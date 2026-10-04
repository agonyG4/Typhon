//! Reverse XDND target-side state. This module keeps root-target wire and
//! XdndSelection progress separate from Clipboard/PRIMARY and C2 transfers.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    num::NonZeroU64,
    os::fd::{AsRawFd, OwnedFd, RawFd},
};

use x11rb::{
    connection::{Connection, RequestConnection, SequenceNumber},
    cookie::Cookie,
    protocol::xproto::{self, Atom, AtomEnum, ConnectionExt as XprotoConnectionExt, Window},
    wrapper::ConnectionExt as XprotoWrapperExt,
};

use super::super::{X11WindowHandle, XwaylandGeneration, Xwm, XwmError, atoms::XwmAtomName};
use crate::xwayland::{
    MAX_PENDING_XWAYLAND_DND_INCOMING_EVENTS, XwaylandDndIncomingEvent,
    XwaylandDndIncomingPositionId, XwaylandDndOfferId,
};

pub(crate) const TARGET_STATUS_TIMEOUT_NS: u64 = 1_000_000_000;
pub(crate) const TARGET_METADATA_TIMEOUT_NS: u64 = 2_000_000_000;
pub(crate) const MAX_PENDING_INCOMING_DND_REPLIES: usize = 64;
pub(crate) const MAX_ACTIVE_INCOMING_DND_TRANSFERS: usize = 16;
pub(crate) const MAX_PENDING_INCOMING_DND_TRANSFER_REPLIES: usize = 4;
pub(crate) const MAX_INCOMING_DND_CHUNK_BYTES: usize = 64 * 1024;
pub(crate) const INCOMING_DND_IDLE_TIMEOUT_NS: u64 = 30_000_000_000;
pub(crate) const INCOMING_DND_DROP_ACK_TIMEOUT_NS: u64 = 1_000_000_000;
pub(crate) const INCOMING_DND_TERMINAL_TIMEOUT_NS: u64 = 60_000_000_000;
pub(crate) const INCOMING_DND_TERMINAL_MAX_LIFETIME_NS: u64 = 600_000_000_000;
pub(crate) const INCOMING_DND_DELETE_TIMEOUT_NS: u64 = 2_000_000_000;
const ROOT_PROXY_VERIFY_TIMEOUT_NS: u64 = 1_000_000_000;

const INCOMING_DND_CHUNK_UNITS: u32 = (MAX_INCOMING_DND_CHUNK_BYTES / 4) as u32;
const XDND_STATUS_WANT_POSITION_UPDATES: u32 = 1 << 1;
const MAX_WRITE_CALLS_PER_DISPATCH: usize = 32;
const MAX_EINTR_RETRIES_PER_DISPATCH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct IncomingPosition {
    pub(crate) position_id: XwaylandDndIncomingPositionId,
    pub(crate) root_x: f64,
    pub(crate) root_y: f64,
    /// Exact X timestamp used for XdndSelection conversion authority.
    pub(crate) timestamp: u32,
    pub(crate) requested_action: crate::xwayland::XwaylandDndAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IncomingDndWirePhase {
    Hover,
    DropSubmitted {
        drop_timestamp: u32,
        action: crate::xwayland::XwaylandDndAction,
        acknowledgement_deadline_ns: u64,
    },
    AwaitingWaylandFinish {
        drop_timestamp: u32,
        action: crate::xwayland::XwaylandDndAction,
        deadline_ns: u64,
        hard_deadline_ns: u64,
        cancel_submitted: bool,
    },
    DeletePending {
        drop_timestamp: u32,
        final_action: crate::xwayland::XwaylandDndAction,
    },
    TerminalConsumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RootProxyAuthority {
    #[default]
    Unpublished,
    Owned {
        generation: XwaylandGeneration,
        proxy: Window,
    },
    BlockedForeign {
        generation: XwaylandGeneration,
        proxy: Window,
    },
    Verifying {
        generation: XwaylandGeneration,
        proxy: Window,
        sequence: SequenceNumber,
        deadline_ns: u64,
    },
    Lost {
        generation: XwaylandGeneration,
        proxy: Window,
    },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct IncomingMoveDelete {
    pub(crate) offer_id: XwaylandDndOfferId,
    pub(crate) generation: XwaylandGeneration,
    pub(crate) source: Window,
    pub(crate) requestor: Window,
    pub(crate) property: Atom,
    pub(crate) drop_timestamp: u32,
    pub(crate) deadline_ns: u64,
}

#[derive(Debug)]
pub(crate) struct IncomingDndSession {
    pub(crate) generation: XwaylandGeneration,
    pub(crate) offer_id: XwaylandDndOfferId,
    pub(crate) source: X11WindowHandle,
    pub(crate) logical_target_root: Window,
    pub(crate) target_proxy: Window,
    pub(crate) version: crate::xwayland::XwaylandDndVersion,
    #[allow(dead_code)] // retained for exact provisional-enter diagnostics/tests
    pub(crate) inline_mime_atoms: Vec<Atom>,
    pub(crate) mime_atoms: Vec<Atom>,
    #[allow(dead_code)] // distinguishes inline metadata from XdndTypeList replies
    pub(crate) more_types: bool,
    pub(crate) type_list_complete: bool,
    pub(crate) pending_atom_names: usize,
    pub(crate) metadata_complete: bool,
    pub(crate) mime_types: Vec<String>,
    pub(crate) atom_to_mime: BTreeMap<Atom, String>,
    pub(crate) source_actions: Vec<crate::xwayland::XwaylandDndAction>,
    pub(crate) available_actions: Vec<crate::xwayland::XwaylandDndAction>,
    pub(crate) action_list_required: bool,
    pub(crate) action_list_cached: bool,
    pub(crate) action_list_complete: bool,
    pub(crate) latest_position: Option<IncomingPosition>,
    pub(crate) wire_phase: IncomingDndWirePhase,
    pub(crate) next_position_serial: u64,
    pub(crate) canonical_started: bool,
    pub(crate) pending_status_deadline_ns: Option<u64>,
    pub(crate) status_pending: bool,
    pub(crate) accepted_mime: Option<String>,
    pub(crate) selected_action: Option<crate::xwayland::XwaylandDndAction>,
    pub(crate) metadata_deadline_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IncomingTransferPhase {
    AwaitSelectionNotify,
    ReadingProperty,
    WaitingForIncrValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IncomingPropertyMode {
    Initial,
    Direct,
    Incr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContinueAfterWrite {
    ReadMoreProperty,
    ReadNextIncrChunk,
    Finish,
    None,
}

#[derive(Debug)]
struct IncomingTransfer {
    id: crate::xwayland::XwaylandDndIncomingTransferId,
    generation: XwaylandGeneration,
    offer_id: XwaylandDndOfferId,
    source: Window,
    requestor: Window,
    target: Atom,
    property: Atom,
    selection_timestamp: u32,
    sink: Option<OwnedFd>,
    phase: IncomingTransferPhase,
    mode: IncomingPropertyMode,
    offset_units: u32,
    bytes_after: u32,
    expected_type_format: Option<(Atom, u8)>,
    buffer: Vec<u8>,
    written: usize,
    continue_after_write: ContinueAfterWrite,
    idle_deadline_ns: u64,
    sink_writable_interest: bool,
    reactor_token: Option<u64>,
    pending_reply: Option<SequenceNumber>,
}

#[derive(Debug, Default)]
pub(crate) struct DndIncomingManager {
    pub(crate) generation: Option<XwaylandGeneration>,
    pub(crate) target_proxy: Option<Window>,
    pub(crate) next_offer_serial: u64,
    pub(crate) next_transfer_serial: u64,
    pub(crate) root_proxy_authority: RootProxyAuthority,
    pub(crate) move_delete: Option<IncomingMoveDelete>,
    pub(crate) events: VecDeque<XwaylandDndIncomingEvent>,
    pub(crate) pending: BTreeMap<SequenceNumber, PendingMetadataReply>,
    transfers: BTreeMap<crate::xwayland::XwaylandDndIncomingTransferId, IncomingTransfer>,
    requestors: HashMap<Window, crate::xwayland::XwaylandDndIncomingTransferId>,
    transfer_replies: BTreeMap<SequenceNumber, crate::xwayland::XwaylandDndIncomingTransferId>,
    retired_requestors: HashSet<Window>,
}

mod terminal;

#[derive(Debug, Clone, Copy)]
pub(crate) enum PendingMetadataReply {
    TypeList {
        offer_id: XwaylandDndOfferId,
        deadline_ns: u64,
    },
    ActionList {
        offer_id: XwaylandDndOfferId,
        position_id: XwaylandDndIncomingPositionId,
        deadline_ns: u64,
    },
    AtomName {
        offer_id: XwaylandDndOfferId,
        atom: Atom,
        deadline_ns: u64,
    },
}

impl DndIncomingManager {
    pub(crate) fn allocate_offer_id(
        &mut self,
        generation: XwaylandGeneration,
    ) -> Option<XwaylandDndOfferId> {
        if self.generation != Some(generation) {
            self.generation = Some(generation);
            self.next_offer_serial = 0;
        }
        self.next_offer_serial = self.next_offer_serial.checked_add(1)?;
        NonZeroU64::new(self.next_offer_serial)
            .map(|serial| XwaylandDndOfferId::new(generation, serial))
    }

    pub(crate) fn push_event(&mut self, event: XwaylandDndIncomingEvent) -> bool {
        if let XwaylandDndIncomingEvent::Position { offer_id, .. } = &event
            && let Some(existing) = self.events.iter_mut().rev().find(|entry| {
                matches!(entry, XwaylandDndIncomingEvent::Position { offer_id: queued, .. } if queued == offer_id)
            })
        {
            *existing = event;
            return true;
        }
        let is_position = matches!(event, XwaylandDndIncomingEvent::Position { .. });
        let is_begin = matches!(event, XwaylandDndIncomingEvent::Begin { .. });
        // Every admitted Begin reserves one bounded slot for its eventual
        // Leave. Coalesced Positions cannot consume that edge capacity.
        let limit = if is_position || is_begin {
            MAX_PENDING_XWAYLAND_DND_INCOMING_EVENTS.saturating_sub(1)
        } else {
            MAX_PENDING_XWAYLAND_DND_INCOMING_EVENTS
        };
        if self.events.len() >= limit {
            if !is_position
                && let Some(index) = self
                    .events
                    .iter()
                    .position(|queued| matches!(queued, XwaylandDndIncomingEvent::Position { .. }))
            {
                self.events.remove(index);
            }
            if self.events.len() >= limit {
                return false;
            }
        }
        self.events.push_back(event);
        true
    }

    pub(crate) fn take_events(&mut self) -> Vec<XwaylandDndIncomingEvent> {
        self.events.drain(..).collect()
    }

    fn allocate_transfer_id(
        &mut self,
        offer_id: XwaylandDndOfferId,
    ) -> Option<crate::xwayland::XwaylandDndIncomingTransferId> {
        self.next_transfer_serial = self.next_transfer_serial.checked_add(1)?;
        NonZeroU64::new(self.next_transfer_serial)
            .map(|serial| crate::xwayland::XwaylandDndIncomingTransferId::new(offer_id, serial))
    }

    pub(crate) fn clear_generation(&mut self, generation: XwaylandGeneration) {
        if self.generation != Some(generation) {
            return;
        }
        self.generation = None;
        self.target_proxy = None;
        self.root_proxy_authority = RootProxyAuthority::Unpublished;
        self.move_delete = None;
        self.pending.clear();
        self.events.clear();
        self.transfers.clear();
        self.requestors.clear();
        self.transfer_replies.clear();
        self.retired_requestors.clear();
    }

    pub(crate) fn sink_interests(
        &self,
    ) -> impl Iterator<Item = (crate::xwayland::XwaylandDndIncomingTransferId, RawFd)> + '_ {
        self.transfers.values().filter_map(|transfer| {
            (transfer.sink_writable_interest && transfer.sink.is_some())
                .then(|| {
                    transfer
                        .sink
                        .as_ref()
                        .map(|sink| (transfer.id, sink.as_raw_fd()))
                })
                .flatten()
        })
    }

    pub(crate) fn bind_reactor_token(
        &mut self,
        id: crate::xwayland::XwaylandDndIncomingTransferId,
        token: Option<u64>,
    ) {
        if let Some(transfer) = self.transfers.get_mut(&id) {
            transfer.reactor_token = token;
        }
    }

    fn transfer_matches_reactor(
        &self,
        id: crate::xwayland::XwaylandDndIncomingTransferId,
        generation: XwaylandGeneration,
        token: u64,
    ) -> bool {
        self.transfers.get(&id).is_some_and(|transfer| {
            transfer.generation == generation
                && transfer.reactor_token == Some(token)
                && transfer.sink.is_some()
        })
    }

    pub(crate) fn owns_requestor(&self, window: Window) -> bool {
        self.requestors.contains_key(&window)
            || self.retired_requestors.contains(&window)
            || self
                .move_delete
                .is_some_and(|delete| delete.requestor == window)
    }
}

/// Create a private, 1x1 InputOnly protocol window. It is never mapped or
/// registered in the desktop window registry. Root discovery is activated
/// separately, after XWM startup has initialized the complete terminal path.
pub(crate) fn initialize_target_proxy(xwm: &mut Xwm) -> Result<(), XwmError> {
    if xwm.data_bridge.dnd_incoming.generation == Some(xwm.generation)
        && xwm.data_bridge.dnd_incoming.target_proxy.is_some()
    {
        return Ok(());
    }
    let proxy = xwm
        .connection
        .generate_id()
        .map_err(|error| XwmError::IdAllocation(error.to_string()))?;
    let create = xwm
        .connection
        .create_window(
            0,
            proxy,
            xwm.root,
            0,
            0,
            1,
            1,
            0,
            xproto::WindowClass::INPUT_ONLY,
            0,
            &xproto::CreateWindowAux::new().event_mask(
                xproto::EventMask::PROPERTY_CHANGE | xproto::EventMask::STRUCTURE_NOTIFY,
            ),
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(create);
    let self_proxy = xwm
        .connection
        .change_property32(
            xproto::PropMode::REPLACE,
            proxy,
            xwm.atoms.get(XwmAtomName::XdndProxy),
            xproto::AtomEnum::WINDOW,
            &[proxy],
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(self_proxy);
    let aware = xwm
        .connection
        .change_property32(
            xproto::PropMode::REPLACE,
            proxy,
            xwm.atoms.get(XwmAtomName::XdndAware),
            xproto::AtomEnum::ATOM,
            &[5],
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(aware);
    xwm.data_bridge.dnd.internal_windows.insert(proxy);
    xwm.data_bridge.dnd_incoming.generation = Some(xwm.generation);
    xwm.data_bridge.dnd_incoming.target_proxy = Some(proxy);
    xwm.data_bridge.dnd_incoming.root_proxy_authority = RootProxyAuthority::Unpublished;
    Ok(())
}

pub(crate) fn target_proxy(xwm: &Xwm) -> Option<Window> {
    (xwm.data_bridge.dnd_incoming.generation == Some(xwm.generation))
        .then_some(xwm.data_bridge.dnd_incoming.target_proxy)
        .flatten()
}

fn root_proxy_may_be_replaced(existing: Option<Window>, valid: bool, own_proxy: Window) -> bool {
    existing.is_none() || existing == Some(own_proxy) || !valid
}

fn root_proxy_should_be_released(current: Option<Window>, own_proxy: Window) -> bool {
    current == Some(own_proxy)
}

fn read_single_u32_property(
    xwm: &Xwm,
    window: Window,
    property: Atom,
    property_type: Atom,
) -> Result<Option<u32>, XwmError> {
    let cookie = xwm
        .connection
        .get_property(false, window, property, AtomEnum::ANY, 0, 2)
        .map_err(XwmError::Connection)?;
    xwm.connection.flush().map_err(XwmError::Connection)?;
    // Startup acquisition holds the server grab and must decide from replies
    // to these exact reads. ReactorStream deliberately never blocks in normal
    // event handling, so wait for the startup reply before consuming it.
    wait_for_startup_reply(xwm)?;
    let reply = cookie.reply_unchecked().map_err(XwmError::Connection)?;
    let Some(reply) = reply.filter(|reply| {
        reply.type_ == property_type && reply.format == 32 && reply.bytes_after == 0
    }) else {
        return Ok(None);
    };
    let values = reply
        .value32()
        .map(|values| values.collect::<Vec<_>>())
        .unwrap_or_default();
    Ok((values.len() == 1).then_some(values[0]))
}

fn wait_for_startup_reply(xwm: &Xwm) -> Result<(), XwmError> {
    const TIMEOUT_MS: i32 = 1_000;
    let mut descriptor = libc::pollfd {
        fd: xwm.connection.stream().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `descriptor` points to one valid XWM socket descriptor.
    let result = unsafe { libc::poll(&mut descriptor, 1, TIMEOUT_MS) };
    if result > 0 {
        Ok(())
    } else if result == 0 {
        Err(XwmError::Connection(
            x11rb::errors::ConnectionError::IoError(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "X11 server did not answer root-proxy acquisition in time",
            )),
        ))
    } else {
        Err(XwmError::Connection(
            x11rb::errors::ConnectionError::IoError(std::io::Error::last_os_error()),
        ))
    }
}

fn foreign_root_proxy_is_valid(xwm: &Xwm, proxy: Window) -> Result<bool, XwmError> {
    let self_proxy = read_single_u32_property(
        xwm,
        proxy,
        xwm.atoms.get(XwmAtomName::XdndProxy),
        u32::from(AtomEnum::WINDOW),
    )?;
    if self_proxy != Some(proxy) {
        return Ok(false);
    }
    let aware = read_single_u32_property(
        xwm,
        proxy,
        xwm.atoms.get(XwmAtomName::XdndAware),
        u32::from(AtomEnum::ATOM),
    )?;
    Ok(aware.is_some_and(|version| version >= 4))
}

/// Acquire root discovery only after the complete reverse terminal path is
/// ready. The server grab keeps foreign-owner validation and publication atomic.
pub(crate) fn acquire_root_proxy(xwm: &mut Xwm) -> Result<bool, XwmError> {
    let Some(proxy) = target_proxy(xwm) else {
        return Ok(false);
    };
    let grab = xwm.connection.grab_server().map_err(XwmError::Connection)?;
    std::mem::forget(grab);
    let result = (|| {
        let current = read_single_u32_property(
            xwm,
            xwm.root,
            xwm.atoms.get(XwmAtomName::XdndProxy),
            u32::from(AtomEnum::WINDOW),
        )?;
        let valid = current
            .filter(|current| *current != proxy)
            .map(|current| foreign_root_proxy_is_valid(xwm, current))
            .transpose()?
            .unwrap_or(false);
        let proxy_self_reference = read_single_u32_property(
            xwm,
            proxy,
            xwm.atoms.get(XwmAtomName::XdndProxy),
            u32::from(AtomEnum::WINDOW),
        )?;
        let proxy_version = read_single_u32_property(
            xwm,
            proxy,
            xwm.atoms.get(XwmAtomName::XdndAware),
            u32::from(AtomEnum::ATOM),
        )?;
        let target_ready = proxy_self_reference == Some(proxy)
            && proxy_version.is_some_and(|version| version >= 5);
        if !target_ready {
            return Ok((false, current, valid, false));
        }
        if !root_proxy_may_be_replaced(current, valid, proxy) {
            return Ok((false, current, valid, true));
        }
        let cookie = xwm
            .connection
            .change_property32(
                xproto::PropMode::REPLACE,
                xwm.root,
                xwm.atoms.get(XwmAtomName::XdndProxy),
                AtomEnum::WINDOW,
                &[proxy],
            )
            .map_err(XwmError::Connection)?;
        std::mem::forget(cookie);
        Ok((true, current, valid, true))
    })();
    let ungrab = match xwm.connection.ungrab_server() {
        Ok(cookie) => {
            std::mem::forget(cookie);
            Ok(())
        }
        Err(error) => Err(XwmError::Connection(error)),
    };
    xwm.connection.flush().map_err(XwmError::Connection)?;
    match result {
        Err(error) => Err(error),
        Ok((acquired, current, valid, target_ready)) => {
            ungrab?;
            xwm.data_bridge.dnd_incoming.root_proxy_authority = if !target_ready {
                RootProxyAuthority::Lost {
                    generation: xwm.generation,
                    proxy,
                }
            } else if acquired || current == Some(proxy) {
                RootProxyAuthority::Owned {
                    generation: xwm.generation,
                    proxy,
                }
            } else if valid {
                RootProxyAuthority::BlockedForeign {
                    generation: xwm.generation,
                    proxy: current.unwrap_or_default(),
                }
            } else {
                RootProxyAuthority::Lost {
                    generation: xwm.generation,
                    proxy,
                }
            };
            Ok(acquired)
        }
    }
}

pub(crate) fn root_proxy_is_owned(xwm: &Xwm) -> bool {
    let Some(proxy) = target_proxy(xwm) else {
        return false;
    };
    matches!(
        xwm.data_bridge.dnd_incoming.root_proxy_authority,
        RootProxyAuthority::Owned { generation, proxy: owned_proxy }
            if generation == xwm.generation && owned_proxy == proxy
    )
}

fn begin_root_proxy_verification(xwm: &mut Xwm, now_ns: u64) -> Result<(), XwmError> {
    let (generation, proxy, superseded_sequence) =
        match xwm.data_bridge.dnd_incoming.root_proxy_authority {
            RootProxyAuthority::Owned { generation, proxy } => (generation, proxy, None),
            RootProxyAuthority::Verifying {
                generation,
                proxy,
                sequence,
                ..
            } => (generation, proxy, Some(sequence)),
            _ => return Ok(()),
        };
    if generation != xwm.generation || target_proxy(xwm) != Some(proxy) {
        return Ok(());
    }
    if let Some(sequence) = superseded_sequence {
        xwm.connection.discard_reply(
            sequence,
            x11rb::connection::RequestKind::HasResponse,
            x11rb::connection::DiscardMode::DiscardReply,
        );
    }
    let cookie = xwm
        .connection
        .get_property(
            false,
            xwm.root,
            xwm.atoms.get(XwmAtomName::XdndProxy),
            u32::from(AtomEnum::WINDOW),
            0,
            2,
        )
        .map_err(XwmError::Connection)?;
    let sequence = cookie.sequence_number();
    std::mem::forget(cookie);
    xwm.data_bridge.dnd_incoming.root_proxy_authority = RootProxyAuthority::Verifying {
        generation,
        proxy,
        sequence,
        deadline_ns: now_ns.saturating_add(ROOT_PROXY_VERIFY_TIMEOUT_NS),
    };
    xwm.connection.flush().map_err(XwmError::Connection)
}

pub(crate) fn poll_root_proxy_verification(xwm: &mut Xwm, now_ns: u64) -> Result<bool, XwmError> {
    let RootProxyAuthority::Verifying {
        generation,
        proxy,
        sequence,
        deadline_ns,
    } = xwm.data_bridge.dnd_incoming.root_proxy_authority
    else {
        return Ok(false);
    };
    if generation != xwm.generation || target_proxy(xwm) != Some(proxy) {
        return Ok(false);
    }
    if now_ns >= deadline_ns {
        xwm.connection.discard_reply(
            sequence,
            x11rb::connection::RequestKind::HasResponse,
            x11rb::connection::DiscardMode::DiscardReply,
        );
        mark_root_proxy_lost(xwm, generation, proxy)?;
        return Ok(true);
    }
    let cookie = Cookie::<super::super::connection::X11Connection, xproto::GetPropertyReply>::new(
        &xwm.connection,
        sequence,
    );
    let reply = match cookie.reply_unchecked() {
        Ok(reply) => reply,
        Err(x11rb::errors::ConnectionError::IoError(error))
            if error.kind() == std::io::ErrorKind::WouldBlock =>
        {
            return Ok(false);
        }
        Err(error) => return Err(XwmError::Connection(error)),
    };
    let still_owned = reply.is_some_and(|reply| {
        if reply.type_ != u32::from(AtomEnum::WINDOW)
            || reply.format != 32
            || reply.bytes_after != 0
        {
            return false;
        }
        let values = reply
            .value32()
            .map(|values| values.collect::<Vec<_>>())
            .unwrap_or_default();
        values.as_slice() == [proxy]
    });
    if still_owned {
        xwm.data_bridge.dnd_incoming.root_proxy_authority =
            RootProxyAuthority::Owned { generation, proxy };
    } else {
        mark_root_proxy_lost(xwm, generation, proxy)?;
    }
    Ok(true)
}

fn mark_root_proxy_lost(
    xwm: &mut Xwm,
    generation: XwaylandGeneration,
    proxy: Window,
) -> Result<(), XwmError> {
    if generation != xwm.generation || target_proxy(xwm) != Some(proxy) {
        return Ok(());
    }
    xwm.data_bridge.dnd_incoming.root_proxy_authority =
        RootProxyAuthority::Lost { generation, proxy };
    terminal::root_proxy_lost(xwm)
}

/// Release root discovery only while the property still names our exact
/// generation-owned proxy.
pub(crate) fn release_root_proxy(xwm: &mut Xwm) -> Result<bool, XwmError> {
    let Some(proxy) = xwm.data_bridge.dnd_incoming.target_proxy else {
        return Ok(false);
    };
    let grab = xwm.connection.grab_server().map_err(XwmError::Connection)?;
    std::mem::forget(grab);
    let result = (|| {
        let current = read_single_u32_property(
            xwm,
            xwm.root,
            xwm.atoms.get(XwmAtomName::XdndProxy),
            u32::from(AtomEnum::WINDOW),
        )?;
        if !root_proxy_should_be_released(current, proxy) {
            return Ok(false);
        }
        let cookie = xwm
            .connection
            .delete_property(xwm.root, xwm.atoms.get(XwmAtomName::XdndProxy))
            .map_err(XwmError::Connection)?;
        std::mem::forget(cookie);
        Ok(true)
    })();
    let ungrab = match xwm.connection.ungrab_server() {
        Ok(cookie) => {
            std::mem::forget(cookie);
            Ok(())
        }
        Err(error) => Err(XwmError::Connection(error)),
    };
    xwm.connection.flush().map_err(XwmError::Connection)?;
    match result {
        Err(error) => Err(error),
        Ok(released) => {
            ungrab?;
            Ok(released)
        }
    }
}

pub(crate) fn is_logical_root_target(xwm: &Xwm, message: &xproto::ClientMessageEvent) -> bool {
    message.window == xwm.root
}

fn is_internal_proxy_target(xwm: &Xwm, message: &xproto::ClientMessageEvent) -> bool {
    target_proxy(xwm).is_some_and(|proxy| {
        proxy == message.window && xwm.data_bridge.dnd.internal_windows.contains(&proxy)
    })
}

pub(crate) fn is_exact_source(
    session: &IncomingDndSession,
    generation: XwaylandGeneration,
    source: Window,
    logical_target: Window,
    proxy: Window,
) -> bool {
    session.generation == generation
        && session.source.xid() == source
        && session.logical_target_root == logical_target
        && session.target_proxy == proxy
}

pub(crate) fn retire_proxy(xwm: &mut Xwm, generation: XwaylandGeneration) {
    if xwm.data_bridge.dnd_incoming.generation != Some(generation) {
        return;
    }
    let _ = release_root_proxy(xwm);
    let requestors = xwm
        .data_bridge
        .dnd_incoming
        .requestors
        .keys()
        .copied()
        .collect::<Vec<_>>();
    if let Some(delete) = xwm.data_bridge.dnd_incoming.move_delete.take() {
        xwm.data_bridge
            .dnd
            .internal_windows
            .remove(&delete.requestor);
        let _ = xwm.connection.destroy_window(delete.requestor);
    }
    for requestor in requestors {
        let _ = xwm.connection.destroy_window(requestor);
    }
    let sequences = xwm
        .data_bridge
        .dnd_incoming
        .pending
        .keys()
        .chain(xwm.data_bridge.dnd_incoming.transfer_replies.keys())
        .copied()
        .chain(match xwm.data_bridge.dnd_incoming.root_proxy_authority {
            RootProxyAuthority::Verifying { sequence, .. } => Some(sequence),
            _ => None,
        })
        .collect::<Vec<_>>();
    for sequence in sequences {
        xwm.connection.discard_reply(
            sequence,
            x11rb::connection::RequestKind::HasResponse,
            x11rb::connection::DiscardMode::DiscardReply,
        );
    }
    if let Some(proxy) = xwm.data_bridge.dnd_incoming.target_proxy.take() {
        xwm.data_bridge.dnd.internal_windows.remove(&proxy);
        let _ = xwm.connection.destroy_window(proxy);
    }
    xwm.data_bridge.dnd_incoming.clear_generation(generation);
}

pub(crate) fn client_message(
    xwm: &mut Xwm,
    event: xproto::ClientMessageEvent,
    now_ns: u64,
) -> Result<bool, XwmError> {
    let message_type = event.type_;
    let is_incoming = [
        XwmAtomName::XdndEnter,
        XwmAtomName::XdndPosition,
        XwmAtomName::XdndLeave,
        XwmAtomName::XdndDrop,
    ]
    .into_iter()
    .any(|name| message_type == xwm.atoms.get(name));
    if !is_incoming {
        return Ok(false);
    }
    // A conforming root-proxy source sends to the proxy but keeps the actual
    // logical target in ClientMessage.window. The proxy XID is infrastructure,
    // never the protocol target. Consume malformed traffic addressed to our
    // private proxy before generic application-window adoption.
    if is_internal_proxy_target(xwm, &event) {
        return Ok(true);
    }
    if !is_logical_root_target(xwm, &event) {
        return Ok(false);
    }
    if !root_proxy_is_owned(xwm) {
        return Ok(true);
    }
    if event.format != 32 {
        return Ok(true);
    }
    let data = event.data.as_data32();
    if message_type == xwm.atoms.get(XwmAtomName::XdndEnter) {
        metadata::begin_enter(xwm, data, now_ns)?;
    } else if message_type == xwm.atoms.get(XwmAtomName::XdndPosition) {
        metadata::position(xwm, data, now_ns)?;
    } else if message_type == xwm.atoms.get(XwmAtomName::XdndLeave) {
        metadata::leave(xwm, data[0])?;
    } else if message_type == xwm.atoms.get(XwmAtomName::XdndDrop) {
        terminal::drop_received(xwm, data, now_ns)?;
    }
    Ok(true)
}

pub(crate) fn property_notify(
    xwm: &mut Xwm,
    event: xproto::PropertyNotifyEvent,
    now_ns: u64,
) -> Result<bool, XwmError> {
    if event.window == xwm.root && event.atom == xwm.atoms.get(XwmAtomName::XdndProxy) {
        begin_root_proxy_verification(xwm, now_ns)?;
        return Ok(true);
    }
    if terminal::property_notify(xwm, event) {
        return Ok(true);
    }
    transfer::property_notify(xwm, event, now_ns)
}

pub(crate) fn resolve_drop(
    xwm: &mut Xwm,
    offer_id: XwaylandDndOfferId,
    accepted: bool,
    now_ns: u64,
) -> Result<(), XwmError> {
    terminal::resolve_drop(xwm, offer_id, accepted, now_ns)
}

pub(crate) fn resolve_cancel_after_drop(
    xwm: &mut Xwm,
    offer_id: XwaylandDndOfferId,
    cancelled: bool,
) -> Result<(), XwmError> {
    terminal::resolve_cancel_after_drop(xwm, offer_id, cancelled)
}

pub(crate) fn apply_transition(
    xwm: &mut Xwm,
    transition: crate::xwayland::XwaylandDndTransition,
    now_ns: u64,
) -> Result<(), XwmError> {
    match transition {
        feedback @ crate::xwayland::XwaylandDndTransition::SourceFeedback { .. } => {
            apply_source_feedback_transition(xwm, feedback)
        }
        crate::xwayland::XwaylandDndTransition::SourceFinished {
            offer_id,
            accepted,
            action,
        } => source_finished(xwm, offer_id, accepted, action, now_ns),
        _ => Ok(()),
    }
}

pub(crate) fn source_finished(
    xwm: &mut Xwm,
    offer_id: XwaylandDndOfferId,
    accepted: bool,
    action: Option<crate::xwayland::XwaylandDndAction>,
    now_ns: u64,
) -> Result<(), XwmError> {
    terminal::source_finished(xwm, offer_id, accepted, action, now_ns)
}

pub(crate) fn requestor_destroyed(xwm: &mut Xwm, requestor: Window) -> bool {
    terminal::requestor_destroyed(xwm, requestor) || transfer::requestor_destroyed(xwm, requestor)
}

pub(crate) fn selection_notify(
    xwm: &mut Xwm,
    event: xproto::SelectionNotifyEvent,
    now_ns: u64,
) -> Result<bool, XwmError> {
    if terminal::selection_notify_delete(xwm, event)? {
        return Ok(true);
    }
    transfer::selection_notify(xwm, event, now_ns)
}

pub(crate) fn expire_deadlines(xwm: &mut Xwm, now_ns: u64) -> Result<(), XwmError> {
    metadata::expire_deadlines(xwm, now_ns)?;
    terminal::expire_deadlines(xwm, now_ns)
}

mod metadata;
#[cfg(test)]
pub(crate) mod tests;
mod transfer;

#[cfg(test)]
use metadata::representable_source_actions;
#[cfg(test)]
pub(crate) use metadata::source_feedback;
pub(crate) use metadata::{apply_source_feedback_transition, next_deadline_ns, poll_replies};
pub(crate) use terminal::{canonical_retired, source_destroyed};
pub(crate) use transfer::{handle_sink_ready, start_data_request};
#[cfg(test)]
use transfer::{next_after_property_chunk, write_sink_bytes};
