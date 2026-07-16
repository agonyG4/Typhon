use std::{num::NonZeroU64, path::PathBuf};

mod association;
mod auth;
mod config;
mod display;
mod generation;
mod launch;
mod metrics;
mod protocol;
mod service;
pub(crate) use protocol::{XWAYLAND_SHELL_V1_VERSION, serial_from_parts};

#[cfg(test)]
mod tests;

pub use association::{
    AssociationError, AssociationRegistry, SurfaceAssociation, SurfaceId, XwaylandAssociationEvent,
};
pub use config::{XwaylandConfig, XwaylandMode};
pub use generation::XwaylandGeneration;
pub use service::{
    XwaylandReactorPurpose, XwaylandReactorRegistration, XwaylandService, XwaylandStateKind,
};

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
