//! Cross-layer XDND identity and semantic metadata.
//!
//! These values describe an XDND session to the canonical compositor and
//! expose bounded transitions to the XWM adapter. They do not implement the
//! XDND ClientMessage protocol and do not share selection ownership state.

use super::{X11WindowHandle, XwaylandGeneration};
use std::{
    collections::{HashSet, VecDeque},
    num::{NonZeroU8, NonZeroU64},
    os::fd::OwnedFd,
};

pub const MAX_XWAYLAND_DND_MIME_TYPES: usize = 64;
pub const MAX_XWAYLAND_DND_MIME_TYPE_BYTES: usize = 255;
pub const MAX_XWAYLAND_DND_ACTIONS: usize = 5;
pub const MAX_PENDING_XWAYLAND_DND_TRANSITIONS: usize = 64;
pub const MAX_PENDING_XWAYLAND_DND_INCOMING_EVENTS: usize = 32;
pub const MAX_XWAYLAND_DND_INCOMING_TRANSFERS: usize = 16;
pub const MAX_XWAYLAND_DND_INCOMING_CHUNK_BYTES: usize = 64 * 1024;
pub const XWAYLAND_DND_INCOMING_IDLE_TIMEOUT_NS: u64 = 30_000_000_000;

/// Incoming root-bridge sources must support the XdndProxy extension (v4).
/// The general version type remains unchanged for the existing C2 path.
pub const fn negotiate_incoming_root_version(source_version: u32) -> Option<XwaylandDndVersion> {
    if source_version < 4 {
        return None;
    }
    let version = if source_version >= 5 {
        5
    } else {
        source_version as u8
    };
    XwaylandDndVersion::new(version)
}

