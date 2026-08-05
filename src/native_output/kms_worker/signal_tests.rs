use super::tests::{reserve_for_test, test_job};
use super::thread::{KmsCommitExecutor, KmsWorkerSubmission, KmsWorkerSubmitFailure};
use super::*;
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug)]
struct SignalMaskExecutor {
    blocked: Arc<AtomicBool>,
    entered: Arc<Barrier>,
}

impl KmsCommitExecutor for SignalMaskExecutor {
    fn submit(&self, _job: &KmsCommitJob) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
        self.blocked.store(
            oblivion_one::process::sigchld_is_blocked_for_current_thread().unwrap(),
            Ordering::Release,
        );
        self.entered.wait();
        Err(KmsWorkerSubmitFailure::new(
            oblivion_one::native::kms::AtomicKmsErrorKind::FlipRejected,
            "signal-mask test",
        ))
    }
}

#[test]
fn kms_worker_inherits_the_early_sigchld_mask() {
    oblivion_one::process::block_sigchld_for_current_thread().unwrap();
    let blocked = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(Barrier::new(2));
    let handle = KmsCommitWorkerHandle::start(Arc::new(SignalMaskExecutor {
        blocked: Arc::clone(&blocked),
        entered: Arc::clone(&entered),
    }))
    .unwrap();
    reserve_for_test(&handle, test_job(9).kind)
        .enqueue(test_job(9))
        .unwrap();

    entered.wait();
    assert!(blocked.load(Ordering::Acquire));
}
