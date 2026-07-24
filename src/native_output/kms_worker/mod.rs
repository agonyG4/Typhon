#![allow(dead_code)]

mod payload;
mod policy;
mod queue;
mod thread;
mod timing;

pub(crate) use payload::{KmsCommitJob, KmsCursorUpdate, KmsPrimaryUpdate, KmsTestOnlyPolicy};
pub(crate) use policy::{
    KmsCommitWorkerPolicy, KmsCommitWorkerStartupError, KmsCommitWorkerTransport,
};
pub(crate) use queue::{KmsCommitAdmissionPermit, KmsWorkerAdmissionError};
pub(crate) use thread::{KmsCommitWorkerHandle, KmsWorkerEvent};
pub(crate) use timing::KmsCommitTimingModel;

#[cfg(test)]
mod tests {
    use super::thread::{
        KmsCommitExecutor, KmsWorkerFatalReason, KmsWorkerSubmission, KmsWorkerSubmitFailure,
    };
    use super::timing::KmsTimingDecision;
    use super::*;
    use crate::native_output::{OutputTransactionId, runtime::AtomicCommitKind};
    use oblivion_one::native::kms::KmsBackendKind;
    use oblivion_one::native::presentation_deadline::{
        MonotonicTimestampNs, PresentationTarget, PresentationTargetReason,
    };
    use std::{
        collections::VecDeque,
        sync::{Arc, Barrier, Mutex},
        time::Duration,
    };

    #[test]
    fn worker_policy_defaults_to_off() {
        assert_eq!(
            KmsCommitWorkerPolicy::from_env_value(None),
            KmsCommitWorkerPolicy::Off
        );
    }

    #[test]
    fn worker_policy_accepts_all_rollout_values() {
        assert_eq!(
            KmsCommitWorkerPolicy::parse(Some("off")).unwrap(),
            KmsCommitWorkerPolicy::Off
        );
        assert_eq!(
            KmsCommitWorkerPolicy::parse(Some("auto")).unwrap(),
            KmsCommitWorkerPolicy::Auto
        );
        assert_eq!(
            KmsCommitWorkerPolicy::parse(Some("force")).unwrap(),
            KmsCommitWorkerPolicy::Force
        );
    }

    #[test]
    fn auto_atomic_uses_worker_when_startup_succeeds() {
        assert_eq!(
            KmsCommitWorkerPolicy::Auto
                .effective(KmsBackendKind::Atomic, true)
                .unwrap(),
            KmsCommitWorkerTransport::Worker
        );
    }

    #[test]
    fn auto_atomic_falls_back_to_sync_when_startup_fails() {
        assert_eq!(
            KmsCommitWorkerPolicy::Auto
                .effective(KmsBackendKind::Atomic, false)
                .unwrap(),
            KmsCommitWorkerTransport::Synchronous
        );
    }

    #[test]
    fn force_legacy_is_unsupported() {
        assert_eq!(
            KmsCommitWorkerPolicy::Force.effective(KmsBackendKind::Legacy, true),
            Err(KmsCommitWorkerStartupError::UnsupportedBackend)
        );
    }

    #[test]
    fn reactive_double_submits_at_not_before() {
        use oblivion_one::native::presentation_deadline::{
            MonotonicTimestampNs, PresentationTarget, PresentationTargetReason,
        };
        use std::time::Duration;

        let target = PresentationTarget {
            sequence: 1,
            presentation_time: MonotonicTimestampNs::new(20_000_000),
            submit_not_before: MonotonicTimestampNs::new(10_000_000),
            render_start_deadline: MonotonicTimestampNs::new(0),
            refresh_interval: Duration::from_millis(16),
            reason: PresentationTargetReason::ReactiveDouble,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
        };

        let model = KmsCommitTimingModel::new(target.refresh_interval);
        assert_eq!(
            model.submit_at(target, 1_000_000),
            KmsTimingDecision {
                submit_at_ns: 10_000_000,
                late: false,
                late_by_ns: 0,
            }
        );
    }

    #[test]
    fn predictive_job_uses_bounded_safety_margin() {
        use oblivion_one::native::presentation_deadline::{
            MonotonicTimestampNs, PresentationTarget, PresentationTargetReason,
        };
        use std::time::Duration;

        let target = PresentationTarget {
            sequence: 1,
            presentation_time: MonotonicTimestampNs::new(20_000_000),
            submit_not_before: MonotonicTimestampNs::new(2_000_000),
            render_start_deadline: MonotonicTimestampNs::new(0),
            refresh_interval: Duration::from_millis(16),
            reason: PresentationTargetReason::Normal,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
        };

        let model = KmsCommitTimingModel::new(target.refresh_interval);
        assert_eq!(model.submit_at(target, 1_000_000).submit_at_ns, 19_000_000);
    }

