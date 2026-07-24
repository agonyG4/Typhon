//! Startup policy for the optional submit-only Atomic KMS worker.

use oblivion_one::native::kms::KmsBackendKind;
use std::{error::Error, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsCommitWorkerPolicy {
    Off,
    Auto,
    Force,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KmsCommitWorkerPolicyError {
    InvalidValue(String),
}

impl fmt::Display for KmsCommitWorkerPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidValue(value) => write!(
                formatter,
                "invalid OBLIVION_ONE_KMS_COMMIT_WORKER={value:?}; expected off, auto, or force"
            ),
        }
    }
}

impl Error for KmsCommitWorkerPolicyError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsCommitWorkerTransport {
    Synchronous,
    Worker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsCommitWorkerStartupError {
    UnsupportedBackend,
    StartupFailed,
}

impl fmt::Display for KmsCommitWorkerStartupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedBackend => formatter.write_str("worker is unsupported for Legacy KMS"),
            Self::StartupFailed => formatter.write_str("worker startup failed"),
        }
    }
}

impl Error for KmsCommitWorkerStartupError {}

impl KmsCommitWorkerPolicy {
    pub(crate) fn parse(value: Option<&str>) -> Result<Self, KmsCommitWorkerPolicyError> {
        match value.unwrap_or("off") {
            "off" => Ok(Self::Off),
            "auto" => Ok(Self::Auto),
            "force" => Ok(Self::Force),
            value => Err(KmsCommitWorkerPolicyError::InvalidValue(value.to_string())),
        }
    }

    pub(crate) fn from_env_value(value: Option<&str>) -> Self {
        Self::parse(value).unwrap_or(Self::Off)
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::Force => "force",
        }
    }

    pub(crate) fn effective(
        self,
        backend: KmsBackendKind,
        startup_succeeded: bool,
    ) -> Result<KmsCommitWorkerTransport, KmsCommitWorkerStartupError> {
        match (self, backend) {
            (Self::Off, _) | (Self::Auto, KmsBackendKind::Legacy) => {
                Ok(KmsCommitWorkerTransport::Synchronous)
            }
            (Self::Force, KmsBackendKind::Legacy) => {
                Err(KmsCommitWorkerStartupError::UnsupportedBackend)
            }
            (Self::Auto, KmsBackendKind::Atomic) if startup_succeeded => {
                Ok(KmsCommitWorkerTransport::Worker)
            }
            (Self::Auto, KmsBackendKind::Atomic) => Ok(KmsCommitWorkerTransport::Synchronous),
            (Self::Force, KmsBackendKind::Atomic) if startup_succeeded => {
                Ok(KmsCommitWorkerTransport::Worker)
            }
            (Self::Force, KmsBackendKind::Atomic) => {
                Err(KmsCommitWorkerStartupError::StartupFailed)
            }
        }
    }
}
