#![allow(dead_code)]

mod bundle;
mod cursor_sidecar;
mod metrics;
mod payload;
mod policy;
mod queue;
mod thread;
mod timing;

pub(crate) use metrics::{
    SignedTimingSummarySnapshot, TimingSummarySnapshot, WorkerTimingMetrics, WorkerTimingSnapshot,
};
#[cfg(test)]
pub(crate) use payload::KmsCommitPayloadError;
pub(crate) use payload::{
    EstablishedKmsBase, KmsCommitJob, KmsCommitTestPolicy, KmsCursorUpdate,
    KmsPrimaryCursorPresentation, KmsPrimaryUpdate, KmsSubmittedOwnership, KmsTestOnlyPolicy,
    KmsValidationBase, ValidationBaseDisposition, validation_base_ready,
};
pub(crate) use policy::{
    KmsCommitWorkerPolicy, KmsCommitWorkerStartup, KmsCommitWorkerStartupError,
    KmsCommitWorkerTransport, kms_worker_doctor_severity,
};
#[cfg(test)]
pub(crate) use queue::KmsWorkerForcedShutdownDisposition;
pub(crate) use queue::{
    AttachablePrimary, CursorSidecarOfferError, KmsCommitAdmissionPermit, KmsWorkerAdmissionError,
    KmsWorkerFatalJob, PendingBundleSnapshot, WorkerInFlight, WorkerMetricsSnapshot,
};
#[cfg(test)]
pub(crate) use thread::{KmsCommitExecutor, KmsWorkerSubmission, KmsWorkerSubmitFailure};
pub(crate) use thread::{KmsCommitWorkerHandle, KmsWorkerEvent, ValidationBaseInvalidationReason};
pub(crate) use timing::{KmsWorkerDispatchBudget, KmsWorkerDispatchModel};

#[cfg(test)]
mod direct_lease_tests;
#[cfg(test)]
mod signal_tests;
#[cfg(test)]
mod task4_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod timing_tests;
pub(crate) use bundle::KmsCursorOwner;
#[cfg(test)]
pub(crate) use bundle::KmsPrimaryOwner;
pub(crate) use bundle::{KmsBundleOwners, KmsCommitBundleIdentity};
pub(crate) use cursor_sidecar::CursorSidecarCoupling;
pub(crate) use cursor_sidecar::{CursorSidecar, CursorSidecarMailbox, CursorSidecarReturnReason};
