use super::atomic_commit::PendingAtomicCommit;
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PipelineSnapshotError {
    MissingTransaction {
        owner: &'static str,
        transaction_id: OutputTransactionId,
    },
    TerminalTransactionOwnsResource {
        owner: &'static str,
        transaction_id: OutputTransactionId,
    },
    TransactionStateMismatch {
        owner: &'static str,
        transaction_id: OutputTransactionId,
        actual: OutputTransactionStateKind,
    },
    IdentityMismatch {
        owner: &'static str,
        field: &'static str,
        transaction_id: OutputTransactionId,
    },
    MissingSwapchainRole {
        owner: &'static str,
        transaction_id: OutputTransactionId,
    },
    UnexpectedSwapchainRole {
        owner: &'static str,
        transaction_id: OutputTransactionId,
    },
    MissingRenderingTarget,
    PipelineInvariant(PipelineValidationError),
}

impl std::fmt::Display for PipelineSnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for PipelineSnapshotError {}

fn record_for_active_owner<'a>(
    ledger: &'a OutputTransactionLedger,
    owner: &'static str,
    transaction_id: OutputTransactionId,
) -> Result<&'a OutputTransactionRecord, PipelineSnapshotError> {
    if let Some(record) = ledger.transaction(transaction_id) {
        return Ok(record);
    }
    if ledger
        .transaction_including_terminal(transaction_id)
        .is_some()
    {
        return Err(PipelineSnapshotError::TerminalTransactionOwnsResource {
            owner,
            transaction_id,
        });
    }
    Err(PipelineSnapshotError::MissingTransaction {
        owner,
        transaction_id,
    })
}

fn validate_state(
    record: &OutputTransactionRecord,
    owner: &'static str,
    expected: OutputTransactionStateKind,
) -> Result<(), PipelineSnapshotError> {
    let actual = record.state().kind();
    if actual != expected {
        return Err(PipelineSnapshotError::TransactionStateMismatch {
            owner,
            transaction_id: record.descriptor().id(),
            actual,
        });
    }
    Ok(())
}

fn identity_mismatch(
    owner: &'static str,
    field: &'static str,
    transaction_id: OutputTransactionId,
) -> PipelineSnapshotError {
    PipelineSnapshotError::IdentityMismatch {
        owner,
        field,
        transaction_id,
    }
}

