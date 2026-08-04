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
pub(crate) enum KmsCommitWorkerStartup {
    IntentionallySynchronous,
    UnsupportedLegacyFallback,
    AutomaticStartupDegraded,
    WorkerStarted,
}

impl KmsCommitWorkerStartup {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::IntentionallySynchronous => "intentionally_synchronous",
            Self::UnsupportedLegacyFallback => "unsupported_legacy_fallback",
            Self::AutomaticStartupDegraded => "automatic_startup_degraded",
            Self::WorkerStarted => "worker_started",
        }
    }
}

impl KmsCommitWorkerTransport {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Synchronous => "synchronous",
            Self::Worker => "worker",
        }
    }
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

pub(crate) fn kms_worker_doctor_severity(
    policy: KmsCommitWorkerPolicy,
    transport: KmsCommitWorkerTransport,
    startup: KmsCommitWorkerStartup,
    worker_present: bool,
) -> oblivion_one::control_snapshots::DoctorSeverity {
    use oblivion_one::control_snapshots::DoctorSeverity;

    match policy {
        KmsCommitWorkerPolicy::Off => DoctorSeverity::Ok,
        KmsCommitWorkerPolicy::Auto => match (transport, worker_present, startup) {
            (KmsCommitWorkerTransport::Worker, true, _) => DoctorSeverity::Ok,
            (
                KmsCommitWorkerTransport::Synchronous,
                false,
                KmsCommitWorkerStartup::AutomaticStartupDegraded,
            ) => DoctorSeverity::Warning,
            (KmsCommitWorkerTransport::Synchronous, false, _) => DoctorSeverity::Ok,
            _ => DoctorSeverity::Error,
        },
        KmsCommitWorkerPolicy::Force => {
            if transport == KmsCommitWorkerTransport::Worker && worker_present {
                DoctorSeverity::Ok
            } else {
                DoctorSeverity::Error
            }
        }
    }
}

#[cfg(test)]
mod doctor_tests {
    use super::*;
    use oblivion_one::control_snapshots::DoctorSeverity;

    #[test]
    fn doctor_severity_matches_kms_worker_policy_matrix() {
        assert_eq!(
            kms_worker_doctor_severity(
                KmsCommitWorkerPolicy::Off,
                KmsCommitWorkerTransport::Synchronous,
                KmsCommitWorkerStartup::IntentionallySynchronous,
                false,
            ),
            DoctorSeverity::Ok
        );
        assert_eq!(
            kms_worker_doctor_severity(
                KmsCommitWorkerPolicy::Auto,
                KmsCommitWorkerTransport::Worker,
                KmsCommitWorkerStartup::WorkerStarted,
                true,
            ),
            DoctorSeverity::Ok
        );
        assert_eq!(
            kms_worker_doctor_severity(
                KmsCommitWorkerPolicy::Auto,
                KmsCommitWorkerTransport::Synchronous,
                KmsCommitWorkerStartup::AutomaticStartupDegraded,
                false,
            ),
            DoctorSeverity::Warning
        );
        assert_eq!(
            kms_worker_doctor_severity(
                KmsCommitWorkerPolicy::Auto,
                KmsCommitWorkerTransport::Synchronous,
                KmsCommitWorkerStartup::UnsupportedLegacyFallback,
                false,
            ),
            DoctorSeverity::Ok
        );
        assert_eq!(
            kms_worker_doctor_severity(
                KmsCommitWorkerPolicy::Force,
                KmsCommitWorkerTransport::Worker,
                KmsCommitWorkerStartup::WorkerStarted,
                true,
            ),
            DoctorSeverity::Ok
        );
        assert_eq!(
            kms_worker_doctor_severity(
                KmsCommitWorkerPolicy::Force,
                KmsCommitWorkerTransport::Synchronous,
                KmsCommitWorkerStartup::AutomaticStartupDegraded,
                false,
            ),
            DoctorSeverity::Error
        );
    }
}
