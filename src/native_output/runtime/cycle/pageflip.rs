use super::super::cursor_cycle::{complete_plane_delta_pageflip, complete_primary_cursor_pageflip};
use super::super::kms_worker::drop_queued_worker_job_with_reason_parts;
use super::super::presentation_cursor::trace_presented_cursor;
use super::super::presentation_transactions::{
    commit_prepared_presented_output_transaction, complete_presentation_feedback_obligation,
    complete_presented_output_transaction, discard_presentation_feedback_obligation,
    prepare_presented_output_transaction, restore_presentation_feedback_obligation,
    settle_dropped_output_transaction,
};
use super::super::presentation_worker::promote_pageflip_and_publish;
use super::super::*;
use super::cycle_direct;
use crate::native_output::kms_worker::{
    KmsCommitJob, KmsCommitWorkerHandle, KmsCursorUpdate, KmsPrimaryCursorPresentation,
    KmsPrimaryUpdate, KmsWorkerDispatchTailObservation, KmsWorkerQueuedCancellation,
};
use crate::native_output::pacing::WorkerPacingTicket;
use crate::native_output::presentation::kms_timing::{KmsPresentationOutcome, KmsSubmitWindow};
use crate::native_output::presentation::plane::{
    CursorCoupling, CursorRevision, PresentedCursorState,
};
use crate::native_output::scanout::{
    AtomicOutputSwapchain, FrozenCursorPlaneOwner, OutputFrameIdentitySnapshot,
};
use oblivion_one::native::adaptive_buffering::{
    AdaptiveRenderJournal, EstimatorRecoveryDisposition, FenceTimestampQuality, ProvenDeadlineMiss,
};
use oblivion_one::native::presentation_deadline::PresentationTarget;

fn is_non_primary_atomic_completion(completion: AtomicCommitCompletion) -> bool {
    matches!(
        completion,
        AtomicCommitCompletion::Completed { kind, .. } if !kind.is_primary()
    )
}

fn confirmed_presentation_for_pageflip(
    transaction: &OutputTransaction,
    output_generation: u64,
    successful_matching_pageflip: bool,
) -> Option<ConfirmedKmsPresentationState> {
    (successful_matching_pageflip && transaction.output_generation() == output_generation).then(
        || ConfirmedKmsPresentationState {
            mode: transaction.presentation_mode(),
            content_type: transaction.content_type(),
            output_generation,
        },
    )
}

fn confirm_kms_presentation_state(
    confirmed: &mut ConfirmedKmsPresentationState,
    transaction: &OutputTransaction,
    output_generation: u64,
    successful_matching_pageflip: bool,
) -> bool {
    let Some(next) = confirmed_presentation_for_pageflip(
        transaction,
        output_generation,
        successful_matching_pageflip,
    ) else {
        return false;
    };
    *confirmed = next;
    true
}

fn pageflip_identity(
    output_id: OutputId,
    token: PageFlipToken,
    output_epoch: OutputEpochId,
    crtc_id: u32,
) -> crate::native_output::presentation::plane::PlanePageflipIdentity {
    crate::native_output::presentation::plane::PlanePageflipIdentity {
        output_id,
        bundle_id:
            crate::native_output::presentation::plane::KmsCommitBundleId::from_pageflip_token(token),
        token,
        output_epoch,
        crtc_id,
    }
}

