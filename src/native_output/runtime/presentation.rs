//! Presentation-cycle module boundary.
//!
//! The implementation lives in the sibling `presentation_cycle` module so
//! the runtime's scheduling loop remains below the source-layout limit.

#[cfg(test)]
use super::*;

#[cfg(test)]
pub(super) use super::presentation_transactions::{
    complete_immediate_output_transaction, complete_immediate_output_transaction_with,
    present_compatibility_frame,
};

#[cfg(test)]
mod pacing_mode_tests;
