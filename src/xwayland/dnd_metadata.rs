//! Cross-layer XDND identity and semantic metadata.
//!
//! These values describe an XDND session to the canonical compositor and
//! expose bounded transitions to the XWM adapter. They do not implement the
//! XDND ClientMessage protocol and do not share selection ownership state.

use super::{X11WindowHandle, XwaylandGeneration};
use std::{
    collections::HashSet,
    num::{NonZeroU8, NonZeroU64},
    os::fd::OwnedFd,
};

pub const MAX_XWAYLAND_DND_MIME_TYPES: usize = 64;
pub const MAX_XWAYLAND_DND_MIME_TYPE_BYTES: usize = 255;
pub const MAX_XWAYLAND_DND_ACTIONS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct XwaylandDndOfferId {
    generation: XwaylandGeneration,
    serial: NonZeroU64,
}

impl XwaylandDndOfferId {
    pub const fn new(generation: XwaylandGeneration, serial: NonZeroU64) -> Self {
        Self { generation, serial }
    }

    pub const fn generation(self) -> XwaylandGeneration {
        self.generation
    }

    pub const fn serial(self) -> u64 {
        self.serial.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CanonicalDndSessionId {
    Wayland(NonZeroU64),
    Xwayland(XwaylandDndOfferId),
}

/// Identity of one XWM adapter view of a canonical drag. A native Wayland
/// session gains its XWayland generation from the exact X11 target; an
/// XWayland-origin session must carry the same generation as its offer ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct XwaylandDndAdapterId {
    session_id: CanonicalDndSessionId,
    generation: XwaylandGeneration,
}

impl XwaylandDndAdapterId {
    pub const fn new(
        session_id: CanonicalDndSessionId,
        generation: XwaylandGeneration,
    ) -> Option<Self> {
        match session_id {
            CanonicalDndSessionId::Wayland(_) => Some(Self {
                session_id,
                generation,
            }),
            CanonicalDndSessionId::Xwayland(offer_id)
                if offer_id.generation().get() == generation.get() =>
            {
                Some(Self {
                    session_id,
                    generation,
                })
            }
            CanonicalDndSessionId::Xwayland(_) => None,
        }
    }

    pub const fn session_id(self) -> CanonicalDndSessionId {
        self.session_id
    }