fn commit_snapshot(
    owner: &'static str,
    pending: PendingAtomicCommit,
    expected_state: OutputTransactionStateKind,
    swapchain_role: Option<QueuedOutputFrameIdentitySnapshot>,
    ledger: &OutputTransactionLedger,
) -> Result<QueuedCommitSnapshot, PipelineSnapshotError> {
    let (transaction_id, commit_frame_id, commit_framebuffer_id) = match pending.kind {
        AtomicCommitKind::CompositedPrimary {
            transaction_id,
            frame_id,
            framebuffer_id,
        } => (transaction_id, Some(frame_id), Some(framebuffer_id)),
        AtomicCommitKind::DirectPrimary {
            transaction_id,
            direct_token,
            framebuffer_id,
        } => {
            if direct_token != pending.token {
                return Err(identity_mismatch(owner, "direct_token", transaction_id));
            }
            (transaction_id, None, Some(framebuffer_id))
        }
        AtomicCommitKind::PlaneDelta { transaction_id, .. } => (transaction_id, None, None),
    };
    let record = record_for_active_owner(ledger, owner, transaction_id)?;
    validate_state(record, owner, expected_state)?;
    let descriptor = record.descriptor();
    if descriptor.output_generation() != pending.generation {
        return Err(identity_mismatch(
            owner,
            "output_generation",
            transaction_id,
        ));
    }
    match record.state() {
        OutputTransactionState::Submitted { token, .. }
            if expected_state == OutputTransactionStateKind::Submitted =>
        {
            if token != pending.token {
                return Err(identity_mismatch(owner, "token", transaction_id));
            }
        }
        OutputTransactionState::Queued { .. }
            if expected_state == OutputTransactionStateKind::Queued => {}
        _ => {
            return Err(PipelineSnapshotError::TransactionStateMismatch {
                owner,
                transaction_id,
                actual: record.state().kind(),
            });
        }
    }

    let kind = match (
        pending.kind,
        descriptor.content(),
        descriptor.planes().primary(),
    ) {
        (
            AtomicCommitKind::CompositedPrimary { .. },
            OutputTransactionContent::Composited {
                frame_id,
                render_generation,
                pool_generation,
                ..
            },
            PrimaryPlaneAssignment::CompositorFramebuffer {
                slot,
                framebuffer_id,
            },
        ) => {
            if Some(frame_id) != commit_frame_id {
                return Err(identity_mismatch(owner, "frame_id", transaction_id));
            }
            if Some(framebuffer_id) != commit_framebuffer_id {
                return Err(identity_mismatch(owner, "framebuffer_id", transaction_id));
            }
            let physical = swapchain_role.ok_or(PipelineSnapshotError::MissingSwapchainRole {
                owner,
                transaction_id,
            })?;
            if physical.token != pending.token {
                return Err(identity_mismatch(owner, "swapchain_token", transaction_id));
            }
            if physical.frame.transaction_id != transaction_id {
                return Err(identity_mismatch(
                    owner,
                    "swapchain_transaction_id",
                    transaction_id,
                ));
            }
            if physical.frame.frame_id != frame_id {
                return Err(identity_mismatch(
                    owner,
                    "swapchain_frame_id",
                    transaction_id,
                ));
            }
            if physical.frame.render_generation != render_generation {
                return Err(identity_mismatch(
                    owner,
                    "swapchain_render_generation",
                    transaction_id,
                ));
            }
            if physical.frame.pool_generation != pool_generation {
                return Err(identity_mismatch(
                    owner,
                    "swapchain_pool_generation",
                    transaction_id,
                ));
            }
            if physical.frame.slot != slot {
                return Err(identity_mismatch(owner, "swapchain_slot", transaction_id));
            }
            if physical.frame.target != descriptor.target() {
                return Err(identity_mismatch(owner, "swapchain_target", transaction_id));
            }
            PipelineCommitKind::CompositedPrimary {
                transaction_id,
                frame_id,
                slot,
                framebuffer_id,
            }
        }
        (
            AtomicCommitKind::DirectPrimary { framebuffer_id, .. },
            OutputTransactionContent::Direct { key, .. },
            PrimaryPlaneAssignment::ClientFramebuffer {
                key: plane_key,
                framebuffer_id: plane_framebuffer_id,
            },
        ) => {
            if swapchain_role.is_some() {
                return Err(PipelineSnapshotError::UnexpectedSwapchainRole {
                    owner,
                    transaction_id,
                });
            }
            if key != plane_key || framebuffer_id != plane_framebuffer_id {
                return Err(identity_mismatch(owner, "direct_content", transaction_id));
            }
            PipelineCommitKind::DirectPrimary {
                transaction_id,
                key,
                framebuffer_id,
            }
        }
        (
            AtomicCommitKind::PlaneDelta {
                cursor_epoch,
                framebuffer_id,
                ..
            },
            OutputTransactionContent::PlaneDelta { changed, .. },
            PrimaryPlaneAssignment::Unchanged,
        ) => {
            if swapchain_role.is_some() {
                return Err(PipelineSnapshotError::UnexpectedSwapchainRole {
                    owner,
                    transaction_id,
                });
            }
            if changed.validate_cursor_delta().is_err() {
                return Err(identity_mismatch(owner, "cursor_epoch", transaction_id));
            }
            match descriptor.planes().cursor() {
                CursorPlaneAssignment::Atomic {
                    desired_epoch,
                    state,
                } if *desired_epoch == cursor_epoch
                    && state.as_ref().and_then(|state| state.framebuffer_id) == framebuffer_id => {}
                _ => return Err(identity_mismatch(owner, "cursor_plan", transaction_id)),
            }
            PipelineCommitKind::PlaneDelta {
                transaction_id,
                cursor_epoch,
                framebuffer_id,
            }
        }
        _ => return Err(identity_mismatch(owner, "content_kind", transaction_id)),
    };

    Ok(QueuedCommitSnapshot {
        token: pending.token,
        output_generation: pending.generation,
        crtc_id: pending.crtc_id,
        target: descriptor.target(),
        kind,
    })
}

