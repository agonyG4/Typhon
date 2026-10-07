//! Native runtime event scheduling primitives.

pub mod adaptive_buffering;
pub mod buffering;
pub mod control;
pub mod dmem_foreground;
pub mod drm;
pub mod event_loop;
#[doc(hidden)]
pub mod explicit_sync;
pub mod kms;
pub mod presentation_deadline;
pub mod presentation_timing;
pub mod scheduler;
#[doc(hidden)]
pub mod sync_file;
pub mod vrr_window;

#[cfg(test)]
mod control_tests;