    #[test]
    fn timing_model_never_violates_submit_not_before_when_late() {
        use oblivion_one::native::presentation_deadline::{
            MonotonicTimestampNs, PresentationTarget, PresentationTargetReason,
        };
        use std::time::Duration;

        let target = PresentationTarget {
            sequence: 1,
            presentation_time: MonotonicTimestampNs::new(20_000_000),
            submit_not_before: MonotonicTimestampNs::new(10_000_000),
            render_start_deadline: MonotonicTimestampNs::new(0),
            refresh_interval: Duration::from_millis(16),
            reason: PresentationTargetReason::Normal,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
        };

        let model = KmsCommitTimingModel::new(target.refresh_interval);
        assert_eq!(model.submit_at(target, 30_000_000).submit_at_ns, 30_000_000);
    }

    #[test]
    fn late_sample_increases_margin_immediately() {
        use std::time::Duration;
        let mut model = KmsCommitTimingModel::new(Duration::from_millis(16));
        model.observe_submit_delta_ns(2_000_000);
        assert_eq!(model.safety_margin_ns(), 2_100_000);
    }

    #[test]
    fn early_samples_decay_margin_gradually() {
        use std::time::Duration;
        let mut model = KmsCommitTimingModel::new(Duration::from_millis(16));
        model.observe_submit_delta_ns(2_000_000);
        let before = model.safety_margin_ns();
        model.observe_submit_delta_ns(-1_000_000);
        assert!(model.safety_margin_ns() < before);
        assert_eq!(model.safety_margin_ns(), before - (before - 100_000) / 16);
    }

    #[test]
    fn margin_is_bounded_by_half_refresh() {
        use std::time::Duration;
        let mut model = KmsCommitTimingModel::new(Duration::from_micros(100));
        model.observe_submit_delta_ns(10_000_000);
        assert_eq!(model.safety_margin_ns(), 50_000);
    }

    fn test_job(token: u64) -> KmsCommitJob {
        let transaction_id = OutputTransactionId::new(
            std::num::NonZeroU64::new(token).expect("test transaction ID is nonzero"),
        );
        KmsCommitJob {
            transaction_id,
            token: oblivion_one::native::kms::PageFlipToken::new(token).unwrap(),
            output_generation: 1,
            crtc_id: 7,
            kind: AtomicCommitKind::DirectPrimary {
                transaction_id,
                direct_token: oblivion_one::native::kms::PageFlipToken::new(token).unwrap(),
                framebuffer_id: 42,
            },
            target: PresentationTarget {
                sequence: token,
                presentation_time: MonotonicTimestampNs::new(0),
                submit_not_before: MonotonicTimestampNs::new(0),
                render_start_deadline: MonotonicTimestampNs::new(0),
                refresh_interval: Duration::from_millis(16),
                reason: PresentationTargetReason::ReactiveDouble,
                clock_generation: 1,
                estimated: true,
                predicted_unreachable: false,
            },
            queued_at: MonotonicTimestampNs::new(0),
            primary: KmsPrimaryUpdate::Framebuffer {
                framebuffer: oblivion_one::native::kms::FramebufferId::new(42).unwrap(),
                in_fence: None,
                request_out_fence: false,
            },
            cursor: KmsCursorUpdate::Unchanged,
            test_only: KmsTestOnlyPolicy::Skip,
        }
    }

    #[derive(Debug)]
    struct BarrierExecutor {
        started: Barrier,
        release: Barrier,
        submitted: Mutex<Vec<u64>>,
    }

