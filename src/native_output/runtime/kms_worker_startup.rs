use super::super::kms_worker::{
    KmsCommitWorkerHandle, KmsCommitWorkerPolicy, KmsCommitWorkerStartup,
    KmsCommitWorkerStartupError, KmsCommitWorkerTransport,
};
use oblivion_one::native::kms::{KmsBackendKind, KmsBackendSelection};
use std::{error::Error, io};

pub(super) fn start_kms_commit_worker(
    policy: KmsCommitWorkerPolicy,
    kms_backend: &KmsBackendSelection,
) -> Result<
    (
        Option<KmsCommitWorkerHandle>,
        KmsCommitWorkerTransport,
        KmsCommitWorkerStartup,
    ),
    Box<dyn Error>,
> {
    let mut worker = None;
    let outcome = match (policy, kms_backend.effective_kind()) {
        (KmsCommitWorkerPolicy::Off, _) => (
            KmsCommitWorkerTransport::Synchronous,
            KmsCommitWorkerStartup::IntentionallySynchronous,
        ),
        (KmsCommitWorkerPolicy::Force, KmsBackendKind::Legacy) => {
            return Err(io::Error::other(KmsCommitWorkerStartupError::UnsupportedBackend).into());
        }
        (_, KmsBackendKind::Legacy) => (
            KmsCommitWorkerTransport::Synchronous,
            KmsCommitWorkerStartup::UnsupportedLegacyFallback,
        ),
        (policy, KmsBackendKind::Atomic) => {
            let submitter = kms_backend
                .atomic()
                .expect("Atomic backend kind has an Atomic implementation")
                .commit_submitter();
            match KmsCommitWorkerHandle::start_atomic(submitter) {
                Ok(started) => {
                    worker = Some(started);
                    (
                        KmsCommitWorkerTransport::Worker,
                        KmsCommitWorkerStartup::WorkerStarted,
                    )
                }
                Err(error) if policy == KmsCommitWorkerPolicy::Auto => {
                    eprintln!(
                        "native KMS commit worker: requested={} startup failed ({error:?}); using synchronous transport",
                        policy.as_str()
                    );
                    (
                        KmsCommitWorkerTransport::Synchronous,
                        KmsCommitWorkerStartup::AutomaticStartupDegraded,
                    )
                }
                Err(error) => {
                    return Err(io::Error::other(format!(
                        "native KMS commit worker startup failed in force mode: {error:?}"
                    ))
                    .into());
                }
            }
        }
    };
    Ok((worker, outcome.0, outcome.1))
}
