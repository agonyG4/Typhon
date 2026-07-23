mod atomic;
mod backend;
mod ioctl;
mod legacy;
mod properties;
mod state;
mod submission;

pub use atomic::*;
pub use backend::*;
pub use ioctl::*;
pub use legacy::*;
pub use properties::*;
pub use state::*;
#[allow(unused_imports)]
pub(crate) use submission::submit_atomic_flip_with;

use std::{error::Error, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KmsPolicy {
    Auto,
    Atomic,
    Legacy,
}

impl KmsPolicy {
    pub fn parse(value: Option<&str>) -> Result<Self, AtomicKmsError> {
        match value.unwrap_or("auto") {
            "auto" => Ok(Self::Auto),
            "atomic" => Ok(Self::Atomic),
            "legacy" => Ok(Self::Legacy),
            value => Err(AtomicKmsError::new(
                AtomicKmsErrorKind::InvalidPolicy,
                format!(
                    "invalid OBLIVION_ONE_KMS_MODE={value:?}; expected auto, atomic, or legacy"
                ),
            )),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Atomic => "atomic",
            Self::Legacy => "legacy",
        }
    }

    pub const fn on_atomic_failure(self, phase: AtomicFailurePhase) -> AtomicFailureAction {
        if matches!(self, Self::Auto)
            && matches!(
                phase,
                AtomicFailurePhase::Capability
                    | AtomicFailurePhase::Discovery
                    | AtomicFailurePhase::TestOnly
            )
        {
            AtomicFailureAction::UseLegacy
        } else {
            AtomicFailureAction::Fail
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KmsBackendKind {
    Atomic,
    Legacy,
}

impl KmsBackendKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Atomic => "atomic",
            Self::Legacy => "legacy",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicFailurePhase {
    Capability,
    Discovery,
    TestOnly,
    InitialCommit,
    Runtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicFailureAction {
    UseLegacy,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicKmsErrorKind {
    InvalidPolicy,
    Unsupported,
    MissingObject,
    MissingProperty,
    DuplicateProperty,
    MalformedPropertyBlob,
    NoCompatiblePrimaryPlane,
    InvalidGeometry,
    DuplicateAssignment,
    AlreadyPending,
    BlobCreation,
    TestOnlyRejected,
    InitialCommitRejected,
    FlipRejected,
    Busy,
    PermissionOrSession,
    DeviceLost,
    RestoreFailed,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicKmsError {
    pub kind: AtomicKmsErrorKind,
    pub detail: String,
}

impl AtomicKmsError {
    pub fn new(kind: AtomicKmsErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for AtomicKmsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.detail)
    }
}

impl Error for AtomicKmsError {}

#[cfg(test)]
mod tests;
