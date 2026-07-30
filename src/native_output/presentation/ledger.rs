#![allow(dead_code)]

use super::transaction::{OutputTransaction, OutputTransactionContent, OutputTransactionId};
use oblivion_one::compositor::CompositorFrameBatchId;
use oblivion_one::native::kms::PageFlipToken;
use oblivion_one::native::presentation_deadline::MonotonicTimestampNs;
use std::collections::{HashMap, VecDeque};

pub(crate) const DEFAULT_OUTPUT_TRANSACTION_ACTIVE_CAPACITY: usize = 8;
pub(crate) const DEFAULT_OUTPUT_TRANSACTION_HISTORY_CAPACITY: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputTransactionState {
    Built,
    Ready {
        ready_at: MonotonicTimestampNs,
    },
    Queued {
        queued_at: MonotonicTimestampNs,
        worker_generation: u64,
    },
    Submitted {
        token: PageFlipToken,
        submitted_at: MonotonicTimestampNs,
    },
    Settling {
        terminal: OutputTransactionTerminal,
    },
    Terminal(OutputTransactionTerminal),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputTransactionTerminal {
    Presented {
        presented_at: MonotonicTimestampNs,
        actual_sequence: Option<u64>,
    },
    Dropped {
        reason: OutputTransactionDropReason,
        at: MonotonicTimestampNs,
    },
    Superseded {
        by: Option<OutputTransactionId>,
        reason: OutputTransactionSupersedeReason,
        at: MonotonicTimestampNs,
    },
    Failed {
        stage: OutputTransactionFailureStage,
        at: MonotonicTimestampNs,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputTransactionDropReason {
    NoVisualChange,
    OutputDestroyed,
    SessionSuspended,
    SafeAbandonment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputTransactionSupersedeReason {
    NewerTransaction,
    SameContentSuppressed,
    DirectTransition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputTransactionFailureStage {
    RenderPreparation,
    RenderExecution,
    FenceExport,
    KmsSubmit,
    BackendOwnershipTransfer,
    PageflipValidation,
    OutputLost,
    SessionLost,
    ShutdownAbandonment,
    BackendCompletion,
    ProtocolSettlement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputTransactionStateKind {
    Built,
    Ready,
    Queued,
    Submitted,
    Settling,
    Terminal,
}

impl OutputTransactionState {
    pub(crate) const fn kind(self) -> OutputTransactionStateKind {
        match self {
            Self::Built => OutputTransactionStateKind::Built,
            Self::Ready { .. } => OutputTransactionStateKind::Ready,
            Self::Queued { .. } => OutputTransactionStateKind::Queued,
            Self::Submitted { .. } => OutputTransactionStateKind::Submitted,
            Self::Settling { .. } => OutputTransactionStateKind::Settling,
            Self::Terminal(_) => OutputTransactionStateKind::Terminal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputTransactionTransitionKind {
    Ready,
    Queued,
    Submitted,
    Presented,
    Dropped,
    Superseded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputTransactionError {
    ActiveCapacityExceeded,
    DuplicateId,
    UnknownTransaction,
    DuplicateObligationOwner,
    InvalidTransition {
        from: OutputTransactionStateKind,
        requested: OutputTransactionTransitionKind,
    },
    TokenMismatch,
    GenerationMismatch,
    FailureStageMismatch {
        state: OutputTransactionStateKind,
        stage: OutputTransactionFailureStage,
    },
}

impl std::fmt::Display for OutputTransactionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for OutputTransactionError {}

#[derive(Debug, Clone)]
pub(crate) struct OutputTransactionRecord {
    descriptor: OutputTransaction,
    state: OutputTransactionState,
}

impl OutputTransactionRecord {
    pub(crate) const fn descriptor(&self) -> &OutputTransaction {
        &self.descriptor
    }

    pub(crate) const fn state(&self) -> OutputTransactionState {
        self.state
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AcceptedTerminalTransition {
    transaction_id: OutputTransactionId,
    obligations: super::transaction::OutputProtocolObligations,
    prior_state: OutputTransactionState,
    terminal: OutputTransactionTerminal,
}

impl AcceptedTerminalTransition {
    pub(crate) const fn transaction_id(self) -> OutputTransactionId {
        self.transaction_id
    }

    pub(crate) const fn obligations(self) -> super::transaction::OutputProtocolObligations {
        self.obligations
    }

    pub(crate) const fn terminal(self) -> OutputTransactionTerminal {
        self.terminal
    }

    pub(crate) const fn prior_state(self) -> OutputTransactionState {
        self.prior_state
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputTransactionCounters {
    pub(crate) built: u64,
    pub(crate) ready: u64,
    pub(crate) submitted: u64,
    pub(crate) presented: u64,
    pub(crate) dropped: u64,
    pub(crate) superseded: u64,
    pub(crate) failed: u64,
    pub(crate) invalid_transitions: u64,
    pub(crate) duplicate_obligation_attempts: u64,
    pub(crate) duplicate_settlement_attempts: u64,
    pub(crate) active_peak: u64,
    pub(crate) terminal_history_overwrites: u64,
    pub(crate) terminal_transitions_accepted: u64,
    pub(crate) terminal_transitions_finalized: u64,
    pub(crate) terminal_transitions_rejected: u64,
    pub(crate) settlement_failures: u64,
    pub(crate) failure_stage_mismatches: u64,
    pub(crate) active_settling_transactions: u64,
    pub(crate) immediate_presentations: u64,
    pub(crate) immediate_presentation_failures: u64,
    pub(crate) immediate_presentations_accepted: u64,
    pub(crate) immediate_presentations_finalized: u64,
    pub(crate) compatibility_noops: u64,
    pub(crate) compatibility_failures: u64,
    pub(crate) queued: u64,
    pub(crate) queued_composited: u64,
    pub(crate) queued_direct: u64,
    pub(crate) queued_cursor_only: u64,
    pub(crate) queued_compatibility: u64,
    pub(crate) queue_wait_ns_total: u64,
    pub(crate) queue_wait_ns_max: u64,
    pub(crate) built_composited: u64,
    pub(crate) built_direct: u64,
    pub(crate) built_cursor_only: u64,
    pub(crate) submitted_composited: u64,
    pub(crate) submitted_direct: u64,
    pub(crate) submitted_cursor_only: u64,
    pub(crate) presented_composited: u64,
    pub(crate) presented_direct: u64,
    pub(crate) presented_cursor_only: u64,
}

#[derive(Debug)]
pub(crate) struct OutputTransactionLedger {
    allocator: super::transaction::OutputTransactionAllocator,
    active_capacity: usize,
    history_capacity: usize,
    active: HashMap<OutputTransactionId, OutputTransactionRecord>,
    obligation_owner: HashMap<CompositorFrameBatchId, OutputTransactionId>,
    recent_terminal: VecDeque<OutputTransactionRecord>,
    counters: OutputTransactionCounters,
    last_created: Option<OutputTransactionId>,
}

impl Default for OutputTransactionLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputTransactionLedger {
    pub(crate) fn new() -> Self {
        let history_capacity = std::env::var("OBLIVION_ONE_OUTPUT_TRANSACTION_HISTORY")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(DEFAULT_OUTPUT_TRANSACTION_HISTORY_CAPACITY)
            .clamp(64, 65_536);
        Self::with_capacities(DEFAULT_OUTPUT_TRANSACTION_ACTIVE_CAPACITY, history_capacity)
    }

    pub(crate) fn with_capacities(active_capacity: usize, history_capacity: usize) -> Self {
        Self {
            allocator: super::transaction::OutputTransactionAllocator::default(),
            active_capacity,
            history_capacity: history_capacity.max(1),
            active: HashMap::new(),
            obligation_owner: HashMap::new(),
            recent_terminal: VecDeque::new(),
            counters: OutputTransactionCounters::default(),
            last_created: None,
        }
    }

    pub(crate) fn allocate_id(
        &mut self,
    ) -> Result<OutputTransactionId, super::transaction::OutputTransactionAllocationError> {
        let id = self.allocator.allocate()?;
        self.last_created = Some(id);
        Ok(id)
    }

    pub(crate) fn insert(
        &mut self,
        descriptor: OutputTransaction,
    ) -> Result<(), OutputTransactionError> {
        let id = descriptor.id();
        if self.active.contains_key(&id)
            || self
                .recent_terminal
                .iter()
                .any(|record| record.descriptor.id() == id)
        {
            return Err(OutputTransactionError::DuplicateId);
        }
        if self.active.len() >= self.active_capacity {
            return Err(OutputTransactionError::ActiveCapacityExceeded);
        }
        if let Some(batch_id) = descriptor.obligations().frame_batch_id()
            && self.obligation_owner.contains_key(&batch_id)
        {
            self.counters.duplicate_obligation_attempts = self
                .counters
                .duplicate_obligation_attempts
                .saturating_add(1);
            return Err(OutputTransactionError::DuplicateObligationOwner);
        }
        if let Some(batch_id) = descriptor.obligations().frame_batch_id() {
            self.obligation_owner.insert(batch_id, id);
        }
        let content = descriptor.content();
        self.active.insert(
            id,
            OutputTransactionRecord {
                descriptor,
                state: OutputTransactionState::Built,
            },
        );
        self.counters.built = self.counters.built.saturating_add(1);
        match content {
            OutputTransactionContent::Composited { .. } => {
                self.counters.built_composited = self.counters.built_composited.saturating_add(1);
            }
            OutputTransactionContent::Direct { .. } => {
                self.counters.built_direct = self.counters.built_direct.saturating_add(1);
            }
            OutputTransactionContent::CompatibilityImmediate { .. } => {}
            OutputTransactionContent::CursorOnly { .. } => {
                self.counters.built_cursor_only = self.counters.built_cursor_only.saturating_add(1);
            }
        }
        self.counters.active_peak = self.counters.active_peak.max(self.active.len() as u64);
        Ok(())
    }

    pub(crate) fn mark_ready(
        &mut self,
        id: OutputTransactionId,
        ready_at: MonotonicTimestampNs,
    ) -> Result<(), OutputTransactionError> {
        let state = self.state(id)?;
        if !matches!(state, OutputTransactionState::Built) {
            return Err(self.invalid_transition(state, OutputTransactionTransitionKind::Ready));
        }
        self.transition(id, OutputTransactionTransitionKind::Ready, |state| {
            *state = OutputTransactionState::Ready { ready_at };
            Ok(())
        })?;
        self.counters.ready = self.counters.ready.saturating_add(1);
        Ok(())
    }

    pub(crate) fn mark_queued(
        &mut self,
        id: OutputTransactionId,
        worker_generation: u64,
        queued_at: MonotonicTimestampNs,
    ) -> Result<(), OutputTransactionError> {
        let state = self.state(id)?;
        if !matches!(
            state,
            OutputTransactionState::Built | OutputTransactionState::Ready { .. }
        ) {
            return Err(self.invalid_transition(state, OutputTransactionTransitionKind::Queued));
        }
        self.transition(id, OutputTransactionTransitionKind::Queued, |state| {
            *state = OutputTransactionState::Queued {
                queued_at,
                worker_generation,
            };
            Ok(())
        })?;
        self.counters.queued = self.counters.queued.saturating_add(1);
        self.note_path_counter(id, |counters, content| match content {
            OutputTransactionContent::Composited { .. } => {
                counters.queued_composited = counters.queued_composited.saturating_add(1)
            }
            OutputTransactionContent::Direct { .. } => {
                counters.queued_direct = counters.queued_direct.saturating_add(1)
            }
            OutputTransactionContent::CursorOnly { .. } => {
                counters.queued_cursor_only = counters.queued_cursor_only.saturating_add(1)
            }
            OutputTransactionContent::CompatibilityImmediate { .. } => {
                counters.queued_compatibility = counters.queued_compatibility.saturating_add(1)
            }
        });
        Ok(())
    }

    pub(crate) fn rollback_queued(
        &mut self,
        id: OutputTransactionId,
    ) -> Result<(), OutputTransactionError> {
        let state = self.state(id)?;
        if !matches!(state, OutputTransactionState::Queued { .. }) {
            return Err(self.invalid_transition(state, OutputTransactionTransitionKind::Queued));
        }
        let content = self
            .active
            .get(&id)
            .expect("queued transaction was observed above")
            .descriptor
            .content();
        self.active
            .get_mut(&id)
            .expect("queued transaction was observed above")
            .state = OutputTransactionState::Built;
        self.counters.queued = self.counters.queued.saturating_sub(1);
        match content {
            OutputTransactionContent::Composited { .. } => {
                self.counters.queued_composited = self.counters.queued_composited.saturating_sub(1)
            }
            OutputTransactionContent::Direct { .. } => {
                self.counters.queued_direct = self.counters.queued_direct.saturating_sub(1)
            }
            OutputTransactionContent::CursorOnly { .. } => {
                self.counters.queued_cursor_only =
                    self.counters.queued_cursor_only.saturating_sub(1)
            }
            OutputTransactionContent::CompatibilityImmediate { .. } => {
                self.counters.queued_compatibility =
                    self.counters.queued_compatibility.saturating_sub(1)
            }
        }
        Ok(())
    }

    pub(crate) fn mark_submitted(
        &mut self,
        id: OutputTransactionId,
        token: PageFlipToken,
        submitted_at: MonotonicTimestampNs,
    ) -> Result<(), OutputTransactionError> {
        let state = self.state(id)?;
        let queued_at = match state {
            OutputTransactionState::Built | OutputTransactionState::Ready { .. } => None,
            OutputTransactionState::Queued { queued_at, .. } => Some(queued_at),
            _ => {
                return Err(
                    self.invalid_transition(state, OutputTransactionTransitionKind::Submitted)
                );
            }
        };
        if let Some(queued_at) = queued_at {
            let wait_ns = submitted_at.get().saturating_sub(queued_at.get());
            self.counters.queue_wait_ns_total =
                self.counters.queue_wait_ns_total.saturating_add(wait_ns);
            self.counters.queue_wait_ns_max = self.counters.queue_wait_ns_max.max(wait_ns);
        }
        if !matches!(
            state,
            OutputTransactionState::Built
                | OutputTransactionState::Ready { .. }
                | OutputTransactionState::Queued { .. }
        ) {
            return Err(self.invalid_transition(state, OutputTransactionTransitionKind::Submitted));
        }
        self.transition(id, OutputTransactionTransitionKind::Submitted, |state| {
            *state = OutputTransactionState::Submitted {
                token,
                submitted_at,
            };
            Ok(())
        })?;
        self.counters.submitted = self.counters.submitted.saturating_add(1);
        self.note_path_counter(id, |counters, content| match content {
            OutputTransactionContent::Composited { .. } => {
                counters.submitted_composited = counters.submitted_composited.saturating_add(1)
            }
            OutputTransactionContent::Direct { .. } => {
                counters.submitted_direct = counters.submitted_direct.saturating_add(1)
            }
            OutputTransactionContent::CursorOnly { .. } => {
                counters.submitted_cursor_only = counters.submitted_cursor_only.saturating_add(1)
            }
            OutputTransactionContent::CompatibilityImmediate { .. } => {}
        });
        Ok(())
    }

    pub(crate) fn accept_presented(
        &mut self,
        id: OutputTransactionId,
        token: PageFlipToken,
        output_generation: u64,
        presented_at: MonotonicTimestampNs,
        actual_sequence: Option<u64>,
    ) -> Result<AcceptedTerminalTransition, OutputTransactionError> {
        let Some((descriptor_generation, state)) = self
            .active
            .get(&id)
            .map(|record| (record.descriptor.output_generation(), record.state))
        else {
            if self
                .recent_terminal
                .iter()
                .any(|record| record.descriptor.id() == id)
            {
                self.counters.duplicate_settlement_attempts = self
                    .counters
                    .duplicate_settlement_attempts
                    .saturating_add(1);
                return Err(self.reject_terminal(OutputTransactionError::UnknownTransaction));
            }
            return Err(self.reject_terminal(OutputTransactionError::UnknownTransaction));
        };
        if descriptor_generation != output_generation {
            return Err(self.reject_terminal(OutputTransactionError::GenerationMismatch));
        }
        match state {
            OutputTransactionState::Submitted {
                token: expected, ..
            } if expected == token => {}
            OutputTransactionState::Submitted { .. } => {
                return Err(self.reject_terminal(OutputTransactionError::TokenMismatch));
            }
            state => {
                return Err(self
                    .reject_invalid_transition(state, OutputTransactionTransitionKind::Presented));
            }
        }
        self.accept_state(
            id,
            OutputTransactionTerminal::Presented {
                presented_at,
                actual_sequence,
            },
        )
    }

    pub(crate) fn accept_immediate_presented(
        &mut self,
        id: OutputTransactionId,
        presented_at: MonotonicTimestampNs,
    ) -> Result<AcceptedTerminalTransition, OutputTransactionError> {
        let Some(record) = self.active.get(&id) else {
            return Err(self.reject_terminal(OutputTransactionError::UnknownTransaction));
        };
        if !matches!(record.state, OutputTransactionState::Built)
            || !matches!(
                record.descriptor.content(),
                OutputTransactionContent::CompatibilityImmediate { .. }
            )
        {
            return Err(self.reject_invalid_transition(
                record.state,
                OutputTransactionTransitionKind::Presented,
            ));
        }
        let accepted = self.accept_state(
            id,
            OutputTransactionTerminal::Presented {
                presented_at,
                actual_sequence: None,
            },
        )?;
        self.counters.immediate_presentations_accepted = self
            .counters
            .immediate_presentations_accepted
            .saturating_add(1);
        Ok(accepted)
    }

    pub(crate) fn mark_presented(
        &mut self,
        id: OutputTransactionId,
        token: PageFlipToken,
        output_generation: u64,
        presented_at: MonotonicTimestampNs,
        actual_sequence: Option<u64>,
    ) -> Result<(), OutputTransactionError> {
        let accepted =
            self.accept_presented(id, token, output_generation, presented_at, actual_sequence)?;
        self.finalize_terminal(accepted)
    }

    pub(crate) fn mark_dropped(
        &mut self,
        id: OutputTransactionId,
        reason: OutputTransactionDropReason,
        at: MonotonicTimestampNs,
    ) -> Result<(), OutputTransactionError> {
        let accepted = self.accept_dropped(id, reason, at)?;
        self.finalize_terminal(accepted)
    }

    pub(crate) fn accept_no_visual_change(
        &mut self,
        id: OutputTransactionId,
        at: MonotonicTimestampNs,
    ) -> Result<AcceptedTerminalTransition, OutputTransactionError> {
        let state = self.state(id)?;
        if !matches!(
            state,
            OutputTransactionState::Built | OutputTransactionState::Ready { .. }
        ) {
            return Err(
                self.reject_invalid_transition(state, OutputTransactionTransitionKind::Dropped)
            );
        }
        self.accept_state(
            id,
            OutputTransactionTerminal::Dropped {
                reason: OutputTransactionDropReason::NoVisualChange,
                at,
            },
        )
    }

    pub(crate) fn mark_no_visual_change(
        &mut self,
        id: OutputTransactionId,
        at: MonotonicTimestampNs,
    ) -> Result<(), OutputTransactionError> {
        let accepted = self.accept_no_visual_change(id, at)?;
        self.finalize_terminal(accepted)
    }

    pub(crate) fn accept_dropped(
        &mut self,
        id: OutputTransactionId,
        reason: OutputTransactionDropReason,
        at: MonotonicTimestampNs,
    ) -> Result<AcceptedTerminalTransition, OutputTransactionError> {
        let state = self.state(id)?;
        if matches!(state, OutputTransactionState::Submitted { .. })
            && !matches!(
                reason,
                OutputTransactionDropReason::OutputDestroyed
                    | OutputTransactionDropReason::SessionSuspended
                    | OutputTransactionDropReason::SafeAbandonment
            )
        {
            return Err(
                self.reject_invalid_transition(state, OutputTransactionTransitionKind::Dropped)
            );
        }
        self.accept_state(id, OutputTransactionTerminal::Dropped { reason, at })
    }

    pub(crate) fn mark_superseded(
        &mut self,
        id: OutputTransactionId,
        by: Option<OutputTransactionId>,
        reason: OutputTransactionSupersedeReason,
        at: MonotonicTimestampNs,
    ) -> Result<(), OutputTransactionError> {
        let accepted = self.accept_superseded(id, by, reason, at)?;
        self.finalize_terminal(accepted)
    }

    pub(crate) fn accept_superseded(
        &mut self,
        id: OutputTransactionId,
        by: Option<OutputTransactionId>,
        reason: OutputTransactionSupersedeReason,
        at: MonotonicTimestampNs,
    ) -> Result<AcceptedTerminalTransition, OutputTransactionError> {
        let state = self.state(id)?;
        if matches!(state, OutputTransactionState::Submitted { .. }) {
            return Err(
                self.reject_invalid_transition(state, OutputTransactionTransitionKind::Superseded)
            );
        }
        self.accept_state(id, OutputTransactionTerminal::Superseded { by, reason, at })
    }

    pub(crate) fn mark_failed(
        &mut self,
        id: OutputTransactionId,
        stage: OutputTransactionFailureStage,
        at: MonotonicTimestampNs,
    ) -> Result<(), OutputTransactionError> {
        let accepted = self.accept_failed(id, stage, at)?;
        self.finalize_terminal(accepted)
    }

    pub(crate) fn accept_failed(
        &mut self,
        id: OutputTransactionId,
        stage: OutputTransactionFailureStage,
        at: MonotonicTimestampNs,
    ) -> Result<AcceptedTerminalTransition, OutputTransactionError> {
        let state = self.state(id)?;
        if !failure_stage_is_compatible(state.kind(), stage) {
            self.counters.failure_stage_mismatches =
                self.counters.failure_stage_mismatches.saturating_add(1);
            return Err(
                self.reject_terminal(OutputTransactionError::FailureStageMismatch {
                    state: state.kind(),
                    stage,
                }),
            );
        }
        self.accept_state(id, OutputTransactionTerminal::Failed { stage, at })
    }

    pub(crate) fn finalize_terminal(
        &mut self,
        accepted: AcceptedTerminalTransition,
    ) -> Result<(), OutputTransactionError> {
        let Some(record) = self.active.get(&accepted.transaction_id) else {
            return Err(self.reject_terminal(OutputTransactionError::UnknownTransaction));
        };
        if record.state
            != (OutputTransactionState::Settling {
                terminal: accepted.terminal,
            })
        {
            return Err(self.reject_invalid_transition(
                record.state,
                transition_for_terminal(accepted.terminal),
            ));
        }
        self.finish_settling(accepted.transaction_id, accepted.terminal)
    }

    pub(crate) fn commit_prepared_terminal(&mut self, accepted: AcceptedTerminalTransition) {
        let record = self
            .active
            .get(&accepted.transaction_id)
            .expect("prepared terminal transaction disappeared before commit");
        debug_assert_eq!(
            record.state,
            OutputTransactionState::Settling {
                terminal: accepted.terminal,
            }
        );
        if let Some(batch_id) = record.descriptor.obligations().frame_batch_id() {
            debug_assert_eq!(
                self.obligation_owner.get(&batch_id),
                Some(&accepted.transaction_id)
            );
        }
        self.finish_settling_committed(accepted.transaction_id, accepted.terminal);
    }

    pub(crate) fn fail_settlement(
        &mut self,
        accepted: AcceptedTerminalTransition,
        at: MonotonicTimestampNs,
    ) -> Result<(), OutputTransactionError> {
        let Some(record) = self.active.get(&accepted.transaction_id) else {
            return Err(self.reject_terminal(OutputTransactionError::UnknownTransaction));
        };
        if record.state
            != (OutputTransactionState::Settling {
                terminal: accepted.terminal,
            })
        {
            return Err(self
                .reject_invalid_transition(record.state, OutputTransactionTransitionKind::Failed));
        }
        self.counters.settlement_failures = self.counters.settlement_failures.saturating_add(1);
        let failure = OutputTransactionTerminal::Failed {
            stage: OutputTransactionFailureStage::ProtocolSettlement,
            at,
        };
        self.active
            .get_mut(&accepted.transaction_id)
            .expect("settling transaction was observed above")
            .state = OutputTransactionState::Settling { terminal: failure };
        self.finish_settling_inner(accepted.transaction_id, failure, false)
    }

    pub(crate) fn cleanup_generation(
        &mut self,
        output_generation: u64,
        reason: OutputTransactionDropReason,
        at: MonotonicTimestampNs,
    ) -> Result<usize, OutputTransactionError> {
        let ids: Vec<_> = self
            .active
            .values()
            .filter(|record| record.descriptor.output_generation() == output_generation)
            .map(|record| record.descriptor.id())
            .collect();
        for id in ids.iter().copied() {
            self.mark_dropped(id, reason, at)?;
        }
        Ok(ids.len())
    }

    pub(crate) fn terminate_all(
        &mut self,
        reason: OutputTransactionDropReason,
        at: MonotonicTimestampNs,
    ) -> Result<usize, OutputTransactionError> {
        let ids: Vec<_> = self.active.keys().copied().collect();
        for id in ids.iter().copied() {
            self.mark_dropped(id, reason, at)?;
        }
        Ok(ids.len())
    }

    pub(crate) fn transaction(&self, id: OutputTransactionId) -> Option<&OutputTransactionRecord> {
        self.active.get(&id)
    }

    pub(crate) fn transaction_including_terminal(
        &self,
        id: OutputTransactionId,
    ) -> Option<&OutputTransactionRecord> {
        self.active.get(&id).or_else(|| {
            self.recent_terminal
                .iter()
                .find(|record| record.descriptor.id() == id)
        })
    }

    pub(crate) fn active_transaction_ids(&self) -> Vec<OutputTransactionId> {
        let mut ids: Vec<_> = self.active.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    pub(crate) fn active_count(&self) -> usize {
        self.active.len()
    }

    pub(crate) fn obligation_owner(
        &self,
        batch_id: CompositorFrameBatchId,
    ) -> Option<OutputTransactionId> {
        self.obligation_owner.get(&batch_id).copied()
    }

    #[cfg(test)]
    pub(crate) fn forget_obligation_owner_for_test(&mut self, batch_id: CompositorFrameBatchId) {
        self.obligation_owner.remove(&batch_id);
    }

    pub(crate) fn submitted_transaction(
        &self,
        token: PageFlipToken,
        output_generation: u64,
    ) -> Option<OutputTransactionId> {
        self.active.values().find_map(|record| {
            (record.descriptor.output_generation() == output_generation
                && matches!(
                    record.state,
                    OutputTransactionState::Submitted {
                        token: submitted_token,
                        ..
                    } if submitted_token == token
                ))
            .then_some(record.descriptor.id())
        })
    }

    pub(crate) fn recent_terminal(&self) -> &VecDeque<OutputTransactionRecord> {
        &self.recent_terminal
    }

    pub(crate) const fn counters(&self) -> OutputTransactionCounters {
        self.counters
    }

    pub(crate) fn note_duplicate_settlement_attempt(&mut self) {
        self.counters.duplicate_settlement_attempts = self
            .counters
            .duplicate_settlement_attempts
            .saturating_add(1);
    }

    pub(crate) const fn last_created(&self) -> Option<OutputTransactionId> {
        self.last_created
    }

    fn state(
        &self,
        id: OutputTransactionId,
    ) -> Result<OutputTransactionState, OutputTransactionError> {
        self.active
            .get(&id)
            .map(|record| record.state)
            .ok_or(OutputTransactionError::UnknownTransaction)
    }

    fn accept_state(
        &mut self,
        id: OutputTransactionId,
        terminal: OutputTransactionTerminal,
    ) -> Result<AcceptedTerminalTransition, OutputTransactionError> {
        let Some(state) = self.active.get(&id).map(|record| record.state) else {
            if let Some(state) = self
                .recent_terminal
                .iter()
                .find(|record| record.descriptor.id() == id)
                .map(|record| record.state)
            {
                if transition_for_terminal(terminal) == OutputTransactionTransitionKind::Presented {
                    self.counters.duplicate_settlement_attempts = self
                        .counters
                        .duplicate_settlement_attempts
                        .saturating_add(1);
                }
                return Err(
                    self.reject_invalid_transition(state, transition_for_terminal(terminal))
                );
            }
            return Err(self.reject_terminal(OutputTransactionError::UnknownTransaction));
        };
        if matches!(state, OutputTransactionState::Settling { .. })
            || matches!(state, OutputTransactionState::Terminal(_))
        {
            if transition_for_terminal(terminal) == OutputTransactionTransitionKind::Presented {
                self.counters.duplicate_settlement_attempts = self
                    .counters
                    .duplicate_settlement_attempts
                    .saturating_add(1);
            }
            return Err(self.reject_invalid_transition(state, transition_for_terminal(terminal)));
        }
        let obligations = self
            .active
            .get(&id)
            .expect("transaction state was observed above")
            .descriptor
            .obligations();
        if let Some(batch_id) = obligations.frame_batch_id()
            && self.obligation_owner.get(&batch_id).copied() != Some(id)
        {
            return Err(self.reject_terminal(OutputTransactionError::DuplicateObligationOwner));
        }
        let accepted = AcceptedTerminalTransition {
            transaction_id: id,
            obligations,
            prior_state: state,
            terminal,
        };
        self.active
            .get_mut(&id)
            .expect("transaction state was observed above")
            .state = OutputTransactionState::Settling { terminal };
        self.counters.terminal_transitions_accepted = self
            .counters
            .terminal_transitions_accepted
            .saturating_add(1);
        self.counters.active_settling_transactions =
            self.counters.active_settling_transactions.saturating_add(1);
        Ok(accepted)
    }

    pub(crate) fn rollback_settlement(
        &mut self,
        accepted: AcceptedTerminalTransition,
    ) -> Result<(), OutputTransactionError> {
        let state = self
            .active
            .get(&accepted.transaction_id)
            .map(|record| record.state)
            .ok_or_else(|| self.reject_terminal(OutputTransactionError::UnknownTransaction))?;
        if state
            != (OutputTransactionState::Settling {
                terminal: accepted.terminal,
            })
        {
            return Err(
                self.reject_invalid_transition(state, transition_for_terminal(accepted.terminal))
            );
        }
        self.active
            .get_mut(&accepted.transaction_id)
            .expect("settling transaction was observed above")
            .state = accepted.prior_state;
        self.counters.active_settling_transactions =
            self.counters.active_settling_transactions.saturating_sub(1);
        Ok(())
    }

    fn finish_settling(
        &mut self,
        id: OutputTransactionId,
        terminal: OutputTransactionTerminal,
    ) -> Result<(), OutputTransactionError> {
        self.finish_settling_inner(id, terminal, true)
    }

    fn finish_settling_inner(
        &mut self,
        id: OutputTransactionId,
        terminal: OutputTransactionTerminal,
        validate_obligation_owner: bool,
    ) -> Result<(), OutputTransactionError> {
        let Some(record) = self.active.get(&id) else {
            return Err(self.reject_terminal(OutputTransactionError::UnknownTransaction));
        };
        if record.state != (OutputTransactionState::Settling { terminal }) {
            return Err(
                self.reject_invalid_transition(record.state, transition_for_terminal(terminal))
            );
        }
        if validate_obligation_owner
            && let Some(batch_id) = record.descriptor.obligations().frame_batch_id()
            && self.obligation_owner.get(&batch_id).copied() != Some(id)
        {
            return Err(self.reject_terminal(OutputTransactionError::DuplicateObligationOwner));
        }
        self.finish_settling_committed(id, terminal);
        Ok(())
    }

    fn finish_settling_committed(
        &mut self,
        id: OutputTransactionId,
        terminal: OutputTransactionTerminal,
    ) {
        let mut record = self
            .active
            .remove(&id)
            .expect("validated settling transaction disappeared before commit");
        let content = record.descriptor.content();
        if let Some(batch_id) = record.descriptor.obligations().frame_batch_id()
            && self.obligation_owner.get(&batch_id).copied() == Some(id)
        {
            self.obligation_owner.remove(&batch_id);
        }
        record.state = OutputTransactionState::Terminal(terminal);
        if self.recent_terminal.len() == self.history_capacity {
            self.recent_terminal.pop_front();
            self.counters.terminal_history_overwrites =
                self.counters.terminal_history_overwrites.saturating_add(1);
        }
        self.recent_terminal.push_back(record);
        self.counters.active_settling_transactions =
            self.counters.active_settling_transactions.saturating_sub(1);
        self.counters.terminal_transitions_finalized = self
            .counters
            .terminal_transitions_finalized
            .saturating_add(1);
        match terminal {
            OutputTransactionTerminal::Presented { .. } => {
                self.counters.presented = self.counters.presented.saturating_add(1);
                match content {
                    OutputTransactionContent::Composited { .. } => {
                        self.counters.presented_composited =
                            self.counters.presented_composited.saturating_add(1)
                    }
                    OutputTransactionContent::Direct { .. } => {
                        self.counters.presented_direct =
                            self.counters.presented_direct.saturating_add(1)
                    }
                    OutputTransactionContent::CursorOnly { .. } => {
                        self.counters.presented_cursor_only =
                            self.counters.presented_cursor_only.saturating_add(1)
                    }
                    OutputTransactionContent::CompatibilityImmediate { .. } => {
                        self.counters.immediate_presentations =
                            self.counters.immediate_presentations.saturating_add(1);
                        self.counters.immediate_presentations_finalized = self
                            .counters
                            .immediate_presentations_finalized
                            .saturating_add(1)
                    }
                }
            }
            OutputTransactionTerminal::Dropped { reason, .. } => {
                self.counters.dropped = self.counters.dropped.saturating_add(1);
                if matches!(reason, OutputTransactionDropReason::NoVisualChange)
                    && matches!(
                        content,
                        OutputTransactionContent::CompatibilityImmediate { .. }
                    )
                {
                    self.counters.compatibility_noops =
                        self.counters.compatibility_noops.saturating_add(1);
                }
            }
            OutputTransactionTerminal::Superseded { .. } => {
                self.counters.superseded = self.counters.superseded.saturating_add(1)
            }
            OutputTransactionTerminal::Failed { .. } => {
                self.counters.failed = self.counters.failed.saturating_add(1);
                if matches!(
                    content,
                    OutputTransactionContent::CompatibilityImmediate { .. }
                ) {
                    self.counters.immediate_presentation_failures = self
                        .counters
                        .immediate_presentation_failures
                        .saturating_add(1);
                    self.counters.compatibility_failures =
                        self.counters.compatibility_failures.saturating_add(1);
                }
            }
        }
    }

    fn reject_terminal(&mut self, error: OutputTransactionError) -> OutputTransactionError {
        self.counters.terminal_transitions_rejected = self
            .counters
            .terminal_transitions_rejected
            .saturating_add(1);
        error
    }

    fn reject_invalid_transition(
        &mut self,
        state: OutputTransactionState,
        requested: OutputTransactionTransitionKind,
    ) -> OutputTransactionError {
        let error = self.invalid_transition(state, requested);
        self.reject_terminal(error)
    }

    fn note_path_counter(
        &mut self,
        id: OutputTransactionId,
        update: impl FnOnce(&mut OutputTransactionCounters, OutputTransactionContent),
    ) {
        let Some(content) = self
            .active
            .get(&id)
            .map(|record| record.descriptor.content())
        else {
            return;
        };
        update(&mut self.counters, content);
    }

    fn invalid_transition(
        &mut self,
        state: OutputTransactionState,
        requested: OutputTransactionTransitionKind,
    ) -> OutputTransactionError {
        self.counters.invalid_transitions = self.counters.invalid_transitions.saturating_add(1);
        OutputTransactionError::InvalidTransition {
            from: state.kind(),
            requested,
        }
    }

    fn transition(
        &mut self,
        id: OutputTransactionId,
        _requested: OutputTransactionTransitionKind,
        apply: impl FnOnce(&mut OutputTransactionState) -> Result<(), OutputTransactionError>,
    ) -> Result<(), OutputTransactionError> {
        let record = self
            .active
            .get_mut(&id)
            .ok_or(OutputTransactionError::UnknownTransaction)?;
        apply(&mut record.state)
    }
}

const fn failure_stage_is_compatible(
    state: OutputTransactionStateKind,
    stage: OutputTransactionFailureStage,
) -> bool {
    match state {
        OutputTransactionStateKind::Built => matches!(
            stage,
            OutputTransactionFailureStage::RenderPreparation
                | OutputTransactionFailureStage::RenderExecution
                | OutputTransactionFailureStage::FenceExport
                | OutputTransactionFailureStage::KmsSubmit
        ),
        OutputTransactionStateKind::Ready => matches!(
            stage,
            OutputTransactionFailureStage::KmsSubmit
                | OutputTransactionFailureStage::BackendOwnershipTransfer
        ),
        OutputTransactionStateKind::Queued => matches!(
            stage,
            OutputTransactionFailureStage::KmsSubmit
                | OutputTransactionFailureStage::BackendOwnershipTransfer
                | OutputTransactionFailureStage::OutputLost
                | OutputTransactionFailureStage::SessionLost
                | OutputTransactionFailureStage::ShutdownAbandonment
        ),
        OutputTransactionStateKind::Submitted => matches!(
            stage,
            OutputTransactionFailureStage::PageflipValidation
                | OutputTransactionFailureStage::OutputLost
                | OutputTransactionFailureStage::SessionLost
                | OutputTransactionFailureStage::ShutdownAbandonment
                | OutputTransactionFailureStage::BackendCompletion
        ),
        OutputTransactionStateKind::Settling | OutputTransactionStateKind::Terminal => false,
    }
}

const fn transition_for_terminal(
    terminal: OutputTransactionTerminal,
) -> OutputTransactionTransitionKind {
    match terminal {
        OutputTransactionTerminal::Presented { .. } => OutputTransactionTransitionKind::Presented,
        OutputTransactionTerminal::Dropped { .. } => OutputTransactionTransitionKind::Dropped,
        OutputTransactionTerminal::Superseded { .. } => OutputTransactionTransitionKind::Superseded,
        OutputTransactionTerminal::Failed { .. } => OutputTransactionTransitionKind::Failed,
    }
}
