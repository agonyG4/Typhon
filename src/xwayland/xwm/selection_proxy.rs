//! Wayland-to-X11 selection ownership and request serving.
//!
//! Private proxy owners become authoritative only after the X server confirms
//! ownership. Selection requests then enter the bounded reverse-serving engine.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    io,
    num::NonZeroU64,
};

use x11rb::{
    CURRENT_TIME, NONE,
    connection::{Connection, DiscardMode, RequestConnection, RequestKind, SequenceNumber},
    cookie::Cookie,
    protocol::xproto::{self, Atom, ConnectionExt as _, Property, Window, WindowClass},
    wrapper::ConnectionExt as XprotoWrapperExt,
};

use super::{
    Xwm, XwmError,
    data_bridge::{BridgeGeneration, SelectionKind},
};
use crate::xwayland::{XwaylandProxySelectionId, XwaylandSelectionKind};

pub(crate) const MAX_PENDING_PROXY_SELECTION_REQUESTS: usize = 64;
pub(crate) const MAX_MULTIPLE_PAIRS: usize = 64;
pub(crate) const MAX_PENDING_OUTGOING_SELECTION_REPLIES: usize = 4;
pub(crate) const OUTGOING_SELECTION_IDLE_TIMEOUT_NS: u64 =
    super::selection_outgoing::OUTGOING_SELECTION_IDLE_TIMEOUT_NS;
const PROXY_OWNERSHIP_TIMEOUT_NS: u64 = 5_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ProxySelectionRequestId(NonZeroU64);

