//! Native runtime event scheduling primitives.

pub mod drm;
pub mod event_loop;
#[doc(hidden)]
pub mod explicit_sync;
pub mod kms;
pub mod scheduler;
