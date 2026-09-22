//! Metadata-only XWayland selection handoff.
//!
//! These types identify resolved external offers without carrying any X11
//! handles or payload-transfer ownership. F11-B2 may connect this contract to
//! the compositor only after it can serve selection bytes.

use super::XwaylandGeneration;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XwaylandSelectionOffer {
    pub id: XwaylandSelectionOfferId,
    pub mime_types: Vec<String>,
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