    pub const fn generation(self) -> XwaylandGeneration {
        self.generation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct XwaylandDndVersion(NonZeroU8);

impl XwaylandDndVersion {
    pub const fn new(version: u8) -> Option<Self> {
        match NonZeroU8::new(version) {
            Some(version) => Some(Self(version)),
            None => None,
        }
    }

    pub const fn get(self) -> u8 {
        self.0.get()
    }
}

/// Semantic XDND actions. These are intentionally not bit flags and do not
/// share numeric values with the core Wayland DND action mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum XwaylandDndAction {
    Copy,
    Move,
    Link,
    Ask,
    Private,
}

impl XwaylandDndAction {
    pub const fn to_wayland_action(self) -> Option<WaylandDndAction> {
        match self {
            Self::Copy => Some(WaylandDndAction::Copy),
            Self::Move => Some(WaylandDndAction::Move),
            Self::Ask => Some(WaylandDndAction::Ask),
            Self::Link | Self::Private => None,
        }
    }
}

/// Semantic core Wayland DND actions and their protocol mask representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WaylandDndAction {
    Copy,
    Move,
    Ask,
}

impl WaylandDndAction {
    pub const fn mask(self) -> u32 {
        match self {
            Self::Copy => 1,
            Self::Move => 2,
            Self::Ask => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct XwaylandDndMimeCatalog(Vec<String>);

impl XwaylandDndMimeCatalog {
    pub const MAX_MIME_TYPES: usize = MAX_XWAYLAND_DND_MIME_TYPES;

    pub fn try_new(mime_types: Vec<String>) -> Result<Self, XwaylandDndMetadataError> {
        if mime_types.len() > MAX_XWAYLAND_DND_MIME_TYPES {
            return Err(XwaylandDndMetadataError::TooManyMimeTypes);
        }
        let mut seen = HashSet::with_capacity(mime_types.len());
        for mime_type in &mime_types {
            if mime_type.is_empty()
                || mime_type.len() > MAX_XWAYLAND_DND_MIME_TYPE_BYTES
                || mime_type.as_bytes().contains(&0)
            {
                return Err(XwaylandDndMetadataError::InvalidMimeType);
            }
            if !seen.insert(mime_type.as_str()) {
                return Err(XwaylandDndMetadataError::DuplicateMimeType);
            }
        }
        Ok(Self(mime_types))
    }

    /// Build the largest valid prefix that fits the cross-layer contract.
    /// This adapts a larger Wayland catalog without changing the native source.
    pub fn bounded_from_iter(mime_types: impl IntoIterator<Item = String>) -> Self {
        let mut result = Vec::with_capacity(MAX_XWAYLAND_DND_MIME_TYPES);
        let mut seen = HashSet::with_capacity(MAX_XWAYLAND_DND_MIME_TYPES);
        for mime_type in mime_types {
            if mime_type.is_empty()
                || mime_type.len() > MAX_XWAYLAND_DND_MIME_TYPE_BYTES
                || mime_type.as_bytes().contains(&0)
                || !seen.insert(mime_type.clone())
            {
                continue;
            }
            result.push(mime_type);
            if result.len() == MAX_XWAYLAND_DND_MIME_TYPES {
                break;
            }
        }
        Self(result)
    }

    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XwaylandDndOffer {
    id: XwaylandDndOfferId,
    source: X11WindowHandle,
    version: XwaylandDndVersion,
    mime_types: XwaylandDndMimeCatalog,
    source_actions: Vec<XwaylandDndAction>,
}

impl XwaylandDndOffer {
    pub fn new(
        id: XwaylandDndOfferId,
        source: X11WindowHandle,
        version: XwaylandDndVersion,
        mime_types: XwaylandDndMimeCatalog,
        source_actions: Vec<XwaylandDndAction>,
    ) -> Result<Self, XwaylandDndMetadataError> {
        if id.generation() != source.generation() {
            return Err(XwaylandDndMetadataError::SourceGenerationMismatch);
        }
        if source_actions.len() > MAX_XWAYLAND_DND_ACTIONS {
            return Err(XwaylandDndMetadataError::TooManyActions);
        }
        let mut seen = HashSet::with_capacity(source_actions.len());
        if source_actions.iter().any(|action| !seen.insert(*action)) {
            return Err(XwaylandDndMetadataError::DuplicateAction);
        }
        Ok(Self {
            id,
            source,
            version,
            mime_types,
            source_actions,
        })
    }

    pub const fn id(&self) -> XwaylandDndOfferId {
        self.id
    }

    pub const fn source(&self) -> X11WindowHandle {
        self.source
    }

    pub const fn version(&self) -> XwaylandDndVersion {
        self.version
    }

    pub fn mime_types(&self) -> &XwaylandDndMimeCatalog {
        &self.mime_types
    }

    pub fn source_actions(&self) -> &[XwaylandDndAction] {
        &self.source_actions
    }

    pub fn wayland_source_actions_mask(&self) -> u32 {
        self.source_actions
            .iter()
            .filter_map(|action| action.to_wayland_action())
            .fold(0, |mask, action| mask | action.mask())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XwaylandDndMetadataError {
    TooManyMimeTypes,
    InvalidMimeType,
    DuplicateMimeType,
    SourceGenerationMismatch,
    TooManyActions,
    DuplicateAction,
}

/// Move-only request for the XWayland service to read one MIME payload from
/// the exact active XDND offer. The sink descriptor is transferred as owned.
#[derive(Debug)]
pub struct XwaylandDndDataRequest {
    pub offer_id: XwaylandDndOfferId,
    pub mime_type: String,
    pub sink: OwnedFd,
}

/// Canonical DND snapshots for a later XWM adapter. The compositor publishes
/// one latest transition at a time and never stores a second active drag here.
/// Each target snapshot carries enough source metadata to reconcile against
/// the adapter's previous wire target if an intermediate position was replaced.
#[derive(Debug, Clone, PartialEq)]
pub enum XwaylandDndTransition {
    TargetEntered {
        session_id: CanonicalDndSessionId,
        target: X11WindowHandle,
        x: f64,
        y: f64,
        mime_types: XwaylandDndMimeCatalog,
        source_actions: Vec<XwaylandDndAction>,
    },
    TargetPositioned {
        session_id: CanonicalDndSessionId,
        target: X11WindowHandle,
        x: f64,
        y: f64,
        accepted_mime: Option<String>,
        action: Option<XwaylandDndAction>,
        mime_types: XwaylandDndMimeCatalog,
        source_actions: Vec<XwaylandDndAction>,
    },
    TargetLeft {
        session_id: CanonicalDndSessionId,
        target: X11WindowHandle,
    },
    DropRequested {
        session_id: CanonicalDndSessionId,
        target: X11WindowHandle,
        mime_type: String,
        action: XwaylandDndAction,
        mime_types: XwaylandDndMimeCatalog,
        source_actions: Vec<XwaylandDndAction>,
    },
    TargetFinished {
        session_id: CanonicalDndSessionId,
        target: X11WindowHandle,
        accepted: bool,
        action: Option<XwaylandDndAction>,
    },
    SourceFeedback {
        offer_id: XwaylandDndOfferId,
        accepted_mime: Option<String>,
        action: Option<XwaylandDndAction>,
    },
    SourceFinished {
        offer_id: XwaylandDndOfferId,
        accepted: bool,
        action: Option<XwaylandDndAction>,
    },
    Retired {
        session_id: CanonicalDndSessionId,
        generation: XwaylandGeneration,
    },
}
