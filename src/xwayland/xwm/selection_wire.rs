//! Internal X11 selection ownership and TARGETS discovery.
//!
//! This module only observes X11 wire state. It does not publish offers to
//! the compositor or move selection payload bytes.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io,
};

use x11rb::{
    connection::{Connection, DiscardMode, RequestConnection, RequestKind, SequenceNumber},
    cookie::Cookie,
    protocol::{
        xfixes::{self, ConnectionExt as XfixesConnectionExt},
        xproto::{self, Atom, ConnectionExt as XprotoConnectionExt, Window, WindowClass},
    },
};

use super::super::{
    XwaylandGeneration,
    selection_metadata::{
        XwaylandProxySelectionId, XwaylandProxySelectionOffer, XwaylandProxySelectionSnapshot,
        XwaylandSelectionEvent, XwaylandSelectionKind, XwaylandSelectionOffer,
        XwaylandSelectionOfferId,
    },
};
use super::{
    Xwm, XwmError,
    atoms::XwmAtomName,
    connection::X11Connection,
    data_bridge::{
        BridgeGeneration, SelectionKind, SelectionOrigin,
        selection::{
            SelectionIdentity, SelectionRevision, SelectionSnapshot, TargetsDiscoveryState,
        },
    },
};

pub(crate) const MAX_PENDING_SELECTION_REPLIES: usize = 4;
const MAX_SELECTION_REQUESTOR_WINDOWS_PER_CHANNEL: usize = 4_096;
const MAX_SELECTION_MIME_TYPES: usize = super::data_bridge::selection::MAX_SELECTION_TARGETS;
const MAX_MIME_TYPE_LEN: usize = 4_096;
const TARGETS_PROPERTY_ITEMS: u32 =
    (super::data_bridge::selection::MAX_SELECTION_TARGETS + 1) as u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestorSafety {
    Clean,
    Poisoned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectionTargetBinding {
    mime_type: String,
    target: Atom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectionTargetCatalog {
    identity: SelectionIdentity,
    bindings: Vec<SelectionTargetBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TargetNameResolution {
    Complete(Option<String>),
    Pending,
    InFlight,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetCatalogResolution {
    identity: SelectionIdentity,
    targets: Vec<Atom>,
    names: Vec<TargetNameResolution>,
    next_ordinal: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProxyTargetBinding {
    pub target: Atom,
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PreparedProxySelection {
    pub id: XwaylandProxySelectionId,
    pub protocol_targets: Vec<Atom>,
    pub data_targets: Vec<ProxyTargetBinding>,
    pub target_order: Vec<Atom>,
    mime_types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProxyMimeResolution {
    Complete(Option<Atom>),
    Pending,
    InFlight,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProxyCatalogResolution {
    id: XwaylandProxySelectionId,
    mime_types: Vec<String>,
    atoms: Vec<ProxyMimeResolution>,
    next_ordinal: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum SelectionResolutionDirection {
    #[default]
    Inbound,
    Outbound,
}

impl SelectionResolutionDirection {
    const fn other(self) -> Self {
        match self {
            Self::Inbound => Self::Outbound,
            Self::Outbound => Self::Inbound,
        }
    }
}

enum SelectionResolutionRequest {
    Inbound {
        kind: SelectionKind,
        identity: SelectionIdentity,
        target: Atom,
        ordinal: usize,
    },
    Outbound {
        kind: SelectionKind,
        proxy_id: XwaylandProxySelectionId,
        mime_type: String,
        ordinal: usize,
    },
}

#[derive(Debug, Clone, Copy)]
struct SelectionWindows {
    observer: Window,
    requestor: Option<Window>,
    requestor_windows_created: usize,
    requestor_safety: RequestorSafety,
}

#[derive(Debug, Clone, Copy)]
enum PendingReplyKind {
    InitialOwnerProbe {
        revision_at_issue: Option<SelectionRevision>,
    },
    TargetsProperty {
        identity: SelectionIdentity,
        requestor: Window,
        property: Atom,
    },
    TargetAtomName {
        identity: SelectionIdentity,
        target: Atom,
        ordinal: usize,
    },
    ProxyMimeAtom {
        proxy_id: XwaylandProxySelectionId,
        ordinal: usize,
    },
}

#[derive(Debug, Clone, Copy)]
struct PendingSelectionReply {
    generation: BridgeGeneration,
    selection: SelectionKind,
    kind: PendingReplyKind,
}

#[derive(Debug, Default)]
pub(crate) struct SelectionWireState {
    active_generation: Option<BridgeGeneration>,
    proxy_generation: Option<BridgeGeneration>,
    last_convert_selection_sequence: Option<SequenceNumber>,
    windows: HashMap<SelectionKind, SelectionWindows>,
    internal_windows: HashSet<Window>,
    pending: BTreeMap<SequenceNumber, PendingSelectionReply>,
    target_catalogs: HashMap<SelectionKind, SelectionTargetCatalog>,
    target_resolutions: HashMap<SelectionKind, TargetCatalogResolution>,
    proxy_offers: HashMap<SelectionKind, XwaylandProxySelectionOffer>,
    proxy_catalog_resolutions: HashMap<SelectionKind, ProxyCatalogResolution>,
    prepared_proxy_selections: HashMap<SelectionKind, PreparedProxySelection>,
    pending_selection_events: HashMap<SelectionKind, XwaylandSelectionEvent>,
    current_offers: HashMap<SelectionKind, XwaylandSelectionOfferId>,
    catalog_turn: Option<SelectionKind>,
    proxy_catalog_turn: Option<SelectionKind>,
    resolution_direction: SelectionResolutionDirection,
}

impl SelectionWireState {
    #[cfg(test)]
    pub(crate) fn is_active(&self) -> bool {
        self.active_generation.is_some()
    }

    #[cfg(test)]
    pub(crate) fn observer_window_for_test(&self, kind: SelectionKind) -> Option<Window> {
        self.windows.get(&kind).map(|windows| windows.observer)
    }

    pub(crate) fn is_internal_window(&self, window: Window) -> bool {
        self.internal_windows.contains(&window)
    }

    pub(crate) fn register_internal_window(&mut self, window: Window) {
        self.internal_windows.insert(window);
        debug_assert!(
            self.internal_windows.len()
                <= 4 + 2 * MAX_SELECTION_REQUESTOR_WINDOWS_PER_CHANNEL
                    + 2 * super::selection_payload::MAX_SELECTION_PAYLOAD_SLOTS_PER_CHANNEL
        );
    }

    pub(crate) fn register_payload_requestor_window(&mut self, window: Window) {
        self.internal_windows.insert(window);
        debug_assert!(
            self.internal_windows.len()
                <= 4 + 2 * MAX_SELECTION_REQUESTOR_WINDOWS_PER_CHANNEL
                    + 2 * super::selection_payload::MAX_SELECTION_PAYLOAD_SLOTS_PER_CHANNEL
        );
    }

    fn requestor(&self, kind: SelectionKind) -> Option<Window> {
        self.windows
            .get(&kind)
            .and_then(|windows| windows.requestor)
    }

    fn replace_requestor(&mut self, kind: SelectionKind, requestor: Window) {
        if let Some(windows) = self.windows.get_mut(&kind) {
            windows.requestor = Some(requestor);
            windows.requestor_windows_created += 1;
            windows.requestor_safety = RequestorSafety::Clean;
            debug_assert!(
                windows.requestor_windows_created <= MAX_SELECTION_REQUESTOR_WINDOWS_PER_CHANNEL
            );
        }
        self.internal_windows.insert(requestor);
        debug_assert!(
            self.internal_windows.len()
                <= 4 + 2 * MAX_SELECTION_REQUESTOR_WINDOWS_PER_CHANNEL
                    + 2 * super::selection_payload::MAX_SELECTION_PAYLOAD_SLOTS_PER_CHANNEL
        );
        self.debug_assert_invariants();
    }

    fn poison_requestor(&mut self, kind: SelectionKind) {
        let Some(windows) = self.windows.get_mut(&kind) else {
            return;
        };
        debug_assert!(windows.requestor.is_some());
        windows.requestor_safety = RequestorSafety::Poisoned;
        self.debug_assert_invariants();
    }

    fn invalidate_selection(&mut self, kind: SelectionKind, generation: XwaylandGeneration) {
        self.target_resolutions.remove(&kind);
        self.target_catalogs.remove(&kind);
        if self.current_offers.remove(&kind).is_some() {
            self.queue_selection_event(XwaylandSelectionEvent::Cleared {
                kind: public_selection_kind(kind),
                generation,
            });
        }
        self.debug_assert_invariants();
    }

    fn queue_selection_event(&mut self, event: XwaylandSelectionEvent) {
        let kind = match event.kind() {
            XwaylandSelectionKind::Clipboard => SelectionKind::Clipboard,
            XwaylandSelectionKind::Primary => SelectionKind::Primary,
        };
        self.pending_selection_events.insert(kind, event);
        debug_assert!(self.pending_selection_events.len() <= 2);
    }

    pub(crate) fn take_selection_events(&mut self) -> Vec<XwaylandSelectionEvent> {
        [SelectionKind::Clipboard, SelectionKind::Primary]
            .into_iter()
            .filter_map(|kind| self.pending_selection_events.remove(&kind))
            .collect()
    }

    pub(crate) fn take_selection_events_for_retirement(&mut self) -> Vec<XwaylandSelectionEvent> {
        let mut clipboard = None;
        let mut primary = None;
        for event in self.take_selection_events() {
            match event.kind() {
                XwaylandSelectionKind::Clipboard => clipboard = Some(event),
                XwaylandSelectionKind::Primary => primary = Some(event),
            }
        }

        for kind in [SelectionKind::Clipboard, SelectionKind::Primary] {
            let Some(offer) = self.current_offers.remove(&kind) else {
                continue;
            };
            let clear = XwaylandSelectionEvent::Cleared {
                kind: public_selection_kind(kind),
                generation: offer.generation,
            };
            match kind {
                SelectionKind::Clipboard => clipboard = Some(clear),
                SelectionKind::Primary => primary = Some(clear),
            }
        }

        [clipboard, primary].into_iter().flatten().collect()
    }

    #[cfg(test)]
    pub(crate) fn seed_external_offer_for_tests(
        &mut self,
        generation: XwaylandGeneration,
        kind: XwaylandSelectionKind,
        revision: u64,
    ) -> XwaylandSelectionOffer {
        let selection_kind = match kind {
            XwaylandSelectionKind::Clipboard => SelectionKind::Clipboard,
            XwaylandSelectionKind::Primary => SelectionKind::Primary,
        };
        let offer = XwaylandSelectionOffer {
            id: XwaylandSelectionOfferId {
                generation,
                kind,
                revision,
            },
            mime_types: vec!["text/plain".to_owned()],
        };
        self.current_offers.insert(selection_kind, offer.id);
        self.queue_selection_event(XwaylandSelectionEvent::OfferChanged {
            kind,
            offer: offer.clone(),
        });
        offer
    }

    #[cfg(test)]
    pub(crate) fn clear_external_offer_for_tests(
        &mut self,
        generation: XwaylandGeneration,
        kind: XwaylandSelectionKind,
    ) {
        let selection_kind = match kind {
            XwaylandSelectionKind::Clipboard => SelectionKind::Clipboard,
            XwaylandSelectionKind::Primary => SelectionKind::Primary,
        };
        self.invalidate_selection(selection_kind, generation);
    }

    #[cfg(test)]
    pub(crate) fn pending_target_atom_name_sequence_for_test(
        &self,
        kind: SelectionKind,
        target: Atom,
    ) -> Option<SequenceNumber> {
        self.pending.iter().find_map(|(sequence, pending)| {
            matches!(
                pending.kind,
                PendingReplyKind::TargetAtomName {
                    identity,
                    target: pending_target,
                    ..
                } if pending.selection == kind
                    && identity.kind == kind
                    && pending_target == target
            )
            .then_some(*sequence)
        })
    }

    #[cfg(test)]
    pub(crate) fn target_for_mime_for_test(
        &self,
        kind: SelectionKind,
        id: XwaylandSelectionOfferId,
        mime_type: &str,
    ) -> Option<Atom> {
        resolve_payload_target(self, id, mime_type)
            .filter(|(resolved_kind, _)| *resolved_kind == kind)
            .map(|(_, target)| target)
    }

    #[cfg(test)]
    pub(crate) fn proxy_mime_atom_sequence_for_test(
        &self,
        proxy_id: XwaylandProxySelectionId,
        mime_type: &str,
    ) -> Option<SequenceNumber> {
        let kind = internal_selection_kind(proxy_id.kind);
        self.pending.iter().find_map(|(sequence, pending)| {
            let PendingReplyKind::ProxyMimeAtom {
                proxy_id: pending_id,
                ordinal,
            } = pending.kind
            else {
                return None;
            };
            (pending_id == proxy_id
                && pending.selection == kind
                && self
                    .proxy_catalog_resolutions
                    .get(&kind)
                    .is_some_and(|resolution| {
                        resolution.id == proxy_id
                            && resolution
                                .mime_types
                                .get(ordinal)
                                .is_some_and(|mime| mime == mime_type)
                    }))
            .then_some(*sequence)
        })
    }

    #[cfg(test)]
    pub(super) fn prepared_proxy_selection_for_test(
        &self,
        kind: SelectionKind,
    ) -> Option<PreparedProxySelection> {
        self.prepared_proxy_selections.get(&kind).cloned()
    }

    #[cfg(test)]
    pub(crate) fn pending_proxy_replies_for_test(&self) -> usize {
        self.pending
            .values()
            .filter(|pending| matches!(pending.kind, PendingReplyKind::ProxyMimeAtom { .. }))
            .count()
    }

    #[cfg(test)]
    pub(crate) fn pending_selection_replies_for_test(&self) -> usize {
        self.pending.len()
    }

    fn next_target_name_request(
        &mut self,
    ) -> Option<(SelectionKind, SelectionIdentity, Atom, usize)> {
        let first = self.catalog_turn.unwrap_or(SelectionKind::Clipboard);
        let kinds = [first, other_selection_kind(first)];
        for kind in kinds {
            let Some(resolution) = self.target_resolutions.get_mut(&kind) else {
                continue;
            };
            while resolution.next_ordinal < resolution.targets.len() {
                let ordinal = resolution.next_ordinal;
                resolution.next_ordinal += 1;
                if resolution.names[ordinal] != TargetNameResolution::Pending {
                    continue;
                }
                resolution.names[ordinal] = TargetNameResolution::InFlight;
                self.catalog_turn = Some(other_selection_kind(kind));
                return Some((
                    kind,
                    resolution.identity,
                    resolution.targets[ordinal],
                    ordinal,
                ));
            }
        }
        None
    }

    fn next_proxy_mime_request(
        &mut self,
    ) -> Option<(SelectionKind, XwaylandProxySelectionId, String, usize)> {
        let first = self.proxy_catalog_turn.unwrap_or(SelectionKind::Clipboard);
        let kinds = [first, other_selection_kind(first)];
        for kind in kinds {
            let Some(resolution) = self.proxy_catalog_resolutions.get_mut(&kind) else {
                continue;
            };
            while resolution.next_ordinal < resolution.mime_types.len() {
                let ordinal = resolution.next_ordinal;
                resolution.next_ordinal += 1;
                if resolution.atoms[ordinal] != ProxyMimeResolution::Pending {
                    continue;
                }
                resolution.atoms[ordinal] = ProxyMimeResolution::InFlight;
                self.proxy_catalog_turn = Some(other_selection_kind(kind));
                return Some((
                    kind,
                    resolution.id,
                    resolution.mime_types[ordinal].clone(),
                    ordinal,
                ));
            }
        }
        None
    }

    fn next_resolution_request(&mut self) -> Option<SelectionResolutionRequest> {
        let first = self.resolution_direction;
        for direction in [first, first.other()] {
            let request = match direction {
                SelectionResolutionDirection::Inbound => {
                    self.next_target_name_request()
                        .map(|(kind, identity, target, ordinal)| {
                            SelectionResolutionRequest::Inbound {
                                kind,
                                identity,
                                target,
                                ordinal,
                            }
                        })
                }
                SelectionResolutionDirection::Outbound => {
                    self.next_proxy_mime_request()
                        .map(|(kind, proxy_id, mime_type, ordinal)| {
                            SelectionResolutionRequest::Outbound {
                                kind,
                                proxy_id,
                                mime_type,
                                ordinal,
                            }
                        })
                }
            };
            if let Some(request) = request {
                self.resolution_direction = direction.other();
                return Some(request);
            }
        }
        None
    }

    fn complete_target_name(
        &mut self,
        identity: SelectionIdentity,
        target: Atom,
        ordinal: usize,
        name: Option<String>,
    ) {
        let Some(resolution) = self.target_resolutions.get_mut(&identity.kind) else {
            return;
        };
        if resolution.identity != identity
            || resolution.targets.get(ordinal).copied() != Some(target)
        {
            return;
        }
        resolution.names[ordinal] = TargetNameResolution::Complete(name);
    }

    fn complete_proxy_mime_atom(
        &mut self,
        proxy_id: XwaylandProxySelectionId,
        ordinal: usize,
        atom: Option<Atom>,
    ) {
        let Some(resolution) = self
            .proxy_catalog_resolutions
            .get_mut(&internal_selection_kind(proxy_id.kind))
        else {
            return;
        };
        if resolution.id != proxy_id || resolution.mime_types.get(ordinal).is_none() {
            return;
        }
        resolution.atoms[ordinal] = ProxyMimeResolution::Complete(atom);
    }

    pub(crate) fn clear_generation(&mut self, generation: BridgeGeneration) -> Vec<SequenceNumber> {
        let sequences = self
            .pending
            .iter()
            .filter_map(|(sequence, pending)| {
                (pending.generation == generation).then_some(*sequence)
            })
            .collect::<Vec<_>>();
        for sequence in &sequences {
            self.pending.remove(sequence);
        }
        if self.active_generation == Some(generation) {
            self.active_generation = None;
            self.last_convert_selection_sequence = None;
            self.windows.clear();
            self.internal_windows.clear();
            self.target_catalogs.clear();
            self.target_resolutions.clear();
            self.pending_selection_events.clear();
            self.current_offers.clear();
            self.catalog_turn = None;
        }
        if self.proxy_generation == Some(generation) {
            self.proxy_generation = None;
            self.proxy_offers.clear();
            self.proxy_catalog_resolutions.clear();
            self.prepared_proxy_selections.clear();
            self.proxy_catalog_turn = None;
            self.resolution_direction = SelectionResolutionDirection::Inbound;
        }
        self.debug_assert_invariants();
        sequences
    }

    #[cfg(test)]
    pub(crate) fn install_fixture_windows(
        &mut self,
        generation: BridgeGeneration,
        clipboard_observer: Window,
        clipboard_requestor: Window,
        primary_observer: Window,
        primary_requestor: Window,
    ) {
        self.active_generation = Some(generation);
        self.windows.insert(
            SelectionKind::Clipboard,
            SelectionWindows {
                observer: clipboard_observer,
                requestor: Some(clipboard_requestor),
                requestor_windows_created: 1,
                requestor_safety: RequestorSafety::Clean,
            },
        );
        for window in [clipboard_observer, clipboard_requestor] {
            self.internal_windows.insert(window);
        }
        self.windows.insert(
            SelectionKind::Primary,
            SelectionWindows {
                observer: primary_observer,
                requestor: Some(primary_requestor),
                requestor_windows_created: 1,
                requestor_safety: RequestorSafety::Clean,
            },
        );
        for window in [primary_observer, primary_requestor] {
            self.internal_windows.insert(window);
        }
    }

    #[cfg(test)]
    pub(crate) fn exhaust_requestor_window_budget_for_test(&mut self, kind: SelectionKind) {
        if let Some(windows) = self.windows.get_mut(&kind) {
            windows.requestor_windows_created = MAX_SELECTION_REQUESTOR_WINDOWS_PER_CHANNEL;
        }
    }

    #[cfg(test)]
    pub(crate) fn last_convert_selection_sequence_for_test(&self) -> Option<SequenceNumber> {
        self.last_convert_selection_sequence
    }

    #[cfg(test)]
    pub(crate) fn pending_sequence_for_test(
        &self,
        kind: SelectionKind,
        targets_property: bool,
    ) -> Option<SequenceNumber> {
        self.pending.iter().find_map(|(sequence, pending)| {
            if pending.selection != kind {
                return None;
            }
            matches!(
                (targets_property, pending.kind),
                (false, PendingReplyKind::InitialOwnerProbe { .. })
                    | (true, PendingReplyKind::TargetsProperty { .. })
            )
            .then_some(*sequence)
        })
    }

    #[cfg(test)]
    pub(crate) fn requestor_poisoned_for_test(&self, kind: SelectionKind) -> bool {
        self.windows
            .get(&kind)
            .is_some_and(|windows| windows.requestor_safety == RequestorSafety::Poisoned)
    }

    fn debug_assert_invariants(&self) {
        debug_assert!(self.windows.values().all(|windows| {
            windows.requestor.is_some() || windows.requestor_safety == RequestorSafety::Clean
        }));
        debug_assert!(self.windows.values().all(|windows| {
            windows.requestor_windows_created <= MAX_SELECTION_REQUESTOR_WINDOWS_PER_CHANNEL
        }));
        debug_assert!(
            self.target_catalogs
                .values()
                .all(|catalog| { catalog.bindings.len() <= MAX_SELECTION_MIME_TYPES })
        );
        debug_assert!(self.target_resolutions.values().all(|resolution| {
            resolution.targets.len() <= super::data_bridge::selection::MAX_SELECTION_TARGETS
                && resolution.targets.len() == resolution.names.len()
        }));
        debug_assert!(self.proxy_offers.len() <= 2);
        debug_assert!(self.proxy_catalog_resolutions.values().all(|resolution| {
            resolution.mime_types.len() <= MAX_SELECTION_MIME_TYPES
                && resolution.mime_types.len() == resolution.atoms.len()
        }));
        debug_assert!(self.prepared_proxy_selections.values().all(|prepared| {
            prepared.mime_types.len() <= MAX_SELECTION_MIME_TYPES
                && prepared.target_order.len() <= 3 + MAX_SELECTION_MIME_TYPES + 2
                && prepared.data_targets.len() <= MAX_SELECTION_MIME_TYPES + 2
        }));
        debug_assert!(self.pending_selection_events.len() <= 2);
    }
}

pub(crate) fn resolve_payload_target(
    wire: &SelectionWireState,
    offer_id: XwaylandSelectionOfferId,
    mime_type: &str,
) -> Option<(SelectionKind, Atom)> {
    let kind = match offer_id.kind {
        XwaylandSelectionKind::Clipboard => SelectionKind::Clipboard,
        XwaylandSelectionKind::Primary => SelectionKind::Primary,
    };
    let generation = BridgeGeneration::from(offer_id.generation);
    if wire.active_generation != Some(generation)
        || wire.current_offers.get(&kind) != Some(&offer_id)
    {
        return None;
    }
    let catalog = wire.target_catalogs.get(&kind)?;
    if catalog.identity.kind != kind
        || catalog.identity.generation != generation
        || catalog.identity.revision.get() != offer_id.revision
    {
        return None;
    }
    let target = catalog
        .bindings
        .iter()
        .find(|binding| binding.mime_type == mime_type)?
        .target;
    Some((kind, target))
}

pub(crate) fn is_internal_window(
    xid: Window,
    supporting_wm_check: Option<Window>,
    selection_wire: Option<&SelectionWireState>,
) -> bool {
    supporting_wm_check == Some(xid)
        || selection_wire.is_some_and(|wire| wire.is_internal_window(xid))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SelectionReplyDrain {
    pub(crate) processed: usize,
    pub(crate) budget_exhausted: bool,
    pub(crate) quiescent: bool,
}

/// Install private windows, subscribe first, then probe both current owners.
/// Missing XFixes keeps the managed XWM selection foundation inactive.
pub(crate) fn initialize(xwm: &mut Xwm) -> Result<(), XwmError> {
    let generation = BridgeGeneration::from(xwm.generation);
    xwm.data_bridge.selection_wire.proxy_generation = Some(generation);
    xwm.data_bridge
        .selection_outgoing
        .initialize_generation(generation);
    super::selection_proxy::initialize(xwm)?;
    if !xwm.capabilities.xfixes {
        return Ok(());
    }

    xwm.data_bridge.selections.initialize_generation(generation);
    xwm.data_bridge
        .selection_payloads
        .initialize_generation(generation);
    let mut channels = HashMap::new();
    for kind in [SelectionKind::Clipboard, SelectionKind::Primary] {
        let observer = xwm
            .connection
            .generate_id()
            .map_err(|error| XwmError::IdAllocation(error.to_string()))?;
        create_private_window(xwm, observer)?;
        let requestor = xwm
            .connection
            .generate_id()
            .map_err(|error| XwmError::IdAllocation(error.to_string()))?;
        create_private_window(xwm, requestor)?;
        channels.insert(
            kind,
            SelectionWindows {
                observer,
                requestor: Some(requestor),
                requestor_windows_created: 1,
                requestor_safety: RequestorSafety::Clean,
            },
        );
        xwm.data_bridge
            .selection_wire
            .internal_windows
            .extend([observer, requestor]);
        debug_assert!(
            xwm.data_bridge.selection_wire.internal_windows.len()
                <= 4 + 2 * MAX_SELECTION_REQUESTOR_WINDOWS_PER_CHANNEL
                    + 2 * super::selection_payload::MAX_SELECTION_PAYLOAD_SLOTS_PER_CHANNEL
        );
    }
    xwm.data_bridge.selection_wire.active_generation = Some(generation);
    xwm.data_bridge.selection_wire.windows = channels;

    let event_mask = xfixes::SelectionEventMask::SET_SELECTION_OWNER
        | xfixes::SelectionEventMask::SELECTION_WINDOW_DESTROY
        | xfixes::SelectionEventMask::SELECTION_CLIENT_CLOSE;
    for kind in [SelectionKind::Clipboard, SelectionKind::Primary] {
        let observer = xwm
            .data_bridge
            .selection_wire
            .windows
            .get(&kind)
            .expect("selection channel initialized")
            .observer;
        let cookie = xwm
            .connection
            .xfixes_select_selection_input(observer, selection_atom(xwm, kind), event_mask)
            .map_err(XwmError::Connection)?;
        std::mem::forget(cookie);
    }

    for kind in [SelectionKind::Clipboard, SelectionKind::Primary] {
        let revision_at_issue = xwm
            .data_bridge
            .selections
            .current(kind)
            .and_then(|state| state.revision);
        let cookie = xwm
            .connection
            .get_selection_owner(selection_atom(xwm, kind))
            .map_err(XwmError::Connection)?;
        let sequence = cookie.sequence_number();
        std::mem::forget(cookie);
        insert_pending(
            xwm,
            sequence,
            PendingSelectionReply {
                generation,
                selection: kind,
                kind: PendingReplyKind::InitialOwnerProbe { revision_at_issue },
            },
        );
    }
    xwm.connection.flush().map_err(XwmError::Connection)
}

/// Replace the bounded latest-state export metadata for each channel. This
/// schedules only asynchronous InternAtom requests and never mutates X11
/// selection ownership or handles conversion requests.
pub(crate) fn submit_proxy_selection_snapshots(
    xwm: &mut Xwm,
    snapshots: impl IntoIterator<Item = XwaylandProxySelectionSnapshot>,
) -> Result<(), XwmError> {
    let generation = BridgeGeneration::from(xwm.generation);
    if xwm.data_bridge.selection_wire.proxy_generation != Some(generation) {
        return Ok(());
    }

    for snapshot in snapshots {
        let kind = internal_selection_kind(snapshot.kind);
        let has_offer = snapshot.offer.is_some();
        let offer = snapshot.offer.and_then(|mut offer| {
            if offer.id.kind != snapshot.kind
                || offer.id.selection_generation != snapshot.selection_generation
                || offer.mime_types.len() > MAX_SELECTION_MIME_TYPES
                || offer
                    .mime_types
                    .iter()
                    .any(|mime| mime.is_empty() || mime.len() > MAX_MIME_TYPE_LEN)
            {
                return None;
            }
            let mut seen = HashSet::with_capacity(offer.mime_types.len());
            offer.mime_types.retain(|mime| seen.insert(mime.clone()));
            (!offer.mime_types.is_empty()).then_some(offer)
        });
        if has_offer && offer.is_none() {
            continue;
        }

        if let Some(offer) = &offer
            && xwm
                .data_bridge
                .selection_wire
                .proxy_offers
                .get(&kind)
                .is_some_and(|current| current == offer)
        {
            continue;
        }

        cancel_proxy_catalog_for_channel(xwm, generation, kind);
        let Some(offer) = offer else {
            continue;
        };
        let mime_types = offer.mime_types.clone();
        let atoms = mime_types
            .iter()
            .map(|mime_type| {
                if is_known_selection_atom_name(mime_type) {
                    ProxyMimeResolution::Complete(None)
                } else {
                    ProxyMimeResolution::Pending
                }
            })
            .collect();
        xwm.data_bridge
            .selection_wire
            .proxy_offers
            .insert(kind, offer.clone());
        xwm.data_bridge
            .selection_wire
            .proxy_catalog_resolutions
            .insert(
                kind,
                ProxyCatalogResolution {
                    id: offer.id,
                    atoms,
                    mime_types,
                    next_ordinal: 0,
                },
            );
    }
    schedule_selection_resolution_queries(xwm)
}

fn cancel_proxy_catalog_for_channel(
    xwm: &mut Xwm,
    generation: BridgeGeneration,
    kind: SelectionKind,
) {
    let sequences = xwm
        .data_bridge
        .selection_wire
        .pending
        .iter()
        .filter_map(|(sequence, pending)| {
            (pending.generation == generation
                && pending.selection == kind
                && matches!(pending.kind, PendingReplyKind::ProxyMimeAtom { .. }))
            .then_some(*sequence)
        })
        .collect::<Vec<_>>();
    for sequence in sequences {
        xwm.connection.discard_reply(
            sequence,
            RequestKind::HasResponse,
            DiscardMode::DiscardReply,
        );
        xwm.data_bridge.selection_wire.pending.remove(&sequence);
    }
    let wire = &mut xwm.data_bridge.selection_wire;
    wire.proxy_offers.remove(&kind);
    wire.proxy_catalog_resolutions.remove(&kind);
    wire.prepared_proxy_selections.remove(&kind);
    wire.debug_assert_invariants();
}

fn create_private_window(xwm: &Xwm, window: Window) -> Result<(), XwmError> {
    let attributes = xproto::CreateWindowAux::new().event_mask(xproto::EventMask::PROPERTY_CHANGE);
    let cookie = xwm
        .connection
        .create_window(
            0,
            window,
            xwm.supporting_wm_check,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &attributes,
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(cookie);
    Ok(())
}

pub(crate) fn prepared_proxy_selection(
    xwm: &Xwm,
    kind: SelectionKind,
) -> Option<&PreparedProxySelection> {
    xwm.data_bridge
        .selection_wire
        .prepared_proxy_selections
        .get(&kind)
}

pub(crate) fn resolve_proxy_target(
    xwm: &Xwm,
    proxy_id: XwaylandProxySelectionId,
    target: Atom,
) -> Option<ProxyTargetBinding> {
    let kind = internal_selection_kind(proxy_id.kind);
    let prepared = prepared_proxy_selection(xwm, kind)?;
    if prepared.id != proxy_id {
        return None;
    }
    prepared
        .data_targets
        .iter()
        .find(|binding| binding.target == target)
        .cloned()
}

fn selection_atom(xwm: &Xwm, kind: SelectionKind) -> Atom {
    match kind {
        SelectionKind::Clipboard => xwm.atoms.get(XwmAtomName::Clipboard),
        SelectionKind::Primary => u32::from(xproto::AtomEnum::PRIMARY),
    }
}

fn public_selection_kind(kind: SelectionKind) -> XwaylandSelectionKind {
    match kind {
        SelectionKind::Clipboard => XwaylandSelectionKind::Clipboard,
        SelectionKind::Primary => XwaylandSelectionKind::Primary,
    }
}

fn internal_selection_kind(kind: XwaylandSelectionKind) -> SelectionKind {
    match kind {
        XwaylandSelectionKind::Clipboard => SelectionKind::Clipboard,
        XwaylandSelectionKind::Primary => SelectionKind::Primary,
    }
}

fn other_selection_kind(kind: SelectionKind) -> SelectionKind {
    match kind {
        SelectionKind::Clipboard => SelectionKind::Primary,
        SelectionKind::Primary => SelectionKind::Clipboard,
    }
}

fn kind_for_atom(xwm: &Xwm, selection: Atom) -> Option<SelectionKind> {
    if selection == xwm.atoms.get(XwmAtomName::Clipboard) {
        Some(SelectionKind::Clipboard)
    } else if selection == u32::from(xproto::AtomEnum::PRIMARY) {
        Some(SelectionKind::Primary)
    } else {
        None
    }
}

pub(crate) fn observe_xfixes(
    xwm: &mut Xwm,
    event: xfixes::SelectionNotifyEvent,
) -> Result<(), XwmError> {
    if !xwm.capabilities.xfixes {
        return Ok(());
    }
    let Some(generation) = xwm.data_bridge.selection_wire.active_generation else {
        return Ok(());
    };
    let Some(kind) = kind_for_atom(xwm, event.selection) else {
        return Ok(());
    };
    let Some(windows) = xwm.data_bridge.selection_wire.windows.get(&kind).copied() else {
        return Ok(());
    };
    if event.window != windows.observer {
        return Ok(());
    }

    let owner = match event.subtype {
        xfixes::SelectionEvent::SET_SELECTION_OWNER => (event.owner != 0).then_some(event.owner),
        xfixes::SelectionEvent::SELECTION_WINDOW_DESTROY
        | xfixes::SelectionEvent::SELECTION_CLIENT_CLOSE => None,
        _ => return Ok(()),
    };
    apply_owner_transition(xwm, generation, kind, owner, event.selection_timestamp)
}

fn replace_requestor(xwm: &mut Xwm, kind: SelectionKind) -> Result<bool, XwmError> {
    let Some(windows) = xwm.data_bridge.selection_wire.windows.get(&kind) else {
        return Ok(false);
    };
    if windows.requestor_windows_created >= MAX_SELECTION_REQUESTOR_WINDOWS_PER_CHANNEL {
        return Ok(false);
    }
    let old_requestor = windows.requestor;
    let requestor = xwm
        .connection
        .generate_id()
        .map_err(|error| XwmError::IdAllocation(error.to_string()))?;
    create_private_window(xwm, requestor)?;
    xwm.data_bridge
        .selection_wire
        .replace_requestor(kind, requestor);
    if let Some(old) = old_requestor {
        let cookie = xwm
            .connection
            .destroy_window(old)
            .map_err(XwmError::Connection)?;
        std::mem::forget(cookie);
    }
    Ok(true)
}

fn start_targets_conversion(xwm: &mut Xwm, identity: SelectionIdentity) -> Result<(), XwmError> {
    let Some(windows) = xwm
        .data_bridge
        .selection_wire
        .windows
        .get(&identity.kind)
        .copied()
    else {
        xwm.data_bridge
            .selections
            .mark_discovery_state(identity, TargetsDiscoveryState::Failed);
        return Ok(());
    };
    let Some(requestor) = windows.requestor else {
        xwm.data_bridge
            .selections
            .mark_discovery_state(identity, TargetsDiscoveryState::Failed);
        return Ok(());
    };
    if windows.requestor_safety == RequestorSafety::Poisoned {
        xwm.data_bridge
            .selections
            .mark_discovery_state(identity, TargetsDiscoveryState::Failed);
        return Ok(());
    }
    if !xwm
        .data_bridge
        .selections
        .mark_discovery_state(identity, TargetsDiscoveryState::AwaitingSelectionNotify)
    {
        return Ok(());
    }
    let timestamp = xwm
        .data_bridge
        .selections
        .current(identity.kind)
        .map(|state| state.timestamp)
        .unwrap_or(0);
    let cookie = xwm
        .connection
        .convert_selection(
            requestor,
            selection_atom(xwm, identity.kind),
            xwm.atoms.get(XwmAtomName::Targets),
            xwm.atoms.get(XwmAtomName::SelectionTargets),
            timestamp,
        )
        .map_err(XwmError::Connection)?;
    xwm.data_bridge
        .selection_wire
        .last_convert_selection_sequence = Some(cookie.sequence_number());
    std::mem::forget(cookie);
    xwm.connection.flush().map_err(XwmError::Connection)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestorPlan {
    Reuse,
    Rotate,
    Blocked,
}

fn requestor_rotation_required(prior: Option<&SelectionSnapshot>, safety: RequestorSafety) -> bool {
    safety == RequestorSafety::Poisoned
        || prior.is_some_and(|prior| {
            prior.targets_state == TargetsDiscoveryState::AwaitingSelectionNotify
        })
}

fn requestor_plan(
    windows: Option<SelectionWindows>,
    prior: Option<&SelectionSnapshot>,
    owner_is_requestor: bool,
) -> RequestorPlan {
    let Some(windows) = windows else {
        return RequestorPlan::Blocked;
    };
    if windows.requestor.is_none() {
        return RequestorPlan::Blocked;
    }
    if !requestor_rotation_required(prior, windows.requestor_safety) {
        return RequestorPlan::Reuse;
    }
    if owner_is_requestor {
        RequestorPlan::Blocked
    } else {
        RequestorPlan::Rotate
    }
}

pub(crate) fn selection_notify(
    xwm: &mut Xwm,
    event: xproto::SelectionNotifyEvent,
) -> Result<(), XwmError> {
    let Some(generation) = xwm.data_bridge.selection_wire.active_generation else {
        return Ok(());
    };
    let Some(kind) = kind_for_atom(xwm, event.selection) else {
        return Ok(());
    };
    let Some(state) = xwm.data_bridge.selections.current(kind).cloned() else {
        return Ok(());
    };
    let Some(identity) = state.identity() else {
        return Ok(());
    };
    let expected_property = xwm.atoms.get(XwmAtomName::SelectionTargets);
    if state.generation != generation
        || state.targets_state != TargetsDiscoveryState::AwaitingSelectionNotify
        || xwm.data_bridge.selection_wire.requestor(kind) != Some(event.requestor)
        || event.selection != selection_atom(xwm, kind)
        || event.target != xwm.atoms.get(XwmAtomName::Targets)
        || (state.timestamp != 0 && event.time != state.timestamp)
        || (event.property != expected_property
            && event.property != u32::from(xproto::AtomEnum::NONE))
    {
        return Ok(());
    }

    if event.property == u32::from(xproto::AtomEnum::NONE) {
        xwm.data_bridge
            .selections
            .mark_discovery_state(identity, TargetsDiscoveryState::Failed);
        return Ok(());
    }
    request_targets_property(xwm, identity, event.requestor, expected_property)
}

fn request_targets_property(
    xwm: &mut Xwm,
    identity: SelectionIdentity,
    requestor: Window,
    property: Atom,
) -> Result<(), XwmError> {
    if !xwm
        .data_bridge
        .selections
        .mark_discovery_state(identity, TargetsDiscoveryState::AwaitingProperty)
    {
        return Ok(());
    }
    let cookie = xwm
        .connection
        .get_property(
            false,
            requestor,
            property,
            xproto::AtomEnum::ATOM,
            0,
            TARGETS_PROPERTY_ITEMS,
        )
        .map_err(XwmError::Connection)?;
    let sequence = cookie.sequence_number();
    std::mem::forget(cookie);
    insert_pending(
        xwm,
        sequence,
        PendingSelectionReply {
            generation: identity.generation,
            selection: identity.kind,
            kind: PendingReplyKind::TargetsProperty {
                identity,
                requestor,
                property,
            },
        },
    );
    xwm.connection.flush().map_err(XwmError::Connection)
}

fn begin_target_catalog_resolution(
    xwm: &mut Xwm,
    identity: SelectionIdentity,
    targets: &[Atom],
) -> Result<(), XwmError> {
    let mut unique_targets = Vec::with_capacity(targets.len());
    let mut seen = HashSet::with_capacity(targets.len());
    for target in targets.iter().copied() {
        if seen.insert(target) {
            unique_targets.push(target);
        }
    }
    debug_assert!(unique_targets.len() <= super::data_bridge::selection::MAX_SELECTION_TARGETS);

    let names = unique_targets
        .iter()
        .copied()
        .map(|target| {
            if is_control_target(xwm, target) || is_compatibility_alias(xwm, target) {
                TargetNameResolution::Complete(None)
            } else {
                TargetNameResolution::Pending
            }
        })
        .collect();
    xwm.data_bridge.selection_wire.target_resolutions.insert(
        identity.kind,
        TargetCatalogResolution {
            identity,
            targets: unique_targets,
            names,
            next_ordinal: 0,
        },
    );
    schedule_selection_resolution_queries(xwm)
}

#[cfg(test)]
pub(crate) fn begin_target_catalog_resolution_for_test(
    xwm: &mut Xwm,
    identity: SelectionIdentity,
    targets: &[Atom],
) -> Result<(), XwmError> {
    begin_target_catalog_resolution(xwm, identity, targets)
}

#[cfg(test)]
pub(crate) fn complete_target_name_for_test(
    xwm: &mut Xwm,
    sequence: SequenceNumber,
    identity: SelectionIdentity,
    target: Atom,
    ordinal: usize,
    name: Option<String>,
) -> Result<(), XwmError> {
    xwm.data_bridge.selection_wire.pending.remove(&sequence);
    xwm.connection.discard_reply(
        sequence,
        RequestKind::HasResponse,
        DiscardMode::DiscardReply,
    );
    xwm.data_bridge
        .selection_wire
        .complete_target_name(identity, target, ordinal, name);
    schedule_selection_resolution_queries(xwm)
}

#[cfg(test)]
pub(crate) fn complete_proxy_mime_atom_for_test(
    xwm: &mut Xwm,
    proxy_id: XwaylandProxySelectionId,
    ordinal: usize,
    atom: Option<Atom>,
) -> Result<(), XwmError> {
    let sequence = xwm
        .data_bridge
        .selection_wire
        .pending
        .iter()
        .find_map(|(sequence, pending)| {
            matches!(
                pending.kind,
                PendingReplyKind::ProxyMimeAtom {
                    proxy_id: pending_id,
                    ordinal: pending_ordinal,
                } if pending_id == proxy_id && pending_ordinal == ordinal
            )
            .then_some(*sequence)
        });
    if let Some(sequence) = sequence {
        xwm.data_bridge.selection_wire.pending.remove(&sequence);
        xwm.connection.discard_reply(
            sequence,
            RequestKind::HasResponse,
            DiscardMode::DiscardReply,
        );
    }
    xwm.data_bridge
        .selection_wire
        .complete_proxy_mime_atom(proxy_id, ordinal, atom);
    schedule_selection_resolution_queries(xwm)
}

fn schedule_selection_resolution_queries(xwm: &mut Xwm) -> Result<(), XwmError> {
    finalize_ready_target_catalogs(xwm);
    finalize_ready_proxy_catalogs(xwm);
    while xwm.data_bridge.selection_wire.pending.len() < MAX_PENDING_SELECTION_REPLIES {
        let Some(request) = xwm.data_bridge.selection_wire.next_resolution_request() else {
            break;
        };
        match request {
            SelectionResolutionRequest::Inbound {
                kind,
                identity,
                target,
                ordinal,
            } => {
                let cookie = xwm
                    .connection
                    .get_atom_name(target)
                    .map_err(XwmError::Connection)?;
                let sequence = cookie.sequence_number();
                std::mem::forget(cookie);
                insert_pending(
                    xwm,
                    sequence,
                    PendingSelectionReply {
                        generation: identity.generation,
                        selection: kind,
                        kind: PendingReplyKind::TargetAtomName {
                            identity,
                            target,
                            ordinal,
                        },
                    },
                );
            }
            SelectionResolutionRequest::Outbound {
                kind,
                proxy_id,
                mime_type,
                ordinal,
            } => {
                let cookie = xwm
                    .connection
                    .intern_atom(false, mime_type.as_bytes())
                    .map_err(XwmError::Connection)?;
                let sequence = cookie.sequence_number();
                std::mem::forget(cookie);
                insert_pending(
                    xwm,
                    sequence,
                    PendingSelectionReply {
                        generation: BridgeGeneration::from(xwm.generation),
                        selection: kind,
                        kind: PendingReplyKind::ProxyMimeAtom { proxy_id, ordinal },
                    },
                );
            }
        }
    }
    finalize_ready_target_catalogs(xwm);
    finalize_ready_proxy_catalogs(xwm);
    xwm.connection.flush().map_err(XwmError::Connection)
}

fn finalize_ready_proxy_catalogs(xwm: &mut Xwm) {
    let ready = xwm
        .data_bridge
        .selection_wire
        .proxy_catalog_resolutions
        .iter()
        .filter(|(_, resolution)| {
            resolution.next_ordinal == resolution.mime_types.len()
                && resolution
                    .atoms
                    .iter()
                    .all(|atom| matches!(atom, ProxyMimeResolution::Complete(_)))
        })
        .map(|(kind, resolution)| (*kind, resolution.clone()))
        .collect::<Vec<_>>();

    for (kind, resolution) in ready {
        let current = xwm
            .data_bridge
            .selection_wire
            .proxy_offers
            .get(&kind)
            .is_some_and(|offer| offer.id == resolution.id);
        if !current {
            if xwm
                .data_bridge
                .selection_wire
                .proxy_catalog_resolutions
                .get(&kind)
                .is_some_and(|current| current.id == resolution.id)
            {
                xwm.data_bridge
                    .selection_wire
                    .proxy_catalog_resolutions
                    .remove(&kind);
            }
            continue;
        }

        let protocol_targets = vec![
            xwm.atoms.get(XwmAtomName::Targets),
            xwm.atoms.get(XwmAtomName::Multiple),
            xwm.atoms.get(XwmAtomName::Timestamp),
        ];
        let mut reserved = protocol_targets.iter().copied().collect::<HashSet<_>>();
        reserved.insert(xwm.atoms.get(XwmAtomName::Incr));
        let mut seen = reserved.clone();
        let mut data_targets = Vec::new();
        for (mime_type, atom) in resolution.mime_types.iter().zip(&resolution.atoms) {
            let ProxyMimeResolution::Complete(Some(target)) = atom else {
                continue;
            };
            if seen.insert(*target) {
                data_targets.push(ProxyTargetBinding {
                    target: *target,
                    mime_type: mime_type.clone(),
                });
            }
        }

        for (mime_type, alias) in [
            ("text/plain;charset=utf-8", XwmAtomName::Utf8String),
            ("text/plain", XwmAtomName::Text),
        ] {
            if !resolution.mime_types.iter().any(|mime| mime == mime_type) {
                continue;
            }
            let target = xwm.atoms.get(alias);
            if seen.insert(target) {
                data_targets.push(ProxyTargetBinding {
                    target,
                    mime_type: mime_type.to_owned(),
                });
            }
        }

        let mut target_order = protocol_targets.clone();
        target_order.extend(data_targets.iter().map(|binding| binding.target));
        let prepared = PreparedProxySelection {
            id: resolution.id,
            protocol_targets,
            data_targets,
            target_order,
            mime_types: resolution.mime_types,
        };
        xwm.data_bridge
            .selection_wire
            .proxy_catalog_resolutions
            .remove(&kind);
        xwm.data_bridge
            .selection_wire
            .prepared_proxy_selections
            .insert(kind, prepared);
    }
}

fn finalize_ready_target_catalogs(xwm: &mut Xwm) {
    let ready = xwm
        .data_bridge
        .selection_wire
        .target_resolutions
        .iter()
        .filter(|(_, resolution)| {
            resolution.next_ordinal == resolution.targets.len()
                && resolution
                    .names
                    .iter()
                    .all(|name| matches!(name, TargetNameResolution::Complete(_)))
        })
        .map(|(kind, resolution)| (*kind, resolution.clone()))
        .collect::<Vec<_>>();

    for (kind, resolution) in ready {
        let current = xwm
            .data_bridge
            .selections
            .current(kind)
            .is_some_and(|state| {
                state.identity() == Some(resolution.identity)
                    && state.generation == resolution.identity.generation
                    && state.origin == Some(SelectionOrigin::X11)
                    && state.targets_state == TargetsDiscoveryState::Resolved
            });
        if !current {
            xwm.data_bridge
                .selection_wire
                .target_resolutions
                .remove(&kind);
            continue;
        }

        let mut bindings = Vec::new();
        for (target, name) in resolution.targets.iter().copied().zip(&resolution.names) {
            let TargetNameResolution::Complete(Some(name)) = name else {
                continue;
            };
            let Some(name) = valid_mime_name(name.as_bytes()) else {
                continue;
            };
            if !bindings
                .iter()
                .any(|binding: &SelectionTargetBinding| binding.mime_type == name)
            {
                bindings.push(SelectionTargetBinding {
                    mime_type: name,
                    target,
                });
            }
        }
        for (target, _name) in resolution.targets.iter().copied().zip(&resolution.names) {
            let mime_type = if name_is_alias(xwm, target, XwmAtomName::Utf8String) {
                Some("text/plain;charset=utf-8")
            } else if name_is_alias(xwm, target, XwmAtomName::Text) {
                Some("text/plain")
            } else {
                None
            };
            let Some(mime_type) = mime_type else {
                continue;
            };
            if !bindings
                .iter()
                .any(|binding: &SelectionTargetBinding| binding.mime_type == mime_type)
            {
                bindings.push(SelectionTargetBinding {
                    mime_type: mime_type.to_owned(),
                    target,
                });
            }
        }

        let catalog = SelectionTargetCatalog {
            identity: resolution.identity,
            bindings,
        };
        let offer_id = XwaylandSelectionOfferId {
            generation: xwm.generation,
            kind: public_selection_kind(kind),
            revision: resolution.identity.revision.get(),
        };
        let mime_types = catalog
            .bindings
            .iter()
            .map(|binding| binding.mime_type.clone())
            .collect::<Vec<_>>();
        xwm.data_bridge
            .selection_wire
            .target_resolutions
            .remove(&kind);
        xwm.data_bridge
            .selection_wire
            .target_catalogs
            .insert(kind, catalog);
        if !mime_types.is_empty() {
            xwm.data_bridge
                .selection_wire
                .current_offers
                .insert(kind, offer_id);
            xwm.data_bridge.selection_wire.queue_selection_event(
                XwaylandSelectionEvent::OfferChanged {
                    kind: public_selection_kind(kind),
                    offer: XwaylandSelectionOffer {
                        id: offer_id,
                        mime_types,
                    },
                },
            );
        }
    }
}

fn valid_mime_name(bytes: &[u8]) -> Option<String> {
    if bytes.len() > MAX_MIME_TYPE_LEN || bytes.contains(&0) {
        return None;
    }
    let name = String::from_utf8(bytes.to_vec()).ok()?;
    (!name.is_empty() && name.contains('/')).then_some(name)
}

fn is_control_target(xwm: &Xwm, target: Atom) -> bool {
    [
        XwmAtomName::Targets,
        XwmAtomName::Timestamp,
        XwmAtomName::Multiple,
        XwmAtomName::Incr,
        XwmAtomName::SelectionTargets,
    ]
    .into_iter()
    .any(|name| xwm.atoms.get(name) == target)
}

fn is_known_selection_atom_name(name: &str) -> bool {
    matches!(
        name,
        "TARGETS"
            | "MULTIPLE"
            | "TIMESTAMP"
            | "UTF8_STRING"
            | "TEXT"
            | "PRIMARY"
            | "CLIPBOARD"
            | "INCR"
    )
}

fn is_compatibility_alias(xwm: &Xwm, target: Atom) -> bool {
    name_is_alias(xwm, target, XwmAtomName::Utf8String)
        || name_is_alias(xwm, target, XwmAtomName::Text)
}

fn name_is_alias(xwm: &Xwm, target: Atom, alias: XwmAtomName) -> bool {
    xwm.atoms.get(alias) == target
}

fn insert_pending(xwm: &mut Xwm, sequence: SequenceNumber, pending: PendingSelectionReply) {
    let tracker = &mut xwm.data_bridge.selection_wire.pending;
    debug_assert!(tracker.len() < MAX_PENDING_SELECTION_REPLIES);
    if tracker.len() < MAX_PENDING_SELECTION_REPLIES {
        tracker.insert(sequence, pending);
    }
}

fn cancel_channel_replies(xwm: &mut Xwm, generation: BridgeGeneration, kind: SelectionKind) {
    let sequences = xwm
        .data_bridge
        .selection_wire
        .pending
        .iter()
        .filter_map(|(sequence, pending)| {
            (pending.generation == generation
                && pending.selection == kind
                && !matches!(pending.kind, PendingReplyKind::ProxyMimeAtom { .. }))
            .then_some(*sequence)
        })
        .collect::<Vec<_>>();
    for sequence in sequences {
        xwm.connection.discard_reply(
            sequence,
            RequestKind::HasResponse,
            DiscardMode::DiscardReply,
        );
        xwm.data_bridge.selection_wire.pending.remove(&sequence);
    }
}

pub(crate) fn poll_replies(xwm: &mut Xwm, budget: usize) -> Result<SelectionReplyDrain, XwmError> {
    if budget == 0 {
        return Ok(SelectionReplyDrain {
            processed: 0,
            budget_exhausted: false,
            quiescent: false,
        });
    }
    let sequences = xwm
        .data_bridge
        .selection_wire
        .pending
        .keys()
        .copied()
        .take(budget)
        .collect::<Vec<_>>();
    let budget_exhausted = sequences.len() < xwm.data_bridge.selection_wire.pending.len();
    let mut processed = 0;
    for sequence in sequences {
        let Some(pending) = xwm
            .data_bridge
            .selection_wire
            .pending
            .get(&sequence)
            .copied()
        else {
            continue;
        };
        match pending.kind {
            PendingReplyKind::InitialOwnerProbe { revision_at_issue } => {
                let cookie = Cookie::<X11Connection, xproto::GetSelectionOwnerReply>::new(
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
                xwm.data_bridge.selection_wire.pending.remove(&sequence);
                processed += 1;
                let Some(reply) = reply else {
                    continue;
                };
                if !probe_revision_is_current(xwm, pending, revision_at_issue) {
                    continue;
                }
                let owner = (reply.owner != 0).then_some(reply.owner);
                apply_owner_transition(xwm, pending.generation, pending.selection, owner, 0)?;
            }
            PendingReplyKind::TargetsProperty {
                identity,
                requestor,
                property,
            } => {
                let cookie = Cookie::<X11Connection, xproto::GetPropertyReply>::new(
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
                xwm.data_bridge.selection_wire.pending.remove(&sequence);
                processed += 1;
                let Some(reply) = reply else {
                    mark_failed_if_current(xwm, identity);
                    continue;
                };
                let current = xwm
                    .data_bridge
                    .selections
                    .current(identity.kind)
                    .is_some_and(|state| {
                        state.identity() == Some(identity)
                            && state.generation == pending.generation
                            && xwm.data_bridge.selection_wire.requestor(identity.kind)
                                == Some(requestor)
                    });
                if !current {
                    continue;
                }
                if property != xwm.atoms.get(XwmAtomName::SelectionTargets) {
                    mark_failed_if_current(xwm, identity);
                } else if let Some(targets) = parse_targets_property(&reply) {
                    if !xwm
                        .data_bridge
                        .selections
                        .resolve_targets(identity, &targets)
                    {
                        mark_failed_if_current(xwm, identity);
                    } else {
                        begin_target_catalog_resolution(xwm, identity, &targets)?;
                    }
                } else {
                    mark_failed_if_current(xwm, identity);
                }
            }
            PendingReplyKind::TargetAtomName {
                identity,
                target,
                ordinal,
            } => {
                let cookie = Cookie::<X11Connection, xproto::GetAtomNameReply>::new(
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
                xwm.data_bridge.selection_wire.pending.remove(&sequence);
                processed += 1;
                let name = reply.and_then(|reply| valid_mime_name(&reply.name));
                xwm.data_bridge
                    .selection_wire
                    .complete_target_name(identity, target, ordinal, name);
            }
            PendingReplyKind::ProxyMimeAtom { proxy_id, ordinal } => {
                let cookie = Cookie::<X11Connection, xproto::InternAtomReply>::new(
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
                xwm.data_bridge.selection_wire.pending.remove(&sequence);
                processed += 1;
                let atom = reply.and_then(|reply| (reply.atom != 0).then_some(reply.atom));
                xwm.data_bridge
                    .selection_wire
                    .complete_proxy_mime_atom(proxy_id, ordinal, atom);
            }
        }
    }
    schedule_selection_resolution_queries(xwm)?;
    Ok(SelectionReplyDrain {
        processed,
        budget_exhausted,
        quiescent: !budget_exhausted && xwm.data_bridge.selection_wire.pending.is_empty(),
    })
}

fn probe_revision_is_current(
    xwm: &Xwm,
    pending: PendingSelectionReply,
    revision_at_issue: Option<SelectionRevision>,
) -> bool {
    pending.generation == BridgeGeneration::from(xwm.generation)
        && xwm
            .data_bridge
            .selections
            .current(pending.selection)
            .is_some_and(|state| {
                state.generation == pending.generation && state.revision == revision_at_issue
            })
}

fn apply_owner_transition(
    xwm: &mut Xwm,
    generation: BridgeGeneration,
    kind: SelectionKind,
    owner: Option<Window>,
    timestamp: u32,
) -> Result<(), XwmError> {
    let prior = xwm.data_bridge.selections.current(kind).cloned();
    let origin = owner.map(|owner| {
        if is_internal_window(
            owner,
            Some(xwm.supporting_wm_check),
            Some(&xwm.data_bridge.selection_wire),
        ) {
            SelectionOrigin::Wayland
        } else {
            SelectionOrigin::X11
        }
    });
    let Some(_revision) = xwm
        .data_bridge
        .selections
        .observe_owner(generation, kind, owner, origin, timestamp)
    else {
        return Ok(());
    };
    xwm.data_bridge
        .selection_wire
        .invalidate_selection(kind, xwm.generation);
    cancel_channel_replies(xwm, generation, kind);
    let current_requestor = xwm.data_bridge.selection_wire.requestor(kind);
    let owner_is_requestor = owner.is_some() && owner == current_requestor;
    let plan = requestor_plan(
        xwm.data_bridge.selection_wire.windows.get(&kind).copied(),
        prior.as_ref(),
        owner_is_requestor,
    );
    let requestor_ready = match plan {
        RequestorPlan::Reuse => true,
        RequestorPlan::Rotate => {
            let replaced = replace_requestor(xwm, kind)?;
            if !replaced {
                xwm.data_bridge.selection_wire.poison_requestor(kind);
            }
            replaced
        }
        RequestorPlan::Blocked => {
            xwm.data_bridge.selection_wire.poison_requestor(kind);
            false
        }
    };
    let Some(_owner) = owner else {
        return Ok(());
    };
    let Some(identity) = xwm
        .data_bridge
        .selections
        .current(kind)
        .and_then(|state| state.identity())
    else {
        return Ok(());
    };
    if origin == Some(SelectionOrigin::Wayland) {
        xwm.data_bridge
            .selections
            .mark_discovery_state(identity, TargetsDiscoveryState::Inactive);
        return Ok(());
    }
    if !requestor_ready {
        xwm.data_bridge
            .selections
            .mark_discovery_state(identity, TargetsDiscoveryState::Failed);
        return Ok(());
    }
    start_targets_conversion(xwm, identity)
}

fn mark_failed_if_current(xwm: &mut Xwm, identity: SelectionIdentity) {
    xwm.data_bridge
        .selections
        .mark_discovery_state(identity, TargetsDiscoveryState::Failed);
}

fn parse_targets_property(reply: &xproto::GetPropertyReply) -> Option<Vec<Atom>> {
    if reply.type_ != u32::from(xproto::AtomEnum::ATOM)
        || reply.format != 32
        || reply.bytes_after != 0
        || !reply.value.len().is_multiple_of(4)
        || reply.value.len() / 4 > super::data_bridge::selection::MAX_SELECTION_TARGETS
        || reply.value_len as usize != reply.value.len() / 4
    {
        return None;
    }
    Some(
        reply
            .value
            .chunks_exact(4)
            .map(|value| u32::from_ne_bytes(value.try_into().expect("four bytes")))
            .collect(),
    )
}

#[cfg(test)]
pub(crate) fn pending_sequence_for_test(
    xwm: &Xwm,
    kind: SelectionKind,
    targets_property: bool,
) -> Option<SequenceNumber> {
    xwm.data_bridge
        .selection_wire
        .pending_sequence_for_test(kind, targets_property)
}

#[cfg(test)]
pub(crate) fn pending_target_atom_name_sequence_for_test(
    xwm: &Xwm,
    kind: SelectionKind,
    target: Atom,
) -> Option<SequenceNumber> {
    xwm.data_bridge
        .selection_wire
        .pending_target_atom_name_sequence_for_test(kind, target)
}

#[cfg(test)]
pub(crate) fn target_for_mime_for_test(
    xwm: &Xwm,
    kind: SelectionKind,
    id: XwaylandSelectionOfferId,
    mime_type: &str,
) -> Option<Atom> {
    xwm.data_bridge
        .selection_wire
        .target_for_mime_for_test(kind, id, mime_type)
}

#[cfg(test)]
pub(crate) fn proxy_mime_atom_sequence_for_test(
    xwm: &Xwm,
    proxy_id: XwaylandProxySelectionId,
    mime_type: &str,
) -> Option<SequenceNumber> {
    xwm.data_bridge
        .selection_wire
        .proxy_mime_atom_sequence_for_test(proxy_id, mime_type)
}

#[cfg(test)]
pub(crate) fn proxy_mime_for_target_for_test(xwm: &Xwm, target: Atom) -> Option<String> {
    xwm.data_bridge
        .selection_wire
        .prepared_proxy_selections
        .values()
        .find_map(|prepared| {
            prepared
                .data_targets
                .iter()
                .find(|binding| binding.target == target)
                .map(|binding| binding.mime_type.clone())
        })
}

#[cfg(test)]
pub(crate) fn prepared_proxy_selection_for_test(
    xwm: &Xwm,
    kind: SelectionKind,
) -> Option<PreparedProxySelection> {
    xwm.data_bridge
        .selection_wire
        .prepared_proxy_selection_for_test(kind)
}

#[cfg(test)]
pub(crate) fn pending_proxy_replies_for_test(xwm: &Xwm) -> usize {
    xwm.data_bridge
        .selection_wire
        .pending_proxy_replies_for_test()
}

#[cfg(test)]
pub(crate) fn pending_selection_replies_for_test(xwm: &Xwm) -> usize {
    xwm.data_bridge
        .selection_wire
        .pending_selection_replies_for_test()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn property_reply(
        type_atom: Atom,
        format: u8,
        bytes_after: u32,
        targets: &[Atom],
    ) -> xproto::GetPropertyReply {
        let mut value = Vec::with_capacity(targets.len() * 4);
        for target in targets {
            value.extend_from_slice(&target.to_ne_bytes());
        }
        xproto::GetPropertyReply {
            format,
            sequence: 0,
            length: value.len() as u32 / 4,
            type_: type_atom,
            bytes_after,
            value_len: targets.len() as u32,
            value,
        }
    }

    #[test]
    fn targets_property_requires_atom32() {
        assert!(parse_targets_property(&property_reply(4, 32, 0, &[1, 2])).is_some());
        assert!(parse_targets_property(&property_reply(5, 32, 0, &[1, 2])).is_none());
        assert!(parse_targets_property(&property_reply(4, 8, 0, &[1, 2])).is_none());
    }

    #[test]
    fn targets_property_is_bounded_and_complete() {
        let oversized = vec![1; super::super::data_bridge::selection::MAX_SELECTION_TARGETS + 1];
        assert!(parse_targets_property(&property_reply(4, 32, 0, &oversized)).is_none());
        assert!(parse_targets_property(&property_reply(4, 32, 4, &[1])).is_none());
    }
}
