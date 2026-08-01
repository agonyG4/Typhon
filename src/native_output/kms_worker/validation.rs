use super::*;

pub(super) fn invalidate_queued_dependents(
    shared: &Arc<WorkerShared>,
    predecessor: KmsCommitBundleIdentity,
    reason: ValidationBaseInvalidationReason,
) {
    let (established, returned) = {
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let established = state.established_base;
        let mut returned = Vec::new();
        let mut retained = VecDeque::with_capacity(state.queued.len());
        while let Some(job) = state.queued.pop_front() {
            let dependent = matches!(
                job.validation_base,
                KmsValidationBase::Predecessor(required) if required == predecessor
            );
            if dependent {
                returned.push(job);
            } else {
                retained.push_back(job);
            }
        }
        state.queued = retained;
        (established, returned)
    };
    for job in returned {
        if !publish_event(
            shared,
            KmsWorkerEvent::ValidationBaseInvalidated {
                expected: job.validation_base,
                job,
                established: established.map(Box::new),
                reason,
            },
        ) {
            return;
        }
    }
    shared.work_wakeup.notify_all();
}

impl KmsCommitWorkerHandle {
    pub(crate) fn attachable_primary(
        &self,
        output_generation: u64,
        crtc_id: u32,
        target: oblivion_one::native::presentation_deadline::PresentationTarget,
    ) -> Option<AttachablePrimary> {
        self.shared
            .attachable_primary(output_generation, crtc_id, target)
    }

    pub(crate) fn set_established_presented_base(
        &self,
        revision: crate::native_output::presentation::plane::PlaneStateRevision,
        output_generation: u64,
        crtc_id: u32,
    ) {
        self.shared
            .set_established_presented_base(revision, output_generation, crtc_id);
    }

    pub(crate) fn invalidate_validation_base(
        &self,
        predecessor: KmsCommitBundleIdentity,
        reason: ValidationBaseInvalidationReason,
    ) {
        invalidate_queued_dependents(&self.shared, predecessor, reason);
    }

    pub(crate) fn ack_pageflip(
        &self,
        token: PageFlipToken,
        transaction_id: OutputTransactionId,
        output_generation: u64,
    ) -> Result<(), KmsWorkerAckError> {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let lifecycle = state.lifecycle;
        let inflight_bundle = state.inflight.map(|inflight| inflight.bundle);
        drop(state);
        let Some(mut identity) = inflight_bundle else {
            if matches!(
                lifecycle,
                KmsWorkerLifecycle::ShutdownQuiescing
                    | KmsWorkerLifecycle::ShutdownAbandoning
                    | KmsWorkerLifecycle::Stopped
            ) {
                return Ok(());
            }
            return Err(KmsWorkerAckError::NoInFlightCommit);
        };
        identity.output_generation = output_generation;
        identity.token = token;
        self.ack_pageflip_identity(identity, transaction_id)
    }

    pub(crate) fn ack_pageflip_identity(
        &self,
        identity: KmsCommitBundleIdentity,
        transaction_id: OutputTransactionId,
    ) -> Result<(), KmsWorkerAckError> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(inflight) = state.inflight else {
            if matches!(
                state.lifecycle,
                KmsWorkerLifecycle::ShutdownQuiescing
                    | KmsWorkerLifecycle::ShutdownAbandoning
                    | KmsWorkerLifecycle::Stopped
            ) {
                return Ok(());
            }
            self.shared
                .metrics
                .duplicate_pageflip_acks
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Err(KmsWorkerAckError::NoInFlightCommit);
        };
        if inflight.token != identity.token {
            self.shared
                .metrics
                .result_mismatches
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Err(KmsWorkerAckError::TokenMismatch);
        }
        if inflight.transaction_id != transaction_id {
            self.shared
                .metrics
                .result_mismatches
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Err(KmsWorkerAckError::TransactionMismatch);
        }
        if inflight.output_generation != identity.output_generation {
            let predecessor = inflight.bundle;
            drop(state);
            invalidate_queued_dependents(
                &self.shared,
                predecessor,
                ValidationBaseInvalidationReason::GenerationChanged,
            );
            self.shared
                .metrics
                .result_mismatches
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Err(KmsWorkerAckError::GenerationMismatch);
        }
        if inflight.bundle.id != identity.id
            || inflight.bundle.primary_transaction_id != identity.primary_transaction_id
            || inflight.bundle.cursor_transaction_id != identity.cursor_transaction_id
        {
            let predecessor = inflight.bundle;
            drop(state);
            invalidate_queued_dependents(
                &self.shared,
                predecessor,
                ValidationBaseInvalidationReason::BundleMismatch,
            );
            self.shared
                .metrics
                .result_mismatches
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Err(KmsWorkerAckError::BundleMismatch);
        }
        if inflight.bundle.crtc_id != identity.crtc_id {
            let predecessor = inflight.bundle;
            drop(state);
            invalidate_queued_dependents(
                &self.shared,
                predecessor,
                ValidationBaseInvalidationReason::BundleMismatch,
            );
            self.shared
                .metrics
                .result_mismatches
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Err(KmsWorkerAckError::CrtcMismatch);
        }
        if matches!(inflight.kind, AtomicCommitKind::PlaneDelta { .. }) {
            self.shared
                .metrics
                .cursor_pageflip_acks
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        } else {
            self.shared
                .metrics
                .primary_pageflip_acks
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let returned_sidecar = state
            .cursor_sidecar
            .take_must_bundle_with(inflight.transaction_id);
        state.established_base = Some(EstablishedKmsBase::Bundle(inflight.bundle));
        let suppress_next_submit = matches!(
            state.lifecycle,
            KmsWorkerLifecycle::Quiescing
                | KmsWorkerLifecycle::ShutdownQuiescing
                | KmsWorkerLifecycle::ShutdownAbandoning
        );
        state.inflight = None;
        state.phase = KmsWorkerPhase::Idle;
        state.executing_primary = None;
        drop(state);
        if let Some(sidecar) = returned_sidecar
            && !publish_event(
                &self.shared,
                KmsWorkerEvent::CursorSidecarReturned {
                    sidecar,
                    reason: CursorSidecarReturnReason::RequiredPrimaryTerminal,
                },
            )
        {
            return Err(KmsWorkerAckError::NoInFlightCommit);
        }
        if suppress_next_submit {
            self.shared
                .metrics
                .shutdown_ack_suppressed_next_submit
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.shared.work_wakeup.notify_all();
        } else {
            self.shared.work_wakeup.notify_one();
        }
        Ok(())
    }
}