fn validate_confirmed_primary(
    current: ConfirmedPrimaryState,
    swapchain: &AtomicOutputSwapchain,
    ledger: &OutputTransactionLedger,
) -> Result<(), PipelineSnapshotError> {
    let (transaction_id, owner) = match current {
        ConfirmedPrimaryState::Composed { transaction_id, .. } => {
            (transaction_id, "current_composed")
        }
        ConfirmedPrimaryState::Direct { transaction_id, .. } => (transaction_id, "current_direct"),
    };
    let record = ledger
        .transaction_including_terminal(transaction_id)
        .ok_or(PipelineSnapshotError::MissingTransaction {
            owner,
            transaction_id,
        })?;
    if !matches!(
        record.state(),
        OutputTransactionState::Terminal(OutputTransactionTerminal::Presented { .. })
    ) {
        return Err(PipelineSnapshotError::TransactionStateMismatch {
            owner,
            transaction_id,
            actual: record.state().kind(),
        });
    }
    match (
        current,
        record.descriptor().content(),
        record.descriptor().planes().primary(),
    ) {
        (
            ConfirmedPrimaryState::Composed { slot, .. },
            OutputTransactionContent::Composited { .. },
            PrimaryPlaneAssignment::CompositorFramebuffer {
                slot: plane_slot, ..
            },
        ) if slot == plane_slot && slot == swapchain.current() => Ok(()),
        (
            ConfirmedPrimaryState::Direct {
                surface_id,
                key,
                framebuffer_id,
                ..
            },
            OutputTransactionContent::Direct {
                key: content_key, ..
            },
            PrimaryPlaneAssignment::ClientFramebuffer {
                key: plane_key,
                framebuffer_id: plane_framebuffer_id,
            },
        ) if key == content_key
            && key == plane_key
            && framebuffer_id == plane_framebuffer_id
            && record.descriptor().obligations().direct_surface_id() == Some(surface_id) =>
        {
            Ok(())
        }
        _ => Err(identity_mismatch(
            owner,
            "confirmed_primary",
            transaction_id,
        )),
    }
}

