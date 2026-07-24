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
    Submitted {
        token: PageFlipToken,
        submitted_at: MonotonicTimestampNs,
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
    OutputTeardown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputTransactionStateKind {
    Built,
    Ready,
    Submitted,
    Terminal,
}

impl OutputTransactionState {
    pub(crate) const fn kind(self) -> OutputTransactionStateKind {
        match self {
            Self::Built => OutputTransactionStateKind::Built,
            Self::Ready { .. } => OutputTransactionStateKind::Ready,
            Self::Submitted { .. } => OutputTransactionStateKind::Submitted,
            Self::Terminal(_) => OutputTransactionStateKind::Terminal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputTransactionTransitionKind {
    Ready,
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
    pub(crate) active_peak: u64,
    pub(crate) terminal_history_overwrites: u64,
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

    pub(crate) fn mark_submitted(
        &mut self,
        id: OutputTransactionId,
        token: PageFlipToken,
        submitted_at: MonotonicTimestampNs,
    ) -> Result<(), OutputTransactionError> {
        let state = self.state(id)?;
        if !matches!(
            state,
            OutputTransactionState::Built | OutputTransactionState::Ready { .. }
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
        });
        Ok(())
    }

    pub(crate) fn mark_presented(
        &mut self,
        id: OutputTransactionId,
        token: PageFlipToken,
        output_generation: u64,
        presented_at: MonotonicTimestampNs,
        actual_sequence: Option<u64>,
    ) -> Result<(), OutputTransactionError> {
        let (content, descriptor_generation, state) = self
            .active
            .get(&id)
            .map(|record| {
                (
                    record.descriptor.content(),
                    record.descriptor.output_generation(),
                    record.state,
                )
            })
            .ok_or(OutputTransactionError::UnknownTransaction)?;
        if descriptor_generation != output_generation {
            return Err(OutputTransactionError::GenerationMismatch);
        }
        match state {
            OutputTransactionState::Submitted {
                token: expected, ..
            } if expected == token => {}
            OutputTransactionState::Submitted { .. } => {
                return Err(OutputTransactionError::TokenMismatch);
            }
            state => {
                return Err(
                    self.invalid_transition(state, OutputTransactionTransitionKind::Presented)
                );
            }
        }
        self.terminalize(
            id,
            OutputTransactionTerminal::Presented {
                presented_at,
                actual_sequence,
            },
        )?;
        self.counters.presented = self.counters.presented.saturating_add(1);
        match content {
            OutputTransactionContent::Composited { .. } => {
                self.counters.presented_composited =
                    self.counters.presented_composited.saturating_add(1)
            }
            OutputTransactionContent::Direct { .. } => {
                self.counters.presented_direct = self.counters.presented_direct.saturating_add(1)
            }
            OutputTransactionContent::CursorOnly { .. } => {
                self.counters.presented_cursor_only =
                    self.counters.presented_cursor_only.saturating_add(1)
            }
        }
        Ok(())
    }

    pub(crate) fn mark_dropped(
        &mut self,
        id: OutputTransactionId,
        reason: OutputTransactionDropReason,
        at: MonotonicTimestampNs,
    ) -> Result<(), OutputTransactionError> {
        let state = self.state(id)?;
        if matches!(state, OutputTransactionState::Submitted { .. })
            && !matches!(
                reason,
                OutputTransactionDropReason::OutputDestroyed
                    | OutputTransactionDropReason::SessionSuspended
                    | OutputTransactionDropReason::SafeAbandonment
            )
        {
            return Err(self.invalid_transition(state, OutputTransactionTransitionKind::Dropped));
        }
        self.terminalize(id, OutputTransactionTerminal::Dropped { reason, at })?;
        self.counters.dropped = self.counters.dropped.saturating_add(1);
        Ok(())
    }

    pub(crate) fn mark_superseded(
        &mut self,
        id: OutputTransactionId,
        by: Option<OutputTransactionId>,
        reason: OutputTransactionSupersedeReason,
        at: MonotonicTimestampNs,
    ) -> Result<(), OutputTransactionError> {
        let state = self.state(id)?;
        if matches!(state, OutputTransactionState::Submitted { .. }) {
            return Err(self.invalid_transition(state, OutputTransactionTransitionKind::Superseded));
        }
        self.terminalize(id, OutputTransactionTerminal::Superseded { by, reason, at })?;
        self.counters.superseded = self.counters.superseded.saturating_add(1);
        Ok(())
    }

    pub(crate) fn mark_failed(
        &mut self,
        id: OutputTransactionId,
        stage: OutputTransactionFailureStage,
        at: MonotonicTimestampNs,
    ) -> Result<(), OutputTransactionError> {
        self.terminalize(id, OutputTransactionTerminal::Failed { stage, at })?;
        self.counters.failed = self.counters.failed.saturating_add(1);
        Ok(())
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

    pub(crate) fn active_count(&self) -> usize {
        self.active.len()
    }

    pub(crate) fn obligation_owner(
        &self,
        batch_id: CompositorFrameBatchId,
    ) -> Option<OutputTransactionId> {
        self.obligation_owner.get(&batch_id).copied()
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

    fn terminalize(
        &mut self,
        id: OutputTransactionId,
        terminal: OutputTransactionTerminal,
    ) -> Result<(), OutputTransactionError> {
        let mut record = self
            .active
            .remove(&id)
            .ok_or(OutputTransactionError::UnknownTransaction)?;
        if let Some(batch_id) = record.descriptor.obligations().frame_batch_id() {
            if self.obligation_owner.get(&batch_id).copied() != Some(id) {
                self.active.insert(id, record);
                return Err(OutputTransactionError::DuplicateObligationOwner);
            }
            self.obligation_owner.remove(&batch_id);
        }
        record.state = OutputTransactionState::Terminal(terminal);
        if self.recent_terminal.len() == self.history_capacity {
            self.recent_terminal.pop_front();
            self.counters.terminal_history_overwrites =
                self.counters.terminal_history_overwrites.saturating_add(1);
        }
        self.recent_terminal.push_back(record);
        Ok(())
    }
}
