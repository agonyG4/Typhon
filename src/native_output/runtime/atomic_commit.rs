use std::io;

use super::*;
use oblivion_one::native::kms::{KmsBackendKind, PageFlipToken};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtomicCommitKind {
    CompositedPrimary {
        transaction_id: OutputTransactionId,
        frame_id: u64,
        framebuffer_id: u32,
    },
    DirectPrimary {
        transaction_id: OutputTransactionId,
        direct_token: PageFlipToken,
        framebuffer_id: u32,
    },
    CursorOnly {
        transaction_id: OutputTransactionId,
        cursor_epoch: u64,
        framebuffer_id: Option<u32>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtomicCommitPhase {
    WorkerQueued {
        queued_at_ns: u64,
    },
    KernelSubmitted {
        submitted_at_ns: u64,
        submit_returned_at_ns: u64,
        watchdog_deadline_ns: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PendingAtomicCommit {
    pub(crate) token: PageFlipToken,
    pub(crate) generation: u64,
    pub(crate) crtc_id: u32,
    pub(crate) kind: AtomicCommitKind,
    pub(crate) phase: AtomicCommitPhase,
    // Kept as compatibility accessors for existing metrics and synchronous
    // callers. Worker-queued commits expose zero until submit acknowledgment.
    pub(crate) submitted_at_ns: u64,
    pub(crate) watchdog_deadline_ns: u64,
    watchdog_reported: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtomicCommitCompletion {
    Completed(AtomicCommitKind),
    DeferredUntilSubmitAck,
    DuplicateEarlyPageflip,
    Mismatched,
    WrongCrtc,
    WrongGeneration,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AtomicCommitArbiter {
    kernel_submitted: Option<PendingAtomicCommit>,
    worker_queued: Option<PendingAtomicCommit>,
    early_pageflip: Option<DrmPresentationEvent>,
    watchdog_interval_ns: u64,
    atomic_commit_watchdog_timeouts_total: u64,
    atomic_cursor_watchdog_timeouts: u64,
    atomic_primary_watchdog_timeouts: u64,
    atomic_commits_submitted_total: u64,
    atomic_commits_completed_total: u64,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn register_atomic_primary_submission(
    arbiter: &mut AtomicCommitArbiter,
    kms_kind: KmsBackendKind,
    token: u64,
    generation: u64,
    crtc_id: u32,
    transaction_id: Option<OutputTransactionId>,
    frame_id: u64,
    framebuffer_id: u32,
    submitted_at_ns: u64,
) -> io::Result<bool> {
    if kms_kind != KmsBackendKind::Atomic {
        return Ok(false);
    }
    let transaction_id = transaction_id
        .ok_or_else(|| io::Error::other("Atomic primary submission has no output transaction"))?;
    let token = PageFlipToken::new(token)
        .ok_or_else(|| io::Error::other("Atomic primary pageflip token is zero"))?;
    arbiter
        .reserve(
            token,
            generation,
            crtc_id,
            AtomicCommitKind::CompositedPrimary {
                transaction_id,
                frame_id,
                framebuffer_id,
            },
            submitted_at_ns,
        )
        .map_err(io::Error::other)?;
    Ok(true)
}

impl AtomicCommitArbiter {
    pub(crate) fn new() -> Self {
        Self::with_watchdog(1_000_000_000, 0)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn with_watchdog(watchdog_interval_ns: u64, _anchor_ns: u64) -> Self {
        Self {
            kernel_submitted: None,
            worker_queued: None,
            early_pageflip: None,
            watchdog_interval_ns: if watchdog_interval_ns == 0 {
                1
            } else {
                watchdog_interval_ns
            },
            atomic_commit_watchdog_timeouts_total: 0,
            atomic_cursor_watchdog_timeouts: 0,
            atomic_primary_watchdog_timeouts: 0,
            atomic_commits_submitted_total: 0,
            atomic_commits_completed_total: 0,
        }
    }

    pub(crate) fn reserve(
        &mut self,
        token: PageFlipToken,
        generation: u64,
        crtc_id: u32,
        kind: AtomicCommitKind,
        submitted_at_ns: u64,
    ) -> Result<(), &'static str> {
        if self.kernel_submitted.is_some() || self.worker_queued.is_some() {
            return Err("an Atomic commit is already pending");
        }
        self.kernel_submitted = Some(PendingAtomicCommit {
            token,
            generation,
            crtc_id,
            kind,
            phase: AtomicCommitPhase::KernelSubmitted {
                submitted_at_ns,
                submit_returned_at_ns: submitted_at_ns,
                watchdog_deadline_ns: submitted_at_ns.saturating_add(self.watchdog_interval_ns),
            },
            submitted_at_ns,
            watchdog_deadline_ns: submitted_at_ns.saturating_add(self.watchdog_interval_ns),
            watchdog_reported: false,
        });
        self.atomic_commits_submitted_total = self.atomic_commits_submitted_total.saturating_add(1);
        Ok(())
    }

    pub(crate) fn reserve_worker_queued(
        &mut self,
        token: PageFlipToken,
        generation: u64,
        crtc_id: u32,
        kind: AtomicCommitKind,
        queued_at_ns: u64,
    ) -> Result<(), &'static str> {
        if self.worker_queued.is_some() {
            return Err("an Atomic worker commit is already queued");
        }
        self.early_pageflip = None;
        self.worker_queued = Some(PendingAtomicCommit {
            token,
            generation,
            crtc_id,
            kind,
            phase: AtomicCommitPhase::WorkerQueued { queued_at_ns },
            submitted_at_ns: 0,
            watchdog_deadline_ns: 0,
            watchdog_reported: false,
        });
        Ok(())
    }

    pub(crate) fn mark_kernel_submitted(
        &mut self,
        token: PageFlipToken,
        submitted_at_ns: u64,
        submit_returned_at_ns: u64,
    ) -> Result<(), &'static str> {
        if self.kernel_submitted.is_some() {
            return Err("an Atomic commit is already kernel submitted");
        }
        if self
            .worker_queued
            .is_none_or(|pending| pending.token != token)
        {
            return Err("Atomic submit token does not match queued commit");
        }
        let Some(mut pending) = self.worker_queued.take() else {
            return Err("no queued Atomic commit");
        };
        if !matches!(pending.phase, AtomicCommitPhase::WorkerQueued { .. }) {
            return Err("Atomic commit is not worker queued");
        }
        let watchdog_deadline_ns = submitted_at_ns.saturating_add(self.watchdog_interval_ns);
        pending.phase = AtomicCommitPhase::KernelSubmitted {
            submitted_at_ns,
            submit_returned_at_ns,
            watchdog_deadline_ns,
        };
        pending.submitted_at_ns = submitted_at_ns;
        pending.watchdog_deadline_ns = watchdog_deadline_ns;
        self.kernel_submitted = Some(pending);
        self.atomic_commits_submitted_total = self.atomic_commits_submitted_total.saturating_add(1);
        Ok(())
    }

    pub(crate) fn reject_worker_queued(
        &mut self,
        token: PageFlipToken,
    ) -> Option<PendingAtomicCommit> {
        if self.worker_queued.is_some_and(|pending| {
            pending.token == token
                && matches!(pending.phase, AtomicCommitPhase::WorkerQueued { .. })
        }) {
            self.early_pageflip = None;
            self.worker_queued.take()
        } else {
            None
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn complete(
        &mut self,
        token: PageFlipToken,
        generation: u64,
        crtc_id: u32,
    ) -> AtomicCommitCompletion {
        self.complete_pageflip(
            DrmPresentationEvent {
                user_data: token.get(),
                sequence: 0,
                timestamp: oblivion_one::native::drm::DrmPresentationTimestamp {
                    seconds: 0,
                    microseconds: 0,
                },
                crtc_id,
            },
            generation,
        )
    }

    pub(crate) fn complete_pageflip(
        &mut self,
        pageflip: DrmPresentationEvent,
        generation: u64,
    ) -> AtomicCommitCompletion {
        let token = match PageFlipToken::new(pageflip.user_data) {
            Some(token) => token,
            None => return AtomicCommitCompletion::Stale,
        };
        let pending = match (self.kernel_submitted, self.worker_queued) {
            (Some(pending), _) | (None, Some(pending)) => pending,
            (None, None) => return AtomicCommitCompletion::Stale,
        };
        if token != pending.token {
            return AtomicCommitCompletion::Mismatched;
        }
        if generation != pending.generation {
            return AtomicCommitCompletion::WrongGeneration;
        }
        if pageflip.crtc_id != pending.crtc_id {
            return AtomicCommitCompletion::WrongCrtc;
        }
        if matches!(pending.phase, AtomicCommitPhase::WorkerQueued { .. }) {
            if self.early_pageflip.is_some() {
                return AtomicCommitCompletion::DuplicateEarlyPageflip;
            }
            self.early_pageflip = Some(DrmPresentationEvent { ..pageflip });
            return AtomicCommitCompletion::DeferredUntilSubmitAck;
        }
        self.kernel_submitted = None;
        self.atomic_commits_completed_total = self.atomic_commits_completed_total.saturating_add(1);
        AtomicCommitCompletion::Completed(pending.kind)
    }

    pub(crate) fn replay_deferred_pageflip(&mut self) -> Option<AtomicCommitCompletion> {
        let pending = self.kernel_submitted?;
        self.early_pageflip.take()?;
        if !matches!(pending.phase, AtomicCommitPhase::KernelSubmitted { .. }) {
            return None;
        }
        self.kernel_submitted = None;
        self.atomic_commits_completed_total = self.atomic_commits_completed_total.saturating_add(1);
        Some(AtomicCommitCompletion::Completed(pending.kind))
    }

    pub(crate) fn deferred_pageflip(&self) -> Option<DrmPresentationEvent> {
        self.early_pageflip
    }

    pub(crate) fn cancel(&mut self, token: PageFlipToken) -> Option<PendingAtomicCommit> {
        if self
            .kernel_submitted
            .is_some_and(|pending| pending.token == token)
        {
            self.kernel_submitted.take()
        } else if self
            .worker_queued
            .is_some_and(|pending| pending.token == token)
        {
            self.worker_queued.take()
        } else {
            None
        }
    }

    pub(crate) const fn atomic_commit_pending(&self) -> bool {
        self.kernel_submitted.is_some() || self.worker_queued.is_some()
    }

    pub(crate) fn worker_job_queued(&self) -> bool {
        self.worker_queued
            .is_some_and(|pending| matches!(pending.phase, AtomicCommitPhase::WorkerQueued { .. }))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn kernel_commit_submitted(&self) -> bool {
        self.kernel_submitted.is_some_and(|pending| {
            matches!(pending.phase, AtomicCommitPhase::KernelSubmitted { .. })
        })
    }

    pub(crate) fn worker_slot_available(&self) -> bool {
        self.worker_queued.is_none()
    }

    pub(crate) fn worker_queued_token(&self) -> Option<PageFlipToken> {
        self.worker_queued.map(|pending| pending.token)
    }

    pub(crate) fn worker_queued_kind(&self) -> Option<AtomicCommitKind> {
        self.worker_queued.map(|pending| pending.kind)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn kernel_submitted_token(&self) -> Option<PageFlipToken> {
        self.kernel_submitted.map(|pending| pending.token)
    }

    pub(crate) fn kernel_submitted_kind(&self) -> Option<AtomicCommitKind> {
        self.kernel_submitted.map(|pending| pending.kind)
    }

    pub(crate) fn pending_atomic_token(&self) -> Option<PageFlipToken> {
        self.kernel_submitted
            .or(self.worker_queued)
            .map(|pending| pending.token)
    }

    pub(crate) fn pending_atomic_kind(&self) -> Option<AtomicCommitKind> {
        self.kernel_submitted
            .or(self.worker_queued)
            .map(|pending| pending.kind)
    }

    pub(crate) fn pending_atomic_commit(&self) -> Option<PendingAtomicCommit> {
        if self.kernel_submitted.is_some() {
            self.kernel_submitted
        } else {
            self.worker_queued
        }
    }

    pub(crate) fn watchdog_deadline_ns(&self) -> Option<u64> {
        self.kernel_submitted
            .and_then(|pending| match pending.phase {
                AtomicCommitPhase::WorkerQueued { .. } => None,
                AtomicCommitPhase::KernelSubmitted {
                    watchdog_deadline_ns,
                    ..
                } => Some(watchdog_deadline_ns),
            })
    }

    pub(crate) fn watchdog_expired(&mut self, now_ns: u64) -> Option<AtomicCommitKind> {
        let pending = self.kernel_submitted.as_mut()?;
        let AtomicCommitPhase::KernelSubmitted {
            watchdog_deadline_ns,
            ..
        } = pending.phase
        else {
            return None;
        };
        if pending.watchdog_reported || now_ns < watchdog_deadline_ns {
            return None;
        }
        pending.watchdog_reported = true;
        self.atomic_commit_watchdog_timeouts_total =
            self.atomic_commit_watchdog_timeouts_total.saturating_add(1);
        if matches!(pending.kind, AtomicCommitKind::CursorOnly { .. }) {
            self.atomic_cursor_watchdog_timeouts =
                self.atomic_cursor_watchdog_timeouts.saturating_add(1);
        } else {
            self.atomic_primary_watchdog_timeouts =
                self.atomic_primary_watchdog_timeouts.saturating_add(1);
        }
        Some(pending.kind)
    }

    pub(crate) fn abandon_for_recovery(&mut self) {
        self.kernel_submitted = None;
        self.worker_queued = None;
        self.early_pageflip = None;
    }

    pub(crate) const fn atomic_commit_watchdog_timeouts_total(&self) -> u64 {
        self.atomic_commit_watchdog_timeouts_total
    }

    pub(crate) const fn cursor_watchdog_timeouts(&self) -> u64 {
        self.atomic_cursor_watchdog_timeouts
    }

    pub(crate) const fn primary_watchdog_timeouts(&self) -> u64 {
        self.atomic_primary_watchdog_timeouts
    }

    pub(crate) const fn atomic_commits_submitted_total(&self) -> u64 {
        self.atomic_commits_submitted_total
    }

    pub(crate) const fn atomic_commits_completed_total(&self) -> u64 {
        self.atomic_commits_completed_total
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_atomic_pageflip(
    arbiter: &mut AtomicCommitArbiter,
    backend_kind: KmsBackendKind,
    event: Option<DrmPresentationEvent>,
    generation: u64,
    now_ns: u64,
    mismatched_events: &mut u64,
    stale_events: &mut u64,
    worker_watchdog: bool,
) -> io::Result<(
    Option<DrmPresentationEvent>,
    Option<AtomicCommitCompletion>,
    Option<AtomicCommitKind>,
)> {
    if backend_kind != KmsBackendKind::Atomic {
        return Ok((event, None, None));
    }
    let mut event = event;
    let completion = event.map(|pageflip| arbiter.complete_pageflip(pageflip, generation));
    if let Some(completion) = completion
        && !matches!(completion, AtomicCommitCompletion::Completed(_))
    {
        match completion {
            AtomicCommitCompletion::Mismatched | AtomicCommitCompletion::WrongCrtc => {
                *mismatched_events = mismatched_events.saturating_add(1);
            }
            AtomicCommitCompletion::WrongGeneration | AtomicCommitCompletion::Stale => {
                *stale_events = stale_events.saturating_add(1);
            }
            AtomicCommitCompletion::DeferredUntilSubmitAck => {
                // The pageflip is retained by the arbiter until the worker
                // success event transfers physical ownership.
            }
            AtomicCommitCompletion::DuplicateEarlyPageflip => {
                *mismatched_events = mismatched_events.saturating_add(1);
            }
            AtomicCommitCompletion::Completed(_) => unreachable!(),
        }
        event = None;
    }
    let timeout = (!worker_watchdog && event.is_none())
        .then(|| arbiter.watchdog_expired(now_ns))
        .flatten();
    Ok((event, completion, timeout))
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use super::*;
    use oblivion_one::native::drm::DrmPresentationTimestamp;

    fn token(value: u64) -> PageFlipToken {
        PageFlipToken::new(value).expect("test token is nonzero")
    }

    fn transaction_id(value: u64) -> OutputTransactionId {
        OutputTransactionId::new(NonZeroU64::new(value).expect("test transaction ID is nonzero"))
    }

    fn cursor_kind() -> AtomicCommitKind {
        AtomicCommitKind::CursorOnly {
            transaction_id: transaction_id(4),
            cursor_epoch: 4,
            framebuffer_id: Some(91),
        }
    }

    fn primary_kind() -> AtomicCommitKind {
        AtomicCommitKind::CompositedPrimary {
            transaction_id: transaction_id(7),
            frame_id: 7,
            framebuffer_id: 81,
        }
    }

    #[test]
    fn atomic_primary_registration_reserves_composited_primary() {
        let mut arbiter = AtomicCommitArbiter::new();
        assert!(
            register_atomic_primary_submission(
                &mut arbiter,
                KmsBackendKind::Atomic,
                41,
                7,
                3,
                Some(transaction_id(9)),
                9,
                81,
                100,
            )
            .unwrap()
        );
        assert_eq!(
            arbiter.pending_atomic_kind(),
            Some(AtomicCommitKind::CompositedPrimary {
                transaction_id: transaction_id(9),
                frame_id: 9,
                framebuffer_id: 81,
            })
        );
    }

    #[test]
    fn legacy_primary_registration_keeps_arbiter_empty() {
        let mut arbiter = AtomicCommitArbiter::new();
        assert!(
            !register_atomic_primary_submission(
                &mut arbiter,
                KmsBackendKind::Legacy,
                41,
                7,
                3,
                Some(transaction_id(10)),
                9,
                81,
                100,
            )
            .unwrap()
        );
        assert!(!arbiter.atomic_commit_pending());
    }

    #[test]
    fn atomic_primary_registration_arms_watchdog_only_for_atomic_backend() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(10, 0);
        assert!(
            register_atomic_primary_submission(
                &mut arbiter,
                KmsBackendKind::Atomic,
                43,
                8,
                3,
                Some(transaction_id(10)),
                10,
                82,
                100,
            )
            .unwrap()
        );
        assert_eq!(
            arbiter.watchdog_expired(110),
            Some(AtomicCommitKind::CompositedPrimary {
                transaction_id: transaction_id(10),
                frame_id: 10,
                framebuffer_id: 82
            })
        );

        let mut legacy = AtomicCommitArbiter::with_watchdog(10, 0);
        assert!(
            !register_atomic_primary_submission(
                &mut legacy,
                KmsBackendKind::Legacy,
                43,
                8,
                3,
                Some(transaction_id(10)),
                10,
                82,
                100,
            )
            .unwrap()
        );
        assert_eq!(legacy.watchdog_expired(110), None);
    }

    #[test]
    fn cursor_only_reserves_same_atomic_slot_as_primary() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(2, 100);
        arbiter
            .reserve(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();

        assert!(arbiter.atomic_commit_pending());
        assert_eq!(arbiter.pending_atomic_token(), Some(token(1)));
        assert!(
            arbiter
                .reserve(token(2), 3, 42, primary_kind(), 101)
                .is_err()
        );
    }

    #[test]
    fn primary_cannot_submit_while_cursor_only_is_pending() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(2, 100);
        arbiter
            .reserve(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();

        assert!(
            arbiter
                .reserve(token(2), 3, 42, primary_kind(), 101)
                .is_err()
        );
    }

    #[test]
    fn cursor_cannot_submit_while_primary_is_pending() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(2, 100);
        arbiter
            .reserve(token(1), 3, 42, primary_kind(), 100)
            .unwrap();

        assert!(
            arbiter
                .reserve(token(2), 3, 42, cursor_kind(), 101)
                .is_err()
        );
    }

    #[test]
    fn pageflip_routes_cursor_only_by_commit_kind() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(2, 100);
        arbiter
            .reserve(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();

        assert_eq!(
            arbiter.complete(token(1), 3, 42),
            AtomicCommitCompletion::Completed(cursor_kind())
        );
    }

    #[test]
    fn pageflip_routes_direct_primary_by_commit_kind() {
        let direct = AtomicCommitKind::DirectPrimary {
            transaction_id: transaction_id(17),
            direct_token: token(17),
            framebuffer_id: 82,
        };
        let mut arbiter = AtomicCommitArbiter::with_watchdog(2, 100);
        arbiter.reserve(token(1), 3, 42, direct, 100).unwrap();

        assert_eq!(
            arbiter.complete(token(1), 3, 42),
            AtomicCommitCompletion::Completed(direct)
        );
    }

    #[test]
    fn stale_token_does_not_complete_any_kind() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(2, 100);
        arbiter
            .reserve(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();

        assert_eq!(
            arbiter.complete(token(2), 3, 42),
            AtomicCommitCompletion::Mismatched
        );
        assert!(arbiter.atomic_commit_pending());
    }

    #[test]
    fn cancel_returns_the_owned_transaction_identity() {
        let mut arbiter = AtomicCommitArbiter::new();
        arbiter
            .reserve(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();

        let canceled = arbiter.cancel(token(1)).expect("pending commit");
        assert_eq!(canceled.kind, cursor_kind());
        assert!(!arbiter.atomic_commit_pending());
    }

    #[test]
    fn wrong_crtc_does_not_complete_any_kind() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(2, 100);
        arbiter
            .reserve(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();

        assert_eq!(
            arbiter.complete(token(1), 3, 43),
            AtomicCommitCompletion::WrongCrtc
        );
        assert!(arbiter.atomic_commit_pending());
    }

    #[test]
    fn wrong_generation_does_not_complete_any_kind() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(2, 100);
        arbiter
            .reserve(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();

        assert_eq!(
            arbiter.complete(token(1), 4, 42),
            AtomicCommitCompletion::WrongGeneration
        );
        assert!(arbiter.atomic_commit_pending());
    }

    #[test]
    fn cursor_only_completion_does_not_complete_frame_batch() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(2, 100);
        arbiter
            .reserve(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();

        let AtomicCommitCompletion::Completed(AtomicCommitKind::CursorOnly { .. }) =
            arbiter.complete(token(1), 3, 42)
        else {
            panic!("cursor completion was not routed as cursor-only");
        };
    }

    #[test]
    fn cursor_only_submission_arms_atomic_watchdog() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(25, 100);
        arbiter
            .reserve(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();

        assert_eq!(arbiter.watchdog_deadline_ns(), Some(125));
    }

    #[test]
    fn worker_queued_commit_does_not_arm_watchdog() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(25, 100);
        arbiter
            .reserve_worker_queued(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();

        assert!(arbiter.atomic_commit_pending());
        assert!(arbiter.worker_job_queued());
        assert!(!arbiter.kernel_commit_submitted());
        assert_eq!(arbiter.watchdog_deadline_ns(), None);
        assert_eq!(arbiter.watchdog_expired(1_000), None);
    }

    #[test]
    fn early_pageflip_is_deferred_until_worker_submit_ack() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(25, 100);
        arbiter
            .reserve_worker_queued(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();

        assert_eq!(
            arbiter.complete(token(1), 3, 42),
            AtomicCommitCompletion::DeferredUntilSubmitAck
        );
        assert!(arbiter.atomic_commit_pending());
        assert!(arbiter.mark_kernel_submitted(token(1), 150, 155).is_ok());
        assert!(arbiter.kernel_commit_submitted());
        assert_eq!(
            arbiter.replay_deferred_pageflip(),
            Some(AtomicCommitCompletion::Completed(cursor_kind()))
        );
        assert!(!arbiter.atomic_commit_pending());
    }

    #[test]
    fn second_early_pageflip_is_rejected() {
        let mut arbiter = AtomicCommitArbiter::new();
        arbiter
            .reserve_worker_queued(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();
        assert_eq!(
            arbiter.complete(token(1), 3, 42),
            AtomicCommitCompletion::DeferredUntilSubmitAck
        );
        assert_eq!(
            arbiter.complete(token(1), 3, 42),
            AtomicCommitCompletion::DuplicateEarlyPageflip
        );
    }

    #[test]
    fn cursor_only_pageflip_clears_atomic_watchdog() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(25, 100);
        arbiter
            .reserve(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();
        arbiter.complete(token(1), 3, 42);

        assert_eq!(arbiter.watchdog_deadline_ns(), None);
    }

    #[test]
    fn lost_cursor_pageflip_triggers_cursor_watchdog() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(25, 100);
        arbiter
            .reserve(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();

        assert_eq!(arbiter.watchdog_expired(125), Some(cursor_kind()));
        assert_eq!(arbiter.cursor_watchdog_timeouts(), 1);
    }

    #[test]
    fn watchdog_does_not_fabricate_cursor_completion() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(25, 100);
        arbiter
            .reserve(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();
        assert!(arbiter.watchdog_expired(125).is_some());

        assert!(arbiter.atomic_commit_pending());
        assert_eq!(arbiter.pending_atomic_token(), Some(token(1)));
    }

    #[test]
    fn cursor_watchdog_preserves_resources_until_recovery() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(25, 100);
        arbiter
            .reserve(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();
        assert!(arbiter.watchdog_expired(125).is_some());
        assert!(arbiter.atomic_commit_pending());

        arbiter.abandon_for_recovery();
        assert!(!arbiter.atomic_commit_pending());
    }

    #[test]
    fn cursor_watchdog_blocks_second_atomic_submit() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(25, 100);
        arbiter
            .reserve(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();
        assert!(arbiter.watchdog_expired(125).is_some());

        assert!(
            arbiter
                .reserve(token(2), 3, 42, primary_kind(), 126)
                .is_err()
        );
    }

    #[test]
    fn primary_and_cursor_timeouts_are_not_double_counted() {
        let mut arbiter = AtomicCommitArbiter::with_watchdog(25, 100);
        arbiter
            .reserve(token(1), 3, 42, cursor_kind(), 100)
            .unwrap();
        assert!(arbiter.watchdog_expired(125).is_some());
        assert!(arbiter.watchdog_expired(126).is_none());
        arbiter.abandon_for_recovery();

        arbiter
            .reserve(token(2), 3, 42, primary_kind(), 200)
            .unwrap();
        assert!(arbiter.watchdog_expired(225).is_some());

        assert_eq!(arbiter.atomic_commit_watchdog_timeouts_total(), 2);
        assert_eq!(arbiter.cursor_watchdog_timeouts(), 1);
        assert_eq!(arbiter.primary_watchdog_timeouts(), 1);
    }

    #[test]
    fn valid_compatibility_atomic_pageflip_completes_registered_primary() {
        let mut arbiter = AtomicCommitArbiter::new();
        register_atomic_primary_submission(
            &mut arbiter,
            KmsBackendKind::Atomic,
            23,
            7,
            42,
            Some(transaction_id(11)),
            11,
            81,
            100,
        )
        .unwrap();
        let mut mismatched = 0;
        let mut stale = 0;
        let event = DrmPresentationEvent {
            crtc_id: 42,
            user_data: 23,
            timestamp: DrmPresentationTimestamp {
                seconds: 0,
                microseconds: 0,
            },
            sequence: 1,
        };

        let (event, completion, watchdog) = validate_atomic_pageflip(
            &mut arbiter,
            KmsBackendKind::Atomic,
            Some(event),
            7,
            101,
            &mut mismatched,
            &mut stale,
            false,
        )
        .unwrap();

        assert!(event.is_some());
        assert_eq!(
            completion,
            Some(AtomicCommitCompletion::Completed(
                AtomicCommitKind::CompositedPrimary {
                    transaction_id: transaction_id(11),
                    frame_id: 11,
                    framebuffer_id: 81,
                },
            ))
        );
        assert_eq!(watchdog, None);
        assert_eq!(mismatched, 0);
        assert_eq!(stale, 0);
        assert_eq!(arbiter.atomic_commits_completed_total(), 1);
    }

    #[test]
    fn runtime_tracks_one_kernel_submitted_plus_one_worker_queued() {
        let mut arbiter = AtomicCommitArbiter::new();
        arbiter
            .reserve(token(1), 3, 42, primary_kind(), 100)
            .unwrap();
        arbiter
            .reserve_worker_queued(token(2), 3, 42, primary_kind(), 110)
            .unwrap();

        assert!(arbiter.kernel_commit_submitted());
        assert!(arbiter.worker_job_queued());
        assert_eq!(arbiter.worker_queued_token(), Some(token(2)));
        assert_eq!(arbiter.kernel_submitted_token(), Some(token(1)));
        assert!(
            arbiter
                .reserve_worker_queued(token(3), 3, 42, primary_kind(), 120)
                .is_err()
        );
    }

    #[test]
    fn pageflip_ack_promotes_queued_next_job() {
        let mut arbiter = AtomicCommitArbiter::new();
        arbiter
            .reserve(token(1), 3, 42, primary_kind(), 100)
            .unwrap();
        arbiter
            .reserve_worker_queued(token(2), 3, 42, primary_kind(), 110)
            .unwrap();
        assert_eq!(
            arbiter.complete(token(1), 3, 42),
            AtomicCommitCompletion::Completed(primary_kind())
        );
        arbiter
            .mark_kernel_submitted(token(2), 130, 135)
            .expect("queued next job should become kernel submitted");
        assert_eq!(arbiter.kernel_submitted_token(), Some(token(2)));
        assert!(!arbiter.worker_job_queued());
    }
}
