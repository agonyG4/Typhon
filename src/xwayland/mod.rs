use std::{num::NonZeroU64, path::PathBuf};

mod association;
mod auth;
mod config;
mod diagnostics;
mod display;
mod displayfd;
pub mod dnd_metadata;
mod fs_security;
mod generation;
mod launch;
mod metrics;
mod protocol;
mod readiness;
pub mod selection_metadata;
mod service;
pub mod trace;
pub mod xwm;
pub(crate) use protocol::{XWAYLAND_SHELL_V1_VERSION, serial_from_parts};

#[cfg(test)]
mod tests;

pub use association::{
    AssociationError, AssociationRegistry, SurfaceAssociation, SurfaceId, XwaylandAssociationEvent,
};
pub use config::{XwaylandConfig, XwaylandMode, XwaylandProfile, XwaylandStartPolicy};
pub use dnd_metadata::{
    CanonicalDndSessionId, MAX_PENDING_XWAYLAND_DND_INCOMING_EVENTS,
    MAX_PENDING_XWAYLAND_DND_TRANSITIONS, MAX_XWAYLAND_DND_ACTIONS,
    MAX_XWAYLAND_DND_INCOMING_CHUNK_BYTES, MAX_XWAYLAND_DND_INCOMING_TRANSFERS,
    MAX_XWAYLAND_DND_MIME_TYPE_BYTES, MAX_XWAYLAND_DND_MIME_TYPES, WaylandDndAction,
    XWAYLAND_DND_INCOMING_IDLE_TIMEOUT_NS, XwaylandDndAction, XwaylandDndAdapterId,
    XwaylandDndDataRequest, XwaylandDndFeedback, XwaylandDndIncomingEvent,
    XwaylandDndIncomingPositionId, XwaylandDndIncomingTransferId, XwaylandDndMetadataError,
    XwaylandDndMimeCatalog, XwaylandDndOffer, XwaylandDndOfferId, XwaylandDndOutbox,
    XwaylandDndSourceDataRequest, XwaylandDndSourceProxyId, XwaylandDndSourceTransferId,
    XwaylandDndTransition, XwaylandDndVersion, negotiate_incoming_root_version,
    unpack_root_coordinates,
};
pub use generation::XwaylandGeneration;
pub use readiness::XwaylandReadinessSnapshot;
pub use selection_metadata::{
    XwaylandProxySelectionDataRequest, XwaylandProxySelectionId, XwaylandProxySelectionOffer,
    XwaylandProxySelectionSnapshot, XwaylandProxySelectionTransferId, XwaylandSelectionDataRequest,
    XwaylandSelectionEvent, XwaylandSelectionKind, XwaylandSelectionOffer,
    XwaylandSelectionOfferId,
};
pub use service::{
    XwaylandReactorPurpose, XwaylandReactorRegistration, XwaylandService, XwaylandStateKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct X11WindowHandle {
    pub(crate) generation: XwaylandGeneration,
    pub(crate) xid: u32,
}

impl X11WindowHandle {
    #[allow(dead_code)]
    pub const fn new(generation: XwaylandGeneration, xid: u32) -> Self {
        Self { generation, xid }
    }

    pub const fn generation(self) -> XwaylandGeneration {
        self.generation
    }

    pub const fn xid(self) -> u32 {
        self.xid
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XwaylandAppEnvironment {
    pub display: String,
    pub xauthority: PathBuf,
}

pub(crate) fn next_nonzero(value: &mut NonZeroU64) -> Option<XwaylandGeneration> {
    let generation = XwaylandGeneration::new(*value);
    *value = value.get().checked_add(1).and_then(NonZeroU64::new)?;
    Some(generation)
}