impl ProxySelectionRequestId {
    pub(crate) const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProxyServingAuthority {
    id: XwaylandProxySelectionId,
    ownership_timestamp: u32,
}

#[derive(Debug, Default)]
struct ProxySelectionChannel {
    owner_window: Option<Window>,
    authority: Option<ProxyServingAuthority>,
    desired_id: Option<XwaylandProxySelectionId>,
    suppressed_id: Option<XwaylandProxySelectionId>,
    held_ownership_timestamp: Option<u32>,
    claim_phase: ProxyClaimPhase,
    disabled: bool,
}

#[derive(Debug, Default, Clone, Copy)]
enum ProxyClaimPhase {
    #[default]
    Idle,
    AwaitingTimestamp {
        probe_id: XwaylandProxySelectionId,
        sequence: SequenceNumber,
        deadline_ns: u64,
    },
    AwaitingOwnerConfirmation {
        id: XwaylandProxySelectionId,
        timestamp: u32,
        sequence: SequenceNumber,
        deadline_ns: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct NotificationSignature {
    requestor: Window,
    selection: Atom,
    target: Atom,
    time: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestState {
    NeedsMultipleRead,
    ProcessingMultiple,
    WaitingTransfer,
    ReadySuccess,
    ReadyFailure,
}

#[derive(Debug)]
struct MultipleState {
    property: Atom,
    pairs: Vec<(Atom, Atom)>,
    cursor: usize,
}

#[derive(Debug)]
struct TopLevelRequest {
    id: ProxySelectionRequestId,
    generation: BridgeGeneration,
    kind: SelectionKind,
    signature: NotificationSignature,
    original_property: Atom,
    effective_property: Atom,
    arrival_sequence: u64,
    deadline_ns: u64,
    requestor_monitored: bool,
    state: RequestState,
    multiple: Option<MultipleState>,
}

#[derive(Debug, Clone, Copy)]
struct PendingMultipleReply {
    request_id: ProxySelectionRequestId,
    generation: BridgeGeneration,
}

#[derive(Debug, Clone, Copy)]
struct RequestorMonitor {
    references: usize,
    restore_mask: xproto::EventMask,
}

#[derive(Debug)]
pub(crate) struct SelectionProxyManager {
    active_generation: Option<BridgeGeneration>,
    channels: [ProxySelectionChannel; 2],
    requests: BTreeMap<ProxySelectionRequestId, TopLevelRequest>,
    notification_order: HashMap<NotificationSignature, VecDeque<ProxySelectionRequestId>>,
    pending_multiple_replies: BTreeMap<SequenceNumber, PendingMultipleReply>,
    requestor_refs: HashMap<Window, RequestorMonitor>,
    next_request_id: u64,
    next_arrival_sequence: u64,
    reply_turn: SelectionKind,
}

impl Default for SelectionProxyManager {
    fn default() -> Self {
        Self {
            active_generation: None,
            channels: std::array::from_fn(|_| ProxySelectionChannel::default()),
            requests: BTreeMap::new(),
            notification_order: HashMap::new(),
            pending_multiple_replies: BTreeMap::new(),
            requestor_refs: HashMap::new(),
            next_request_id: 0,
            next_arrival_sequence: 0,
            reply_turn: SelectionKind::Clipboard,
        }
    }
}

impl SelectionProxyManager {
    pub(crate) fn clear_generation(&mut self, generation: BridgeGeneration) -> Vec<SequenceNumber> {
        if self.active_generation != Some(generation) {
            return Vec::new();
        }
        let mut sequences: Vec<SequenceNumber> = std::mem::take(&mut self.pending_multiple_replies)
            .into_keys()
            .collect();
        for channel in &self.channels {
            if let ProxyClaimPhase::AwaitingOwnerConfirmation { sequence, .. } = channel.claim_phase
            {
                sequences.push(sequence);
            }
        }
        self.active_generation = None;
        self.channels = std::array::from_fn(|_| ProxySelectionChannel::default());
        self.requests.clear();
        self.notification_order.clear();
        self.requestor_refs.clear();
        self.reply_turn = SelectionKind::Clipboard;
        sequences
    }
}

pub(crate) fn initialize(xwm: &mut Xwm) -> Result<(), XwmError> {
    let generation = BridgeGeneration::from(xwm.generation);
    if xwm.data_bridge.selection_proxy.active_generation == Some(generation) {
        return Ok(());
    }
    if let Some(old_generation) = xwm.data_bridge.selection_proxy.active_generation {
        for sequence in xwm
            .data_bridge
            .selection_proxy
            .clear_generation(old_generation)
        {
            xwm.connection.discard_reply(
                sequence,
                RequestKind::HasResponse,
                DiscardMode::DiscardReply,
            );
        }
    }
    xwm.data_bridge.selection_proxy.active_generation = Some(generation);
    for kind in [SelectionKind::Clipboard, SelectionKind::Primary] {
        let owner = xwm
            .connection
            .generate_id()
            .map_err(|error| XwmError::IdAllocation(error.to_string()))?;
        let cookie = xwm
            .connection
            .create_window(
                0,
                owner,
                xwm.supporting_wm_check,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &xproto::CreateWindowAux::new().event_mask(xproto::EventMask::PROPERTY_CHANGE),
            )
            .map_err(XwmError::Connection)?;
        std::mem::forget(cookie);
        xwm.data_bridge
            .selection_wire
            .register_internal_window(owner);
        xwm.data_bridge
            .selection_proxy
            .channel_mut(kind)
            .owner_window = Some(owner);
    }
    Ok(())
}

impl SelectionProxyManager {
    fn channel(&self, kind: SelectionKind) -> &ProxySelectionChannel {
        &self.channels[kind_index(kind)]
    }

    fn channel_mut(&mut self, kind: SelectionKind) -> &mut ProxySelectionChannel {
        &mut self.channels[kind_index(kind)]
    }

    pub(crate) fn owner_window(&self, kind: SelectionKind) -> Option<Window> {
        self.channel(kind).owner_window
    }

    fn owner_kind(&self, owner: Window) -> Option<SelectionKind> {
        [SelectionKind::Clipboard, SelectionKind::Primary]
            .into_iter()
            .find(|kind| self.channel(*kind).owner_window == Some(owner))
    }

    #[cfg(test)]
    pub(crate) fn install_test_authority(
        &mut self,
        kind: SelectionKind,
        id: XwaylandProxySelectionId,
        ownership_timestamp: u32,
    ) -> bool {
        if self.channel(kind).owner_window.is_none() {
            return false;
        }
        self.channel_mut(kind).authority = Some(ProxyServingAuthority {
            id,
            ownership_timestamp,
        });
        true
    }
}

#[cfg(test)]
pub(crate) fn install_test_authority(
    xwm: &mut Xwm,
    kind: SelectionKind,
    id: XwaylandProxySelectionId,
    ownership_timestamp: u32,
) -> bool {
    let Some(prepared) = super::selection_wire::prepared_proxy_selection(xwm, kind) else {
        return false;
    };
    if prepared.id != id {
        return false;
    }
    xwm.data_bridge
        .selection_proxy
        .install_test_authority(kind, id, ownership_timestamp)
}

#[cfg(test)]
pub(crate) fn disable_claim_for_test(xwm: &mut Xwm, kind: SelectionKind) {
    let channel = xwm.data_bridge.selection_proxy.channel_mut(kind);
    channel.desired_id = None;
    channel.suppressed_id = None;
    channel.held_ownership_timestamp = None;
    channel.claim_phase = ProxyClaimPhase::Idle;
}

pub(crate) fn desired_proxy_selection_changed(
    xwm: &mut Xwm,
    kind: SelectionKind,
    desired_id: Option<XwaylandProxySelectionId>,
    now_ns: u64,
) -> Result<(), XwmError> {
    let old_id = xwm.data_bridge.selection_proxy.channel(kind).desired_id;
    if old_id == desired_id {
        return Ok(());
    }

    let revoke_authority = xwm
        .data_bridge
        .selection_proxy
        .channel(kind)
        .authority
        .is_some_and(|authority| Some(authority.id) != desired_id);
    {
        let channel = xwm.data_bridge.selection_proxy.channel_mut(kind);
        channel.desired_id = desired_id;
        if desired_id.is_some_and(|id| channel.suppressed_id != Some(id)) {
            channel.suppressed_id = None;
        }
        if revoke_authority {
            channel.authority = None;
        }
    }
    if revoke_authority {
        cancel_channel(xwm, kind, now_ns)?;
    }

    let phase = xwm.data_bridge.selection_proxy.channel(kind).claim_phase;
    match (desired_id, phase) {
        (None, ProxyClaimPhase::AwaitingTimestamp { .. }) => {
            // The property event has no request identity. Keep this probe
            // outstanding until its one event is consumed.
            if let Some(timestamp) = xwm
                .data_bridge
                .selection_proxy
                .channel(kind)
                .held_ownership_timestamp
            {
                safe_release(xwm, kind, timestamp)?;
                xwm.data_bridge
                    .selection_proxy
                    .channel_mut(kind)
                    .held_ownership_timestamp = None;
            }
        }
        (
            None,
            ProxyClaimPhase::AwaitingOwnerConfirmation {
                sequence,
                timestamp,
                ..
            },
        ) => {
            discard_owner_confirmation(xwm, kind, sequence);
            xwm.data_bridge
                .selection_proxy
                .channel_mut(kind)
                .claim_phase = ProxyClaimPhase::Idle;
            safe_release(xwm, kind, timestamp)?;
            xwm.data_bridge
                .selection_proxy
                .channel_mut(kind)
                .held_ownership_timestamp = None;
        }
        (None, ProxyClaimPhase::Idle) => {
            if let Some(timestamp) = xwm
                .data_bridge
                .selection_proxy
                .channel(kind)
                .held_ownership_timestamp
            {
                safe_release(xwm, kind, timestamp)?;
                xwm.data_bridge
                    .selection_proxy
                    .channel_mut(kind)
                    .held_ownership_timestamp = None;
            }
        }
        (Some(_), ProxyClaimPhase::AwaitingOwnerConfirmation { sequence, .. }) => {
            discard_owner_confirmation(xwm, kind, sequence);
            xwm.data_bridge
                .selection_proxy
                .channel_mut(kind)
                .claim_phase = ProxyClaimPhase::Idle;
        }
        _ => {}
    }

    if desired_id.is_some() {
        reconcile_channel(xwm, kind, now_ns)?;
    }
    xwm.connection.flush().map_err(XwmError::Connection)
}

pub(crate) fn prepared_catalog_changed(
    xwm: &mut Xwm,
    kind: SelectionKind,
    now_ns: u64,
) -> Result<(), XwmError> {
    reconcile_channel(xwm, kind, now_ns)?;
    xwm.connection.flush().map_err(XwmError::Connection)
}

fn reconcile_channel(xwm: &mut Xwm, kind: SelectionKind, now_ns: u64) -> Result<(), XwmError> {
    let generation = BridgeGeneration::from(xwm.generation);
    let manager = &xwm.data_bridge.selection_proxy;
    let channel = manager.channel(kind);
    if manager.active_generation != Some(generation)
        || !xwm.capabilities.xfixes
        || channel.disabled
        || !matches!(channel.claim_phase, ProxyClaimPhase::Idle)
    {
        return Ok(());
    }
    let Some(id) = channel.desired_id else {
        return Ok(());
    };
    if channel.suppressed_id == Some(id) {
        return Ok(());
    }
    let Some(prepared) = super::selection_wire::prepared_proxy_selection(xwm, kind) else {
        return Ok(());
    };
    if prepared.id != id {
        return Ok(());
    }
    if prepared.data_targets.is_empty() {
        let held_timestamp = channel.held_ownership_timestamp;
        let revoke = xwm
            .data_bridge
            .selection_proxy
            .channel(kind)
            .authority
            .is_some();
        if revoke {
            xwm.data_bridge.selection_proxy.channel_mut(kind).authority = None;
            cancel_channel(xwm, kind, now_ns)?;
        }
        if let Some(timestamp) = held_timestamp {
            safe_release(xwm, kind, timestamp)?;
            xwm.data_bridge
                .selection_proxy
                .channel_mut(kind)
                .held_ownership_timestamp = None;
        }
        return Ok(());
    }
    if xwm
        .data_bridge
        .selection_proxy
        .channel(kind)
        .authority
        .is_some_and(|authority| authority.id == id)
    {
        return Ok(());
    }
    let Some(owner) = channel.owner_window else {
        return Ok(());
    };
    let cookie = xwm
        .connection
        .change_property8(
            xproto::PropMode::APPEND,
            owner,
            xwm.atoms.get(super::atoms::XwmAtomName::SelectionProxyTime),
            xproto::AtomEnum::INTEGER,
            &[],
        )
        .map_err(XwmError::Connection)?;
    let sequence = cookie.sequence_number();
    std::mem::forget(cookie);
    xwm.data_bridge
        .selection_proxy
        .channel_mut(kind)
        .claim_phase = ProxyClaimPhase::AwaitingTimestamp {
        probe_id: id,
        sequence,
        deadline_ns: now_ns.saturating_add(PROXY_OWNERSHIP_TIMEOUT_NS),
    };
    Ok(())
}

fn safe_release(xwm: &Xwm, kind: SelectionKind, timestamp: u32) -> Result<(), XwmError> {
    let cookie = xwm
        .connection
        .set_selection_owner(NONE, selection_atom(xwm, kind), timestamp)
        .map_err(XwmError::Connection)?;
    std::mem::forget(cookie);
    Ok(())
}

fn discard_owner_confirmation(xwm: &mut Xwm, kind: SelectionKind, sequence: SequenceNumber) {
    xwm.data_bridge
        .selection_proxy
        .channel_mut(kind)
        .claim_phase = ProxyClaimPhase::Idle;
    xwm.connection.discard_reply(
        sequence,
        RequestKind::HasResponse,
        DiscardMode::DiscardReply,
    );
}

pub(crate) fn cancel_channel(
    xwm: &mut Xwm,
    kind: SelectionKind,
    now_ns: u64,
) -> Result<(), XwmError> {
    let ids = xwm
        .data_bridge
        .selection_proxy
        .requests
        .values()
        .filter(|request| request.kind == kind)
        .map(|request| request.id)
        .collect::<Vec<_>>();
    for id in &ids {
        cancel_pending_reply_for(xwm, *id);
        super::selection_outgoing::cancel_owner(xwm, *id, now_ns)?;
    }
    super::selection_outgoing::cancel_channel(xwm, public_kind(kind))?;
    for request in xwm.data_bridge.selection_proxy.requests.values_mut() {
        if request.kind == kind {
            request.state = RequestState::ReadyFailure;
        }
    }
    flush_ordered_notifies(xwm)
}

pub(crate) fn owns_timestamp_property(xwm: &Xwm, window: Window, atom: Atom) -> bool {
    atom == xwm.atoms.get(super::atoms::XwmAtomName::SelectionProxyTime)
        && xwm.data_bridge.selection_proxy.owner_kind(window).is_some()
}

pub(crate) fn timestamp_property_notify(
    xwm: &mut Xwm,
    event: xproto::PropertyNotifyEvent,
    now_ns: u64,
) -> Result<(), XwmError> {
    let Some(kind) = xwm.data_bridge.selection_proxy.owner_kind(event.window) else {
        return Ok(());
    };
    if event.atom != xwm.atoms.get(super::atoms::XwmAtomName::SelectionProxyTime)
        || event.state != Property::NEW_VALUE
    {
        return Ok(());
    }
    let phase = xwm.data_bridge.selection_proxy.channel(kind).claim_phase;
    let ProxyClaimPhase::AwaitingTimestamp {
        probe_id,
        sequence: _probe_sequence,
        deadline_ns,
    } = phase
    else {
        return Ok(());
    };
    xwm.data_bridge
        .selection_proxy
        .channel_mut(kind)
        .claim_phase = ProxyClaimPhase::Idle;
    let eligible = {
        let channel = xwm.data_bridge.selection_proxy.channel(kind);
        let prepared = super::selection_wire::prepared_proxy_selection(xwm, kind);
        channel.desired_id == Some(probe_id)
            && channel.suppressed_id != Some(probe_id)
            && !channel.disabled
            && now_ns < deadline_ns
            && xwm.capabilities.xfixes
            && xwm.data_bridge.selection_proxy.active_generation
                == Some(BridgeGeneration::from(xwm.generation))
            && prepared.is_some_and(|prepared| {
                prepared.id == probe_id && !prepared.data_targets.is_empty()
            })
    };
    if eligible {
        let owner = xwm
            .data_bridge
            .selection_proxy
            .owner_window(kind)
            .expect("timestamp probe owner window remains generation local");
        let timestamp = event.time;
        let set_cookie = xwm
            .connection
            .set_selection_owner(owner, selection_atom(xwm, kind), timestamp)
            .map_err(XwmError::Connection)?;
        std::mem::forget(set_cookie);
        let get_cookie = xwm
            .connection
            .get_selection_owner(selection_atom(xwm, kind))
            .map_err(XwmError::Connection)?;
        let sequence = get_cookie.sequence_number();
        std::mem::forget(get_cookie);
        let channel = xwm.data_bridge.selection_proxy.channel_mut(kind);
        channel.held_ownership_timestamp = Some(timestamp);
        channel.claim_phase = ProxyClaimPhase::AwaitingOwnerConfirmation {
            id: probe_id,
            timestamp,
            sequence,
            deadline_ns: now_ns.saturating_add(PROXY_OWNERSHIP_TIMEOUT_NS),
        };
    } else if now_ns >= deadline_ns {
        let channel = xwm.data_bridge.selection_proxy.channel_mut(kind);
        channel.suppressed_id = Some(probe_id);
    }
    reconcile_channel(xwm, kind, now_ns)?;
    xwm.connection.flush().map_err(XwmError::Connection)
}

pub(crate) fn selection_clear(
    xwm: &mut Xwm,
    event: xproto::SelectionClearEvent,
    now_ns: u64,
) -> Result<bool, XwmError> {
    let Some(kind) = xwm.data_bridge.selection_proxy.owner_kind(event.owner) else {
        return Ok(false);
    };
    if event.selection != selection_atom(xwm, kind) {
        return Ok(false);
    }
    let phase = xwm.data_bridge.selection_proxy.channel(kind).claim_phase;
    {
        let channel = xwm.data_bridge.selection_proxy.channel_mut(kind);
        if let Some(id) = channel.desired_id {
            channel.suppressed_id = Some(id);
        }
        channel.authority = None;
        channel.held_ownership_timestamp = None;
    }
    if let ProxyClaimPhase::AwaitingOwnerConfirmation { sequence, .. } = phase {
        discard_owner_confirmation(xwm, kind, sequence);
    }
    cancel_channel(xwm, kind, now_ns)?;
    xwm.connection.flush().map_err(XwmError::Connection)?;
    Ok(true)
}

pub(crate) fn observe_xfixes_owner_transition(
    xwm: &mut Xwm,
    kind: SelectionKind,
    owner: Option<Window>,
    _selection_timestamp: u32,
    now_ns: u64,
) -> Result<(), XwmError> {
    let channel = xwm.data_bridge.selection_proxy.channel(kind);
    let Some(proxy_owner) = channel.owner_window else {
        return Ok(());
    };
    if owner == Some(proxy_owner) {
        return Ok(());
    }
    let phase = channel.claim_phase;
    let had_activity = channel.authority.is_some()
        || channel.held_ownership_timestamp.is_some()
        || !matches!(phase, ProxyClaimPhase::Idle);
    if !had_activity {
        return Ok(());
    }
    let claim_id = match phase {
        ProxyClaimPhase::AwaitingTimestamp { probe_id, .. }
        | ProxyClaimPhase::AwaitingOwnerConfirmation { id: probe_id, .. } => Some(probe_id),
        ProxyClaimPhase::Idle => None,
    };
    {
        let channel = xwm.data_bridge.selection_proxy.channel_mut(kind);
        if let Some(id) = channel.desired_id.or(claim_id) {
            channel.suppressed_id = Some(id);
        }
        channel.authority = None;
        channel.held_ownership_timestamp = None;
    }
    if let ProxyClaimPhase::AwaitingOwnerConfirmation { sequence, .. } = phase {
        discard_owner_confirmation(xwm, kind, sequence);
    }
    cancel_channel(xwm, kind, now_ns)?;
    xwm.connection.flush().map_err(XwmError::Connection)
}

pub(crate) fn proxy_owner_destroyed(
    xwm: &mut Xwm,
    owner: Window,
    now_ns: u64,
) -> Result<bool, XwmError> {
    let Some(kind) = xwm.data_bridge.selection_proxy.owner_kind(owner) else {
        return Ok(false);
    };
    let phase = xwm.data_bridge.selection_proxy.channel(kind).claim_phase;
    {
        let channel = xwm.data_bridge.selection_proxy.channel_mut(kind);
        channel.disabled = true;
        if let Some(id) = channel.desired_id {
            channel.suppressed_id = Some(id);
        }
        channel.authority = None;
        channel.held_ownership_timestamp = None;
        channel.claim_phase = ProxyClaimPhase::Idle;
    }
    if let ProxyClaimPhase::AwaitingOwnerConfirmation { sequence, .. } = phase {
        xwm.connection.discard_reply(
            sequence,
            RequestKind::HasResponse,
            DiscardMode::DiscardReply,
        );
    }
    cancel_channel(xwm, kind, now_ns)?;
    xwm.connection.flush().map_err(XwmError::Connection)?;
    Ok(true)
}

pub(crate) fn handle_selection_request(
    xwm: &mut Xwm,
    event: xproto::SelectionRequestEvent,
    now_ns: u64,
) -> Result<(), XwmError> {
    let clipboard = xwm.atoms.get(super::atoms::XwmAtomName::Clipboard);
    let primary = u32::from(xproto::AtomEnum::PRIMARY);
    let selection_kind = if event.selection == primary {
        Some(SelectionKind::Primary)
    } else if event.selection == clipboard {
        Some(SelectionKind::Clipboard)
    } else {
        None
    };
    let kind = selection_kind.unwrap_or(SelectionKind::Clipboard);
    let multiple = event.target == xwm.atoms.get(super::atoms::XwmAtomName::Multiple);
    let effective_property = if event.property == NONE && !multiple {
        event.target
    } else {
        event.property
    };
    let signature = NotificationSignature {
        requestor: event.requestor,
        selection: event.selection,
        target: event.target,
        time: event.time,
    };
    let Some(request_id) = admit_request(
        xwm,
        kind,
        signature,
        event.property,
        effective_property,
        now_ns,
    )?
    else {
        send_selection_notify(
            xwm,
            event.requestor,
            event.selection,
            event.target,
            event.time,
            NONE,
        )?;
        return Ok(());
    };

    if selection_kind.is_none()
        || (multiple && event.property == NONE)
        || xwm
            .data_bridge
            .selection_wire
            .is_internal_window(event.requestor)
        || event.requestor == xwm.supporting_wm_check
    {
        set_ready(xwm, request_id, false, now_ns)?;
    } else if let Some(authority) = xwm.data_bridge.selection_proxy.channel(kind).authority {
        let prepared = super::selection_wire::prepared_proxy_selection(xwm, kind);
        let owner = xwm.data_bridge.selection_proxy.owner_window(kind);
        let valid_time = event.time == CURRENT_TIME
            || super::focus::x11_time_after_eq(event.time, authority.ownership_timestamp);
        if xwm.data_bridge.selection_proxy.active_generation
            != Some(BridgeGeneration::from(xwm.generation))
            || authority.id.kind != public_kind(kind)
            || authority.id.selection_generation == 0
            || owner != Some(event.owner)
            || event.selection != selection_atom(xwm, kind)
            || !valid_time
            || prepared.is_none_or(|catalog| catalog.id != authority.id)
        {
            set_ready(xwm, request_id, false, now_ns)?;
        } else if multiple {
            let request = xwm
                .data_bridge
                .selection_proxy
                .requests
                .get_mut(&request_id)
                .expect("admitted request remains present");
            request.state = RequestState::NeedsMultipleRead;
            schedule_multiple_reads(xwm)?;
        } else {
            let owner = super::selection_outgoing::ConversionOwner {
                request_id,
                multiple_pair: None,
            };
            match start_conversion(
                xwm,
                authority.id,
                request_id,
                event.target,
                effective_property,
                owner,
                now_ns,
            )? {
                ConversionStart::Ready(success) => set_ready(xwm, request_id, success, now_ns)?,
                ConversionStart::Waiting => {
                    if let Some(request) = xwm
                        .data_bridge
                        .selection_proxy
                        .requests
                        .get_mut(&request_id)
                    {
                        request.state = RequestState::WaitingTransfer;
                    }
                }
            }
        }
    } else {
        set_ready(xwm, request_id, false, now_ns)?;
    }
    flush_ordered_notifies(xwm)?;
    xwm.connection.flush().map_err(XwmError::Connection)
}

fn kind_index(kind: SelectionKind) -> usize {
    match kind {
        SelectionKind::Clipboard => 0,
        SelectionKind::Primary => 1,
    }
}

fn selection_atom(xwm: &Xwm, kind: SelectionKind) -> Atom {
    match kind {
        SelectionKind::Clipboard => xwm.atoms.get(super::atoms::XwmAtomName::Clipboard),
        SelectionKind::Primary => u32::from(xproto::AtomEnum::PRIMARY),
    }
}

fn selection_kind(kind: XwaylandSelectionKind) -> SelectionKind {
    match kind {
        XwaylandSelectionKind::Clipboard => SelectionKind::Clipboard,
        XwaylandSelectionKind::Primary => SelectionKind::Primary,
    }
}

fn public_kind(kind: SelectionKind) -> XwaylandSelectionKind {
    match kind {
        SelectionKind::Clipboard => XwaylandSelectionKind::Clipboard,
        SelectionKind::Primary => XwaylandSelectionKind::Primary,
    }
}

fn admit_request(
    xwm: &mut Xwm,
    kind: SelectionKind,
    signature: NotificationSignature,
    original_property: Atom,
    effective_property: Atom,
    now_ns: u64,
) -> Result<Option<ProxySelectionRequestId>, XwmError> {
    if xwm.data_bridge.selection_proxy.requests.len() >= MAX_PENDING_PROXY_SELECTION_REQUESTS {
        let oldest = xwm
            .data_bridge
            .selection_proxy
            .requests
            .values()
            .min_by_key(|request| request.arrival_sequence)
            .map(|request| request.id);
        if let Some(oldest) = oldest {
            cancel_pending_reply_for(xwm, oldest);
            super::selection_outgoing::cancel_owner(xwm, oldest, now_ns)?;
            set_ready(xwm, oldest, false, now_ns)?;
            flush_ordered_notifies(xwm)?;
        }
    }
    if xwm.data_bridge.selection_proxy.requests.len() >= MAX_PENDING_PROXY_SELECTION_REQUESTS {
        return Ok(None);
    }
    let (next_id, next_arrival) = {
        let manager = &xwm.data_bridge.selection_proxy;
        let Some(next_id) = manager.next_request_id.checked_add(1) else {
            return Ok(None);
        };
        let Some(next_arrival) = manager.next_arrival_sequence.checked_add(1) else {
            return Ok(None);
        };
        (next_id, next_arrival)
    };
    let Some(id_value) = NonZeroU64::new(next_id) else {
        return Ok(None);
    };
    let requestor_monitored = retain_requestor(xwm, signature.requestor)?;
    let id = ProxySelectionRequestId(id_value);
    let manager = &mut xwm.data_bridge.selection_proxy;
    manager.next_request_id = next_id;
    manager.next_arrival_sequence = next_arrival;
    let request = TopLevelRequest {
        id,
        generation: BridgeGeneration::from(xwm.generation),
        kind,
        signature,
        original_property,
        effective_property,
        arrival_sequence: next_arrival,
        deadline_ns: now_ns.saturating_add(OUTGOING_SELECTION_IDLE_TIMEOUT_NS),
        requestor_monitored,
        state: RequestState::WaitingTransfer,
        multiple: None,
    };
    manager
        .notification_order
        .entry(signature)
        .or_default()
        .push_back(id);
    manager.requests.insert(id, request);
    Ok(Some(id))
}

pub(crate) fn retain_requestor(xwm: &mut Xwm, requestor: Window) -> Result<bool, XwmError> {
    if xwm.data_bridge.selection_wire.is_internal_window(requestor)
        || requestor == xwm.supporting_wm_check
    {
        return Ok(false);
    }
    if let Some(monitor) = xwm
        .data_bridge
        .selection_proxy
        .requestor_refs
        .get_mut(&requestor)
    {
        monitor.references = monitor.references.saturating_add(1);
        return Ok(true);
    }
    let restore_mask = if requestor == xwm.root {
        let required = xproto::EventMask::SUBSTRUCTURE_REDIRECT
            | xproto::EventMask::SUBSTRUCTURE_NOTIFY
            | xproto::EventMask::PROPERTY_CHANGE
            | xproto::EventMask::FOCUS_CHANGE;
        xwm.root_event_mask.unwrap_or(required)
    } else {
        xproto::EventMask::NO_EVENT
    };
    let mask =
        restore_mask | xproto::EventMask::PROPERTY_CHANGE | xproto::EventMask::STRUCTURE_NOTIFY;
    let cookie = xwm
        .connection
        .change_window_attributes(
            requestor,
            &xproto::ChangeWindowAttributesAux::new().event_mask(mask),
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(cookie);
    xwm.data_bridge.selection_proxy.requestor_refs.insert(
        requestor,
        RequestorMonitor {
            references: 1,
            restore_mask,
        },
    );
    Ok(true)
}

pub(crate) fn release_requestor(
    xwm: &mut Xwm,
    requestor: Window,
    destroyed: bool,
) -> Result<(), XwmError> {
    let Some(monitor) = xwm
        .data_bridge
        .selection_proxy
        .requestor_refs
        .get_mut(&requestor)
    else {
        return Ok(());
    };
    if monitor.references > 1 {
        monitor.references -= 1;
        return Ok(());
    }
    let restore_mask = monitor.restore_mask;
    xwm.data_bridge
        .selection_proxy
        .requestor_refs
        .remove(&requestor);
    if !destroyed {
        let cookie = xwm
            .connection
            .change_window_attributes(
                requestor,
                &xproto::ChangeWindowAttributesAux::new().event_mask(restore_mask),
            )
            .map_err(XwmError::Connection)?;
        std::mem::forget(cookie);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConversionStart {
    Ready(bool),
    Waiting,
}

fn start_conversion(
    xwm: &mut Xwm,
    proxy_id: XwaylandProxySelectionId,
    request_id: ProxySelectionRequestId,
    target: Atom,
    property: Atom,
    owner: super::selection_outgoing::ConversionOwner,
    now_ns: u64,
) -> Result<ConversionStart, XwmError> {
    let Some(prepared) =
        super::selection_wire::prepared_proxy_selection(xwm, selection_kind(proxy_id.kind))
    else {
        return Ok(ConversionStart::Ready(false));
    };
    if prepared.id != proxy_id || property == NONE {
        return Ok(ConversionStart::Ready(false));
    }
    let requestor = requestor_for(xwm, request_id);
    let collides_with_another_request =
        xwm.data_bridge
            .selection_proxy
            .requests
            .values()
            .any(|request| {
                request.id != request_id
                    && request.signature.requestor == requestor
                    && request.effective_property == property
            });
    let aliases_multiple_reply = owner.multiple_pair.is_some_and(|_| {
        xwm.data_bridge
            .selection_proxy
            .requests
            .get(&request_id)
            .and_then(|request| request.multiple.as_ref())
            .is_some_and(|multiple| multiple.property == property)
    });
    if collides_with_another_request
        || aliases_multiple_reply
        || super::selection_outgoing::property_in_use(xwm, requestor, property)
    {
        return Ok(ConversionStart::Ready(false));
    }
    let targets = xwm.atoms.get(super::atoms::XwmAtomName::Targets);
    let timestamp = xwm.atoms.get(super::atoms::XwmAtomName::Timestamp);
    let multiple = xwm.atoms.get(super::atoms::XwmAtomName::Multiple);
    if target == targets {
        change_property32(
            xwm,
            requestor_for(xwm, request_id),
            property,
            xproto::AtomEnum::ATOM.into(),
            &prepared.target_order,
            now_ns,
        )?;
        return Ok(ConversionStart::Ready(true));
    }
    if target == timestamp {
        let channel = xwm
            .data_bridge
            .selection_proxy
            .channel(selection_kind(proxy_id.kind));
        let Some(authority) = channel.authority else {
            return Ok(ConversionStart::Ready(false));
        };
        if authority.id != proxy_id {
            return Ok(ConversionStart::Ready(false));
        }
        change_property32(
            xwm,
            requestor_for(xwm, request_id),
            property,
            xproto::AtomEnum::INTEGER.into(),
            &[authority.ownership_timestamp],
            now_ns,
        )?;
        return Ok(ConversionStart::Ready(true));
    }
    if target == multiple {
        return Ok(ConversionStart::Ready(false));
    }
    let Some(binding) = super::selection_wire::resolve_proxy_target(xwm, proxy_id, target) else {
        return Ok(ConversionStart::Ready(false));
    };
    let Some(requestor) = xwm
        .data_bridge
        .selection_proxy
        .requests
        .get(&request_id)
        .map(|request| request.signature.requestor)
    else {
        return Ok(ConversionStart::Ready(false));
    };
    let transfer = super::selection_outgoing::start_transfer(
        xwm,
        super::selection_outgoing::StartTransfer {
            proxy_id,
            requestor,
            target,
            property,
            property_type: binding.target,
            mime_type: binding.mime_type,
            owner,
            now_ns,
        },
    )?;
    Ok(transfer.map_or(ConversionStart::Ready(false), |_| ConversionStart::Waiting))
}

fn change_property32(
    xwm: &Xwm,
    requestor: Window,
    property: Atom,
    property_type: Atom,
    values: &[Atom],
    _now_ns: u64,
) -> Result<(), XwmError> {
    let cookie = xwm
        .connection
        .change_property32(
            xproto::PropMode::REPLACE,
            requestor,
            property,
            property_type,
            values,
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(cookie);
    Ok(())
}

fn requestor_for(xwm: &Xwm, request_id: ProxySelectionRequestId) -> Window {
    xwm.data_bridge.selection_proxy.requests[&request_id]
        .signature
        .requestor
}

fn set_ready(
    xwm: &mut Xwm,
    request_id: ProxySelectionRequestId,
    success: bool,
    _now_ns: u64,
) -> Result<(), XwmError> {
    let Some(request) = xwm
        .data_bridge
        .selection_proxy
        .requests
        .get_mut(&request_id)
    else {
        return Ok(());
    };
    request.state = if success {
        RequestState::ReadySuccess
    } else {
        RequestState::ReadyFailure
    };
    flush_ordered_notifies(xwm)
}

fn cancel_pending_reply_for(xwm: &mut Xwm, request_id: ProxySelectionRequestId) {
    let sequences = xwm
        .data_bridge
        .selection_proxy
        .pending_multiple_replies
        .iter()
        .filter_map(|(sequence, pending)| (pending.request_id == request_id).then_some(*sequence))
        .collect::<Vec<_>>();
    for sequence in sequences {
        xwm.data_bridge
            .selection_proxy
            .pending_multiple_replies
            .remove(&sequence);
        xwm.connection.discard_reply(
            sequence,
            RequestKind::HasResponse,
            DiscardMode::DiscardReply,
        );
    }
}

fn flush_ordered_notifies(xwm: &mut Xwm) -> Result<(), XwmError> {
    let signatures = xwm
        .data_bridge
        .selection_proxy
        .notification_order
        .keys()
        .copied()
        .collect::<Vec<_>>();
    for signature in signatures {
        loop {
            let id = xwm
                .data_bridge
                .selection_proxy
                .notification_order
                .get(&signature)
                .and_then(VecDeque::front)
                .copied();
            let Some(id) = id else { break };
            let Some(request) = xwm.data_bridge.selection_proxy.requests.get(&id) else {
                if let Some(order) = xwm
                    .data_bridge
                    .selection_proxy
                    .notification_order
                    .get_mut(&signature)
                {
                    order.pop_front();
                }
                continue;
            };
            let success = match request.state {
                RequestState::ReadySuccess => true,
                RequestState::ReadyFailure => false,
                _ => break,
            };
            let requestor_monitored = request.requestor_monitored;
            let signature = request.signature;
            let property = if success {
                request.effective_property
            } else {
                NONE
            };
            let requestor = signature.requestor;
            send_selection_notify(
                xwm,
                requestor,
                signature.selection,
                signature.target,
                signature.time,
                property,
            )?;
            xwm.data_bridge.selection_proxy.requests.remove(&id);
            if let Some(order) = xwm
                .data_bridge
                .selection_proxy
                .notification_order
                .get_mut(&signature)
            {
                order.pop_front();
                if order.is_empty() {
                    xwm.data_bridge
                        .selection_proxy
                        .notification_order
                        .remove(&signature);
                }
            }
            if requestor_monitored {
                release_requestor(xwm, requestor, false)?;
            }
        }
    }
    Ok(())
}

fn selection_notify_event(
    requestor: Window,
    selection: Atom,
    target: Atom,
    time: u32,
    property: Atom,
) -> xproto::SelectionNotifyEvent {
    xproto::SelectionNotifyEvent {
        response_type: xproto::SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time,
        requestor,
        selection,
        target,
        property,
    }
}

fn send_selection_notify(
    xwm: &Xwm,
    requestor: Window,
    selection: Atom,
    target: Atom,
    time: u32,
    property: Atom,
) -> Result<(), XwmError> {
    let event = selection_notify_event(requestor, selection, target, time, property);
    xwm.connection
        .send_event(false, requestor, xproto::EventMask::NO_EVENT, event)
        .map_err(XwmError::Connection)?;
    Ok(())
}

pub(crate) fn conversion_ready(
    xwm: &mut Xwm,
    owner: super::selection_outgoing::ConversionOwner,
    success: bool,
    now_ns: u64,
) -> Result<(), XwmError> {
    let Some(request) = xwm
        .data_bridge
        .selection_proxy
        .requests
        .get_mut(&owner.request_id)
    else {
        return Ok(());
    };
    if let Some(pair_index) = owner.multiple_pair {
        let Some(multiple) = request.multiple.as_mut() else {
            return Ok(());
        };
        if multiple.cursor != pair_index || pair_index >= multiple.pairs.len() {
            return Ok(());
        }
        if !success {
            multiple.pairs[pair_index].0 = NONE;
        }
        multiple.cursor += 1;
        request.state = RequestState::ProcessingMultiple;
        request.deadline_ns = now_ns.saturating_add(OUTGOING_SELECTION_IDLE_TIMEOUT_NS);
        advance_multiple(xwm, owner.request_id, now_ns)?;
    } else {
        request.state = if success {
            RequestState::ReadySuccess
        } else {
            RequestState::ReadyFailure
        };
        request.deadline_ns = now_ns.saturating_add(OUTGOING_SELECTION_IDLE_TIMEOUT_NS);
        flush_ordered_notifies(xwm)?;
    }
    xwm.connection.flush().map_err(XwmError::Connection)?;
    Ok(())
}

pub(crate) fn conversion_progress(
    xwm: &mut Xwm,
    owner: super::selection_outgoing::ConversionOwner,
    now_ns: u64,
) {
    if let Some(request) = xwm
        .data_bridge
        .selection_proxy
        .requests
        .get_mut(&owner.request_id)
    {
        request.deadline_ns = now_ns.saturating_add(OUTGOING_SELECTION_IDLE_TIMEOUT_NS);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProxyReplyDrain {
    pub processed: usize,
    pub budget_exhausted: bool,
    pub quiescent: bool,
}

pub(crate) fn poll_replies(
    xwm: &mut Xwm,
    budget: usize,
    now_ns: u64,
) -> Result<ProxyReplyDrain, XwmError> {
    if budget == 0 {
        return Ok(ProxyReplyDrain {
            processed: 0,
            budget_exhausted: false,
            quiescent: false,
        });
    }
    let sequences = xwm
        .data_bridge
        .selection_proxy
        .pending_multiple_replies
        .keys()
        .copied()
        .take(budget)
        .collect::<Vec<_>>();
    let mut budget_exhausted = sequences.len()
        < xwm
            .data_bridge
            .selection_proxy
            .pending_multiple_replies
            .len();
    let mut processed = 0;
    for sequence in sequences {
        let Some(pending) = xwm
            .data_bridge
            .selection_proxy
            .pending_multiple_replies
            .get(&sequence)
            .copied()
        else {
            continue;
        };
        let cookie = Cookie::<super::connection::X11Connection, xproto::GetPropertyReply>::new(
            &xwm.connection,
            sequence,
        );
        let reply = match cookie.reply_unchecked() {
            Ok(reply) => reply,
            Err(x11rb::errors::ConnectionError::IoError(error))
                if error.kind() == io::ErrorKind::WouldBlock =>
            {
                continue;
            }
            Err(error) => return Err(XwmError::Connection(error)),
        };
        xwm.data_bridge
            .selection_proxy
            .pending_multiple_replies
            .remove(&sequence);
        processed += 1;
        let current = xwm
            .data_bridge
            .selection_proxy
            .requests
            .get(&pending.request_id)
            .is_some_and(|request| {
                request.generation == pending.generation
                    && request.state == RequestState::NeedsMultipleRead
            });
        if !current || pending.generation != BridgeGeneration::from(xwm.generation) {
            continue;
        }
        let pairs = reply.and_then(|reply| parse_multiple_pairs(xwm, reply));
        if let Some(pairs) = pairs {
            if let Some(request) = xwm
                .data_bridge
                .selection_proxy
                .requests
                .get_mut(&pending.request_id)
            {
                request.multiple = Some(MultipleState {
                    property: request.effective_property,
                    pairs,
                    cursor: 0,
                });
                request.state = RequestState::ProcessingMultiple;
            }
            advance_multiple(xwm, pending.request_id, now_ns)?;
        } else {
            set_ready(xwm, pending.request_id, false, now_ns)?;
        }
    }
    let owner_replies = pending_owner_confirmations(&xwm.data_bridge.selection_proxy);
    let remaining_budget = budget.saturating_sub(processed);
    if owner_replies.len() > remaining_budget {
        budget_exhausted = true;
    }
    for (kind, sequence) in owner_replies.into_iter().take(remaining_budget) {
        let phase = xwm.data_bridge.selection_proxy.channel(kind).claim_phase;
        let ProxyClaimPhase::AwaitingOwnerConfirmation {
            id,
            timestamp,
            sequence: current_sequence,
            deadline_ns,
        } = phase
        else {
            continue;
        };
        if current_sequence != sequence {
            continue;
        }
        let cookie =
            Cookie::<super::connection::X11Connection, xproto::GetSelectionOwnerReply>::new(
                &xwm.connection,
                sequence,
            );
        let reply = match cookie.reply_unchecked() {
            Ok(reply) => reply,
            Err(x11rb::errors::ConnectionError::IoError(error))
                if error.kind() == io::ErrorKind::WouldBlock =>
            {
                continue;
            }
            Err(error) => return Err(XwmError::Connection(error)),
        };
        processed += 1;
        let reply_owner = reply.map(|reply| reply.owner).unwrap_or(NONE);
        let owner = xwm.data_bridge.selection_proxy.owner_window(kind);
        let prepared = super::selection_wire::prepared_proxy_selection(xwm, kind);
        let confirmed = reply_owner != NONE
            && Some(reply_owner) == owner
            && owner.is_some()
            && deadline_ns > now_ns
            && xwm.data_bridge.selection_proxy.active_generation
                == Some(BridgeGeneration::from(xwm.generation))
            && xwm.data_bridge.selection_proxy.channel(kind).desired_id == Some(id)
            && xwm.data_bridge.selection_proxy.channel(kind).suppressed_id != Some(id)
            && xwm
                .data_bridge
                .selection_proxy
                .channel(kind)
                .held_ownership_timestamp
                == Some(timestamp)
            && !xwm.data_bridge.selection_proxy.channel(kind).disabled
            && xwm.capabilities.xfixes
            && prepared
                .is_some_and(|prepared| prepared.id == id && !prepared.data_targets.is_empty());
        let channel = xwm.data_bridge.selection_proxy.channel_mut(kind);
        channel.claim_phase = ProxyClaimPhase::Idle;
        if confirmed {
            channel.authority = Some(ProxyServingAuthority {
                id,
                ownership_timestamp: timestamp,
            });
        } else {
            channel.authority = None;
            if channel.desired_id == Some(id) {
                channel.suppressed_id = Some(id);
            }
            if channel.held_ownership_timestamp == Some(timestamp) {
                channel.held_ownership_timestamp = None;
            }
            safe_release(xwm, kind, timestamp)?;
        }
        reconcile_channel(xwm, kind, now_ns)?;
    }
    schedule_multiple_reads(xwm)?;
    flush_ordered_notifies(xwm)?;
    xwm.connection.flush().map_err(XwmError::Connection)?;
    Ok(ProxyReplyDrain {
        processed,
        budget_exhausted,
        quiescent: !budget_exhausted
            && xwm
                .data_bridge
                .selection_proxy
                .pending_multiple_replies
                .is_empty()
            && !xwm
                .data_bridge
                .selection_proxy
                .requests
                .values()
                .any(|request| request.state == RequestState::NeedsMultipleRead)
            && pending_owner_confirmations(&xwm.data_bridge.selection_proxy).is_empty(),
    })
}

fn pending_owner_confirmations(
    manager: &SelectionProxyManager,
) -> Vec<(SelectionKind, SequenceNumber)> {
    [SelectionKind::Clipboard, SelectionKind::Primary]
        .into_iter()
        .filter_map(|kind| match manager.channel(kind).claim_phase {
            ProxyClaimPhase::AwaitingOwnerConfirmation { sequence, .. } => Some((kind, sequence)),
            _ => None,
        })
        .collect()
}

fn parse_multiple_pairs(xwm: &Xwm, reply: xproto::GetPropertyReply) -> Option<Vec<(Atom, Atom)>> {
    if reply.type_ != xwm.atoms.get(super::atoms::XwmAtomName::AtomPair)
        || reply.format != 32
        || reply.bytes_after != 0
    {
        return None;
    }
    let values = reply.value32()?.collect::<Vec<_>>();
    if values.len() % 2 != 0 || values.len() / 2 > MAX_MULTIPLE_PAIRS {
        return None;
    }
    let pairs = values
        .chunks_exact(2)
        .map(|pair| (pair[0], pair[1]))
        .collect::<Vec<_>>();
    pairs
        .iter()
        .all(|(_, property)| *property != NONE)
        .then_some(pairs)
}

fn schedule_multiple_reads(xwm: &mut Xwm) -> Result<(), XwmError> {
    while xwm
        .data_bridge
        .selection_proxy
        .pending_multiple_replies
        .len()
        < MAX_PENDING_OUTGOING_SELECTION_REPLIES
    {
        let turn = xwm.data_bridge.selection_proxy.reply_turn;
        let candidate = [turn, other_kind(turn)].into_iter().find_map(|kind| {
            xwm.data_bridge
                .selection_proxy
                .requests
                .values()
                .find(|request| {
                    request.kind == kind && request.state == RequestState::NeedsMultipleRead
                })
                .map(|request| request.id)
        });
        let Some(request_id) = candidate else { break };
        let request = xwm
            .data_bridge
            .selection_proxy
            .requests
            .get(&request_id)
            .expect("selected pending request");
        let cookie = xwm
            .connection
            .get_property(
                false,
                request.signature.requestor,
                request.effective_property,
                xproto::AtomEnum::ANY,
                0,
                (MAX_MULTIPLE_PAIRS * 2) as u32,
            )
            .map_err(XwmError::Connection)?;
        let sequence = cookie.sequence_number();
        std::mem::forget(cookie);
        xwm.data_bridge
            .selection_proxy
            .pending_multiple_replies
            .insert(
                sequence,
                PendingMultipleReply {
                    request_id,
                    generation: request.generation,
                },
            );
        xwm.data_bridge.selection_proxy.reply_turn = other_kind(request.kind);
    }
    Ok(())
}

fn other_kind(kind: SelectionKind) -> SelectionKind {
    match kind {
        SelectionKind::Clipboard => SelectionKind::Primary,
        SelectionKind::Primary => SelectionKind::Clipboard,
    }
}

fn advance_multiple(
    xwm: &mut Xwm,
    request_id: ProxySelectionRequestId,
    now_ns: u64,
) -> Result<(), XwmError> {
    loop {
        let Some((proxy_id, pair_index, target, property, finished)) = xwm
            .data_bridge
            .selection_proxy
            .requests
            .get(&request_id)
            .and_then(|request| {
                let multiple = request.multiple.as_ref()?;
                let done = multiple.cursor >= multiple.pairs.len();
                if done {
                    Some((
                        xwm.data_bridge
                            .selection_proxy
                            .channel(request.kind)
                            .authority?
                            .id,
                        multiple.cursor,
                        NONE,
                        multiple.property,
                        true,
                    ))
                } else {
                    let (target, property) = multiple.pairs[multiple.cursor];
                    Some((
                        xwm.data_bridge
                            .selection_proxy
                            .channel(request.kind)
                            .authority?
                            .id,
                        multiple.cursor,
                        target,
                        property,
                        false,
                    ))
                }
            })
        else {
            return Ok(());
        };
        if finished {
            let pairs = xwm
                .data_bridge
                .selection_proxy
                .requests
                .get(&request_id)
                .and_then(|request| request.multiple.as_ref())
                .map(|multiple| {
                    multiple
                        .pairs
                        .iter()
                        .flat_map(|(target, property)| [*target, *property])
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let requestor = xwm
                .data_bridge
                .selection_proxy
                .requests
                .get(&request_id)
                .expect("request")
                .signature
                .requestor;
            let cookie = xwm
                .connection
                .change_property32(
                    xproto::PropMode::REPLACE,
                    requestor,
                    property,
                    xwm.atoms.get(super::atoms::XwmAtomName::AtomPair),
                    &pairs,
                )
                .map_err(XwmError::Connection)?;
            std::mem::forget(cookie);
            set_ready(xwm, request_id, true, now_ns)?;
            return Ok(());
        }
        let owner = super::selection_outgoing::ConversionOwner {
            request_id,
            multiple_pair: Some(pair_index),
        };
        match start_conversion(xwm, proxy_id, request_id, target, property, owner, now_ns)? {
            ConversionStart::Ready(true) => {
                let request = xwm
                    .data_bridge
                    .selection_proxy
                    .requests
                    .get_mut(&request_id)
                    .expect("request");
                let multiple = request.multiple.as_mut().expect("multiple group");
                multiple.cursor += 1;
            }
            ConversionStart::Ready(false) => {
                let request = xwm
                    .data_bridge
                    .selection_proxy
                    .requests
                    .get_mut(&request_id)
                    .expect("request");
                let multiple = request.multiple.as_mut().expect("multiple group");
                multiple.pairs[pair_index].0 = NONE;
                multiple.cursor += 1;
            }
            ConversionStart::Waiting => {
                if let Some(request) = xwm
                    .data_bridge
                    .selection_proxy
                    .requests
                    .get_mut(&request_id)
                {
                    request.state = RequestState::WaitingTransfer;
                    request.deadline_ns = now_ns.saturating_add(OUTGOING_SELECTION_IDLE_TIMEOUT_NS);
                }
                return Ok(());
            }
        }
    }
}

pub(crate) fn take_managed_data_requests(
    xwm: &mut Xwm,
) -> Vec<crate::xwayland::XwaylandProxySelectionDataRequest> {
    super::selection_outgoing::take_requests(xwm)
}

pub(crate) fn resolve_managed_data_requests(
    xwm: &mut Xwm,
    results: impl IntoIterator<Item = (crate::xwayland::XwaylandProxySelectionTransferId, bool)>,
    now_ns: u64,
) -> Result<bool, XwmError> {
    super::selection_outgoing::resolve_requests(xwm, results, now_ns)
}

pub(crate) fn requestor_destroyed(xwm: &mut Xwm, requestor: Window) -> Result<(), XwmError> {
    let ids = xwm
        .data_bridge
        .selection_proxy
        .requests
        .values()
        .filter(|request| request.signature.requestor == requestor)
        .map(|request| request.id)
        .collect::<std::collections::HashSet<_>>();
    for id in &ids {
        xwm.data_bridge.selection_proxy.requests.remove(id);
    }
    for order in xwm
        .data_bridge
        .selection_proxy
        .notification_order
        .values_mut()
    {
        order.retain(|id| !ids.contains(id));
    }
    xwm.data_bridge
        .selection_proxy
        .notification_order
        .retain(|_, order| !order.is_empty());
    let sequences = xwm
        .data_bridge
        .selection_proxy
        .pending_multiple_replies
        .iter()
        .filter_map(|(sequence, pending)| ids.contains(&pending.request_id).then_some(*sequence))
        .collect::<Vec<_>>();
    for sequence in sequences {
        xwm.data_bridge
            .selection_proxy
            .pending_multiple_replies
            .remove(&sequence);
        xwm.connection.discard_reply(
            sequence,
            RequestKind::HasResponse,
            DiscardMode::DiscardReply,
        );
    }
    xwm.data_bridge
        .selection_proxy
        .requestor_refs
        .remove(&requestor);
    super::selection_outgoing::cancel_requestor(xwm, requestor);
    Ok(())
}

pub(crate) fn next_deadline_ns(xwm: &Xwm) -> Option<u64> {
    let request_deadline = xwm
        .data_bridge
        .selection_proxy
        .requests
        .values()
        .map(|request| request.deadline_ns)
        .min();
    let claim_deadline = [SelectionKind::Clipboard, SelectionKind::Primary]
        .into_iter()
        .filter_map(
            |kind| match xwm.data_bridge.selection_proxy.channel(kind).claim_phase {
                ProxyClaimPhase::AwaitingTimestamp { deadline_ns, .. }
                | ProxyClaimPhase::AwaitingOwnerConfirmation { deadline_ns, .. }
                    if deadline_ns != u64::MAX =>
                {
                    Some(deadline_ns)
                }
                _ => None,
            },
        )
        .min();
    request_deadline.into_iter().chain(claim_deadline).min()
}

#[cfg(test)]
pub(crate) fn pending_reply_sequence_for_test(xwm: &Xwm) -> Option<SequenceNumber> {
    xwm.data_bridge
        .selection_proxy
        .pending_multiple_replies
        .keys()
        .next()
        .copied()
}

#[cfg(test)]
pub(crate) fn pending_reply_count_for_test(xwm: &Xwm) -> usize {
    xwm.data_bridge
        .selection_proxy
        .pending_multiple_replies
        .len()
}

#[cfg(test)]
pub(crate) fn request_count_for_test(xwm: &Xwm) -> usize {
    xwm.data_bridge.selection_proxy.requests.len()
}

#[cfg(test)]
pub(crate) fn requestor_ref_count_for_test(xwm: &Xwm, requestor: Window) -> usize {
    xwm.data_bridge
        .selection_proxy
        .requestor_refs
        .get(&requestor)
        .map(|monitor| monitor.references)
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) fn authority_for_test(
    xwm: &Xwm,
    kind: SelectionKind,
) -> Option<(XwaylandProxySelectionId, u32)> {
    xwm.data_bridge
        .selection_proxy
        .channel(kind)
        .authority
        .map(|authority| (authority.id, authority.ownership_timestamp))
}

#[cfg(test)]
pub(crate) fn pending_owner_confirmation_for_test(
    xwm: &Xwm,
    kind: SelectionKind,
) -> Option<(XwaylandProxySelectionId, u32, SequenceNumber)> {
    match xwm.data_bridge.selection_proxy.channel(kind).claim_phase {
        ProxyClaimPhase::AwaitingOwnerConfirmation {
            id,
            timestamp,
            sequence,
            ..
        } => Some((id, timestamp, sequence)),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn suppressed_id_for_test(
    xwm: &Xwm,
    kind: SelectionKind,
) -> Option<XwaylandProxySelectionId> {
    xwm.data_bridge.selection_proxy.channel(kind).suppressed_id
}

#[cfg(test)]
pub(crate) fn held_timestamp_for_test(xwm: &Xwm, kind: SelectionKind) -> Option<u32> {
    xwm.data_bridge
        .selection_proxy
        .channel(kind)
        .held_ownership_timestamp
}

#[cfg(test)]
pub(crate) fn timestamp_probe_sequence_for_test(
    xwm: &Xwm,
    kind: SelectionKind,
) -> Option<SequenceNumber> {
    match xwm.data_bridge.selection_proxy.channel(kind).claim_phase {
        ProxyClaimPhase::AwaitingTimestamp { sequence, .. } => Some(sequence),
        _ => None,
    }
}

pub(crate) fn expire_deadlines(
    xwm: &mut Xwm,
    now_ns: u64,
) -> Result<Vec<SequenceNumber>, XwmError> {
    let expired = xwm
        .data_bridge
        .selection_proxy
        .requests
        .values()
        .filter(|request| request.deadline_ns <= now_ns)
        .map(|request| request.id)
        .collect::<Vec<_>>();
    let mut sequences = Vec::new();
    let expired_claims = [SelectionKind::Clipboard, SelectionKind::Primary]
        .into_iter()
        .filter_map(|kind| {
            let phase = xwm.data_bridge.selection_proxy.channel(kind).claim_phase;
            match phase {
                ProxyClaimPhase::AwaitingTimestamp { deadline_ns, .. }
                | ProxyClaimPhase::AwaitingOwnerConfirmation { deadline_ns, .. }
                    if deadline_ns <= now_ns =>
                {
                    Some((kind, phase))
                }
                _ => None,
            }
        })
        .collect::<Vec<_>>();
    for (kind, phase) in expired_claims {
        match phase {
            ProxyClaimPhase::AwaitingTimestamp {
                probe_id, sequence, ..
            } => {
                let held_timestamp = xwm
                    .data_bridge
                    .selection_proxy
                    .channel(kind)
                    .held_ownership_timestamp;
                let channel = xwm.data_bridge.selection_proxy.channel_mut(kind);
                channel.suppressed_id = Some(probe_id);
                channel.authority = None;
                channel.claim_phase = ProxyClaimPhase::AwaitingTimestamp {
                    probe_id,
                    sequence,
                    deadline_ns: u64::MAX,
                };
                if let Some(timestamp) = held_timestamp {
                    safe_release(xwm, kind, timestamp)?;
                    xwm.data_bridge
                        .selection_proxy
                        .channel_mut(kind)
                        .held_ownership_timestamp = None;
                }
            }
            ProxyClaimPhase::AwaitingOwnerConfirmation {
                id,
                timestamp,
                sequence,
                ..
            } => {
                let channel = xwm.data_bridge.selection_proxy.channel_mut(kind);
                channel.suppressed_id = Some(id);
                channel.authority = None;
                channel.claim_phase = ProxyClaimPhase::Idle;
                channel.held_ownership_timestamp = None;
                sequences.push(sequence);
                safe_release(xwm, kind, timestamp)?;
            }
            ProxyClaimPhase::Idle => unreachable!("only expired claim phases are collected"),
        }
    }
    for request_id in expired {
        let pending = xwm
            .data_bridge
            .selection_proxy
            .pending_multiple_replies
            .iter()
            .filter_map(|(sequence, reply)| (reply.request_id == request_id).then_some(*sequence))
            .collect::<Vec<_>>();
        for sequence in pending {
            xwm.data_bridge
                .selection_proxy
                .pending_multiple_replies
                .remove(&sequence);
            sequences.push(sequence);
        }
        super::selection_outgoing::cancel_owner(xwm, request_id, now_ns)?;
        if let Some(request) = xwm
            .data_bridge
            .selection_proxy
            .requests
            .get_mut(&request_id)
            && request.deadline_ns <= now_ns
        {
            request.state = RequestState::ReadyFailure;
            // A failed request can remain queued behind an earlier
            // identical ICCCM signature. Do not keep scheduling a timer
            // for an already-completed timeout decision.
            request.deadline_ns = u64::MAX;
        }
    }
    flush_ordered_notifies(xwm)?;
    xwm.connection.flush().map_err(XwmError::Connection)?;
    Ok(sequences)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(
        type_: Atom,
        format: u8,
        bytes_after: u32,
        values: &[Atom],
    ) -> xproto::GetPropertyReply {
        let value = values
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect::<Vec<_>>();
        xproto::GetPropertyReply {
            format,
            sequence: 0,
            length: (value.len() / 4) as u32,
            type_,
            bytes_after,
            value_len: value.len() as u32,
            value,
        }
    }

    #[test]
    fn multiple_property_parser_rejects_every_malformed_shape() {
        let (xwm, _peer) = crate::xwayland::xwm::events::tests::test_fixture(
            crate::xwayland::xwm::events::tests::generation(400),
        );
        let atom_pair = xwm.atoms.get(super::super::atoms::XwmAtomName::AtomPair);
        assert!(parse_multiple_pairs(&xwm, reply(atom_pair, 32, 0, &[1, 2])).is_some());
        assert!(parse_multiple_pairs(&xwm, reply(atom_pair + 1, 32, 0, &[1, 2])).is_none());
        assert!(parse_multiple_pairs(&xwm, reply(atom_pair, 8, 0, &[1, 2])).is_none());
        assert!(parse_multiple_pairs(&xwm, reply(atom_pair, 32, 4, &[1, 2])).is_none());
        assert!(parse_multiple_pairs(&xwm, reply(atom_pair, 32, 0, &[1])).is_none());
        let oversized = vec![1; (MAX_MULTIPLE_PAIRS + 1) * 2];
        assert!(parse_multiple_pairs(&xwm, reply(atom_pair, 32, 0, &oversized)).is_none());
        assert!(parse_multiple_pairs(&xwm, reply(atom_pair, 32, 0, &[1, NONE])).is_none());
    }
}
