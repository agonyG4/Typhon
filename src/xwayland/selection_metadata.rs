//! XWayland selection contracts.
//!
//! Imported X11 offers and their move-only payload requests support the B2
//! direction. The B3-A proxy snapshot types below carry only canonical source
//! identity and MIME metadata; they do not claim X11 ownership or carry bytes.

use super::XwaylandGeneration;
use crate::compositor::SelectionSourceKey;
use std::os::fd::OwnedFd;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum XwaylandSelectionKind {
    Clipboard,
    Primary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct XwaylandSelectionOfferId {
    pub generation: XwaylandGeneration,
    pub kind: XwaylandSelectionKind,
    pub revision: u64,
}

/// Exact canonical compositor source identity for a future X11 proxy owner.
///
/// B3-B will use this identity to revalidate the channel generation and
/// source key before asking the compositor for any payload bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct XwaylandProxySelectionId {
    pub kind: XwaylandSelectionKind,
    pub selection_generation: u64,
    pub source_key: SelectionSourceKey,
}

/// Bounded, metadata-only view of one canonical non-XWayland source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XwaylandProxySelectionOffer {
    pub id: XwaylandProxySelectionId,
    pub mime_types: Vec<String>,
}

/// Latest canonical compositor selection metadata for one X11 selection
/// channel. `offer: None` means the channel must not represent a Wayland
/// source; it is also how XWayland-origin selections avoid reflection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XwaylandProxySelectionSnapshot {
    pub kind: XwaylandSelectionKind,
    pub selection_generation: u64,
    pub offer: Option<XwaylandProxySelectionOffer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XwaylandSelectionOffer {
    pub id: XwaylandSelectionOfferId,
    pub mime_types: Vec<String>,
}

/// Move-only X11 selection payload request handed from the compositor to the
/// XWayland service.  The descriptor is the Wayland client's receiving end.
#[derive(Debug)]
pub struct XwaylandSelectionDataRequest {
    pub offer_id: XwaylandSelectionOfferId,
    pub mime_type: String,
    pub sink: OwnedFd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XwaylandSelectionEvent {
    OfferChanged {
        kind: XwaylandSelectionKind,
        offer: XwaylandSelectionOffer,
    },
    Cleared {
        kind: XwaylandSelectionKind,
        generation: XwaylandGeneration,
    },
}

impl XwaylandSelectionEvent {
    pub(crate) const fn kind(&self) -> XwaylandSelectionKind {
        match self {
            Self::OfferChanged { kind, .. } | Self::Cleared { kind, .. } => *kind,
        }
    }
}