    impl KmsCommitExecutor for BarrierExecutor {
        fn submit(
            &self,
            job: &mut KmsCommitJob,
        ) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
            self.started.wait();
            self.release.wait();
            self.submitted.lock().unwrap().push(job.token.get());
            Ok(KmsWorkerSubmission { out_fence: None })
        }
    }

    #[derive(Debug)]
    struct ScriptedExecutor {
        outcomes: Mutex<VecDeque<Result<(), oblivion_one::native::kms::AtomicKmsErrorKind>>>,
        submitted: Mutex<Vec<u64>>,
    }

    impl KmsCommitExecutor for ScriptedExecutor {
        fn submit(
            &self,
            job: &mut KmsCommitJob,
        ) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
            self.submitted.lock().unwrap().push(job.token.get());
            let result = self.outcomes.lock().unwrap().pop_front().unwrap_or(Ok(()));
            match result {
                Ok(()) => Ok(KmsWorkerSubmission { out_fence: None }),
                Err(kind) => Err(KmsWorkerSubmitFailure::new(kind, "fake Atomic ioctl")),
            }
        }
    }

    fn collect_events(handle: &KmsCommitWorkerHandle) -> Vec<KmsWorkerEvent> {
        handle.drain_eventfd().unwrap();
        handle.drain_events()
    }

    #[test]
    fn main_thread_admission_returns_immediately_when_full() {
        let executor = Arc::new(BarrierExecutor {
            started: Barrier::new(2),
            release: Barrier::new(2),
            submitted: Mutex::new(Vec::new()),
        });
        let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
        handle
            .try_reserve_admission(AtomicCommitKind::DirectPrimary {
                transaction_id: test_job(1).transaction_id,
                direct_token: test_job(1).token,
                framebuffer_id: 42,
            })
            .unwrap()
            .enqueue(test_job(1))
            .unwrap();
        executor.started.wait();

        handle
            .try_reserve_admission(AtomicCommitKind::DirectPrimary {
                transaction_id: test_job(2).transaction_id,
                direct_token: test_job(2).token,
                framebuffer_id: 42,
            })
            .unwrap()
            .enqueue(test_job(2))
            .unwrap();
        assert!(matches!(
            handle.try_reserve_admission(test_job(3).kind),
            Err(KmsWorkerAdmissionError::QueueFull)
        ));

        executor.release.wait();
        for _ in 0..100 {
            if handle.inflight() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        handle.ack_pageflip(test_job(1).token).unwrap();
        handle.request_quiesce();
        handle.join().unwrap();
    }

    #[test]
    fn idle_worker_has_only_one_reserved_ready_slot() {
        let executor = Arc::new(ScriptedExecutor {
            outcomes: Mutex::new(VecDeque::new()),
            submitted: Mutex::new(Vec::new()),
        });
        let handle = KmsCommitWorkerHandle::start(executor).unwrap();
        let permit = handle.try_reserve_admission(test_job(20).kind).unwrap();
        assert!(matches!(
            handle.try_reserve_admission(test_job(21).kind),
            Err(KmsWorkerAdmissionError::QueueFull)
        ));
        drop(permit);
        handle.request_quiesce();
        handle.join().unwrap();
    }

    #[test]
    fn fifo_order_is_preserved_and_second_submit_waits_for_ack() {
        let executor = Arc::new(ScriptedExecutor {
            outcomes: Mutex::new(VecDeque::from([Ok(()), Ok(())])),
            submitted: Mutex::new(Vec::new()),
        });
        let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
        handle
            .try_reserve_admission(test_job(1).kind)
            .unwrap()
            .enqueue(test_job(1))
            .unwrap();
        for _ in 0..100 {
            if handle.submission_active() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        handle
            .try_reserve_admission(test_job(2).kind)
            .unwrap()
            .enqueue(test_job(2))
            .unwrap();

        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(*executor.submitted.lock().unwrap(), vec![1]);
        handle.ack_pageflip(test_job(1).token).unwrap();
        for _ in 0..100 {
            if executor.submitted.lock().unwrap().len() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(*executor.submitted.lock().unwrap(), vec![1, 2]);
        handle.ack_pageflip(test_job(2).token).unwrap();
        handle.request_quiesce();
        let _ = collect_events(&handle);
        handle.join().unwrap();
    }

    #[test]
    fn quiesce_rejects_new_admission() {
        let executor = Arc::new(ScriptedExecutor {
            outcomes: Mutex::new(VecDeque::new()),
            submitted: Mutex::new(Vec::new()),
        });
        let handle = KmsCommitWorkerHandle::start(executor).unwrap();
        handle.request_quiesce();
        assert!(matches!(
            handle.try_reserve_admission(test_job(1).kind),
            Err(KmsWorkerAdmissionError::Quiescing)
        ));
        handle.join().unwrap();
    }

    #[test]
    fn busy_retry_budget_is_bounded_and_returns_one_terminal_event() {
        let executor = Arc::new(ScriptedExecutor {
            outcomes: Mutex::new(VecDeque::from([
                Err(oblivion_one::native::kms::AtomicKmsErrorKind::Busy),
                Err(oblivion_one::native::kms::AtomicKmsErrorKind::Busy),
                Err(oblivion_one::native::kms::AtomicKmsErrorKind::Busy),
            ])),
            submitted: Mutex::new(Vec::new()),
        });
        let handle = KmsCommitWorkerHandle::start(executor.clone()).unwrap();
        handle
            .try_reserve_admission(test_job(9).kind)
            .unwrap()
            .enqueue(test_job(9))
            .unwrap();

        let mut events = Vec::new();
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(1));
            events.extend(collect_events(&handle));
            if events
                .iter()
                .any(|event| matches!(event, KmsWorkerEvent::BusyExhausted { .. }))
            {
                break;
            }
        }
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, KmsWorkerEvent::BusyDeferred { .. }))
                .count(),
            2
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, KmsWorkerEvent::BusyExhausted { .. }))
                .count(),
            1
        );
        assert_eq!(*executor.submitted.lock().unwrap(), vec![9, 9, 9]);
        handle.request_quiesce();
        handle.join().unwrap();
    }

    #[test]
    fn one_eventfd_wakeup_drains_all_available_worker_results() {
        let executor = Arc::new(ScriptedExecutor {
            outcomes: Mutex::new(VecDeque::from([
                Err(oblivion_one::native::kms::AtomicKmsErrorKind::FlipRejected),
                Err(oblivion_one::native::kms::AtomicKmsErrorKind::FlipRejected),
            ])),
            submitted: Mutex::new(Vec::new()),
        });
        let handle = KmsCommitWorkerHandle::start(executor).unwrap();
        handle
            .try_reserve_admission(test_job(10).kind)
            .unwrap()
            .enqueue(test_job(10))
            .unwrap();
        let mut first_events = Vec::new();
        for _ in 0..100 {
            first_events.extend(collect_events(&handle));
            if first_events.iter().any(|event| {
                matches!(event, KmsWorkerEvent::SubmitRejected { job, .. } if job.token.get() == 10)
            }) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        handle
            .try_reserve_admission(test_job(11).kind)
            .unwrap()
            .enqueue(test_job(11))
            .unwrap();
        let mut events = first_events;
        for _ in 0..100 {
            events.extend(collect_events(&handle));
            if events.iter().any(|event| {
                matches!(event, KmsWorkerEvent::SubmitRejected { job, .. } if job.token.get() == 11)
            }) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(events.iter().any(|event| {
            matches!(event, KmsWorkerEvent::SubmitRejected { job, .. } if job.token.get() == 11)
        }));
        handle.request_quiesce();
        handle.join().unwrap();
    }

    #[test]
    fn worker_emits_one_pageflip_timeout_for_inflight_commit() {
        let executor = Arc::new(ScriptedExecutor {
            outcomes: Mutex::new(VecDeque::from([Ok(())])),
            submitted: Mutex::new(Vec::new()),
        });
        let handle = KmsCommitWorkerHandle::start(executor).unwrap();
        handle
            .try_reserve_admission(test_job(13).kind)
            .unwrap()
            .enqueue(test_job(13))
            .unwrap();
        let mut events = Vec::new();
        for _ in 0..1_200 {
            events.extend(collect_events(&handle));
            if events
                .iter()
                .any(|event| matches!(event, KmsWorkerEvent::PageflipTimeout { .. }))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, KmsWorkerEvent::PageflipTimeout { .. }))
                .count(),
            1
        );
        handle.request_quiesce();
        handle.join().unwrap();
    }

    struct PanicExecutor;

    impl KmsCommitExecutor for PanicExecutor {
        fn submit(
            &self,
            _job: &mut KmsCommitJob,
        ) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
            panic!("fake worker panic");
        }
    }

    #[test]
    fn worker_panic_becomes_fatal_event() {
        let handle = KmsCommitWorkerHandle::start(Arc::new(PanicExecutor)).unwrap();
        handle
            .try_reserve_admission(test_job(12).kind)
            .unwrap()
            .enqueue(test_job(12))
            .unwrap();
        let mut events = Vec::new();
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(1));
            events.extend(collect_events(&handle));
            if events
                .iter()
                .any(|event| matches!(event, KmsWorkerEvent::Fatal { .. }))
            {
                break;
            }
        }
        assert!(events.iter().any(|event| {
            matches!(
                event,
                KmsWorkerEvent::Fatal {
                    reason: KmsWorkerFatalReason::Panic,
                    uncertain_submit: true,
                }
            )
        }));
        handle.join().unwrap();
    }
}