const DRM_TIMESTAMP_QUANTIZATION_NS: u64 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageflipTimestampUncertainty {
    BoundedNs(u64),
    Unbounded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PageflipTimestampEstimate {
    presented_at: MonotonicTimestampNs,
    uncertainty: PageflipTimestampUncertainty,
}

fn estimate_pageflip_timestamp(
    clock: oblivion_one::native::drm::DrmTimestampClock,
    seconds: u32,
    microseconds: u32,
    drm_sampled_at_us: u64,
    monotonic_before_sample_ns: u64,
    monotonic_after_sample_ns: u64,
    compositor_receive_ns: u64,
) -> PageflipTimestampEstimate {
    let event_timestamp_us = u64::from(seconds)
        .saturating_mul(1_000_000)
        .saturating_add(u64::from(microseconds));
    let sample_delay_ns = drm_sampled_at_us
        .saturating_sub(event_timestamp_us)
        .saturating_mul(DRM_TIMESTAMP_QUANTIZATION_NS);
    let receive_based_estimate =
        MonotonicTimestampNs::new(compositor_receive_ns.saturating_sub(sample_delay_ns));

    if clock == oblivion_one::native::drm::DrmTimestampClock::Realtime
        || drm_sampled_at_us < event_timestamp_us
        || monotonic_after_sample_ns < monotonic_before_sample_ns
    {
        return PageflipTimestampEstimate {
            presented_at: receive_based_estimate,
            uncertainty: PageflipTimestampUncertainty::Unbounded,
        };
    }

    // DRM pageflip timestamps and the sampled DRM clock both have microsecond
    // resolution. Bracket the current CLOCK_MONOTONIC sample to bound the
    // clock-domain mapping, then account for at most 999ns of relative
    // truncation between the two microsecond values.
    let bracket_ns = monotonic_after_sample_ns - monotonic_before_sample_ns;
    let half_bracket_ns = bracket_ns / 2 + bracket_ns % 2;
    let uncertainty_ns =
        half_bracket_ns.saturating_add(DRM_TIMESTAMP_QUANTIZATION_NS.saturating_sub(1));
    let sample_midpoint_ns = monotonic_before_sample_ns.saturating_add(bracket_ns / 2);
    PageflipTimestampEstimate {
        presented_at: MonotonicTimestampNs::new(sample_midpoint_ns.saturating_sub(sample_delay_ns)),
        uncertainty: PageflipTimestampUncertainty::BoundedNs(uncertainty_ns),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VariablePhysicalTimeViolation {
    MissingVrrWindow,
    TimestampUncertaintyUnavailable {
        actual_ns: u64,
        required_lower_bound_ns: u64,
    },
    EarlierThanLowerBound {
        actual_ns: u64,
        required_lower_bound_ns: u64,
        uncertainty_ns: u64,
    },
}

fn variable_presentation_lower_bound(
    reservation: crate::native_output::presentation::transaction::FramePresentationReservation,
) -> Option<MonotonicTimestampNs> {
    let crate::native_output::presentation::transaction::FramePresentationReservation::Variable(
        variable,
    ) = reservation
    else {
        return None;
    };
    let oblivion_one::native::presentation_timing::PresentationTiming::VrrWindow {
        earliest_present,
        presentation_not_before,
        ..
    } = variable.timing()
    else {
        return None;
    };
    Some(
        presentation_not_before
            .unwrap_or(earliest_present)
            .max(earliest_present),
    )
}

fn validate_variable_physical_time(
    reservation: crate::native_output::presentation::transaction::FramePresentationReservation,
    timestamp: PageflipTimestampEstimate,
) -> Result<(), VariablePhysicalTimeViolation> {
    let Some(required_lower_bound) = variable_presentation_lower_bound(reservation) else {
        return Err(VariablePhysicalTimeViolation::MissingVrrWindow);
    };
    let actual_ns = timestamp.presented_at.get();
    let PageflipTimestampUncertainty::BoundedNs(uncertainty_ns) = timestamp.uncertainty else {
        return Err(
            VariablePhysicalTimeViolation::TimestampUncertaintyUnavailable {
                actual_ns,
                required_lower_bound_ns: required_lower_bound.get(),
            },
        );
    };
    if actual_ns.saturating_add(uncertainty_ns) < required_lower_bound.get() {
        return Err(VariablePhysicalTimeViolation::EarlierThanLowerBound {
            actual_ns,
            required_lower_bound_ns: required_lower_bound.get(),
            uncertainty_ns,
        });
    }
    Ok(())
}

fn record_variable_composited_pageflip(
    frame: &crate::native_output::scanout::PresentedOutputFrame,
    timestamp: PageflipTimestampEstimate,
    clock_generation: u64,
    opportunity_sequence: u64,
    transport: crate::native_output::kms_worker::KmsCommitWorkerTransport,
    render_journal: &mut AdaptiveRenderJournal,
    frame_pacing: &mut NativeFramePacing,
) -> io::Result<()> {
    let crate::native_output::presentation::transaction::FramePresentationReservation::Variable(
        reservation,
    ) = frame.reservation
    else {
        return Err(io::Error::other(
            "variable pageflip accounting received a fixed reservation",
        ));
    };
    validate_variable_physical_time(frame.reservation, timestamp).map_err(|violation| {
        io::Error::other(format!(
            "invalid Variable physical pageflip evidence: {violation:?}"
        ))
    })?;
    let presented_at = timestamp.presented_at;
    let oblivion_one::native::presentation_timing::PresentationTiming::VrrWindow {
        anchor,
        earliest_present,
        preferred_present,
        ..
    } = reservation.timing()
    else {
        return Err(io::Error::other(
            "variable pageflip reservation has no VRR window",
        ));
    };
    let opportunity_id = reservation.opportunity().id();
    if opportunity_id.clock_generation() != clock_generation
        || opportunity_id.sequence() != opportunity_sequence
    {
        return Err(io::Error::other(
            "variable pageflip telemetry opportunity identity mismatch",
        ));
    }

    record_composited_service_costs(frame, transport, render_journal, frame_pacing);

    if frame_pacing.trace_enabled() {
        let preferred_lateness =
            i64::try_from(presented_at.get().abs_diff(preferred_present.get())).unwrap_or(i64::MAX);
        let preferred_lateness = if presented_at >= preferred_present {
            preferred_lateness
        } else {
            -preferred_lateness
        };
        frame_pacing.log(
            "vrr_physical_present",
            vec![
                PacingField::u64("frame_id", frame.frame_id),
                PacingField::u64("transaction_id", frame.transaction_id.get()),
                PacingField::u64("clock_generation", clock_generation),
                PacingField::u64("opportunity_sequence", opportunity_sequence),
                PacingField::u64("anchor_ns", anchor.get()),
                PacingField::u64("earliest_present_ns", earliest_present.get()),
                PacingField::u64("preferred_present_ns", preferred_present.get()),
                PacingField::u64("actual_pageflip_ns", presented_at.get()),
                PacingField::u64(
                    "actual_interval_ns",
                    presented_at.get().saturating_sub(anchor.get()),
                ),
                PacingField::i64("preferred_lateness_ns", preferred_lateness),
                PacingField::str(
                    "transport",
                    match transport {
                        crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker => {
                            "worker"
                        }
                        crate::native_output::kms_worker::KmsCommitWorkerTransport::Synchronous => {
                            "synchronous"
                        }
                    },
                ),
            ],
        );
    }
    Ok(())
}

fn record_composited_service_costs(
    frame: &crate::native_output::scanout::PresentedOutputFrame,
    transport: crate::native_output::kms_worker::KmsCommitWorkerTransport,
    render_journal: &mut AdaptiveRenderJournal,
    frame_pacing: &mut NativeFramePacing,
) {
    if transport == crate::native_output::kms_worker::KmsCommitWorkerTransport::Synchronous {
        render_journal.record_atomic_submit(
            frame
                .submit_returned_at
                .get()
                .saturating_sub(frame.submit_started_at.get()),
        );
    }
    if !frame.fence_timing_accounted {
        if let Some((signaled_at, quality)) = frame.fence_signal {
            frame_pacing.note_fence_timestamp_quality(quality);
            render_journal.record_render_sample(
                render_sample_duration_ns(frame.composite_started_at, signaled_at),
                signaled_at,
            );
        } else {
            render_journal.record_render_sample(
                render_sample_duration_ns(frame.composite_started_at, frame.rendered_at),
                frame.rendered_at,
            );
        }
    }
}

fn prepare_variable_physical_claim(
    deadline: &oblivion_one::native::presentation_deadline::PresentationDeadlinePlanner,
    variable: crate::native_output::presentation::transaction::VariablePresentationReservation,
    presented_at: MonotonicTimestampNs,
) -> io::Result<oblivion_one::native::presentation_deadline::PreparedPhysicalPresentation> {
    let prepared = deadline
        .prepare_variable_presented(presented_at)
        .ok_or_else(|| io::Error::other("variable physical opportunity sequence is exhausted"))?;
    if prepared.claim().opportunity_id() != variable.opportunity().id() {
        return Err(io::Error::other(
            "variable pageflip physical opportunity does not match its reservation",
        ));
    }
    Ok(prepared)
}

fn prepare_physical_presentation_for_reservation(
    deadline: &oblivion_one::native::presentation_deadline::PresentationDeadlinePlanner,
    reservation: crate::native_output::presentation::transaction::FramePresentationReservation,
    timestamp: PageflipTimestampEstimate,
) -> io::Result<oblivion_one::native::presentation_deadline::PreparedPhysicalPresentation> {
    match reservation {
        crate::native_output::presentation::transaction::FramePresentationReservation::Fixed(_) => {
            Ok(deadline.prepare_presented(timestamp.presented_at))
        }
        crate::native_output::presentation::transaction::FramePresentationReservation::Variable(
            variable,
        ) => {
            validate_variable_physical_time(
                crate::native_output::presentation::transaction::FramePresentationReservation::Variable(
                    variable,
                ),
                timestamp,
            )
            .map_err(|violation| {
                io::Error::other(format!(
                    "invalid Variable physical pageflip evidence: {violation:?}"
                ))
            })?;
            prepare_variable_physical_claim(deadline, variable, timestamp.presented_at)
        }
        crate::native_output::presentation::transaction::FramePresentationReservation::DeferredO1(
            _,
        ) => Err(io::Error::other(
            "deferred O1 reservation reached a physical pageflip",
        )),
    }
}

fn classify_pending_fence_deadline_miss(
    reservation: crate::native_output::presentation::transaction::FramePresentationReservation,
    presentation_timing: crate::native_output::presentation::kms_timing::KmsPresentationTiming,
    quality: FenceTimestampQuality,
    signaled_at: MonotonicTimestampNs,
    render_journal: &AdaptiveRenderJournal,
    apply_guard_ns: u64,
) -> io::Result<Option<ProvenDeadlineMiss>> {
    match (reservation, presentation_timing) {
        (
            crate::native_output::presentation::transaction::FramePresentationReservation::Fixed(
                target,
            ),
            crate::native_output::presentation::kms_timing::KmsPresentationTiming::Fixed(
                submit_window,
            ),
        ) => {
            let before =
                render_journal.prediction_with_kms_guard(target.refresh_interval, apply_guard_ns);
            let commit_deadline_ns = submit_window.commit_complete_deadline_ns();
            Ok(match quality {
                FenceTimestampQuality::ExactSyncFile if signaled_at.get() > commit_deadline_ns => {
                    Some(ProvenDeadlineMiss::ExactRender)
                }
                FenceTimestampQuality::ObservedApproximate
                    if approximate_observation_is_late(
                        signaled_at.get(),
                        commit_deadline_ns,
                        before.p95_wake_lateness_ns,
                    ) =>
                {
                    Some(ProvenDeadlineMiss::GuardedApproximateRender)
                }
                _ => None,
            })
        }
        (
            crate::native_output::presentation::transaction::FramePresentationReservation::Variable(
                _,
            ),
            crate::native_output::presentation::kms_timing::KmsPresentationTiming::Variable {
                ..
            },
        ) => Ok(None),
        _ => Err(io::Error::other(
            "pending render fence reservation and KMS timing domains differ",
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdvisoryOpportunitySlip {
    KmsDispatch,
}

impl AdvisoryOpportunitySlip {
    const fn as_str(self) -> &'static str {
        match self {
            Self::KmsDispatch => "kms_dispatch",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PresentationDeadlineAssessment {
    None,
    Proven(ProvenDeadlineMiss),
    Advisory(AdvisoryOpportunitySlip),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderReadinessSource {
    ExactSyncFile,
    ObservedApproximate,
    RenderedAtFallback,
}

impl RenderReadinessSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ExactSyncFile => "exact_sync_file",
            Self::ObservedApproximate => "guarded_approximate",
            Self::RenderedAtFallback => "rendered_at_fallback",
        }
    }

    const fn fence_timestamp_quality_str(self) -> &'static str {
        match self {
            Self::ExactSyncFile => "exact_sync_file",
            Self::ObservedApproximate => "observed_approximate",
            Self::RenderedAtFallback => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RenderReadinessObservation {
    // For RenderedAtFallback this is a compositor-side lower bound, not GPU
    // payload-completion proof.
    observed_at: MonotonicTimestampNs,
    source: RenderReadinessSource,
}

impl RenderReadinessObservation {
    const fn from_frame(
        fence_signal: Option<(MonotonicTimestampNs, FenceTimestampQuality)>,
        rendered_at: MonotonicTimestampNs,
    ) -> Self {
        match fence_signal {
            Some((observed_at, FenceTimestampQuality::ExactSyncFile)) => Self {
                observed_at,
                source: RenderReadinessSource::ExactSyncFile,
            },
            Some((observed_at, FenceTimestampQuality::ObservedApproximate)) => Self {
                observed_at,
                source: RenderReadinessSource::ObservedApproximate,
            },
            None => Self {
                observed_at: rendered_at,
                source: RenderReadinessSource::RenderedAtFallback,
            },
        }
    }

    const fn observed_at(self) -> MonotonicTimestampNs {
        self.observed_at
    }

    const fn source(self) -> RenderReadinessSource {
        self.source
    }

    fn fence_ready_at_ns(self) -> Option<u64> {
        match self.source {
            RenderReadinessSource::ExactSyncFile | RenderReadinessSource::ObservedApproximate => {
                Some(self.observed_at.get())
            }
            RenderReadinessSource::RenderedAtFallback => None,
        }
    }

    fn fence_lateness_ns(self, deadline_ns: u64) -> Option<u64> {
        self.fence_ready_at_ns()
            .map(|observed_at| observed_at.saturating_sub(deadline_ns))
    }

    fn observation_lateness_ns(self, deadline_ns: u64) -> u64 {
        self.observed_at.get().saturating_sub(deadline_ns)
    }
}

fn render_readiness_diagnostic_fields(
    readiness: RenderReadinessObservation,
    submit_window: &KmsSubmitWindow,
    composite_started_at: MonotonicTimestampNs,
    rendered_at: MonotonicTimestampNs,
) -> Vec<PacingField> {
    let deadline_ns = submit_window.commit_complete_deadline_ns();
    vec![
        PacingField::str("render_readiness_source", readiness.source().as_str()),
        PacingField::str(
            "fence_timestamp_quality",
            readiness.source().fence_timestamp_quality_str(),
        ),
        PacingField::option_u64("payload_ready_at_ns", readiness.fence_ready_at_ns()),
        PacingField::u64("commit_complete_deadline_ns", deadline_ns),
        PacingField::option_u64(
            "render_readiness_lateness_ns",
            readiness.fence_lateness_ns(deadline_ns),
        ),
        PacingField::u64(
            "render_readiness_observed_at_ns",
            readiness.observed_at().get(),
        ),
        PacingField::u64(
            "render_readiness_observation_lateness_ns",
            readiness.observation_lateness_ns(deadline_ns),
        ),
        PacingField::u64("composite_started_ns", composite_started_at.get()),
        PacingField::u64("rendered_at_ns", rendered_at.get()),
    ]
}

fn assess_presentation_deadline(
    target: PresentationTarget,
    outcome: KmsPresentationOutcome,
    readiness: RenderReadinessObservation,
) -> PresentationDeadlineAssessment {
    match outcome {
        KmsPresentationOutcome::TargetHit => PresentationDeadlineAssessment::None,
        KmsPresentationOutcome::RenderReadinessMiss => {
            PresentationDeadlineAssessment::Proven(match readiness.source() {
                RenderReadinessSource::ObservedApproximate => {
                    ProvenDeadlineMiss::GuardedApproximateRender
                }
                RenderReadinessSource::ExactSyncFile
                | RenderReadinessSource::RenderedAtFallback => {
                    // `rendered_at` is not GPU payload proof, but once the
                    // physical classifier has established this miss it is an
                    // exact lower-bound proof that compositor render/fence
                    // export completion was late.
                    ProvenDeadlineMiss::ExactRender
                }
            })
        }
        KmsPresentationOutcome::KmsDispatchMiss => {
            if target.is_binding() {
                PresentationDeadlineAssessment::Proven(ProvenDeadlineMiss::KmsDispatch)
            } else {
                PresentationDeadlineAssessment::Advisory(AdvisoryOpportunitySlip::KmsDispatch)
            }
        }
        KmsPresentationOutcome::KmsApplyGuardMiss => {
            PresentationDeadlineAssessment::Proven(ProvenDeadlineMiss::KmsApplyGuard)
        }
    }
}

fn resolve_presentation_deadline_assessment(
    pending_miss: Option<ProvenDeadlineMiss>,
    target: PresentationTarget,
    outcome: Option<KmsPresentationOutcome>,
    readiness: RenderReadinessObservation,
) -> PresentationDeadlineAssessment {
    if let Some(miss) = pending_miss {
        return PresentationDeadlineAssessment::Proven(miss);
    }
    outcome.map_or(PresentationDeadlineAssessment::None, |outcome| {
        assess_presentation_deadline(target, outcome, readiness)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProvenDeadlineRecovery {
    miss: ProvenDeadlineMiss,
    disposition: EstimatorRecoveryDisposition,
    recovery_remaining_before: usize,
    recovery_remaining_after: usize,
}

fn apply_deadline_assessment(
    assessment: PresentationDeadlineAssessment,
    render_journal: &mut AdaptiveRenderJournal,
    refresh_interval: Duration,
    dispatch_tail: Option<KmsWorkerDispatchTailObservation>,
    apply_guard: Option<KmsPresentationTimingObservation>,
) -> Option<ProvenDeadlineRecovery> {
    let PresentationDeadlineAssessment::Proven(miss) = assessment else {
        return None;
    };
    let disposition = recovery_disposition(miss, dispatch_tail, apply_guard);
    let recovery_remaining_before = render_journal
        .prediction(refresh_interval)
        .miss_recovery_remaining;
    render_journal.note_proven_deadline_miss(disposition);
    let recovery_remaining_after = render_journal
        .prediction(refresh_interval)
        .miss_recovery_remaining;
    Some(ProvenDeadlineRecovery {
        miss,
        disposition,
        recovery_remaining_before,
        recovery_remaining_after,
    })
}

fn target_reason_name(reason: PresentationTargetReason) -> &'static str {
    match reason {
        PresentationTargetReason::ReactiveDouble => "reactive_double",
        PresentationTargetReason::Normal => "normal",
        PresentationTargetReason::PredictedPressure => "predicted_pressure",
        PresentationTargetReason::ProvenReadinessMiss => "proven_readiness_miss",
        PresentationTargetReason::ForcedValidation => "forced_validation",
        PresentationTargetReason::CommitTiming => "commit_timing",
    }
}

fn recovery_disposition(
    miss: ProvenDeadlineMiss,
    dispatch_tail: Option<KmsWorkerDispatchTailObservation>,
    apply_guard: Option<KmsPresentationTimingObservation>,
) -> EstimatorRecoveryDisposition {
    match miss {
        ProvenDeadlineMiss::ExactRender | ProvenDeadlineMiss::GuardedApproximateRender => {
            EstimatorRecoveryDisposition::ResetIndependentHorizon
        }
        ProvenDeadlineMiss::KmsDispatch => {
            let absorbed = dispatch_tail.is_some_and(|observation| {
                observation.binding_target
                    && observation.fair_dispatch_chance
                    && observation.deadline_overrun_ns > 0
                    && observation.increased
                    && !observation.cap_hit
            });
            if absorbed {
                EstimatorRecoveryDisposition::PreserveEstimatorState
            } else {
                EstimatorRecoveryDisposition::ResetIndependentHorizon
            }
        }
        ProvenDeadlineMiss::KmsApplyGuard => {
            let absorbed = apply_guard.is_some_and(|observation| {
                observation.accepted
                    && observation.apply_guard_increased
                    && !observation.apply_guard_cap_hit
            });
            if absorbed {
                EstimatorRecoveryDisposition::PreserveEstimatorState
            } else {
                EstimatorRecoveryDisposition::ResetIndependentHorizon
            }
        }
    }
}

fn dispatch_budget_diagnostic_fields(
    used_ns: Option<u64>,
    after_adaptation_ns: Option<u64>,
) -> [PacingField; 2] {
    [
        PacingField::option_u64("dispatch_budget_used_ns", used_ns),
        PacingField::option_u64("dispatch_budget_after_adaptation_ns", after_adaptation_ns),
    ]
}

fn presented_primary_from_worker_job(
    job: &crate::native_output::kms_worker::KmsCommitJob,
    swapchain: Option<&AtomicOutputSwapchain>,
) -> Option<PresentedPrimaryAssignment> {
    let transaction = job.owners.primary()?.transaction.as_ref();
    let bundle_identity = job.identity();
    let pageflip = crate::native_output::presentation::plane::PlanePageflipIdentity {
        output_id: bundle_identity.output_id,
        bundle_id: bundle_identity.id,
        token: bundle_identity.token,
        output_epoch: OutputEpochId::new(bundle_identity.output_generation)
            .expect("KMS pageflip output epoch is nonzero"),
        crtc_id: bundle_identity.crtc_id,
    };
    match job.kind {
        AtomicCommitKind::CompositedPrimary { .. } => match transaction.planes().primary() {
            PrimaryPlaneAssignment::CompositorFramebuffer {
                slot,
                framebuffer_id,
            } => {
                let swapchain = swapchain?;
                if slot != swapchain.current()
                    || swapchain.current_framebuffer_id().map(|id| id.get()) != Some(framebuffer_id)
                {
                    return None;
                }
                Some(PresentedPrimaryAssignment::Composed {
                    transaction_id: job.transaction_id,
                    token: job.token,
                    pageflip,
                    slot,
                    framebuffer_id,
                    pool_generation: swapchain.pool_generation(),
                    presentation_serial: swapchain.presentation_serial(),
                })
            }
            PrimaryPlaneAssignment::CompatibilityFramebuffer { .. }
            | PrimaryPlaneAssignment::ClientFramebuffer { .. }
            | PrimaryPlaneAssignment::Unchanged
            | PrimaryPlaneAssignment::Disabled => None,
        },
        AtomicCommitKind::DirectPrimary { .. } => {
            let OutputTransactionContent::Direct { key, .. } = transaction.content() else {
                return None;
            };
            let PrimaryPlaneAssignment::ClientFramebuffer { framebuffer_id, .. } =
                transaction.planes().primary()
            else {
                return None;
            };
            Some(PresentedPrimaryAssignment::Direct {
                transaction_id: job.transaction_id,
                token: job.token,
                pageflip,
                surface_id: transaction.obligations().direct_surface_id()?,
                key,
                framebuffer_id,
            })
        }
        AtomicCommitKind::PlaneDelta { .. } => None,
    }
}

fn presented_cursor_from_worker_update(
    update: &KmsCursorUpdate,
    revision: CursorRevision,
    coupling: CursorCoupling,
    delivery: crate::native_output::presentation::plane::PresentedCursorDelivery,
    fallback: &PresentedCursorState,
) -> Option<PresentedCursorState> {
    let state = match update {
        KmsCursorUpdate::Set(state) => state.clone(),
        KmsCursorUpdate::Disable => {
            let mut hidden = *fallback;
            hidden.revision = revision;
            hidden.coupling = coupling;
            hidden.delivery = delivery;
            hidden.visible = false;
            hidden.framebuffer_id = None;
            return Some(hidden);
        }
        KmsCursorUpdate::Unchanged => {
            let mut preserved = *fallback;
            preserved.revision = revision;
            preserved.coupling = coupling;
            preserved.delivery = delivery;
            if delivery
                != crate::native_output::presentation::plane::PresentedCursorDelivery::Hardware
            {
                preserved.visible = false;
                preserved.framebuffer_id = None;
            }
            return Some(preserved);
        }
    };
    Some(PresentedCursorState::from_atomic_with_delivery(
        revision, coupling, delivery, &state,
    ))
}

fn frozen_primary_cursor_presentation(
    presentation: KmsPrimaryCursorPresentation,
) -> Option<PresentedCursorState> {
    match presentation {
        KmsPrimaryCursorPresentation::Preserve => None,
        KmsPrimaryCursorPresentation::Promote(state) => Some(state),
    }
}

fn select_cursor_promotion(
    primary_presentation: KmsPrimaryCursorPresentation,
    cursor_plane_promotion: Option<PresentedCursorState>,
) -> Option<PresentedCursorState> {
    match primary_presentation {
        KmsPrimaryCursorPresentation::Promote(state) => Some(state),
        KmsPrimaryCursorPresentation::Preserve => cursor_plane_promotion,
    }
}

#[derive(Debug)]
enum OutputFrameRetirementInvariantError {
    PhysicalReadyIdentityChanged,
    SuspendedPhysicalIdentityMissing,
    PacingOwnerMissing,
    SceneReadyIdentityMismatch,
    SchedulerReadyOwnershipMismatch,
    OutputTransactionMissing,
    OutputTransactionIdentityMismatch,
    OutputTransactionNotUnsubmitted,
    ProtocolObligationMismatch,
    PhysicalReadyCannotBeSuspended,
}

impl std::fmt::Display for OutputFrameRetirementInvariantError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PhysicalReadyIdentityChanged => {
                formatter.write_str("unsubmitted READY identity changed before terminalization")
            }
            Self::SuspendedPhysicalIdentityMissing => {
                formatter.write_str("Variable retry has no matching suspended physical frame")
            }
            Self::PacingOwnerMissing => formatter.write_str("overtaken READY has no pacing owner"),
            Self::SceneReadyIdentityMismatch => {
                formatter.write_str("unsubmitted READY has no matching scene-history READY owner")
            }
            Self::SchedulerReadyOwnershipMismatch => {
                formatter.write_str("unsubmitted READY disagrees with scheduler READY ownership")
            }
            Self::OutputTransactionMissing => {
                formatter.write_str("unsubmitted READY has no active output transaction")
            }
            Self::OutputTransactionIdentityMismatch => {
                formatter.write_str("unsubmitted READY disagrees with its output transaction")
            }
            Self::OutputTransactionNotUnsubmitted => {
                formatter.write_str("unsubmitted READY transaction is already physically owned")
            }
            Self::ProtocolObligationMismatch => {
                formatter.write_str("unsubmitted READY transaction owns a different frame batch")
            }
            Self::PhysicalReadyCannotBeSuspended => {
                formatter.write_str("unsubmitted READY could not be moved to suspended ownership")
            }
        }
    }
}

impl std::error::Error for OutputFrameRetirementInvariantError {}

#[derive(Debug, Clone, Copy)]
struct ValidatedUnsubmittedFrameOwners {
    owner: OutputFrameIdentitySnapshot,
    pacing_lane: crate::native_output::pacing::UnsubmittedPhysicalFrameLane,
}

fn validate_unsubmitted_output_frame_owners(
    owner: OutputFrameIdentitySnapshot,
    output_epoch: OutputEpochId,
    scene_history: &NativeSceneHistory,
    frame_pacing: &NativeFramePacing,
    frame_scheduler: &NativeFrameScheduler,
    output_transactions: &OutputTransactionLedger,
) -> NativeResult<ValidatedUnsubmittedFrameOwners> {
    if !frame_pacing.validate_physical_ready_owner(owner) {
        return Err(OutputFrameRetirementInvariantError::PacingOwnerMissing.into());
    }
    let pacing_lane = frame_pacing
        .unsubmitted_physical_frame_lane(owner)
        .ok_or(OutputFrameRetirementInvariantError::PacingOwnerMissing)?;
    if matches!(
        pacing_lane,
        crate::native_output::pacing::UnsubmittedPhysicalFrameLane::Ready
    ) && !frame_scheduler.ready_frame_queued()
    {
        return Err(OutputFrameRetirementInvariantError::SchedulerReadyOwnershipMismatch.into());
    }
    if scene_history.ready_frame_identity() != Some((owner.frame_id, owner.render_generation)) {
        return Err(OutputFrameRetirementInvariantError::SceneReadyIdentityMismatch.into());
    }
    let transaction = output_transactions
        .transaction(owner.transaction_id)
        .ok_or(OutputFrameRetirementInvariantError::OutputTransactionMissing)?;
    let descriptor = transaction.descriptor();
    if descriptor.id() != owner.transaction_id
        || descriptor.output_id() != owner.output_id
        || descriptor.output_generation() != output_epoch.get()
        || descriptor.composited_frame_identity()
            != Some((
                owner.frame_id,
                owner.render_generation,
                owner.pool_generation,
            ))
    {
        return Err(OutputFrameRetirementInvariantError::OutputTransactionIdentityMismatch.into());
    }
    if !matches!(
        transaction.state().kind(),
        OutputTransactionStateKind::Built
            | OutputTransactionStateKind::Ready
            | OutputTransactionStateKind::ReadyUnbound
    ) {
        return Err(OutputFrameRetirementInvariantError::OutputTransactionNotUnsubmitted.into());
    }
    if descriptor.obligations().frame_batch_id() != Some(owner.protocol_batch_id) {
        return Err(OutputFrameRetirementInvariantError::ProtocolObligationMismatch.into());
    }
    let pacing_lane = match pacing_lane {
        crate::native_output::pacing::UnsubmittedPhysicalFrameLane::Untracked => {
            if frame_scheduler.ready_frame_queued() {
                crate::native_output::pacing::UnsubmittedPhysicalFrameLane::Ready
            } else {
                crate::native_output::pacing::UnsubmittedPhysicalFrameLane::Active
            }
        }
        lane => lane,
    };
    Ok(ValidatedUnsubmittedFrameOwners { owner, pacing_lane })
}

fn validate_unsubmitted_ready_owners(
    swapchain: &crate::native_output::scanout::AtomicOutputSwapchain,
    owner: OutputFrameIdentitySnapshot,
    output_epoch: OutputEpochId,
    scene_history: &NativeSceneHistory,
    frame_pacing: &NativeFramePacing,
    frame_scheduler: &NativeFrameScheduler,
    output_transactions: &OutputTransactionLedger,
) -> NativeResult<ValidatedUnsubmittedFrameOwners> {
    if swapchain.ready_identity() != Some(owner) {
        return Err(OutputFrameRetirementInvariantError::PhysicalReadyIdentityChanged.into());
    }
    swapchain
        .validate_suspend_abandon_ready()
        .map_err(|_| OutputFrameRetirementInvariantError::PhysicalReadyCannotBeSuspended)?;
    validate_unsubmitted_output_frame_owners(
        owner,
        output_epoch,
        scene_history,
        frame_pacing,
        frame_scheduler,
        output_transactions,
    )
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ExplicitWorkerReplanRequest {
    pub(crate) output_id: OutputId,
    pub(crate) output_epoch: OutputEpochId,
    pub(crate) crtc_id: u32,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) token: PageFlipToken,
    pub(crate) kind: AtomicCommitKind,
    pub(crate) frame_id: u64,
    pub(crate) framebuffer_id: u32,
    pub(crate) reservation: FramePresentationReservation,
    pub(crate) queued_at: MonotonicTimestampNs,
    pub(crate) ready_submit: bool,
    pub(crate) pacing_ticket: Option<WorkerPacingTicket>,
    pub(crate) submission_fence_available: bool,
}

impl ExplicitWorkerReplanRequest {
    pub(crate) fn from_job(job: &KmsCommitJob) -> NativeResult<Self> {
        let AtomicCommitKind::CompositedPrimary {
            transaction_id,
            frame_id,
            framebuffer_id,
        } = job.kind
        else {
            return Err(io::Error::other(
                "explicit worker re-plan requires a composited primary job",
            )
            .into());
        };
        let KmsPrimaryUpdate::Framebuffer {
            framebuffer,
            in_fence,
            ..
        } = &job.primary
        else {
            return Err(io::Error::other(
                "explicit composited re-plan has no primary framebuffer payload",
            )
            .into());
        };
        if transaction_id != job.transaction_id
            || framebuffer.get() != framebuffer_id
            || job
                .pacing_ticket
                .is_some_and(|ticket| ticket.ready_submit() != job.ready_submit)
        {
            return Err(io::Error::other("explicit worker re-plan job identities disagree").into());
        }
        Ok(Self {
            output_id: job.output_id,
            output_epoch: job.output_epoch,
            crtc_id: job.crtc_id,
            transaction_id,
            token: job.token,
            kind: job.kind,
            frame_id,
            framebuffer_id,
            reservation: job.reservation,
            queued_at: job.queued_at,
            ready_submit: job.ready_submit,
            pacing_ticket: job.pacing_ticket,
            submission_fence_available: in_fence.is_some(),
        })
    }

    pub(crate) fn pacing_origin(self) -> &'static str {
        match self.pacing_ticket {
            Some(ticket) if ticket.ready_submit() => "ready",
            Some(_) => "active",
            None => "untracked",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ValidatedExplicitWorkerReplan {
    request: ExplicitWorkerReplanRequest,
    identity: OutputFrameIdentitySnapshot,
    ready_at: MonotonicTimestampNs,
}

impl ValidatedExplicitWorkerReplan {
    pub(crate) const fn frame_identity(self) -> OutputFrameIdentitySnapshot {
        self.identity
    }
}

pub(crate) fn preflight_explicit_worker_replan(
    swapchain: &AtomicOutputSwapchain,
    request: ExplicitWorkerReplanRequest,
    scene_history: &NativeSceneHistory,
    frame_pacing: &NativeFramePacing,
    frame_scheduler: &NativeFrameScheduler,
    atomic_commit_arbiter: &AtomicCommitArbiter,
    output_transactions: &OutputTransactionLedger,
) -> NativeResult<ValidatedExplicitWorkerReplan> {
    let queued = swapchain
        .worker_queued_identity()
        .ok_or_else(|| io::Error::other("explicit worker re-plan has no physical worker owner"))?;
    let identity = queued.frame;
    if queued.token != request.token
        || identity.output_id != request.output_id
        || identity.transaction_id != request.transaction_id
        || identity.frame_id != request.frame_id
        || identity.framebuffer_id.get() != request.framebuffer_id
        || identity.reservation != request.reservation
        || request.kind
            != (AtomicCommitKind::CompositedPrimary {
                transaction_id: request.transaction_id,
                frame_id: request.frame_id,
                framebuffer_id: request.framebuffer_id,
            })
    {
        return Err(io::Error::other(
            "explicit worker re-plan job does not match its physical owner",
        )
        .into());
    }
    swapchain.validate_worker_queued_for_replan(
        request.token,
        identity,
        request.submission_fence_available,
    )?;

    if atomic_commit_arbiter
        .worker_queued_commit()
        .is_none_or(|commit| {
            commit.token != request.token
                || commit.generation != request.output_epoch.get()
                || commit.crtc_id != request.crtc_id
                || commit.kind != request.kind
                || !matches!(
                    commit.phase,
                    AtomicCommitPhase::WorkerQueued { queued_at_ns }
                        if queued_at_ns == request.queued_at.get()
                )
        })
    {
        return Err(io::Error::other(
            "explicit worker re-plan job does not match Atomic arbiter ownership",
        )
        .into());
    }

    let record = output_transactions
        .transaction(request.transaction_id)
        .ok_or_else(|| io::Error::other("explicit worker re-plan transaction is missing"))?;
    let ready_at = match record.state() {
        OutputTransactionState::Queued {
            queued_at,
            worker_generation,
            predecessor:
                crate::native_output::presentation::ledger::OutputTransactionQueuedPredecessor::Ready {
                    ready_at,
                },
        } if queued_at == request.queued_at && worker_generation == request.output_epoch.get() => {
            ready_at
        }
        _ => {
            return Err(io::Error::other(
                "explicit worker re-plan transaction has no matching READY predecessor",
            )
            .into());
        }
    };
    let descriptor = record.descriptor();
    if descriptor.id() != identity.transaction_id
        || descriptor.output_id() != identity.output_id
        || descriptor.output_generation() != request.output_epoch.get()
        || descriptor.reservation() != identity.reservation
        || descriptor.composited_frame_identity()
            != Some((
                identity.frame_id,
                identity.render_generation,
                identity.pool_generation,
            ))
        || descriptor.planes().primary()
            != (PrimaryPlaneAssignment::CompositorFramebuffer {
                slot: identity.slot,
                framebuffer_id: request.framebuffer_id,
            })
        || descriptor.obligations().frame_batch_id() != Some(identity.protocol_batch_id)
    {
        return Err(io::Error::other(
            "explicit worker re-plan transaction identity does not match its physical frame",
        )
        .into());
    }
    scene_history
        .validate_submission_to_ready_rollback(
            request.token.get(),
            request.output_id,
            identity.frame_id,
            identity.render_generation,
        )
        .map_err(io::Error::other)?;
    frame_pacing
        .validate_optional_worker_submission_replan(
            request.pacing_ticket,
            identity,
            request.ready_submit,
        )
        .map_err(io::Error::other)?;
    if frame_scheduler.ready_frame_queued() {
        return Err(io::Error::other("explicit worker re-plan scheduler still owns READY").into());
    }

    Ok(ValidatedExplicitWorkerReplan {
        request,
        identity,
        ready_at,
    })
}

pub(crate) fn restore_explicit_worker_replan(
    validated: ValidatedExplicitWorkerReplan,
    swapchain: &mut AtomicOutputSwapchain,
    submission_fence: &mut Option<std::os::fd::OwnedFd>,
    cursor_owner: &mut Option<FrozenCursorPlaneOwner>,
    scene_history: &mut NativeSceneHistory,
    frame_pacing: &mut NativeFramePacing,
    frame_scheduler: &mut NativeFrameScheduler,
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    output_transactions: &mut OutputTransactionLedger,
    now_ns: u64,
) -> NativeResult<()> {
    let request = validated.request;
    let identity = validated.identity;
    if !swapchain.return_worker_queued_for_replan(request.token, submission_fence, cursor_owner)? {
        return Err(io::Error::other(
            "explicit worker re-plan could not return its physical frame",
        )
        .into());
    }
    if atomic_commit_arbiter
        .reject_worker_queued(request.token)
        .is_none()
    {
        return Err(
            io::Error::other("explicit worker re-plan lost Atomic arbiter ownership").into(),
        );
    }
    output_transactions
        .rollback_queued(request.transaction_id)
        .map_err(io::Error::other)?;
    scene_history
        .rollback_submission_to_ready(
            request.token.get(),
            request.output_id,
            identity.frame_id,
            identity.render_generation,
        )
        .map_err(io::Error::other)?;
    frame_pacing
        .replan_optional_worker_submission(
            request.pacing_ticket,
            identity,
            request.ready_submit,
            now_ns,
        )
        .map_err(io::Error::other)?;
    frame_scheduler.note_ready_frame(identity.reservation.fixed_target());

    if output_transactions
        .transaction(request.transaction_id)
        .is_none_or(|record| {
            record.state()
                != OutputTransactionState::Ready {
                    ready_at: validated.ready_at,
                }
        })
    {
        return Err(io::Error::other(
            "explicit worker re-plan did not restore the exact READY transaction predecessor",
        )
        .into());
    }
    validate_reconfiguration_frame_ownership_for_swapchain(
        swapchain,
        request.output_epoch,
        scene_history,
        frame_pacing,
        frame_scheduler,
        output_transactions,
    )
}

pub(crate) fn validate_reconfiguration_frame_ownership(
    explicit: &AtomicEglGbmScanout,
    output_epoch: OutputEpochId,
    scene_history: &NativeSceneHistory,
    frame_pacing: &NativeFramePacing,
    frame_scheduler: &NativeFrameScheduler,
    output_transactions: &OutputTransactionLedger,
) -> NativeResult<()> {
    let swapchain = explicit.swapchain()?;
    validate_reconfiguration_frame_ownership_for_swapchain(
        swapchain,
        output_epoch,
        scene_history,
        frame_pacing,
        frame_scheduler,
        output_transactions,
    )
}

pub(crate) fn validate_reconfiguration_frame_ownership_for_swapchain(
    swapchain: &AtomicOutputSwapchain,
    output_epoch: OutputEpochId,
    scene_history: &NativeSceneHistory,
    frame_pacing: &NativeFramePacing,
    frame_scheduler: &NativeFrameScheduler,
    output_transactions: &OutputTransactionLedger,
) -> NativeResult<()> {
    let physical_ready = swapchain.ready_identity();
    if let Some(owner) = physical_ready {
        validate_unsubmitted_ready_owners(
            swapchain,
            owner,
            output_epoch,
            scene_history,
            frame_pacing,
            frame_scheduler,
            output_transactions,
        )?;
        return Ok(());
    }
    if frame_pacing.ready.is_some()
        || frame_pacing.has_unbound_ready_owner()
        || scene_history.has_ready_frame()
        || frame_scheduler.ready_frame_queued()
    {
        return Err(OutputFrameRetirementInvariantError::PhysicalReadyIdentityChanged.into());
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn abandon_overtaken_ready(
    explicit: &mut AtomicEglGbmScanout,
    owner: OutputFrameIdentitySnapshot,
    output_epoch: OutputEpochId,
    scene_history: &mut NativeSceneHistory,
    frame_pacing: &mut NativeFramePacing,
    frame_scheduler: &mut NativeFrameScheduler,
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    presented_at: MonotonicTimestampNs,
) -> NativeResult<()> {
    let swapchain = explicit.swapchain_mut()?;
    terminalize_unsubmitted_ready_frame(
        swapchain,
        owner,
        output_epoch,
        scene_history,
        frame_pacing,
        frame_scheduler,
        output_transactions,
        presented_at,
        |obligations| {
            discard_presentation_feedback_obligation(server, obligations);
            debug_assert_eq!(obligations.frame_batch_id(), Some(owner.protocol_batch_id));
            server.complete_frame_batch_after_safe_abandonment(
                owner.protocol_batch_id,
                FrameBatchDiscardReason::SuspendAbandonment,
            );
            Ok(())
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn terminalize_unsubmitted_ready_frame<F>(
    swapchain: &mut crate::native_output::scanout::AtomicOutputSwapchain,
    owner: OutputFrameIdentitySnapshot,
    output_epoch: OutputEpochId,
    scene_history: &mut NativeSceneHistory,
    frame_pacing: &mut NativeFramePacing,
    frame_scheduler: &mut NativeFrameScheduler,
    output_transactions: &mut OutputTransactionLedger,
    retired_at: MonotonicTimestampNs,
    settle_protocol_obligations: F,
) -> NativeResult<()>
where
    F: FnOnce(
        crate::native_output::presentation::transaction::OutputProtocolObligations,
    ) -> NativeResult<()>,
{
    let validated = validate_unsubmitted_ready_owners(
        swapchain,
        owner,
        output_epoch,
        scene_history,
        frame_pacing,
        frame_scheduler,
        output_transactions,
    )?;
    terminalize_unsubmitted_output_frame_owners(
        validated,
        scene_history,
        frame_pacing,
        frame_scheduler,
        output_transactions,
        retired_at,
        settle_protocol_obligations,
    )?;
    if !swapchain.suspend_abandon_ready()? {
        return Err(OutputFrameRetirementInvariantError::PhysicalReadyCannotBeSuspended.into());
    }
    Ok(())
}

fn terminalize_unsubmitted_output_frame_owners<F>(
    validated: ValidatedUnsubmittedFrameOwners,
    scene_history: &mut NativeSceneHistory,
    frame_pacing: &mut NativeFramePacing,
    frame_scheduler: &mut NativeFrameScheduler,
    output_transactions: &mut OutputTransactionLedger,
    retired_at: MonotonicTimestampNs,
    settle_protocol_obligations: F,
) -> NativeResult<()>
where
    F: FnOnce(
        crate::native_output::presentation::transaction::OutputProtocolObligations,
    ) -> NativeResult<()>,
{
    let owner = validated.owner;
    settle_dropped_output_transaction(
        output_transactions,
        owner.transaction_id,
        OutputTransactionDropReason::SafeAbandonment,
        retired_at,
        settle_protocol_obligations,
    )?;
    if !scene_history.discard_ready_if_identity(owner.frame_id, owner.render_generation) {
        return Err(OutputFrameRetirementInvariantError::SceneReadyIdentityMismatch.into());
    }
    frame_pacing
        .terminalize_unsubmitted_physical_frame(owner)
        .map_err(|_| OutputFrameRetirementInvariantError::PacingOwnerMissing)?;
    if frame_scheduler.ready_frame_queued() {
        frame_scheduler.discard_ready_frame();
    }
    Ok(())
}

pub(crate) fn terminalize_suspended_frame_for_variable_retry(
    swapchain: &mut crate::native_output::scanout::AtomicOutputSwapchain,
    owner: OutputFrameIdentitySnapshot,
    output_epoch: OutputEpochId,
    scene_history: &mut NativeSceneHistory,
    frame_pacing: &mut NativeFramePacing,
    frame_scheduler: &mut NativeFrameScheduler,
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    retired_at: MonotonicTimestampNs,
    rejection: crate::native_output::scanout::VariableRetryRejection,
    perf: &NativePerfLogger,
) -> NativeResult<()> {
    if !swapchain.contains_suspended_frame(owner) {
        return Err(OutputFrameRetirementInvariantError::SuspendedPhysicalIdentityMissing.into());
    }
    let validated = validate_unsubmitted_output_frame_owners(
        owner,
        output_epoch,
        scene_history,
        frame_pacing,
        frame_scheduler,
        output_transactions,
    )?;
    let pacing_lane = match validated.pacing_lane {
        crate::native_output::pacing::UnsubmittedPhysicalFrameLane::Ready => "ready",
        crate::native_output::pacing::UnsubmittedPhysicalFrameLane::Active
        | crate::native_output::pacing::UnsubmittedPhysicalFrameLane::Untracked => "active",
    };
    terminalize_unsubmitted_output_frame_owners(
        validated,
        scene_history,
        frame_pacing,
        frame_scheduler,
        output_transactions,
        retired_at,
        |obligations| {
            restore_presentation_feedback_obligation(server, obligations);
            let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                io::Error::other("rejected Variable transaction has no compositor frame batch")
            })?;
            server.restore_frame_batch_after_render_failure(batch_id);
            Ok(())
        },
    )?;
    let rejection = match rejection {
        crate::native_output::scanout::VariableRetryRejection::TestOnly => "test_only",
        crate::native_output::scanout::VariableRetryRejection::RealSubmit => "real_submit",
    };
    perf.log("native.variable_presentation_retry", || {
        vec![
            NativePerfField::str("rejection", rejection),
            NativePerfField::u64("frame_id", owner.frame_id),
            NativePerfField::u64("transaction_id", owner.transaction_id.get()),
            NativePerfField::u64("output_epoch", output_epoch.get()),
            NativePerfField::str("pacing_lane", pacing_lane),
            NativePerfField::str("physical_state", "suspended"),
        ]
    });
    Ok(())
}

pub(crate) fn terminalize_unsubmitted_frame_for_reconfiguration(
    explicit: &mut AtomicEglGbmScanout,
    output_epoch: OutputEpochId,
    scene_history: &mut NativeSceneHistory,
    frame_pacing: &mut NativeFramePacing,
    frame_scheduler: &mut NativeFrameScheduler,
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    retired_at: MonotonicTimestampNs,
) -> NativeResult<()> {
    let physical_ready = explicit.swapchain()?.ready_identity();
    if let Some(owner) = physical_ready {
        abandon_overtaken_ready(
            explicit,
            owner,
            output_epoch,
            scene_history,
            frame_pacing,
            frame_scheduler,
            server,
            output_transactions,
            retired_at,
        )?;
    } else {
        validate_reconfiguration_frame_ownership(
            explicit,
            output_epoch,
            scene_history,
            frame_pacing,
            frame_scheduler,
            output_transactions,
        )?;
        if output_transactions.active_count() > 0 {
            // A built or submitted transaction without a physical READY owner
            // is still governed by its current asynchronous owner. The shared
            // boundary will wait for that owner to retire.
            return Ok(());
        }
        if frame_pacing.active_physical_identity().is_some() {
            return Err(OutputFrameRetirementInvariantError::PhysicalReadyIdentityChanged.into());
        }
        if frame_pacing.active.is_some() {
            frame_pacing.complete_unsubmitted_render_without_visual_change();
        }
        frame_scheduler.consume_visual_work();
        frame_scheduler.discard_ready_frame();
    }
    if frame_pacing.active.is_some()
        && frame_pacing.active_physical_identity().is_none()
        && output_transactions.active_count() == 0
    {
        frame_pacing.complete_unsubmitted_render_without_visual_change();
    }
    if frame_pacing.has_unsettled_output_frame_ownership() {
        return Err(io::Error::other(
            "old output epoch retained pacing ownership after frame terminalization",
        )
        .into());
    }
    frame_scheduler.consume_visual_work();
    frame_scheduler.discard_ready_frame();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn abandon_overtaken_worker_queued(
    worker: &KmsCommitWorkerHandle,
    owner: QueuedOutputFrameIdentitySnapshot,
    crtc_id: u32,
    explicit: &mut AtomicEglGbmScanout,
    scene_history: &mut NativeSceneHistory,
    frame_pacing: &mut NativeFramePacing,
    frame_scheduler: &mut NativeFrameScheduler,
    atomic_cursor: &mut Option<NativeAtomicCursor>,
    cursor_output_arbitration: &mut NativeCursorOutputArbitration,
    atomic_commit_arbiter: &mut AtomicCommitArbiter,
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    emergency_worker_jobs: &mut Vec<KmsCommitJob>,
) -> NativeResult<()> {
    match worker.cancel_queued_primary(
        owner.token,
        owner.frame.transaction_id,
        owner.frame.pool_generation,
        crtc_id,
        owner.frame.reservation,
        owner.frame.frame_id,
    ) {
        KmsWorkerQueuedCancellation::Cancelled(job) => {
            if job.ready_submit
                && matches!(
                    job.reservation,
                    crate::native_output::presentation::transaction::FramePresentationReservation::Fixed(
                        _,
                    )
                )
            {
                frame_pacing
                    .note_predictive_ready_overtaken_worker_queued_physical(Some(owner.frame));
            }
            drop_queued_worker_job_with_reason_parts(
                *job,
                OutputTransactionDropReason::SafeAbandonment,
                scene_history,
                explicit,
                frame_pacing,
                frame_scheduler,
                atomic_cursor,
                cursor_output_arbitration,
                atomic_commit_arbiter,
                server,
                output_transactions,
                Some(worker),
                emergency_worker_jobs,
            )
        }
        KmsWorkerQueuedCancellation::NotQueued { phase } => Err(io::Error::other(format!(
            "overtaken worker primary crossed ownership boundary before cancellation: {phase:?}"
        ))
        .into()),
        KmsWorkerQueuedCancellation::IdentityMismatch => Err(io::Error::other(
            "overtaken worker primary identity does not match the queued job",
        )
        .into()),
    }
}

pub(crate) fn register_suspended_fence_if_needed(
    explicit: &AtomicEglGbmScanout,
    event_loop: &mut NativeEventLoop,
    output_render_fence_token: &mut Option<ReactorToken>,
) -> NativeResult<()> {
    if output_render_fence_token.is_none()
        && let Some(fd) = explicit.suspended_fence_fd()?
    {
        *output_render_fence_token =
            Some(event_loop.register(fd, NativeEventSource::OutputRenderFence)?);
    }
    Ok(())
}

fn should_continue_resuming_recovery(
    output_render_fence_wake: bool,
    session_is_resuming: bool,
    disabled_observed: bool,
) -> bool {
    output_render_fence_wake && session_is_resuming && !disabled_observed
}

impl NativeRuntime {
    #[allow(clippy::collapsible_if)]
    pub(super) fn wait_for_events_and_pageflips(&mut self) -> NativeResult<NativeCycleState> {
        self.termination.set_phase(RuntimePhase::ReactorWait);
        let wakeup = self.event_loop.wait()?;
        self.termination.set_phase(RuntimePhase::RuntimeCycle);
        let slow_cycle_start_ns = if self.slow_cycle_trace.enabled() {
            let start_ns = monotonic_now_ns()?;
            let xwayland_totals = self.xwayland.slow_cycle_totals();
            self.slow_cycle_trace.start_cycle_after_wake(
                start_ns,
                self.presentation_timing.mode().refresh_interval_ns(),
                SlowCycleContext {
                    wake_reasons: wakeup.reasons.bits(),
                    continuation_reasons: wakeup.continuation.bits(),
                    ready_sources: wakeup.ready_sources,
                    blocked_ns: wakeup.blocked_ns,
                    timer_lateness_ns: wakeup.timer_lateness_ns,
                    x11_events_total: xwayland_totals.0,
                    x11_property_replies_total: xwayland_totals.1,
                    xwm_budget_exhaustions_total: xwayland_totals.2,
                    ..SlowCycleContext::default()
                },
            );
            Some(start_ns)
        } else {
            None
        };
        self.service_dmabuf_gpu_releases(&wakeup.dmabuf_gpu_release_tokens)?;
        self.termination.set_phase(RuntimePhase::KmsWorker);
        self.check_kms_commit_worker_health()?;
        if wakeup.reasons.kms_commit_worker() || wakeup.reasons.drm() {
            // Submit acknowledgments establish physical pending ownership
            // before the DRM pageflip validation below runs.
            self.process_kms_worker_events()?;
        }
        if wakeup.reasons.render_worker() {
            self.process_render_worker_events()?;
        }
        self.termination.set_phase(RuntimePhase::RuntimeCycle);
        self.retire_settled_output_terminals();
        let deferred_worker_pageflip = self.deferred_worker_pageflip.take();
        let deferred_worker_completion = self.deferred_worker_completion.take();
        let worker_timeout_pending = self.worker_timeout_pending.take();
        self.termination.set_phase(RuntimePhase::SeatEvents);
        let disabled_observed = self.dispatch_runtime_seat_events(&wakeup)?;
        self.termination.set_phase(RuntimePhase::RuntimeCycle);
        let resuming_recovery_wake = should_continue_resuming_recovery(
            wakeup.reasons.output_render_fence(),
            self.session.is_resuming(),
            disabled_observed,
        );
        if resuming_recovery_wake {
            if let Some(token) = self.output_render_fence_token.take() {
                self.event_loop.unregister(token)?;
            }
            self.termination.set_phase(RuntimePhase::SessionRecovery);
            self.continue_native_session_recovery()?;
            self.termination.set_phase(RuntimePhase::RuntimeCycle);
        }
        if self.session.permits_output()
            && (wakeup.reasons.drm()
                || (wakeup.reasons.timer()
                    && (self.scanout.page_flip_pending()
                        || self.atomic_commit_arbiter.atomic_commit_pending())))
        {
            NativeSessionIo::observe(self, NativeIoOperation::PageflipDrain);
        }
        let perf = self.perf;
        let Self {
            server,
            perf: _,
            kms,
            kms_backend,
            target,
            mode_label: _,
            output_refresh_rate,
            output_epoch,
            drm_timestamp_clock,
            presentation_clock,
            scanout,
            frame_renderer: _,
            input_state: _,
            cursor_preference: _,
            cursor_render_mode: _,
            cursor_output_arbitration,
            atomic_cursor,
            legacy_cursor: _,
            input_devices: _,
            seat_session: _,
            session: _,
            acquire_notifier: _,
            acquire_watches,
            parked_acquire_watches: _,
            event_loop,
            dmabuf_gpu_release_registry,
            drm_reactor_token: _,
            output_render_fence_token,
            pending_output_configuration,
            kms_commit_worker,
            kms_commit_worker_transport,
            submitted_worker_ownership,
            emergency_quarantined_worker_jobs,
            deferred_worker_pageflip: _,
            deferred_worker_completion: _,
            worker_timeout_pending: _,
            frame_scheduler,
            atomic_commit_arbiter,
            output_transactions,
            presented_planes,
            confirmed_kms_presentation,
            presentation_deadline,
            presentation_timing,
            cursor_timing,
            scheduled_presentation_target,
            render_journal,
            adaptive_buffering,
            vrr_policy,
            pending_proven_deadline_miss,
            effective_app_gpu_policy: _,
            last_primary_presented_at_ns,
            direct_fallback_tracker,
            last_refresh_sequence,
            scene_history,
            queued_redraw_requested,
            frame_index,
            known_toplevels: _,
            pending_launches: _,
            mismatched_pageflip_events,
            stale_pageflip_events,
            presentation_cadence,
            frame_pacing,
            last_acquire_ready_at_ns: _,
            resize_perf: _,
            pointer_constraint_backend: _,
            process_supervisor: _,
            shutdown,
            cursor_reveal_trace,
            ..
        } = self;
        let scheduler_state_before = frame_scheduler.state();
        perf.log("native.wakeup", || {
            vec![
                NativePerfField::u64("ready_mask", u64::from(wakeup.reasons.bits())),
                NativePerfField::usize("ready_sources", wakeup.ready_sources),
                NativePerfField::u64("blocked_us", wakeup.blocked_ns / 1_000),
                NativePerfField::u64(
                    "deadline_late_us",
                    wakeup.timer_lateness_ns.unwrap_or(0) / 1_000,
                ),
                NativePerfField::str("scheduler_before", format!("{scheduler_state_before:?}")),
                NativePerfField::bool("pageflip_pending", scanout.page_flip_pending()),
            ]
        });
        if wakeup.reasons.timer() {
            let wake_lateness_ns = wakeup.timer_lateness_ns.unwrap_or(0);
            render_journal.record_wake_lateness(wake_lateness_ns);
            frame_pacing.note_wake_lateness(wake_lateness_ns);
            perf.log("native.deadline", || {
                vec![
                    NativePerfField::u64("lateness_us", wake_lateness_ns / 1_000),
                    NativePerfField::u64("scheduler_wakeup_lateness_ns", wake_lateness_ns),
                    NativePerfField::str("scheduler_state", format!("{scheduler_state_before:?}")),
                    NativePerfField::bool("pageflip_watchdog", frame_scheduler.page_flip_pending()),
                ]
            });
        }
        if wakeup.reasons.output_render_fence() && !resuming_recovery_wake && !disabled_observed {
            if pending_output_configuration
                .as_ref()
                .is_some_and(PendingOutputConfigurationRequest::waiting_on_render_fence)
            {
                if let Some(token) = output_render_fence_token.take() {
                    event_loop.unregister(token)?;
                }
            } else {
                if let Some(token) = output_render_fence_token.take() {
                    event_loop.unregister(token)?;
                }
                if let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut **scanout {
                    if explicit.recover_suspended_ready_if_signaled()? {
                        *queued_redraw_requested = true;
                    }
                    register_suspended_fence_if_needed(
                        explicit,
                        event_loop,
                        output_render_fence_token,
                    )?;
                }
                if let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut **scanout
                    && let Some(timing) = explicit
                        .sample_pending_timing(MonotonicTimestampNs::new(monotonic_now_ns()?))?
                {
                    frame_pacing.note_fence_timestamp_quality(timing.quality);
                    render_journal.record_render_sample(
                        timing
                            .signaled_at
                            .get()
                            .saturating_sub(timing.composite_started_at.get()),
                        timing.signaled_at,
                    );
                    let observed_miss = classify_pending_fence_deadline_miss(
                        timing.reservation,
                        timing.presentation_timing,
                        timing.quality,
                        timing.signaled_at,
                        render_journal,
                        presentation_timing.apply_guard_ns(),
                    )?;
                    if let Some(miss) = observed_miss {
                        pending_proven_deadline_miss.get_or_insert((timing.frame_id, miss));
                        let desired_credit_before = adaptive_buffering.desired_credit();
                        let buffering_mode_before = adaptive_buffering.mode();
                        adaptive_buffering.observe_o1_outcome(Some(miss));
                        if desired_credit_before == 1 && adaptive_buffering.desired_credit() == 2 {
                            frame_pacing.note_o1_credit2_grant();
                        }
                        frame_pacing.note_adaptive_transition(
                            buffering_mode_before,
                            adaptive_buffering.mode(),
                            Some(miss),
                        );
                    }
                    perf.log("native.render_fence", || {
                        let mut fields = vec![
                            NativePerfField::u64("frame_id", timing.frame_id),
                            NativePerfField::u64("signal_ns", timing.signaled_at.get()),
                            NativePerfField::u64(
                                "render_fence_signal_latency_ns",
                                timing
                                    .signaled_at
                                    .get()
                                    .saturating_sub(timing.composite_started_at.get()),
                            ),
                            NativePerfField::str("quality", format!("{:?}", timing.quality)),
                        ];
                        if let Some(target) = timing.reservation.fixed_target() {
                            fields.push(NativePerfField::u64(
                                "target_ns",
                                target.presentation_time.get(),
                            ));
                        }
                        fields
                    });
                }
            }
        }
        if !self.session.permits_output() {
            if let Some(start_ns) = slow_cycle_start_ns {
                self.slow_cycle_trace.record_phase(
                    SlowCyclePhase::PostWakeMaintenance,
                    start_ns,
                    monotonic_now_ns()?,
                );
            }
            return Ok(NativeCycleState {
                wakeup,
                work_class: NativeWorkClass::NoOutputWork,
                fast_path_completed: false,
                pageflip_drain_us: 0,
                pageflip_completed: false,
                completed_pageflip_token: None,
                frame_completed: false,
                frame_rendered: false,
                frame_submitted: false,
                presentation_admitted: false,
                present_us: 0,
                pageflip_pending_at_tick: false,
                tick_us: 0,
                accepted: 0,
                redraw_requested: false,
                skipped_input_repaints: 0,
                input_drain_us: 0,
                raw_input_events: 0,
                coalesced_input_events: 0,
                cursor_motion_sample_id: None,
                shutdown_requested: false,
            });
        }
        let pageflip_drain_start = Instant::now();
        if let Some(start_ns) = slow_cycle_start_ns {
            self.slow_cycle_trace.record_phase(
                SlowCyclePhase::PostWakeMaintenance,
                start_ns,
                monotonic_now_ns()?,
            );
        }
        let should_drain_pageflips = wakeup.reasons.drm()
            || (wakeup.reasons.timer()
                && (frame_scheduler.page_flip_pending()
                    || atomic_commit_arbiter.atomic_commit_pending()
                    || shutdown.state() == ShutdownState::Draining));
        let pageflip_drain = if should_drain_pageflips {
            scanout
                .drain_page_flip_events(kms.file().as_raw_fd(), kms_backend.effective_kind())
                .map_err(|error| {
                    native_runtime_error(
                        NativeRuntimeStage::DrainPageFlipEvents,
                        scanout.kind(),
                        target.crtc_id,
                        *frame_index,
                        error,
                    )
                })?
        } else {
            NativePageFlipDrain::default()
        };
        let pageflip_drain_us = elapsed_micros(pageflip_drain_start);
        if self.slow_cycle_trace.enabled() {
            self.slow_cycle_trace.record_phase_duration(
                SlowCyclePhase::PageflipDrain,
                pageflip_drain_us.saturating_mul(1_000),
            );
        }
        let slow_post_wake_after_pageflip_start_ns = self
            .slow_cycle_trace
            .enabled()
            .then(monotonic_now_ns)
            .transpose()?;
        *mismatched_pageflip_events =
            mismatched_pageflip_events.saturating_add(pageflip_drain.mismatched_events);
        *stale_pageflip_events = stale_pageflip_events.saturating_add(pageflip_drain.stale_events);
        if pageflip_drain.mismatched_events > 0 || pageflip_drain.stale_events > 0 {
            perf.log("native.pageflip_event_error", || {
                vec![
                    NativePerfField::u64("mismatched", pageflip_drain.mismatched_events),
                    NativePerfField::u64("stale", pageflip_drain.stale_events),
                    NativePerfField::u64(
                        "expected_token",
                        pageflip_drain.last_mismatch.map_or(0, |value| value.0),
                    ),
                    NativePerfField::u64(
                        "received_token",
                        pageflip_drain.last_mismatch.map_or(0, |value| value.1),
                    ),
                    NativePerfField::u64(
                        "stale_token",
                        pageflip_drain.last_stale_token.unwrap_or(0),
                    ),
                    NativePerfField::str("kms_backend", kms_backend.effective_kind().as_str()),
                    NativePerfField::u64("backend_generation", output_epoch.get()),
                ]
            });
        }
        let wrong_crtc_pageflip = pageflip_drain
            .completion
            .is_some_and(|event| event.crtc_id != target.crtc_id);
        if wrong_crtc_pageflip {
            *mismatched_pageflip_events = mismatched_pageflip_events.saturating_add(1);
        }
        let pageflip_event = deferred_worker_pageflip.or(pageflip_drain
            .completion
            .filter(|event| event.crtc_id == target.crtc_id));
        let (pageflip_event, atomic_completion, atomic_watchdog_kind) =
            if deferred_worker_completion.is_some() {
                (pageflip_event, deferred_worker_completion, None)
            } else {
                validate_atomic_pageflip(
                    atomic_commit_arbiter,
                    kms_backend.effective_kind(),
                    pageflip_event,
                    output_epoch.get(),
                    monotonic_now_ns()?,
                    mismatched_pageflip_events,
                    stale_pageflip_events,
                    *kms_commit_worker_transport
                        == crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker,
                )?
            };
        if let Some(kind) = atomic_watchdog_kind {
            perf.log("native.atomic_commit_watchdog", || {
                vec![
                    NativePerfField::str("kind", format!("{kind:?}")),
                    NativePerfField::u64(
                        "token",
                        atomic_commit_arbiter
                            .pending_atomic_token()
                            .map_or(0, PageFlipToken::get),
                    ),
                    NativePerfField::u64("crtc", u64::from(target.crtc_id)),
                    NativePerfField::u64("generation", output_epoch.get()),
                    NativePerfField::bool("final_drain_completed", false),
                ]
            });
            acquire_watches.shutdown(event_loop)?;
            return Err(io::Error::other(
                "native Atomic commit watchdog expired; final DRM drain found no completion",
            )
            .into());
        }
        if let Some((token, detected_at)) = worker_timeout_pending {
            let handled_at = monotonic_now_ns().unwrap_or(0);
            if pageflip_event.is_some() {
                if let Some(worker) = kms_commit_worker.as_ref()
                    && handled_at.saturating_sub(detected_at) > 2_000_000
                {
                    worker.record_main_thread_stall();
                }
            } else {
                if let Some(worker) = kms_commit_worker.as_ref() {
                    worker.record_driver_timeout_suspicion();
                }
                perf.log("native.kms_commit_worker_timeout", || {
                    vec![
                        NativePerfField::u64("token", token.get()),
                        NativePerfField::u64("detected_at_ns", detected_at),
                        NativePerfField::u64("handled_at_ns", handled_at),
                    ]
                });
                acquire_watches.shutdown(event_loop)?;
                return Err(io::Error::other(
                    "native Atomic worker pageflip timeout; DRM drain found no completion",
                )
                .into());
            }
        }
        let pageflip_completed = pageflip_event.is_some();
        let mut completed_pageflip_token = None;
        let mut frame_completed = false;
        let frame_rendered = false;
        let frame_submitted = false;
        let mut pageflip_presentation: Option<FramePresentation> = None;
        if let Some(pageflip) = pageflip_event {
            *last_refresh_sequence = u64::from(pageflip.sequence);
            if let Some(tracker) = direct_fallback_tracker.as_mut() {
                tracker.observe_refresh(*last_refresh_sequence);
                scanout.note_direct_fallback_cycles(tracker.cycles);
            }
            completed_pageflip_token = Some(pageflip.user_data);
            let compositor_receive_ns = monotonic_now_ns()?;
            let cursor_commit = atomic_completion.is_some_and(|completion| {
                matches!(
                    completion,
                    AtomicCommitCompletion::Completed {
                        kind: AtomicCommitKind::PlaneDelta { .. },
                        ..
                    }
                )
            });
            let cursor_transaction_id = match atomic_completion {
                Some(AtomicCommitCompletion::Completed {
                    kind: AtomicCommitKind::PlaneDelta { transaction_id, .. },
                    ..
                }) => Some(transaction_id),
                _ => None,
            };
            if cursor_commit {
                let monotonic_before_sample_ns = monotonic_now_ns()?;
                let compositor_receive_us = sample_clock_microseconds(*drm_timestamp_clock)?;
                let monotonic_after_sample_ns = monotonic_now_ns()?;
                let cursor_presented_at = estimate_pageflip_timestamp(
                    *drm_timestamp_clock,
                    pageflip.timestamp.seconds,
                    pageflip.timestamp.microseconds,
                    compositor_receive_us,
                    monotonic_before_sample_ns,
                    monotonic_after_sample_ns,
                    compositor_receive_ns,
                )
                .presented_at;
                let prepared_cursor_presentation =
                    presentation_deadline.prepare_presented(cursor_presented_at);
                let actual_logical_sequence = prepared_cursor_presentation.logical_sequence();
                let cursor_kms_evidence = if *kms_commit_worker_transport
                    == crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker
                {
                    PageFlipToken::new(pageflip.user_data)
                        .and_then(|token| {
                            submitted_worker_ownership
                                .iter()
                                .find(|ownership| ownership.job.token == token)
                        })
                        .and_then(|ownership| {
                            let (
                                crate::native_output::presentation::transaction::FramePresentationReservation::Fixed(target),
                                crate::native_output::presentation::kms_timing::KmsPresentationTiming::Fixed(window),
                            ) = (ownership.job.reservation, ownership.job.presentation_timing)
                            else {
                                return None;
                            };
                            (ownership.job.output_epoch == *output_epoch).then(|| {
                                crate::native_output::presentation::cursor_timing::CursorKmsSubmissionEvidence::new(
                                    ownership.job.token,
                                    ownership.job.output_epoch.get(),
                                    target,
                                    window,
                                    ownership.job.queued_at.get(),
                                    ownership.actual_worker_wait_returned_at.get(),
                                    ownership.submit_returned_at.get(),
                                )
                            })
                        })
                } else {
                    PageFlipToken::new(pageflip.user_data).and_then(|token| {
                        cursor_timing.take_synchronous_submission(token, output_epoch.get())
                    })
                };
                if let Some(evidence) = cursor_kms_evidence
                    && evidence.output_generation == output_epoch.get()
                    && cursor_timing.output_generation() == output_epoch.get()
                    && evidence.window.mode_key() == Some(cursor_timing.mode_key())
                {
                    let outcome = evidence.classify(actual_logical_sequence);
                    cursor_timing.observe_kms(outcome, cursor_presented_at.get());
                    perf.log("native.cursor_kms_timing", || {
                        vec![
                            NativePerfField::u64("target_sequence", evidence.target_sequence),
                            NativePerfField::u64(
                                "actual_logical_sequence",
                                actual_logical_sequence,
                            ),
                            NativePerfField::u64(
                                "actual_pageflip_sequence",
                                u64::from(pageflip.sequence),
                            ),
                            NativePerfField::u64(
                                "target_presentation_ns",
                                evidence.target_presentation_ns,
                            ),
                            NativePerfField::u64(
                                "submit_window_deadline_ns",
                                evidence.window.commit_complete_deadline_ns(),
                            ),
                            NativePerfField::u64("queued_at_ns", evidence.queued_at_ns),
                            NativePerfField::u64(
                                "submit_returned_at_ns",
                                evidence.submit_returned_at_ns,
                            ),
                            NativePerfField::str("outcome", format!("{outcome:?}")),
                            NativePerfField::u64(
                                "cursor_apply_guard_us",
                                cursor_timing.apply_guard_ns(cursor_presented_at.get()) / 1_000,
                            ),
                        ]
                    });
                }
                // Cursor-only completion is not a complete compositor cycle.
                // Continue through protocol and input dispatch so a primary
                // producer can be observed and scheduled immediately.
                if let Some(transaction_id) = cursor_transaction_id {
                    let cursor_surface_damage = output_transactions.surface_damage(transaction_id);
                    let cursor_presentation = output_transactions
                        .transaction(transaction_id)
                        .map(|record| {
                            let presentation_mode = record.descriptor().presentation_mode();
                            let presentation = if presentation_mode.is_async() {
                                FramePresentation::tearing(
                                    *presentation_clock,
                                    pageflip.timestamp.seconds,
                                    pageflip.timestamp.microseconds,
                                    pageflip.sequence,
                                )
                            } else {
                                FramePresentation::synchronized(
                                    *presentation_clock,
                                    pageflip.timestamp.seconds,
                                    pageflip.timestamp.microseconds,
                                    pageflip.sequence,
                                )
                            };
                            presentation.map(|presentation| {
                                presentation.with_presentation_mode(presentation_mode)
                            })
                        })
                        .transpose()?;
                    complete_presented_output_transaction(
                        output_transactions,
                        &mut self.presentation_trace,
                        transaction_id,
                        PageFlipToken::new(pageflip.user_data)
                            .ok_or_else(|| io::Error::other("pageflip token is zero"))?,
                        output_epoch.get(),
                        MonotonicTimestampNs::new(compositor_receive_ns),
                        Some(u64::from(pageflip.sequence)),
                        |obligations| {
                            debug_assert!(obligations.frame_batch_id().is_none());
                            if let Some(presentation) = cursor_presentation {
                                complete_presentation_feedback_obligation(
                                    server,
                                    obligations,
                                    presentation,
                                )?;
                            }
                            if let Some(surface_damage) = cursor_surface_damage {
                                server.commit_surface_damage_presented(surface_damage);
                            }
                            let _ = complete_plane_delta_pageflip(
                                atomic_cursor,
                                pageflip.user_data,
                                output_epoch.get(),
                                perf,
                            )?;
                            Ok(())
                        },
                    )?;
                    self.presentation_trace.record_cursor_pageflip_sequence(
                        transaction_id,
                        u64::from(pageflip.sequence),
                    );
                } else {
                    let _ = complete_plane_delta_pageflip(
                        atomic_cursor,
                        pageflip.user_data,
                        output_epoch.get(),
                        perf,
                    )?;
                }
                if *kms_commit_worker_transport
                    != crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker
                    && let Some(cursor) = atomic_cursor.as_ref()
                {
                    let token = PageFlipToken::new(pageflip.user_data)
                        .ok_or_else(|| io::Error::other("cursor pageflip token is zero"))?;
                    let identity =
                        pageflip_identity(self.output_id, token, *output_epoch, target.crtc_id);
                    let previous_cursor = presented_planes.cursor;
                    let presented_cursor = cursor.presented_plane_state();
                    if !presented_planes.promote_bundle(
                        identity,
                        identity,
                        None,
                        Some(presented_cursor),
                    ) {
                        return Err(io::Error::other(
                            "cursor pageflip promotion identity mismatch",
                        )
                        .into());
                    }
                    trace_presented_cursor(
                        server,
                        cursor_reveal_trace,
                        identity,
                        previous_cursor,
                        presented_cursor,
                    );
                }
                presentation_deadline.commit_presented(prepared_cursor_presentation);
            }
            let direct_pending = matches!(
                atomic_completion,
                Some(AtomicCommitCompletion::Completed {
                    kind: AtomicCommitKind::DirectPrimary { .. },
                    ..
                })
            );
            let completion = if atomic_completion.is_some_and(is_non_primary_atomic_completion) {
                // Cursor-only Atomic commits are validated and completed by
                // the Atomic arbiter, not by the primary frame scheduler.
                PageFlipCompletionResult::Stale
            } else if let Some(AtomicCommitCompletion::Completed {
                submitted_at_ns, ..
            }) = atomic_completion
            {
                PageFlipCompletionResult::Completed { submitted_at_ns }
            } else {
                frame_scheduler.complete_kernel_pageflip(pageflip.user_data, compositor_receive_ns)
            };
            let pageflip_token = PageFlipToken::new(pageflip.user_data)
                .ok_or_else(|| io::Error::other("pageflip token is zero"))?;
            let atomic_transaction_id = match atomic_completion {
                Some(AtomicCommitCompletion::Completed { kind, .. }) => Some(kind.transaction_id()),
                _ => None,
            };
            let compatibility_transaction_id =
                matches!(completion, PageFlipCompletionResult::Completed { .. })
                    .then(|| {
                        output_transactions
                            .submitted_transaction(pageflip_token, output_epoch.get())
                    })
                    .flatten();
            let presentation_transaction_id =
                atomic_transaction_id.or(compatibility_transaction_id);
            if let Some(transaction_id) = presentation_transaction_id
                && let Some(record) =
                    output_transactions.transaction_including_terminal(transaction_id)
            {
                let descriptor = record.descriptor();
                if confirm_kms_presentation_state(
                    confirmed_kms_presentation,
                    descriptor,
                    output_epoch.get(),
                    true,
                ) {
                    let presentation_mode = descriptor.presentation_mode();
                    perf.log("native.output_presentation_confirmed", || {
                        vec![
                            NativePerfField::str("configured_policy", vrr_policy.as_str()),
                            NativePerfField::bool(
                                "drm_connector_capable",
                                kms_backend.atomic_connector_vrr_capable(),
                            ),
                            NativePerfField::bool(
                                "crtc_vrr_property_available",
                                kms_backend.atomic_crtc_vrr_property_available(),
                            ),
                            NativePerfField::str("effective_mode", presentation_mode.as_str()),
                            NativePerfField::str("submitted_mode", presentation_mode.as_str()),
                            NativePerfField::str(
                                "pageflip_confirmed_mode",
                                presentation_mode.as_str(),
                            ),
                            NativePerfField::u64("output_generation", output_epoch.get()),
                        ]
                    });
                }
            }
            if matches!(completion, PageFlipCompletionResult::Completed { .. }) {
                if let Some(token) = pageflip_drain.deferred_promotion_token {
                    scanout
                        .promote_page_flip(PageFlipToken::new(token).ok_or_else(|| {
                            io::Error::other("pageflip promotion token is zero")
                        })?)?;
                } else if deferred_worker_completion.is_some() {
                    scanout.promote_worker_early_page_flip(
                        PageFlipToken::new(pageflip.user_data)
                            .ok_or_else(|| io::Error::other("pageflip token is zero"))?,
                    )?;
                }
                let explicit_composited_pageflip =
                    matches!(&**scanout, NativeScanoutBackend::AtomicEglGbm(_)) && !direct_pending;
                if !explicit_composited_pageflip {
                    let _ = promote_pageflip_and_publish(scene_history, pageflip.user_data, server);
                }
            }
            if let PageFlipCompletionResult::Completed { submitted_at_ns } = completion {
                let completed_frame_id = frame_pacing.pending;
                let presentation_mode = presentation_transaction_id
                    .and_then(|transaction_id| {
                        output_transactions
                            .transaction_including_terminal(transaction_id)
                            .map(|record| record.descriptor().presentation_mode())
                    })
                    .unwrap_or(OutputPresentationMode::Vsync);
                let presentation = (if presentation_mode.is_async() && direct_pending {
                    FramePresentation::tearing_zero_copy(
                        *presentation_clock,
                        pageflip.timestamp.seconds,
                        pageflip.timestamp.microseconds,
                        pageflip.sequence,
                    )?
                } else if presentation_mode.is_async() {
                    FramePresentation::tearing(
                        *presentation_clock,
                        pageflip.timestamp.seconds,
                        pageflip.timestamp.microseconds,
                        pageflip.sequence,
                    )?
                } else if direct_pending {
                    FramePresentation::synchronized_zero_copy(
                        *presentation_clock,
                        pageflip.timestamp.seconds,
                        pageflip.timestamp.microseconds,
                        pageflip.sequence,
                    )?
                } else {
                    FramePresentation::synchronized(
                        *presentation_clock,
                        pageflip.timestamp.seconds,
                        pageflip.timestamp.microseconds,
                        pageflip.sequence,
                    )?
                })
                .with_presentation_mode(presentation_mode);
                pageflip_presentation = Some(presentation);
                let monotonic_before_sample_ns = monotonic_now_ns()?;
                let compositor_receive_us = sample_clock_microseconds(*drm_timestamp_clock)?;
                let monotonic_after_sample_ns = monotonic_now_ns()?;
                let kernel_timestamp_us = u64::from(pageflip.timestamp.seconds)
                    .saturating_mul(1_000_000)
                    .saturating_add(u64::from(pageflip.timestamp.microseconds));
                let timestamp_estimate = estimate_pageflip_timestamp(
                    *drm_timestamp_clock,
                    pageflip.timestamp.seconds,
                    pageflip.timestamp.microseconds,
                    compositor_receive_us,
                    monotonic_before_sample_ns,
                    monotonic_after_sample_ns,
                    compositor_receive_ns,
                );
                let presented_at = timestamp_estimate.presented_at;
                let presented_at_ns = presented_at.get();
                *last_primary_presented_at_ns = Some(presented_at_ns);
                let mut completed_physical_identity = None;
                if direct_pending {
                    let prepared_physical = presentation_deadline.prepare_presented(presented_at);
                    let actual_logical_sequence = prepared_physical.logical_sequence();
                    let transaction_id = match atomic_completion {
                        Some(AtomicCommitCompletion::Completed {
                            kind: AtomicCommitKind::DirectPrimary { transaction_id, .. },
                            ..
                        }) => transaction_id,
                        _ => {
                            return Err(
                                io::Error::other("direct pageflip has no transaction").into()
                            );
                        }
                    };
                    let pageflip_token = PageFlipToken::new(pageflip.user_data)
                        .ok_or_else(|| io::Error::other("direct pageflip token is zero"))?;
                    if !presentation_mode.is_async()
                        && let Some(ownership) = submitted_worker_ownership
                            .iter()
                            .find(|ownership| ownership.job.token == pageflip_token)
                    {
                        if let (
                            crate::native_output::presentation::transaction::FramePresentationReservation::Fixed(target),
                            crate::native_output::presentation::kms_timing::KmsPresentationTiming::Fixed(window),
                        ) = (ownership.job.reservation, ownership.job.presentation_timing)
                        {
                            let outcome = KmsPresentationOutcome::classify(
                                &window,
                                None,
                                ownership.submit_returned_at.get(),
                                target.sequence,
                                actual_logical_sequence,
                            );
                            if let Some(mode_key) = window.mode_key() {
                                presentation_timing.observe_pageflip(
                                    ownership.job.output_epoch.get(),
                                    mode_key,
                                    outcome,
                                );
                            }
                        }
                    }
                    let direct_pageflip_identity = pageflip_identity(
                        self.output_id,
                        pageflip_token,
                        *output_epoch,
                        target.crtc_id,
                    );
                    cycle_direct::settle_direct_pageflip(
                        scanout,
                        scene_history,
                        server,
                        output_transactions,
                        &mut self.presentation_trace,
                        atomic_cursor,
                        output_epoch.get(),
                        direct_pageflip_identity,
                        transaction_id,
                        pageflip_token,
                        pageflip.user_data,
                        pageflip.sequence,
                        presented_at,
                        presented_at_ns,
                        actual_logical_sequence,
                        presentation,
                        &mut presented_planes.primary,
                        render_journal,
                        frame_pacing,
                        scheduled_presentation_target,
                    )?;
                    presentation_deadline.commit_presented(prepared_physical);
                    if *kms_commit_worker_transport
                        != crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker
                    {
                        let identity = pageflip_identity(
                            self.output_id,
                            pageflip_token,
                            *output_epoch,
                            target.crtc_id,
                        );
                        let previous_cursor = presented_planes.cursor;
                        let presented_cursor = atomic_cursor
                            .as_ref()
                            .map(NativeAtomicCursor::presented_plane_state);
                        if !presented_planes.promote_bundle(
                            identity,
                            identity,
                            presented_planes.primary,
                            presented_cursor,
                        ) {
                            return Err(io::Error::other(
                                "direct pageflip promotion identity mismatch",
                            )
                            .into());
                        }
                        if let Some(presented_cursor) = presented_cursor {
                            trace_presented_cursor(
                                server,
                                cursor_reveal_trace,
                                identity,
                                previous_cursor,
                                presented_cursor,
                            );
                        }
                    }
                } else if let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut **scanout {
                    if let Some(token) = output_render_fence_token.take() {
                        event_loop.unregister(token)?;
                    }
                    let transaction_id = match atomic_completion {
                        Some(AtomicCommitCompletion::Completed {
                            kind: AtomicCommitKind::CompositedPrimary { transaction_id, .. },
                            ..
                        }) => transaction_id,
                        _ => {
                            return Err(
                                io::Error::other("composited pageflip has no transaction").into()
                            );
                        }
                    };
                    let pageflip_token = PageFlipToken::new(pageflip.user_data)
                        .ok_or_else(|| io::Error::other("composited pageflip token is zero"))?;
                    let submitted_ownership = submitted_worker_ownership
                        .iter()
                        .find(|ownership| ownership.job.token == pageflip_token);
                    let dispatch_tail_observation = submitted_ownership
                        .and_then(|ownership| ownership.dispatch_tail_observation);
                    let dispatch_budget_used_ns = submitted_ownership
                        .and_then(|ownership| match ownership.job.presentation_timing {
                            crate::native_output::presentation::kms_timing::KmsPresentationTiming::Fixed(window) => {
                                Some(window.dispatch_budget_ns())
                            }
                            crate::native_output::presentation::kms_timing::KmsPresentationTiming::Variable { .. } => None,
                        });
                    let dispatch_budget_after_adaptation_ns =
                        submitted_ownership.and_then(|ownership| ownership.submission_budget_ns);
                    let pending_identity =
                        explicit.swapchain()?.pending_identity().ok_or_else(|| {
                            io::Error::other("composited pageflip has no pending identity")
                        })?;
                    if pending_identity.token != pageflip_token
                        || pending_identity.frame.transaction_id != transaction_id
                    {
                        return Err(io::Error::other(
                            "composited pageflip predecessor identity mismatch",
                        )
                        .into());
                    }
                    let variable_physical_violation = match pending_identity.frame.reservation {
                        crate::native_output::presentation::transaction::FramePresentationReservation::Variable(
                            _,
                        ) => validate_variable_physical_time(
                            pending_identity.frame.reservation,
                            timestamp_estimate,
                        )
                        .err(),
                        crate::native_output::presentation::transaction::FramePresentationReservation::Fixed(
                            _,
                        )
                        | crate::native_output::presentation::transaction::FramePresentationReservation::DeferredO1(
                            _,
                        ) => None,
                    };
                    if let Some(violation) = variable_physical_violation {
                        let required_lower_bound_ns =
                            variable_presentation_lower_bound(pending_identity.frame.reservation)
                                .map(MonotonicTimestampNs::get);
                        let timestamp_uncertainty_ns = match timestamp_estimate.uncertainty {
                            PageflipTimestampUncertainty::BoundedNs(value) => Some(value),
                            PageflipTimestampUncertainty::Unbounded => None,
                        };
                        let reason = format!("{violation:?}");
                        frame_pacing.log(
                            "vrr_physical_lower_bound_violation",
                            vec![
                                PacingField::u64("transaction_id", transaction_id.get()),
                                PacingField::u64("output_generation", output_epoch.get()),
                                PacingField::u64("actual_pageflip_ns", presented_at_ns),
                                PacingField::option_u64(
                                    "required_lower_bound_ns",
                                    required_lower_bound_ns,
                                ),
                                PacingField::option_u64(
                                    "timestamp_uncertainty_ns",
                                    timestamp_uncertainty_ns,
                                ),
                                PacingField::str("reason", reason.clone()),
                            ],
                        );
                        perf.log("native.vrr_physical_lower_bound_violation", || {
                            vec![
                                NativePerfField::u64("transaction_id", transaction_id.get()),
                                NativePerfField::u64("output_generation", output_epoch.get()),
                                NativePerfField::u64("actual_pageflip_ns", presented_at_ns),
                                NativePerfField::str(
                                    "required_lower_bound_ns",
                                    required_lower_bound_ns.map_or_else(
                                        || "unavailable".to_owned(),
                                        |value| value.to_string(),
                                    ),
                                ),
                                NativePerfField::str(
                                    "timestamp_uncertainty_ns",
                                    timestamp_uncertainty_ns.map_or_else(
                                        || "unbounded".to_owned(),
                                        |value| value.to_string(),
                                    ),
                                ),
                                NativePerfField::str("reason", reason),
                            ]
                        });
                        explicit.reject_variable_pageflip_for_generation(output_epoch.get());
                        if let Some(key) = output_transactions
                            .transaction_including_terminal(transaction_id)
                            .and_then(|record| record.descriptor().presentation_validation_key())
                        {
                            explicit.note_composited_presentation_validation(key, false);
                        }
                        *queued_redraw_requested = true;
                        *last_primary_presented_at_ns = Some(compositor_receive_ns);
                    }
                    let claim_time = if variable_physical_violation.is_some() {
                        MonotonicTimestampNs::new(compositor_receive_ns)
                    } else {
                        presented_at
                    };
                    let prepared_physical = match pending_identity.frame.reservation {
                        crate::native_output::presentation::transaction::FramePresentationReservation::Variable(
                            variable,
                        ) if variable_physical_violation.is_some() => {
                            prepare_variable_physical_claim(
                                presentation_deadline,
                                variable,
                                claim_time,
                            )?
                        }
                        reservation => prepare_physical_presentation_for_reservation(
                            presentation_deadline,
                            reservation,
                            timestamp_estimate,
                        )?,
                    };
                    let actual_logical_sequence = prepared_physical.logical_sequence();
                    let previous_assignment = presented_planes.primary;
                    let presented_transition = scene_history
                        .prepare_pageflip_transition(
                            pageflip_token.get(),
                            target.width,
                            target.height,
                        )
                        .ok_or_else(|| {
                            io::Error::other("composited pageflip has no matching scene transition")
                        })?;
                    let physical_claim = prepared_physical.claim();
                    loop {
                        match explicit.revalidate_physical_primary_presentation(physical_claim)? {
                            PhysicalPrimaryClaimRevalidation::Valid => break,
                            PhysicalPrimaryClaimRevalidation::OvertakesReady { owner } => {
                                frame_pacing.note_physical_claim_overtake_ready();
                                let recovered = abandon_overtaken_ready(
                                    explicit,
                                    owner,
                                    *output_epoch,
                                    scene_history,
                                    frame_pacing,
                                    frame_scheduler,
                                    server,
                                    output_transactions,
                                    presented_at,
                                )
                                .is_ok();
                                frame_pacing.note_physical_claim_overtake_recovery(recovered);
                                if !recovered {
                                    return Err(io::Error::other(
                                        "failed to recover an overtaken READY primary",
                                    )
                                    .into());
                                }
                                *queued_redraw_requested = true;
                                register_suspended_fence_if_needed(
                                    explicit,
                                    event_loop,
                                    output_render_fence_token,
                                )?;
                            }
                            PhysicalPrimaryClaimRevalidation::OvertakesWorkerQueued { owner } => {
                                frame_pacing.note_physical_claim_overtake_worker_queued();
                                let worker = kms_commit_worker.as_ref().ok_or_else(|| {
                                    io::Error::other(
                                        "overtaken worker primary has no worker handle",
                                    )
                                })?;
                                let recovered = abandon_overtaken_worker_queued(
                                    worker,
                                    owner,
                                    target.crtc_id,
                                    explicit,
                                    scene_history,
                                    frame_pacing,
                                    frame_scheduler,
                                    atomic_cursor,
                                    cursor_output_arbitration,
                                    atomic_commit_arbiter,
                                    server,
                                    output_transactions,
                                    emergency_quarantined_worker_jobs,
                                )
                                .is_ok();
                                frame_pacing.note_physical_claim_overtake_recovery(recovered);
                                if !recovered {
                                    return Err(io::Error::other(
                                        "failed to recover an overtaken queued primary",
                                    )
                                    .into());
                                }
                                *queued_redraw_requested = true;
                                register_suspended_fence_if_needed(
                                    explicit,
                                    event_loop,
                                    output_render_fence_token,
                                )?;
                            }
                            PhysicalPrimaryClaimRevalidation::Fatal(violation) => {
                                frame_pacing.note_physical_claim_fatal_violation();
                                return Err(io::Error::other(format!(
                                    "physical primary claim violation: {violation:?}"
                                ))
                                .into());
                            }
                        }
                    }
                    retire_settled_output_terminals(
                        output_transactions,
                        dmabuf_gpu_release_registry,
                    );
                    let CompositedPageflipCompletion {
                        presented: frame,
                        protocol_batch_id,
                        surface_damage,
                    } = explicit
                        .complete_pageflip(pageflip_token, presented_transition.damage.clone())?;
                    if frame.frame_id != presented_transition.current_frame_id {
                        return Err(io::Error::other(
                            "composited pageflip frame does not match scene transition",
                        )
                        .into());
                    }
                    if frame.transaction_id != transaction_id {
                        return Err(io::Error::other(
                            "composited pageflip transaction does not match output completion",
                        )
                        .into());
                    }
                    completed_physical_identity = Some(frame.physical_identity);
                    explicit.note_physical_primary_presentation(physical_claim)?;
                    match explicit.bind_ready_deferred_o1(
                        output_transactions,
                        output_epoch.get(),
                        presented_at,
                    )? {
                        DeferredO1BindingResult::NotReady => {}
                        DeferredO1BindingResult::WaitingForPredecessor => {}
                        DeferredO1BindingResult::Bound { advanced_intervals } => {
                            frame_pacing.note_predictive_binding_after_predecessor_pageflip(
                                advanced_intervals,
                            );
                        }
                        DeferredO1BindingResult::Stale(failure) => {
                            let owner =
                                explicit.swapchain()?.ready_identity().ok_or_else(|| {
                                    io::Error::other(
                                        "stale deferred O1 binding has no ready frame to abandon",
                                    )
                                })?;
                            match failure {
                                DeferredO1BindingFailure::IdentityMismatch => {
                                    frame_pacing
                                        .note_predictive_unbound_abandoned_identity_physical(Some(
                                            owner,
                                        ));
                                }
                                DeferredO1BindingFailure::GenerationMismatch => {
                                    frame_pacing
                                        .note_predictive_unbound_abandoned_generation_physical(
                                            Some(owner),
                                        );
                                }
                            }
                            abandon_overtaken_ready(
                                explicit,
                                owner,
                                *output_epoch,
                                scene_history,
                                frame_pacing,
                                frame_scheduler,
                                server,
                                output_transactions,
                                presented_at,
                            )?;
                        }
                    }
                    if variable_physical_violation.is_some() {
                        let conservative_anchor = presentation_deadline
                            .prepare_presented(MonotonicTimestampNs::new(compositor_receive_ns));
                        presentation_deadline.commit_presented(conservative_anchor);
                    } else {
                        presentation_deadline.commit_presented(prepared_physical);
                    }
                    self.presentation_trace.push(
                        PresentationTransactionEvent::PresentedTransition {
                            transaction_id,
                            timestamp_ns: monotonic_now_ns()?,
                            token: pageflip_token.get(),
                            previous_frame_id: presented_transition.previous_frame_id,
                            current_frame_id: presented_transition.current_frame_id,
                            transition_damage_signature: presented_transition
                                .damage
                                .identity_signature(),
                        },
                    );
                    dmabuf_gpu_release_registry
                        .note_composited_pageflip(transaction_id, presented_at_ns);
                    server.trace_surface_pipeline_active_surfaces(
                        oblivion_one::compositor::SurfacePipelineEvent::OutputFramePresented,
                        Some(protocol_batch_id.get()),
                        Some(transaction_id.get()),
                        Some(frame.frame_id),
                        Some(pageflip_token.get()),
                    );
                    if !promote_pageflip_and_publish(scene_history, pageflip_token.get(), server) {
                        return Err(io::Error::other(
                            "composited pageflip scene promotion did not match transition",
                        )
                        .into());
                    }
                    let prepared_logical = prepare_presented_output_transaction(
                        output_transactions,
                        transaction_id,
                        pageflip_token,
                        output_epoch.get(),
                        presented_at,
                        Some(actual_logical_sequence),
                    )?;
                    debug_assert!(prepared_logical.obligations().direct_surface_id().is_none());
                    let direct_transition = match previous_assignment {
                        Some(PresentedPrimaryAssignment::Direct {
                            transaction_id,
                            token,
                            surface_id,
                            key: candidate_key,
                            framebuffer_id,
                            ..
                        }) => {
                            let expected = ExpectedPresentedDirectPrimary {
                                transaction_id,
                                token,
                                surface_id,
                                candidate_key,
                                framebuffer_id,
                            };
                            let worker_content_keys = kms_commit_worker
                                .as_ref()
                                .map(|worker| worker.direct_content_keys())
                                .unwrap_or((None, None, None));
                            if let Err(reason) = explicit
                                .validate_composited_transition(expected, worker_content_keys)
                            {
                                output_transactions
                                    .rollback_settlement(prepared_logical)
                                    .map_err(io::Error::other)?;
                                return cycle_direct::fail_composited_transition(
                                    kms_commit_worker.as_ref(),
                                    direct_fallback_tracker,
                                    scanout,
                                    frame_scheduler,
                                    atomic_commit_arbiter,
                                    reason,
                                );
                            }
                            Some((expected, worker_content_keys))
                        }
                        _ => None,
                    };
                    let completed_frame_id = frame.frame_id;
                    commit_prepared_presented_output_transaction(
                        output_transactions,
                        &mut self.presentation_trace,
                        prepared_logical,
                        |obligations| {
                            debug_assert!(obligations.direct_surface_id().is_none());
                            server.commit_surface_damage_presented(surface_damage);
                            complete_presentation_feedback_obligation(
                                server,
                                obligations,
                                presentation,
                            )?;
                            server.complete_presented_frame_batch(
                                completed_frame_id,
                                protocol_batch_id,
                                presentation,
                            );
                            complete_primary_cursor_pageflip(
                                atomic_cursor,
                                pageflip.user_data,
                                output_epoch.get(),
                            )?;
                            Ok(())
                        },
                    )?;
                    if let Some((expected, worker_content_keys)) = direct_transition {
                        match explicit.complete_composited_transition(expected, worker_content_keys)
                        {
                            CompositedTransitionResult::Completed { .. } => {
                                debug_assert!(explicit.direct_scanout_presented_info().is_none());
                            }
                            CompositedTransitionResult::Fatal { reason } => {
                                return cycle_direct::fail_composited_transition(
                                    kms_commit_worker.as_ref(),
                                    direct_fallback_tracker,
                                    scanout,
                                    frame_scheduler,
                                    atomic_commit_arbiter,
                                    reason,
                                );
                            }
                        }
                    }
                    if let Some(mut tracker) = direct_fallback_tracker.take() {
                        tracker.observe_refresh(*last_refresh_sequence);
                        explicit.note_direct_composited_fallback(tracker.cycles);
                    }
                    let swapchain = explicit.swapchain()?;
                    let slot = swapchain.current();
                    let pool_generation = swapchain.pool_generation();
                    let presentation_serial = swapchain.presentation_serial();
                    let framebuffer_id = explicit.framebuffer(slot)?.get();
                    let pageflip = pageflip_identity(
                        self.output_id,
                        pageflip_token,
                        *output_epoch,
                        target.crtc_id,
                    );
                    let presented_primary = Some(PresentedPrimaryAssignment::Composed {
                        transaction_id,
                        token: pageflip_token,
                        pageflip,
                        slot,
                        framebuffer_id,
                        pool_generation,
                        presentation_serial,
                    });
                    if *kms_commit_worker_transport
                        != crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker
                    {
                        let identity = pageflip_identity(
                            self.output_id,
                            pageflip_token,
                            *output_epoch,
                            target.crtc_id,
                        );
                        let previous_cursor = presented_planes.cursor;
                        let presented_cursor = atomic_cursor
                            .as_ref()
                            .map(NativeAtomicCursor::presented_plane_state);
                        if !presented_planes.promote_bundle(
                            identity,
                            identity,
                            presented_primary,
                            presented_cursor,
                        ) {
                            return Err(io::Error::other(
                                "composited pageflip promotion identity mismatch",
                            )
                            .into());
                        }
                        if let Some(presented_cursor) = presented_cursor {
                            trace_presented_cursor(
                                server,
                                cursor_reveal_trace,
                                identity,
                                previous_cursor,
                                presented_cursor,
                            );
                        }
                    }
                    if matches!(
                        frame.reservation,
                        crate::native_output::presentation::transaction::FramePresentationReservation::Variable(_)
                    ) {
                        // Variable presentation has no fixed target or commit deadline. Keep
                        // domain-neutral service evidence, and discard any stale fixed miss evidence.
                        pending_proven_deadline_miss.take();
                        if variable_physical_violation.is_some() {
                            record_composited_service_costs(
                                &frame,
                                *kms_commit_worker_transport,
                                render_journal,
                                frame_pacing,
                            );
                        } else {
                            record_variable_composited_pageflip(
                                &frame,
                                timestamp_estimate,
                                physical_claim.clock_generation,
                                physical_claim.sequence,
                                *kms_commit_worker_transport,
                                render_journal,
                                frame_pacing,
                            )?;
                        }
                    } else {
                        let (target, submit_window) = match (
                            frame.reservation,
                            frame.presentation_timing,
                        ) {
                            (
                                crate::native_output::presentation::transaction::FramePresentationReservation::Fixed(target),
                                crate::native_output::presentation::kms_timing::KmsPresentationTiming::Fixed(submit_window),
                            ) => (target, submit_window),
                            _ => {
                                return Err(io::Error::other(
                                    "composited pageflip reservation and KMS timing domains differ",
                                )
                                .into());
                            }
                        };
                    render_journal.record_target_slip(
                        presented_at
                            .get()
                            .saturating_sub(target.presentation_time.get()),
                    );
                    if *kms_commit_worker_transport
                        == crate::native_output::kms_worker::KmsCommitWorkerTransport::Synchronous
                    {
                        render_journal.record_atomic_submit(
                            frame
                                .submit_returned_at
                                .get()
                                .saturating_sub(frame.submit_started_at.get()),
                        );
                    }
                    let pending_miss = pending_proven_deadline_miss
                        .take()
                        .and_then(|(frame_id, miss)| (frame_id == frame.frame_id).then_some(miss));
                    let readiness = RenderReadinessObservation::from_frame(
                        frame.fence_signal,
                        frame.rendered_at,
                    );
                    if !frame.fence_timing_accounted
                        && let Some((signaled_at, quality)) = frame.fence_signal
                    {
                        frame_pacing.note_fence_timestamp_quality(quality);
                        render_journal.record_render_sample(
                            render_sample_duration_ns(frame.composite_started_at, signaled_at),
                            signaled_at,
                        );
                    }
                    let outcome = if presentation_mode.is_async() {
                        None
                    } else {
                        Some(KmsPresentationOutcome::classify(
                            &submit_window,
                            Some(readiness.observed_at().get()),
                            frame.submit_returned_at.get(),
                            target.sequence,
                            actual_logical_sequence,
                        ))
                    };
                    let mut apply_guard_observation = None;
                    if let Some(outcome) = outcome {
                        if let Some(mode_key) = submit_window.mode_key() {
                            apply_guard_observation =
                                Some(presentation_timing.observe_pageflip_with_evidence(
                                    output_epoch.get(),
                                    mode_key,
                                    outcome,
                                ));
                        }
                    }
                    let assessment = resolve_presentation_deadline_assessment(
                        pending_miss,
                        target,
                        outcome,
                        readiness,
                    );
                    render_journal.record_frame_service_observation(FrameTimingObservation {
                        frame_id: frame.frame_id,
                        target: target,
                        composite_started_at: frame.composite_started_at,
                        fence_signaled_at: frame.fence_signal,
                        submit_started_at: Some(frame.submit_started_at),
                        submit_returned_at: Some(frame.submit_returned_at),
                    });
                    let recovery_remaining_before = render_journal
                        .prediction(target.refresh_interval)
                        .miss_recovery_remaining;
                    let recovery = apply_deadline_assessment(
                        assessment,
                        render_journal,
                        target.refresh_interval,
                        dispatch_tail_observation,
                        apply_guard_observation,
                    );
                    let proven_miss = recovery.as_ref().map(|value| value.miss);
                    if let Some(recovery) = recovery {
                        let miss = recovery.miss;
                        let mut recovery_fields = vec![
                            PacingField::u64("frame_id", frame.frame_id),
                            PacingField::u64("pageflip_token", pageflip_token.get()),
                            PacingField::str("miss_cause", miss.as_str()),
                            PacingField::str(
                                "target_reason",
                                target_reason_name(target.reason),
                            ),
                            PacingField::bool("target_binding", target.is_binding()),
                            PacingField::u64("target_sequence", target.sequence),
                            PacingField::u64("actual_sequence", actual_logical_sequence),
                            PacingField::str("recovery_disposition", recovery.disposition.as_str()),
                            PacingField::usize(
                                "recovery_remaining_before",
                                recovery.recovery_remaining_before,
                            ),
                            PacingField::usize(
                                "recovery_remaining_after",
                                recovery.recovery_remaining_after,
                            ),
                            PacingField::str(
                                "recovery_estimator_mode_after",
                                render_journal
                                    .prediction(target.refresh_interval)
                                    .estimator_mode
                                    .as_str(),
                            ),
                            PacingField::bool(
                                "dispatch_evidence_available",
                                dispatch_tail_observation.is_some(),
                            ),
                            PacingField::bool(
                                "apply_timing_observation_available",
                                apply_guard_observation.is_some(),
                            ),
                        ];
                        if let Some(observation) = dispatch_tail_observation {
                            recovery_fields.extend([
                                PacingField::bool(
                                    "dispatch_target_binding",
                                    observation.binding_target,
                                ),
                                PacingField::bool(
                                    "dispatch_dequeued_before_planned_wake",
                                    observation.dequeued_before_planned_wake,
                                ),
                                PacingField::bool(
                                    "dispatch_fair_chance",
                                    observation.fair_dispatch_chance,
                                ),
                                PacingField::u64(
                                    "dispatch_deadline_overrun_ns",
                                    observation.deadline_overrun_ns,
                                ),
                                PacingField::u64(
                                    "dispatch_tail_guard_before_ns",
                                    observation.guard_before_ns,
                                ),
                                PacingField::u64(
                                    "dispatch_tail_guard_after_ns",
                                    observation.guard_ns,
                                ),
                                PacingField::bool(
                                    "dispatch_tail_guard_increased",
                                    observation.increased,
                                ),
                                PacingField::bool(
                                    "dispatch_tail_guard_cap_hit",
                                    observation.cap_hit,
                                ),
                            ]);
                        }
                        if matches!(miss, ProvenDeadlineMiss::KmsDispatch) {
                            recovery_fields.extend(dispatch_budget_diagnostic_fields(
                                dispatch_budget_used_ns,
                                dispatch_budget_after_adaptation_ns,
                            ));
                        }
                        if let Some(observation) = apply_guard_observation {
                            recovery_fields.extend([
                                PacingField::bool(
                                    "apply_timing_observation_accepted",
                                    observation.accepted,
                                ),
                                PacingField::u64(
                                    "apply_guard_before_ns",
                                    observation.apply_guard_before_ns,
                                ),
                                PacingField::u64(
                                    "apply_guard_after_ns",
                                    observation.apply_guard_after_ns,
                                ),
                                PacingField::bool(
                                    "apply_guard_increased",
                                    observation.apply_guard_increased,
                                ),
                                PacingField::bool(
                                    "apply_guard_cap_hit",
                                    observation.apply_guard_cap_hit,
                                ),
                            ]);
                        }
                        if matches!(
                            miss,
                            ProvenDeadlineMiss::ExactRender
                                | ProvenDeadlineMiss::GuardedApproximateRender
                        ) {
                            recovery_fields.extend(render_readiness_diagnostic_fields(
                                readiness,
                                &submit_window,
                                frame.composite_started_at,
                                frame.rendered_at,
                            ));
                        }
                        frame_pacing.log("proven_deadline_miss", recovery_fields);
                    }
                    if let PresentationDeadlineAssessment::Advisory(slip) = assessment {
                        frame_pacing.note_advisory_dispatch_slip();
                        let recovery_remaining_after = render_journal
                            .prediction(target.refresh_interval)
                            .miss_recovery_remaining;
                        let mut advisory_fields = vec![
                            PacingField::u64("frame_id", frame.frame_id),
                            PacingField::u64("pageflip_token", pageflip_token.get()),
                            PacingField::str("slip_cause", slip.as_str()),
                            PacingField::str(
                                "target_reason",
                                target_reason_name(target.reason),
                            ),
                            PacingField::str("target_authority", "advisory"),
                            PacingField::bool("target_binding", target.is_binding()),
                            PacingField::u64("target_sequence", target.sequence),
                            PacingField::u64("actual_sequence", actual_logical_sequence),
                            PacingField::bool(
                                "dispatch_evidence_available",
                                dispatch_tail_observation.is_some(),
                            ),
                            PacingField::option_bool(
                                "dispatch_dequeued_before_planned_wake",
                                dispatch_tail_observation
                                    .map(|observation| observation.dequeued_before_planned_wake),
                            ),
                            PacingField::option_bool(
                                "dispatch_fair_chance",
                                dispatch_tail_observation
                                    .map(|observation| observation.fair_dispatch_chance),
                            ),
                            PacingField::option_u64(
                                "dispatch_deadline_overrun_ns",
                                dispatch_tail_observation
                                    .map(|observation| observation.deadline_overrun_ns),
                            ),
                            PacingField::usize(
                                "recovery_remaining_before",
                                recovery_remaining_before,
                            ),
                            PacingField::usize(
                                "recovery_remaining_after",
                                recovery_remaining_after,
                            ),
                            PacingField::str(
                                "recovery_estimator_mode_after",
                                render_journal
                                    .prediction(target.refresh_interval)
                                    .estimator_mode
                                    .as_str(),
                            ),
                        ];
                        advisory_fields.extend(dispatch_budget_diagnostic_fields(
                            dispatch_budget_used_ns,
                            dispatch_budget_after_adaptation_ns,
                        ));
                        if let Some(observation) = dispatch_tail_observation {
                            advisory_fields.extend([
                                PacingField::bool(
                                    "dispatch_target_binding",
                                    observation.binding_target,
                                ),
                                PacingField::u64(
                                    "dispatch_tail_guard_before_ns",
                                    observation.guard_before_ns,
                                ),
                                PacingField::u64(
                                    "dispatch_tail_guard_after_ns",
                                    observation.guard_ns,
                                ),
                                PacingField::bool(
                                    "dispatch_tail_guard_increased",
                                    observation.increased,
                                ),
                                PacingField::bool(
                                    "dispatch_tail_guard_cap_hit",
                                    observation.cap_hit,
                                ),
                            ]);
                        }
                        frame_pacing.log("advisory_opportunity_slip", advisory_fields);
                    }
                    let desired_credit_before = adaptive_buffering.desired_credit();
                    let buffering_mode_before = adaptive_buffering.mode();
                    adaptive_buffering.observe_o1_outcome(proven_miss);
                    if desired_credit_before == 1 && adaptive_buffering.desired_credit() == 2 {
                        frame_pacing.note_o1_credit2_grant();
                    }
                    frame_pacing.note_adaptive_transition(
                        buffering_mode_before,
                        adaptive_buffering.mode(),
                        proven_miss,
                    );
                    frame_pacing.note_o1_credit2_outcome(
                        frame.o1_admission,
                        matches!(assessment, PresentationDeadlineAssessment::None),
                    );
                    frame_pacing.note_explicit_present(ExplicitPresentationObservation {
                        planned_sequence: target.sequence,
                        actual_sequence: actual_logical_sequence,
                        target_ns: target.presentation_time.get(),
                        presented_ns: presented_at_ns,
                        composite_started_ns: frame.composite_started_at.get(),
                        rendered_ns: frame.rendered_at.get(),
                        submit_started_ns: frame.submit_started_at.get(),
                        submit_returned_ns: frame.submit_returned_at.get(),
                        reactive_double: target.reason
                            == PresentationTargetReason::ReactiveDouble,
                        target_reason: target.reason,
                        target_selection: target.selection_evidence(),
                        previous_primary_sequence: None,
                        client_commit_ns: frame.client_commit_ns,
                        callback_reaction_ns: frame.callback_reaction_ns,
                        callback_admission_ns: frame.callback_admission_ns,
                        callback_surface_id: frame.callback_surface_id,
                        callback_surface_is_exclusive: frame.callback_surface_is_exclusive,
                        refresh_interval_ns: output_refresh_rate.interval_ns(),
                        render_missed: matches!(
                            outcome,
                            Some(KmsPresentationOutcome::RenderReadinessMiss)
                        ),
                        submit_missed: matches!(
                            outcome,
                            Some(KmsPresentationOutcome::KmsDispatchMiss)
                        ),
                        kms_slipped: matches!(
                            outcome,
                            Some(KmsPresentationOutcome::KmsApplyGuardMiss)
                        ),
                    });
                    *scheduled_presentation_target = None;
                    }
                } else {
                    complete_primary_cursor_pageflip(
                        atomic_cursor,
                        pageflip.user_data,
                        output_epoch.get(),
                    )?;
                    if let Some(transaction_id) = compatibility_transaction_id {
                        complete_presented_output_transaction(
                            output_transactions,
                            &mut self.presentation_trace,
                            transaction_id,
                            PageFlipToken::new(pageflip.user_data)
                                .ok_or_else(|| io::Error::other("pageflip token is zero"))?,
                            output_epoch.get(),
                            MonotonicTimestampNs::new(presented_at_ns),
                            None,
                            |obligations| {
                                let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                                    io::Error::other(
                                        "compatibility pageflip transaction has no frame batch",
                                    )
                                })?;
                                server.finish_presented_frame_batch(batch_id, presentation)?;
                                Ok(())
                            },
                        )?;
                    } else {
                        server.finish_frame_with_presentation(presentation);
                    }
                }
                frame_pacing.note_pageflip_exact(
                    completed_physical_identity,
                    presented_at_ns,
                    submitted_at_ns,
                    pageflip.user_data,
                    output_refresh_rate.interval_ns() / 1_000,
                );
                let mut pacing_fields = vec![
                    frame_id_field(completed_frame_id),
                    PacingField::u64("render_generation", server.render_generation()),
                    PacingField::u64("pageflip_token", pageflip.user_data),
                    PacingField::u64("pageflip_complete_ns", presented_at_ns),
                ];
                pacing_fields.extend(snapshot_fields(scanout.buffer_snapshot()));
                frame_pacing.log("frame_complete", pacing_fields);
                let refresh_interval_us = output_refresh_rate.interval_ns() / 1_000;
                let cadence = presentation_cadence.record_with_refresh(
                    pageflip.sequence,
                    presented_at_ns / 1_000,
                    refresh_interval_us,
                );
                let finish_frame_start = Instant::now();
                if !server.has_unowned_frame_work() {
                    frame_scheduler.complete_protocol_only();
                }
                frame_completed = true;
                perf.log("native.finish_frame", || {
                    vec![
                        NativePerfField::str("reason", "pageflip_complete"),
                        NativePerfField::u64("elapsed_us", elapsed_micros(finish_frame_start)),
                        NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                        NativePerfField::u64("render_generation", server.render_generation()),
                        NativePerfField::u64("pageflip_token", pageflip.user_data),
                        NativePerfField::str("kms_backend", kms_backend.effective_kind().as_str()),
                        NativePerfField::u64("backend_generation", output_epoch.get()),
                        NativePerfField::u64("kernel_sequence", u64::from(pageflip.sequence)),
                        NativePerfField::u64("kernel_timestamp_us", kernel_timestamp_us),
                        NativePerfField::u64("presentations", cadence.presentations),
                        NativePerfField::u64(
                            "presentation_interval_us",
                            cadence.interval_us.unwrap_or(0),
                        ),
                        NativePerfField::u64(
                            "presentation_sequence_delta",
                            cadence.sequence_delta.map(u64::from).unwrap_or(0),
                        ),
                        NativePerfField::u64(
                            "logical_presentation_sequence",
                            cadence.logical_sequence,
                        ),
                        NativePerfField::u64(
                            "logical_presentation_sequence_delta",
                            cadence.logical_sequence_delta.unwrap_or(0),
                        ),
                        NativePerfField::bool(
                            "timestamp_sequence_fallback",
                            cadence.timestamp_sequence_fallback,
                        ),
                        NativePerfField::bool("presentation_sequence_gap", cadence.sequence_gap),
                        NativePerfField::u64(
                            "presented_hz_millihz",
                            cadence.estimated_hz_millihz.unwrap_or(0),
                        ),
                        NativePerfField::u64("presentation_sequence_gaps", cadence.sequence_gaps),
                        NativePerfField::u64("compositor_receive_us", compositor_receive_us),
                        NativePerfField::u64(
                            "receive_delay_us",
                            compositor_receive_us.saturating_sub(kernel_timestamp_us),
                        ),
                        NativePerfField::u64(
                            "submit_to_completion_us",
                            compositor_receive_ns.saturating_sub(submitted_at_ns) / 1_000,
                        ),
                        NativePerfField::str(
                            "completion_owner",
                            if atomic_completion.is_some() {
                                "atomic_arbiter"
                            } else {
                                "compatibility_scheduler"
                            },
                        ),
                    ]
                });
            }
            if *kms_commit_worker_transport
                == crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker
                && let Some(AtomicCommitCompletion::Completed { kind, .. }) = atomic_completion
            {
                let path_completed = match kind {
                    AtomicCommitKind::PlaneDelta { .. } => cursor_commit,
                    AtomicCommitKind::CompositedPrimary { .. }
                    | AtomicCommitKind::DirectPrimary { .. } => {
                        matches!(completion, PageFlipCompletionResult::Completed { .. })
                    }
                };
                if path_completed && let Some(worker) = kms_commit_worker.as_ref() {
                    let transaction_id = match kind {
                        AtomicCommitKind::CompositedPrimary { transaction_id, .. }
                        | AtomicCommitKind::DirectPrimary { transaction_id, .. }
                        | AtomicCommitKind::PlaneDelta { transaction_id, .. } => transaction_id,
                    };
                    let pageflip_token = PageFlipToken::new(pageflip.user_data)
                        .ok_or_else(|| io::Error::other("pageflip token is zero"))?;
                    let worker_promotion = submitted_worker_ownership
                        .iter()
                        .find(|ownership| ownership.job.token == pageflip_token)
                        .map(|ownership| {
                            let cursor_owner = ownership.job.owners.cursor();
                            (
                                presented_primary_from_worker_job(
                                    &ownership.job,
                                    scanout.explicit_output_swapchain(),
                                ),
                                cursor_owner.map(|owner| {
                                    (
                                        owner.revision,
                                        owner.sidecar_id.is_some(),
                                        Some(owner.transaction.id()),
                                        ownership.job.cursor.clone(),
                                        ownership.job.cursor_delivery,
                                    )
                                }),
                                ownership.job.primary_cursor_presentation,
                                ownership.job.identity(),
                            )
                        });
                    let sidecar_transaction_id = worker_promotion
                        .as_ref()
                        .and_then(|(_, cursor, _, _)| cursor.as_ref())
                        .filter(|(_, sidecar, _, _, _)| *sidecar)
                        .and_then(|(_, _, transaction_id, _, _)| *transaction_id);
                    let sidecar_surface_damage =
                        sidecar_transaction_id.and_then(|transaction_id| {
                            output_transactions.surface_damage(transaction_id)
                        });
                    let worker_identity = worker_promotion
                        .as_ref()
                        .map(|(_, _, _, identity)| *identity)
                        .ok_or_else(|| {
                            io::Error::other(
                                "worker pageflip has no matching submitted bundle identity",
                            )
                        })?;
                    worker
                        .ack_pageflip_identity(worker_identity, transaction_id)
                        .map_err(|error| {
                            io::Error::other(format!("worker pageflip ack: {error:?}"))
                        })?;
                    if let Some(sidecar_transaction_id) = sidecar_transaction_id {
                        let pageflip_presentation = pageflip_presentation.ok_or_else(|| {
                            io::Error::other(
                                "worker sidecar pageflip has no physical presentation evidence",
                            )
                        })?;
                        complete_presented_output_transaction(
                            output_transactions,
                            &mut self.presentation_trace,
                            sidecar_transaction_id,
                            pageflip_token,
                            output_epoch.get(),
                            MonotonicTimestampNs::new(compositor_receive_ns),
                            None,
                            |obligations| {
                                debug_assert!(obligations.frame_batch_id().is_none());
                                debug_assert!(obligations.direct_surface_id().is_none());
                                complete_presentation_feedback_obligation(
                                    server,
                                    obligations,
                                    pageflip_presentation,
                                )?;
                                if let Some(surface_damage) = sidecar_surface_damage {
                                    server.commit_surface_damage_presented(surface_damage);
                                }
                                Ok(())
                            },
                        )?;
                    }
                    if let Some((
                        primary,
                        cursor_owner,
                        primary_cursor_presentation,
                        bundle_identity,
                    )) = worker_promotion
                    {
                        if bundle_identity.token != pageflip_token
                            || bundle_identity.output_id != self.output_id
                            || bundle_identity.output_generation != output_epoch.get()
                            || bundle_identity.crtc_id != target.crtc_id
                        {
                            return Err(io::Error::other(
                                "worker pageflip promotion bundle identity mismatch",
                            )
                            .into());
                        }
                        let identity =
                            crate::native_output::presentation::plane::PlanePageflipIdentity {
                                output_id: self.output_id,
                                bundle_id: bundle_identity.id,
                                token: bundle_identity.token,
                                output_epoch: OutputEpochId::new(bundle_identity.output_generation)
                                    .expect("KMS pageflip output epoch is nonzero"),
                                crtc_id: bundle_identity.crtc_id,
                            };
                        let cursor_plane_promotion = cursor_owner
                            .and_then(|(revision, sidecar, _, update, delivery)| {
                                let coupling = if delivery
                                    == crate::native_output::presentation::plane::PresentedCursorDelivery::Software
                                {
                                    CursorCoupling::EmbeddedInPrimary
                                } else if !matches!(
                                    &update,
                                    KmsCursorUpdate::Set(state) if state.visible
                                ) {
                                    CursorCoupling::Hidden
                                } else if sidecar {
                                    CursorCoupling::IndependentPlane
                                } else {
                                    CursorCoupling::EmbeddedInPrimary
                                };
                                presented_cursor_from_worker_update(
                                    &update,
                                    revision,
                                    coupling,
                                    delivery,
                                    &presented_planes.cursor,
                                )
                            });
                        let cursor = select_cursor_promotion(
                            primary_cursor_presentation,
                            cursor_plane_promotion.or_else(|| {
                                frozen_primary_cursor_presentation(primary_cursor_presentation)
                            }),
                        );
                        let previous_cursor = presented_planes.cursor;
                        let cursor_for_trace = cursor;
                        if (primary.is_some() || cursor.is_some())
                            && !presented_planes.promote_bundle(identity, identity, primary, cursor)
                        {
                            return Err(io::Error::other(
                                "worker pageflip promotion identity mismatch",
                            )
                            .into());
                        }
                        if let Some(cursor) = cursor_for_trace {
                            trace_presented_cursor(
                                server,
                                cursor_reveal_trace,
                                identity,
                                previous_cursor,
                                cursor,
                            );
                        }
                        if let Some(worker) = kms_commit_worker.as_ref() {
                            worker.set_established_presented_base_for_output(
                                self.output_id,
                                presented_planes.revision,
                                output_epoch.get(),
                                target.crtc_id,
                            );
                        }
                    }
                    submitted_worker_ownership
                        .retain(|ownership| ownership.job.token != pageflip_token);
                }
            }
        }
        if let Some(start_ns) = slow_post_wake_after_pageflip_start_ns {
            self.slow_cycle_trace.record_phase(
                SlowCyclePhase::PostWakeMaintenance,
                start_ns,
                monotonic_now_ns()?,
            );
        }
        Ok(NativeCycleState {
            wakeup,
            work_class: NativeWorkClass::NoOutputWork,
            fast_path_completed: false,
            pageflip_drain_us,
            pageflip_completed,
            completed_pageflip_token,
            frame_completed,
            frame_rendered,
            frame_submitted,
            presentation_admitted: false,
            present_us: 0,
            pageflip_pending_at_tick: false,
            tick_us: 0,
            accepted: 0,
            redraw_requested: *queued_redraw_requested,
            skipped_input_repaints: 0,
            input_drain_us: 0,
            raw_input_events: 0,
            coalesced_input_events: 0,
            cursor_motion_sample_id: None,
            shutdown_requested: false,
        })
    }
}

#[cfg(test)]
#[path = "pageflip_tests.rs"]
mod tests;