pub(super) fn require_explicit_output_swapchain(
    scanout: &NativeScanoutBackend,
) -> io::Result<&AtomicOutputSwapchain> {
    scanout
        .explicit_output_swapchain()
        .ok_or_else(|| io::Error::other("explicit Atomic presentation has no output swapchain"))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_output_pipeline_snapshot(
    output_generation: u64,
    pacing_mode: NativeOutputPacingMode,
    swapchain: &AtomicOutputSwapchain,
    ledger: &OutputTransactionLedger,
    arbiter: &AtomicCommitArbiter,
    current_primary: Option<ConfirmedPrimaryState>,
    rendering_target: Option<PresentationTarget>,
    triple_capability: TripleCapability,
    legacy_cursor: Option<&crate::native_output::output::NativeAtomicCursor>,
) -> Result<OutputPipelineSnapshot, PipelineSnapshotError> {
    swapchain
        .validate_invariants_for(pacing_mode)
        .map_err(|_| {
            PipelineSnapshotError::PipelineInvariant(PipelineValidationError::SlotAliasing {
                slot: swapchain.current(),
            })
        })?;
    if let Some(current) = current_primary {
        validate_confirmed_primary(current, swapchain, ledger)?;
    }
    let kernel_submitted = arbiter
        .kernel_submitted_commit()
        .map(|pending| {
            commit_snapshot(
                "kernel_submitted",
                pending,
                OutputTransactionStateKind::Submitted,
                swapchain.pending_identity(),
                ledger,
            )
        })
        .transpose()?;
    if kernel_submitted.is_none()
        && let Some(physical) = swapchain.pending_identity()
    {
        return Err(PipelineSnapshotError::UnexpectedSwapchainRole {
            owner: "swapchain_pending",
            transaction_id: physical.frame.transaction_id,
        });
    }
    let worker_queued_next = arbiter
        .worker_queued_commit()
        .map(|pending| {
            commit_snapshot(
                "worker_queued_next",
                pending,
                OutputTransactionStateKind::Queued,
                swapchain.worker_queued_identity(),
                ledger,
            )
        })
        .transpose()?;
    if worker_queued_next.is_none()
        && let Some(physical) = swapchain.worker_queued_identity()
    {
        return Err(PipelineSnapshotError::UnexpectedSwapchainRole {
            owner: "swapchain_worker_queued",
            transaction_id: physical.frame.transaction_id,
        });
    }
    let prepared = if let Some(ready) = swapchain.ready_identity() {
        let record = record_for_active_owner(ledger, "prepared_ready", ready.transaction_id)?;
        validate_state(record, "prepared_ready", OutputTransactionStateKind::Ready)?;
        let descriptor = record.descriptor();
        match (descriptor.content(), descriptor.planes().primary()) {
            (
                OutputTransactionContent::Composited {
                    frame_id,
                    render_generation,
                    pool_generation,
                    ..
                },
                PrimaryPlaneAssignment::CompositorFramebuffer { slot, .. },
            ) if frame_id == ready.frame_id
                && pool_generation == ready.pool_generation
                && render_generation == ready.render_generation
                && slot == ready.slot
                && descriptor.target() == ready.target
                && descriptor.output_generation() == output_generation =>
            {
                PreparedCompositedState::Ready {
                    transaction_id: ready.transaction_id,
                    slot: ready.slot,
                    target: ready.target,
                    fence_state: PreparedFenceState::SubmitWithInFence,
                }
            }
            _ => {
                return Err(identity_mismatch(
                    "prepared_ready",
                    "ready_identity",
                    ready.transaction_id,
                ));
            }
        }
    } else if let Some(slot) = swapchain.rendering_slot() {
        PreparedCompositedState::Rendering {
            slot,
            target: rendering_target.ok_or(PipelineSnapshotError::MissingRenderingTarget)?,
        }
    } else {
        PreparedCompositedState::None
    };
    let mut snapshot = OutputPipelineSnapshot {
        output_generation,
        pacing_mode,
        presented_planes: crate::native_output::presentation::plane::PresentedPlaneSnapshot::legacy(
            current_primary,
        ),
        current_primary,
        kernel_submitted,
        worker_queued_next,
        prepared,
        free_compositor_slots: u8::try_from(swapchain.free_slot_count()).unwrap_or(u8::MAX),
        triple_capability,
    };
    if let Some(cursor) = legacy_cursor {
        snapshot.presented_planes.cursor = cursor.presented_plane_state();
        debug_assert!(
            snapshot
                .presented_planes
                .cursor
                .kms_equivalent_to(cursor.current())
        );
    }
    snapshot
        .validate()
        .map_err(PipelineSnapshotError::PipelineInvariant)?;
    Ok(snapshot)
}

impl NativeRuntime {
    pub(super) fn validate_output_pipeline(
        &self,
    ) -> Result<Option<OutputPipelineSnapshot>, PipelineSnapshotError> {
        let Some(swapchain) = self.scanout.explicit_output_swapchain() else {
            return Ok(None);
        };
        let capability = if swapchain.is_poisoned() {
            TripleCapability::Unavailable(TripleCapabilityBlocker::SwapchainPoisoned)
        } else {
            TripleCapability::Capable
        };
        let snapshot = build_output_pipeline_snapshot(
            self.drm_file_generation,
            self.adaptive_buffering.pacing_mode(),
            swapchain,
            &self.output_transactions,
            &self.atomic_commit_arbiter,
            self.confirmed_primary_assignment,
            self.scheduled_presentation_target,
            capability,
            self.atomic_cursor.as_ref(),
        )?;
        Ok(Some(snapshot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn transaction_id(value: u64) -> OutputTransactionId {
        OutputTransactionId::new(NonZeroU64::new(value).unwrap())
    }

    fn token(value: u64) -> PageFlipToken {
        PageFlipToken::new(value).unwrap()
    }

    fn slots() -> OutputSlotSet {
        OutputSlotSet::new([
            OutputSlotId::new(0).unwrap(),
            OutputSlotId::new(1).unwrap(),
            OutputSlotId::new(2).unwrap(),
        ])
        .unwrap()
    }

    fn swapchain() -> AtomicOutputSwapchain {
        AtomicOutputSwapchain::from_presented_slots(slots(), OutputSlotId::new(0).unwrap(), 1)
            .unwrap()
    }

    fn sync_fd() -> OwnedFd {
        let mut pipe = [-1; 2];
        assert_eq!(
            unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        unsafe { libc::close(pipe[1]) };
        unsafe { OwnedFd::from_raw_fd(pipe[0]) }
    }

    fn ready_swapchain() -> AtomicOutputSwapchain {
        let mut swapchain = swapchain();
        let slot = swapchain.acquire_render_slot().unwrap();
        swapchain
            .finish_render(
                slot,
                5,
                crate::egl_renderer::native_fence::NativeRenderFence::from_submission_fd(sync_fd()),
            )
            .unwrap();
        swapchain
    }

    fn frame_batch(frame_id: u64) -> oblivion_one::compositor::CompositorFrameBatchId {
        static NEXT_SOCKET: AtomicU64 = AtomicU64::new(1);
        let socket = format!(
            "typhon-pipeline-snapshot-test-{}-{}",
            std::process::id(),
            NEXT_SOCKET.fetch_add(1, Ordering::Relaxed),
        );
        let mut server =
            OwnCompositorServer::bind(socket).expect("pipeline snapshot test Wayland socket");
        server.take_frame_batch_for_render(frame_id)
    }

    fn insert_composited(
        ledger: &mut OutputTransactionLedger,
        identity: OutputFrameIdentitySnapshot,
        slot: OutputSlotId,
        framebuffer_id: u32,
    ) {
        let transaction = OutputTransaction::composited(
            identity.transaction_id,
            1,
            MonotonicTimestampNs::new(0),
            identity.target,
            NativeOutputPacingMode::PredictiveTriple,
            identity.frame_id,
            identity.render_generation,
            identity.pool_generation,
            slot,
            framebuffer_id,
            None,
            frame_batch(identity.frame_id),
        )
        .unwrap();
        ledger.insert(transaction).unwrap();
        ledger
            .mark_ready(identity.transaction_id, MonotonicTimestampNs::new(0))
            .unwrap();
    }

    fn cursor_target() -> PresentationTarget {
        PresentationTarget {
            sequence: 1,
            presentation_time: MonotonicTimestampNs::new(10),
            submit_not_before: MonotonicTimestampNs::new(8),
            render_start_deadline: MonotonicTimestampNs::new(6),
            refresh_interval: std::time::Duration::from_nanos(10),
            reason: PresentationTargetReason::ForcedValidation,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
        }
    }

    fn insert_cursor(ledger: &mut OutputTransactionLedger) -> OutputTransactionId {
        let id = transaction_id(1);
        ledger
            .insert(
                OutputTransaction::cursor_plane_delta(
                    id,
                    1,
                    MonotonicTimestampNs::new(0),
                    cursor_target(),
                    NativeOutputPacingMode::ReactiveDouble,
                    7,
                    Some(AtomicCursorVisualState {
                        visible: true,
                        x: 0,
                        y: 0,
                        hotspot_x: 0,
                        hotspot_y: 0,
                        width: 64,
                        height: 64,
                        framebuffer_id: Some(90),
                        image_generation: 1,
                    }),
                    OutputReleasePlan::Pageflip,
                )
                .unwrap(),
            )
            .unwrap();
        id
    }

    fn direct_key() -> DirectScanoutCandidateKey {
        DirectScanoutCandidateKey {
            content: OutputContentKey::new(
                9,
                NonZeroU64::new(42).unwrap(),
                ContentEpochId::new(NonZeroU64::new(3).unwrap()),
                1920,
                1080,
                0x3432_5241,
                0,
                0,
                1_000,
                0,
            ),
            output_generation: 1,
            cursor_content_key: None,
            color_epoch: 0,
        }
    }

    #[test]
    fn exact_ready_mapping_builds_snapshot() {
        let swapchain = ready_swapchain();
        let ready = swapchain.ready_identity().unwrap();
        let mut ledger = OutputTransactionLedger::new();
        insert_composited(&mut ledger, ready, ready.slot, 42);

        let snapshot = build_output_pipeline_snapshot(
            1,
            NativeOutputPacingMode::PredictiveTriple,
            &swapchain,
            &ledger,
            &AtomicCommitArbiter::new(),
            None,
            None,
            TripleCapability::Capable,
            None,
        )
        .unwrap();

        assert!(matches!(
            snapshot.prepared,
            PreparedCompositedState::Ready {
                transaction_id,
                slot,
                target,
                ..
            } if transaction_id == ready.transaction_id
                && slot == ready.slot
                && target == ready.target
        ));
    }

    #[test]
    fn ready_transaction_missing_from_ledger_is_rejected() {
        let swapchain = ready_swapchain();
        assert!(matches!(
            build_output_pipeline_snapshot(
                1,
                NativeOutputPacingMode::PredictiveTriple,
                &swapchain,
                &OutputTransactionLedger::new(),
                &AtomicCommitArbiter::new(),
                None,
                None,
                TripleCapability::Capable,
                None,
            ),
            Err(PipelineSnapshotError::MissingTransaction {
                owner: "prepared_ready",
                ..
            })
        ));
    }

    #[test]
    fn ready_slot_mismatch_between_ledger_and_swapchain_is_rejected() {
        let swapchain = ready_swapchain();
        let ready = swapchain.ready_identity().unwrap();
        let mut ledger = OutputTransactionLedger::new();
        insert_composited(&mut ledger, ready, OutputSlotId::new(2).unwrap(), 42);

        assert!(matches!(
            build_output_pipeline_snapshot(
                1,
                NativeOutputPacingMode::PredictiveTriple,
                &swapchain,
                &ledger,
                &AtomicCommitArbiter::new(),
                None,
                None,
                TripleCapability::Capable,
                None,
            ),
            Err(PipelineSnapshotError::IdentityMismatch {
                owner: "prepared_ready",
                field: "ready_identity",
                ..
            })
        ));
    }

    #[test]
    fn arbiter_token_mismatch_with_submitted_ledger_is_rejected() {
        let mut ledger = OutputTransactionLedger::new();
        let id = insert_cursor(&mut ledger);
        ledger
            .mark_submitted(id, token(2), MonotonicTimestampNs::new(1))
            .unwrap();
        let mut arbiter = AtomicCommitArbiter::new();
        arbiter
            .reserve(
                token(1),
                1,
                7,
                AtomicCommitKind::PlaneDelta {
                    transaction_id: id,
                    cursor_epoch: 7,
                    framebuffer_id: Some(90),
                },
                1,
            )
            .unwrap();

        assert_eq!(
            build_output_pipeline_snapshot(
                1,
                NativeOutputPacingMode::ReactiveDouble,
                &swapchain(),
                &ledger,
                &arbiter,
                None,
                None,
                TripleCapability::Capable,
                None,
            ),
            Err(PipelineSnapshotError::IdentityMismatch {
                owner: "kernel_submitted",
                field: "token",
                transaction_id: id,
            })
        );
    }

    #[test]
    fn terminal_transaction_still_owned_by_worker_is_rejected() {
        let mut ledger = OutputTransactionLedger::new();
        let id = insert_cursor(&mut ledger);
        ledger
            .mark_queued(id, 1, MonotonicTimestampNs::new(1))
            .unwrap();
        ledger
            .mark_failed(
                id,
                OutputTransactionFailureStage::KmsSubmit,
                MonotonicTimestampNs::new(2),
            )
            .unwrap();
        let mut arbiter = AtomicCommitArbiter::new();
        arbiter
            .reserve_worker_queued(
                token(1),
                1,
                7,
                AtomicCommitKind::PlaneDelta {
                    transaction_id: id,
                    cursor_epoch: 7,
                    framebuffer_id: Some(90),
                },
                1,
            )
            .unwrap();

        assert_eq!(
            build_output_pipeline_snapshot(
                1,
                NativeOutputPacingMode::ReactiveDouble,
                &swapchain(),
                &ledger,
                &arbiter,
                None,
                None,
                TripleCapability::Capable,
                None,
            ),
            Err(PipelineSnapshotError::TerminalTransactionOwnsResource {
                owner: "worker_queued_next",
                transaction_id: id,
            })
        );
    }

    #[test]
    fn direct_transaction_incorrectly_owning_output_slot_is_rejected() {
        let mut swapchain = ready_swapchain();
        let ready = swapchain.ready_identity().unwrap();
        let pageflip = token(1);
        let _fence = swapchain
            .take_ready_for_worker(pageflip, MonotonicTimestampNs::new(1))
            .unwrap();
        let key = direct_key();
        let mut ledger = OutputTransactionLedger::new();
        ledger
            .insert(
                OutputTransaction::direct(
                    ready.transaction_id,
                    1,
                    MonotonicTimestampNs::new(0),
                    ready.target,
                    NativeOutputPacingMode::PredictiveTriple,
                    ready.frame_id,
                    key,
                    42,
                    None,
                    frame_batch(ready.frame_id),
                    9,
                    OutputReleasePlan::Pageflip,
                )
                .unwrap(),
            )
            .unwrap();
        ledger
            .mark_queued(ready.transaction_id, 1, MonotonicTimestampNs::new(1))
            .unwrap();
        let mut arbiter = AtomicCommitArbiter::new();
        arbiter
            .reserve_worker_queued(
                pageflip,
                1,
                7,
                AtomicCommitKind::DirectPrimary {
                    transaction_id: ready.transaction_id,
                    direct_token: pageflip,
                    framebuffer_id: 42,
                },
                1,
            )
            .unwrap();

        assert_eq!(
            build_output_pipeline_snapshot(
                1,
                NativeOutputPacingMode::PredictiveTriple,
                &swapchain,
                &ledger,
                &arbiter,
                None,
                None,
                TripleCapability::Capable,
                None,
            ),
            Err(PipelineSnapshotError::UnexpectedSwapchainRole {
                owner: "worker_queued_next",
                transaction_id: ready.transaction_id,
            })
        );
    }

    #[test]
    fn exact_kernel_submission_and_current_mapping_are_validated() {
        let mut swapchain = ready_swapchain();
        let ready = swapchain.ready_identity().unwrap();
        let mut ledger = OutputTransactionLedger::new();
        insert_composited(&mut ledger, ready, ready.slot, 42);
        let pageflip = token(11);
        swapchain.submit_ready(pageflip, None).unwrap();
        ledger
            .mark_submitted(ready.transaction_id, pageflip, MonotonicTimestampNs::new(1))
            .unwrap();
        let mut arbiter = AtomicCommitArbiter::new();
        arbiter
            .reserve(
                pageflip,
                1,
                7,
                AtomicCommitKind::CompositedPrimary {
                    transaction_id: ready.transaction_id,
                    frame_id: ready.frame_id,
                    framebuffer_id: 42,
                },
                1,
            )
            .unwrap();

        let submitted = build_output_pipeline_snapshot(
            1,
            NativeOutputPacingMode::PredictiveTriple,
            &swapchain,
            &ledger,
            &arbiter,
            None,
            None,
            TripleCapability::Capable,
            None,
        )
        .unwrap();
        assert_eq!(
            submitted.kernel_submitted.unwrap().kind,
            PipelineCommitKind::CompositedPrimary {
                transaction_id: ready.transaction_id,
                frame_id: ready.frame_id,
                slot: ready.slot,
                framebuffer_id: 42,
            }
        );

        swapchain.complete_pageflip(pageflip, 1).unwrap();
        arbiter.complete(pageflip, 1, 7);
        ledger
            .mark_presented(
                ready.transaction_id,
                pageflip,
                1,
                MonotonicTimestampNs::new(2),
                Some(1),
            )
            .unwrap();
        let current = ConfirmedPrimaryState::Composed {
            transaction_id: ready.transaction_id,
            token: pageflip,
            slot: ready.slot,
        };
        let confirmed = build_output_pipeline_snapshot(
            1,
            NativeOutputPacingMode::ReactiveDouble,
            &swapchain,
            &ledger,
            &arbiter,
            Some(current),
            None,
            TripleCapability::Capable,
            None,
        )
        .unwrap();
        assert_eq!(confirmed.current_primary, Some(current));
    }
}
