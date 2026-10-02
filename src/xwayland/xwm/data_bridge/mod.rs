//! X11 selection and Xdnd adapter state.
//!
//! The compositor's Wayland data-device state remains authoritative.  This
//! module stores only X11-side ownership, conversion, and transfer bookkeeping
//! and every entry is bound to the active XWayland generation.

pub mod dnd;
pub(crate) mod dnd_adapter;
pub(crate) mod dnd_incoming;
pub(crate) mod dnd_selection;
pub(crate) mod dnd_wire;
pub mod selection;
pub mod transfer;

use std::num::NonZeroU64;

use x11rb::connection::SequenceNumber;

use super::super::XwaylandGeneration;

pub(crate) fn is_internal_window(xwm: &super::Xwm, window: u32) -> bool {
    window == xwm.supporting_wm_check
        || xwm.data_bridge.selection_wire.is_internal_window(window)
        || super::selection_payload::owns_window(xwm, window)
        || dnd::is_internal_window(xwm, window)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectionKind {
    Clipboard,
    Primary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionOrigin {
    Wayland,
    X11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BridgeGeneration(NonZeroU64);

impl BridgeGeneration {
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }
}

impl From<XwaylandGeneration> for BridgeGeneration {
    fn from(value: XwaylandGeneration) -> Self {
        Self(NonZeroU64::new(value.get()).expect("XWayland generations are nonzero"))
    }
}

#[derive(Debug, Default)]
pub struct DataBridge {
    pub selections: selection::SelectionBridge,
    pub(crate) selection_wire: super::selection_wire::SelectionWireState,
    pub(crate) selection_payloads: super::selection_payload::SelectionPayloadManager,
    pub(crate) selection_proxy: super::selection_proxy::SelectionProxyManager,
    pub(crate) selection_outgoing: super::selection_outgoing::SelectionOutgoingManager,
    pub transfers: transfer::TransferManager,
    pub dnd: dnd::DndManager,
    pub(crate) dnd_incoming: dnd_incoming::DndIncomingManager,
    pub(crate) dnd_outgoing: super::dnd_outgoing::DndOutgoingManager,
}

impl DataBridge {
    pub fn clear_generation(&mut self, generation: XwaylandGeneration) -> Vec<SequenceNumber> {
        let bridge_generation = BridgeGeneration::from(generation);
        self.selections.clear_generation(bridge_generation);
        let mut pending_selection_replies = self.selection_wire.clear_generation(bridge_generation);
        pending_selection_replies
            .extend(self.selection_payloads.clear_generation(bridge_generation));
        pending_selection_replies.extend(self.selection_proxy.clear_generation(bridge_generation));
        pending_selection_replies
            .extend(self.selection_outgoing.clear_generation(bridge_generation));
        self.transfers.clear_generation(bridge_generation);
        self.dnd.clear_generation(generation);
        self.dnd_incoming.clear_generation(generation);
        self.dnd_outgoing.clear_generation(generation);
        pending_selection_replies
    }

    pub fn active_transfers(&self) -> usize {
        self.transfers.len()
    }
}
