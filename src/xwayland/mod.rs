use std::{num::NonZeroU64, path::PathBuf};

mod association;
mod auth;
mod config;
mod display;
mod generation;
mod launch;
mod metrics;
mod protocol;
mod readiness;
mod service;
pub mod xwm;
pub(crate) use protocol::{XWAYLAND_SHELL_V1_VERSION, serial_from_parts};

#[cfg(test)]
mod tests;

pub use association::{
    AssociationError, AssociationRegistry, SurfaceAssociation, SurfaceId, XwaylandAssociationEvent,
};
pub use config::{XwaylandConfig, XwaylandMode, XwaylandProfile, XwaylandStartPolicy};
pub use generation::XwaylandGeneration;
pub use readiness::XwaylandReadinessSnapshot;
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
    pub(crate) const fn new(generation: XwaylandGeneration, xid: u32) -> Self {
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