/// Decode XDND's packed pair of signed 16-bit root coordinates. The current
/// single-output compositor maps root coordinates 1:1; a future output layout
/// transform belongs at the runtime/compositor boundary.
pub const fn unpack_root_coordinates(packed: u32) -> (f64, f64) {
    let x = (packed >> 16) as u16 as i16;
    let y = packed as u16 as i16;
    (x as f64, y as f64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

/// Internal identity of one semantic incoming Position. Its serial is scoped
/// by the offer and is intentionally independent of the X selection timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct XwaylandDndIncomingPositionId {
    offer_id: XwaylandDndOfferId,
    serial: NonZeroU64,
}

impl XwaylandDndIncomingPositionId {
    pub const fn new(offer_id: XwaylandDndOfferId, serial: NonZeroU64) -> Self {
        Self { offer_id, serial }
    }

    pub const fn offer_id(self) -> XwaylandDndOfferId {
        self.offer_id
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
    pub const MIN_SUPPORTED_TARGET_VERSION: u8 = 3;
    pub const CURRENT_VERSION: u8 = 5;

    pub const fn new(version: u8) -> Option<Self> {
        if version < Self::MIN_SUPPORTED_TARGET_VERSION || version > Self::CURRENT_VERSION {
            return None;
        }
        Some(Self(
            NonZeroU8::new(version).expect("supported XDND version is nonzero"),
        ))
    }

    pub const fn get(self) -> u8 {
        self.0.get()
    }

    /// Negotiate one target's advertised highest version with Typhon's live
    /// XDND wire version. Targets below version 3 are outside the supported
    /// bridge range; future versions are capped at the version we speak.
    pub const fn negotiate_target(target_version: u32) -> Option<Self> {
        if target_version < Self::MIN_SUPPORTED_TARGET_VERSION as u32 {
            return None;
        }
        let negotiated = if target_version < Self::CURRENT_VERSION as u32 {
            target_version as u8
        } else {
            Self::CURRENT_VERSION
        };
        match NonZeroU8::new(negotiated) {
            Some(version) => Some(Self(version)),
            None => None,
        }
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

    pub fn replace_source_actions(
        &mut self,
        source_actions: Vec<XwaylandDndAction>,
    ) -> Result<(), XwaylandDndMetadataError> {
        validate_source_actions(&source_actions)?;
        self.source_actions = source_actions;
        Ok(())
    }

    pub fn wayland_source_actions_mask(&self) -> u32 {
        self.source_actions
            .iter()
            .filter_map(|action| action.to_wayland_action())
            .fold(0, |mask, action| mask | action.mask())
    }
}

fn validate_source_actions(
    source_actions: &[XwaylandDndAction],
) -> Result<(), XwaylandDndMetadataError> {
    if source_actions.len() > MAX_XWAYLAND_DND_ACTIONS {
        return Err(XwaylandDndMetadataError::TooManyActions);
    }
    let mut seen = HashSet::with_capacity(source_actions.len());
    if source_actions.iter().any(|action| !seen.insert(*action)) {
        return Err(XwaylandDndMetadataError::DuplicateAction);
    }
    Ok(())
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

/// Generation-qualified identity for one incoming XdndSelection payload read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct XwaylandDndIncomingTransferId {
    offer_id: XwaylandDndOfferId,
    serial: NonZeroU64,
}

impl XwaylandDndIncomingTransferId {
    pub const fn new(offer_id: XwaylandDndOfferId, serial: NonZeroU64) -> Self {
        Self { offer_id, serial }
    }

    pub const fn offer_id(self) -> XwaylandDndOfferId {
        self.offer_id
    }

    pub const fn serial(self) -> u64 {
        self.serial.get()
    }
}

/// Direction-specific semantic events from the XWM's root target proxy.
/// They contain no raw X11 protocol values except the exact source timestamp.
#[derive(Debug, Clone, PartialEq)]
pub enum XwaylandDndIncomingEvent {
    Begin {
        offer: XwaylandDndOffer,
        position_id: XwaylandDndIncomingPositionId,
        x: f64,
        y: f64,
        requested_action: XwaylandDndAction,
        x_timestamp: u32,
    },
    Position {
        offer_id: XwaylandDndOfferId,
        position_id: XwaylandDndIncomingPositionId,
        x: f64,
        y: f64,
        requested_action: XwaylandDndAction,
        source_actions: Vec<XwaylandDndAction>,
        x_timestamp: u32,
    },
    Leave {
        offer_id: XwaylandDndOfferId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct XwaylandDndSourceProxyId {
    pub adapter_id: XwaylandDndAdapterId,
    pub xid: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct XwaylandDndSourceTransferId {
    pub source: XwaylandDndSourceProxyId,
    pub serial: NonZeroU64,
}

/// Ordered semantic feedback from the bounded XWM wire adapter. Status may
/// coalesce before a terminal edge; terminal authority is always preserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XwaylandDndFeedback {
    Status {
        session_id: CanonicalDndSessionId,
        target: X11WindowHandle,
        accepted: bool,
        action: Option<XwaylandDndAction>,
    },
    Terminal {
        session_id: CanonicalDndSessionId,
        target: X11WindowHandle,
        accepted: bool,
        action: Option<XwaylandDndAction>,
    },
}

/// Move-only Wayland source read requested by the exact active XDND session.
#[derive(Debug)]
pub struct XwaylandDndSourceDataRequest {
    pub transfer_id: XwaylandDndSourceTransferId,
    pub target: X11WindowHandle,
    pub requestor: u32,
    pub mime_type: String,
    pub sink: OwnedFd,
}

/// Canonical DND snapshots consumed by the active XWM adapter. The compositor
/// owns the sole active drag; this outbox carries bounded transitions for that
/// drag. Each target snapshot carries enough source metadata to reconcile
/// against the adapter's prior wire target if an intermediate position was
/// replaced.
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
        /// Source/user action requested in the outgoing XdndPosition.
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
        /// Target-accepted action frozen by physical release.
        action: XwaylandDndAction,
        mime_types: XwaylandDndMimeCatalog,
        source_actions: Vec<XwaylandDndAction>,
    },
    TargetFinished {
        session_id: CanonicalDndSessionId,
        target: X11WindowHandle,
        accepted: bool,
        /// Concrete action performed at terminal completion; absent on reject.
        action: Option<XwaylandDndAction>,
    },
    SourceFeedback {
        offer_id: XwaylandDndOfferId,
        position_id: XwaylandDndIncomingPositionId,
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

impl XwaylandDndTransition {
    pub const fn canonical_session_id(&self) -> CanonicalDndSessionId {
        match self {
            Self::TargetEntered { session_id, .. }
            | Self::TargetPositioned { session_id, .. }
            | Self::TargetLeft { session_id, .. }
            | Self::DropRequested { session_id, .. }
            | Self::TargetFinished { session_id, .. }
            | Self::Retired { session_id, .. } => *session_id,
            Self::SourceFeedback { offer_id, .. } | Self::SourceFinished { offer_id, .. } => {
                CanonicalDndSessionId::Xwayland(*offer_id)
            }
        }
    }

    pub const fn generation(&self) -> XwaylandGeneration {
        match self {
            Self::TargetEntered { target, .. }
            | Self::TargetPositioned { target, .. }
            | Self::TargetLeft { target, .. }
            | Self::DropRequested { target, .. }
            | Self::TargetFinished { target, .. } => target.generation(),
            Self::SourceFeedback { offer_id, .. } | Self::SourceFinished { offer_id, .. } => {
                offer_id.generation()
            }
            Self::Retired { generation, .. } => *generation,
        }
    }
}

/// Ordered, bounded semantic transitions from canonical compositor state to
/// the active XWM adapter. Only adjacent continuous updates with identical
/// session/target or offer identity may replace one another.
#[derive(Debug, Default)]
pub struct XwaylandDndOutbox {
    pending: VecDeque<XwaylandDndTransition>,
}

impl XwaylandDndOutbox {
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn push(&mut self, transition: XwaylandDndTransition) -> Result<(), XwaylandDndTransition> {
        let coalesces_with_tail = match (self.pending.back(), &transition) {
            (
                Some(XwaylandDndTransition::TargetPositioned {
                    session_id: old_session,
                    target: old_target,
                    ..
                }),
                XwaylandDndTransition::TargetPositioned {
                    session_id, target, ..
                },
            ) => old_session == session_id && old_target == target,
            (
                Some(XwaylandDndTransition::SourceFeedback {
                    offer_id: old_offer,
                    position_id: old_position,
                    ..
                }),
                XwaylandDndTransition::SourceFeedback {
                    offer_id,
                    position_id,
                    ..
                },
            ) => old_offer == offer_id && old_position == position_id,
            _ => false,
        };

        if coalesces_with_tail {
            *self
                .pending
                .back_mut()
                .expect("coalescing requires a pending tail") = transition;
            return Ok(());
        }
        if self.pending.len() >= MAX_PENDING_XWAYLAND_DND_TRANSITIONS {
            return Err(transition);
        }
        self.pending.push_back(transition);
        Ok(())
    }

    pub fn drain(&mut self) -> Vec<XwaylandDndTransition> {
        self.pending.drain(..).collect()
    }

    /// Drop only transitions whose exact source, target, or retirement
    /// identity belongs to the generation being torn down.
    pub fn clear_generation(&mut self, generation: XwaylandGeneration) {
        self.pending.retain(|transition| match transition {
            XwaylandDndTransition::TargetEntered { target, .. }
            | XwaylandDndTransition::TargetPositioned { target, .. }
            | XwaylandDndTransition::TargetLeft { target, .. }
            | XwaylandDndTransition::DropRequested { target, .. }
            | XwaylandDndTransition::TargetFinished { target, .. } => {
                target.generation() != generation
            }
            XwaylandDndTransition::SourceFeedback { offer_id, .. }
            | XwaylandDndTransition::SourceFinished { offer_id, .. } => {
                offer_id.generation() != generation
            }
            XwaylandDndTransition::Retired {
                generation: retired_generation,
                ..
            } => *retired_generation != generation,
        });
    }

    /// Replace unusable pending adapter history with one bounded recovery
    /// transition for the exact canonical session and generation.
    pub fn replace_with_retired(
        &mut self,
        session_id: CanonicalDndSessionId,
        generation: XwaylandGeneration,
    ) {
        self.pending.clear();
        self.pending.push_back(XwaylandDndTransition::Retired {
            session_id,
            generation,
        });
    }
}

#[cfg(test)]
mod xdnd_version_tests {
    use super::{XwaylandDndVersion, negotiate_incoming_root_version, unpack_root_coordinates};

    #[test]
    fn target_version_negotiation_rejects_old_and_caps_future_versions() {
        assert_eq!(XwaylandDndVersion::negotiate_target(2), None);
        assert_eq!(
            XwaylandDndVersion::negotiate_target(3).map(|v| v.get()),
            Some(3)
        );
        assert_eq!(
            XwaylandDndVersion::negotiate_target(4).map(|v| v.get()),
            Some(4)
        );
        assert_eq!(
            XwaylandDndVersion::negotiate_target(5).map(|v| v.get()),
            Some(5)
        );
        assert_eq!(
            XwaylandDndVersion::negotiate_target(6).map(|v| v.get()),
            Some(5)
        );
        assert_eq!(
            XwaylandDndVersion::negotiate_target(u32::MAX).map(|v| v.get()),
            Some(5)
        );
        assert!(XwaylandDndVersion::new(2).is_none());
        assert_eq!(XwaylandDndVersion::new(3).map(|v| v.get()), Some(3));
        assert_eq!(XwaylandDndVersion::new(5).map(|v| v.get()), Some(5));
        assert!(XwaylandDndVersion::new(6).is_none());
    }

    #[test]
    fn incoming_root_bridge_requires_v4_and_caps_at_v5() {
        assert_eq!(negotiate_incoming_root_version(3), None);
        assert_eq!(
            negotiate_incoming_root_version(4).map(|version| version.get()),
            Some(4)
        );
        assert_eq!(
            negotiate_incoming_root_version(5).map(|version| version.get()),
            Some(5)
        );
        assert_eq!(
            negotiate_incoming_root_version(12).map(|version| version.get()),
            Some(5)
        );
    }

    #[test]
    fn incoming_root_coordinates_preserve_signed_i16_halves() {
        let packed = (u32::from(i16::MIN as u16) << 16) | u32::from((-1_i16) as u16);
        assert_eq!(unpack_root_coordinates(packed), (-32768.0, -1.0));
    }
}
