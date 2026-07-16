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

#[cfg(test)]
mod tests;

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

pub(crate) fn next_nonzero(value: &mut NonZeroU64) -> XwaylandGeneration {
    let generation = XwaylandGeneration::new(*value);
    *value = NonZeroU64::new(value.get().saturating_add(1)).unwrap_or(NonZeroU64::MAX);
    generation
}
