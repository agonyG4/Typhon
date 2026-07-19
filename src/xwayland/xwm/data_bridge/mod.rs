//! X11 selection and Xdnd adapter state.
//!
//! The compositor's Wayland data-device state remains authoritative.  This
//! module stores only X11-side ownership, conversion, and transfer bookkeeping
//! and every entry is bound to the active XWayland generation.

pub mod dnd;
pub mod selection;
pub mod transfer;

use std::num::NonZeroU64;

use super::super::XwaylandGeneration;

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
    pub transfers: transfer::TransferManager,
    pub dnd: dnd::DndManager,
}

impl DataBridge {
    pub fn clear_generation(&mut self, generation: BridgeGeneration) {
        self.selections.clear_generation(generation);
        self.transfers.clear_generation(generation);
        self.dnd.clear_generation(generation);
    }

    pub fn active_transfers(&self) -> usize {
        self.transfers.len()
    }
}
