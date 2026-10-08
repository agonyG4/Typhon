use super::transaction::{DirectScanoutCandidateKey, OutputTransactionId};
use oblivion_one::core::OutputId;
use std::{
    collections::{HashMap, VecDeque},
    fmt::Write as _,
};

const HISTOGRAM_BUCKETS_NS: [u64; 8] = [
    100_000,
    500_000,
    1_000_000,
    2_000_000,
    5_000_000,
    10_000_000,
    25_000_000,
    u64::MAX,
];

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PresentationTransactionEvent {
    ContentObserved {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
    AcquireReady {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
    TransactionBuilt {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
    CursorEpochTransactionBuilt {
        output_id: OutputId,
        cursor_epoch: u64,
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
    RenderStarted {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
    RenderCompleted {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
    SceneIdentity {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
        frame_id: u64,
        render_generation: u64,
        resolved_scene_signature: u64,
        snapshot_scene_signature: u64,
        render_damage_signature: u64,
        repair_damage_signature: u64,
        presented_at_render_frame_id: Option<u64>,
        framebuffer_slot: u8,
        buffer_age: Option<u32>,
    },
    PresentedTransition {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
        token: u64,
        previous_frame_id: Option<u64>,
        current_frame_id: u64,
        transition_damage_signature: u64,
    },
    DirectIdentity {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
        surface_id: u32,
        framebuffer_id: u32,
        content_epoch: u64,
        candidate_key: DirectScanoutCandidateKey,
        submission_token: u64,
        pageflip_token: u64,
    },
    TestOnlyStarted {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
    TestOnlyCompleted {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
    KmsSubmitStarted {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
    KmsSubmitReturned {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
    WorkerQueued {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
    PageflipPresented {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
    ImmediatePresented {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
    Superseded {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
    SameBufferSuppressed {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
    Rejected {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
    FeedbackCompleted {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
    BufferReleased {
        transaction_id: OutputTransactionId,
        timestamp_ns: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorEpochPresentationTrace {
    pub(crate) output_id: OutputId,
    pub(crate) cursor_epoch: u64,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) transaction_built_at_ns: u64,
    pub(crate) worker_queued_at_ns: Option<u64>,
    pub(crate) kms_submit_started_at_ns: Option<u64>,
    pub(crate) kms_submit_returned_at_ns: Option<u64>,
    pub(crate) pageflip_observed_at_ns: Option<u64>,
}

impl PresentationTransactionEvent {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::ContentObserved { .. } => "content_observed",
            Self::AcquireReady { .. } => "acquire_ready",
            Self::TransactionBuilt { .. } => "transaction_built",
            Self::CursorEpochTransactionBuilt { .. } => "cursor_epoch_transaction_built",
            Self::RenderStarted { .. } => "render_started",
            Self::RenderCompleted { .. } => "render_completed",
            Self::SceneIdentity { .. } => "scene_identity",
            Self::PresentedTransition { .. } => "presented_transition",
            Self::DirectIdentity { .. } => "direct_identity",
            Self::TestOnlyStarted { .. } => "test_only_started",
            Self::TestOnlyCompleted { .. } => "test_only_completed",
            Self::KmsSubmitStarted { .. } => "kms_submit_started",
            Self::KmsSubmitReturned { .. } => "kms_submit_returned",
            Self::WorkerQueued { .. } => "worker_queued",
            Self::PageflipPresented { .. } => "pageflip_presented",
            Self::ImmediatePresented { .. } => "immediate_presented",
            Self::Superseded { .. } => "superseded",
            Self::SameBufferSuppressed { .. } => "same_buffer_suppressed",
            Self::Rejected { .. } => "rejected",
            Self::FeedbackCompleted { .. } => "feedback_completed",
            Self::BufferReleased { .. } => "buffer_released",
        }
    }

    #[allow(dead_code)]
    pub(crate) const fn transaction_id(self) -> OutputTransactionId {
        match self {
            Self::ContentObserved { transaction_id, .. }
            | Self::AcquireReady { transaction_id, .. }
            | Self::TransactionBuilt { transaction_id, .. }
            | Self::CursorEpochTransactionBuilt { transaction_id, .. }
            | Self::RenderStarted { transaction_id, .. }
            | Self::RenderCompleted { transaction_id, .. }
            | Self::SceneIdentity { transaction_id, .. }
            | Self::PresentedTransition { transaction_id, .. }
            | Self::DirectIdentity { transaction_id, .. }
            | Self::TestOnlyStarted { transaction_id, .. }
            | Self::TestOnlyCompleted { transaction_id, .. }
            | Self::KmsSubmitStarted { transaction_id, .. }
            | Self::KmsSubmitReturned { transaction_id, .. }
            | Self::WorkerQueued { transaction_id, .. }
            | Self::PageflipPresented { transaction_id, .. }
            | Self::ImmediatePresented { transaction_id, .. }
            | Self::Superseded { transaction_id, .. }
            | Self::SameBufferSuppressed { transaction_id, .. }
            | Self::Rejected { transaction_id, .. }
            | Self::FeedbackCompleted { transaction_id, .. }
            | Self::BufferReleased { transaction_id, .. } => transaction_id,
        }
    }

    #[allow(dead_code)]
    pub(crate) const fn timestamp_ns(self) -> u64 {
        match self {
            Self::ContentObserved { timestamp_ns, .. }
            | Self::AcquireReady { timestamp_ns, .. }
            | Self::TransactionBuilt { timestamp_ns, .. }
            | Self::CursorEpochTransactionBuilt { timestamp_ns, .. }
            | Self::RenderStarted { timestamp_ns, .. }
            | Self::RenderCompleted { timestamp_ns, .. }
            | Self::SceneIdentity { timestamp_ns, .. }
            | Self::PresentedTransition { timestamp_ns, .. }
            | Self::DirectIdentity { timestamp_ns, .. }
            | Self::TestOnlyStarted { timestamp_ns, .. }
            | Self::TestOnlyCompleted { timestamp_ns, .. }
            | Self::KmsSubmitStarted { timestamp_ns, .. }
            | Self::KmsSubmitReturned { timestamp_ns, .. }
            | Self::WorkerQueued { timestamp_ns, .. }
            | Self::PageflipPresented { timestamp_ns, .. }
            | Self::ImmediatePresented { timestamp_ns, .. }
            | Self::Superseded { timestamp_ns, .. }
            | Self::SameBufferSuppressed { timestamp_ns, .. }
            | Self::Rejected { timestamp_ns, .. }
            | Self::FeedbackCompleted { timestamp_ns, .. }
            | Self::BufferReleased { timestamp_ns, .. } => timestamp_ns,
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PresentationTransactionSummary {
    pub(crate) observe_to_submit_ns: Option<u64>,
    pub(crate) submit_to_pageflip_ns: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct PresentationTransactionTraceRing {
    capacity: usize,
    events: VecDeque<PresentationTransactionEvent>,
    dropped: u64,
    enabled: bool,
    cursor_epoch_transactions: HashMap<OutputTransactionId, CursorEpochPresentationTrace>,
    cursor_epoch_transaction_order: VecDeque<OutputTransactionId>,
    cursor_epoch_transactions_dropped: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TraceExport {
    Unchanged,
    Append(String),
    Replace(String),
}

impl PresentationTransactionTraceRing {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            events: VecDeque::with_capacity(capacity),
            dropped: 0,
            enabled: true,
            cursor_epoch_transactions: HashMap::with_capacity(capacity),
            cursor_epoch_transaction_order: VecDeque::with_capacity(capacity),
            cursor_epoch_transactions_dropped: 0,
        }
    }

    pub(crate) fn disabled(capacity: usize) -> Self {
        Self {
            capacity,
            events: VecDeque::new(),
            dropped: 0,
            enabled: false,
            cursor_epoch_transactions: HashMap::new(),
            cursor_epoch_transaction_order: VecDeque::new(),
            cursor_epoch_transactions_dropped: 0,
        }
    }

    pub(crate) fn from_env() -> Self {
        let cursor_trace_enabled = crate::pointer_debug::cursor_responsiveness_trace_enabled();
        let enabled = std::env::var("OBLIVION_ONE_PRESENTATION_TRACE")
            .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "on"))
            || cursor_trace_enabled;
        let capacity = std::env::var("OBLIVION_ONE_PRESENTATION_TRACE_CAPACITY")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(if cursor_trace_enabled { 65_536 } else { 4096 })
            .clamp(1, 65_536);
        if enabled {
            Self::new(capacity)
        } else {
            Self::disabled(capacity)
        }
    }

    pub(crate) fn push(&mut self, event: PresentationTransactionEvent) {
        if !self.enabled || self.capacity == 0 {
            if self.enabled {
                self.dropped = self.dropped.saturating_add(1);
            }
            return;
        }
        self.record_cursor_epoch_event(event);
        if self.events.len() == self.capacity {
            self.events.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.events.push_back(event);
    }

    pub(crate) const fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn len(&self) -> usize {
        self.events.len()
    }

    pub(crate) const fn dropped(&self) -> u64 {
        self.dropped
    }

    pub(crate) fn cursor_epoch_transactions(
        &self,
    ) -> impl Iterator<Item = CursorEpochPresentationTrace> + '_ {
        self.cursor_epoch_transaction_order
            .iter()
            .filter_map(|transaction_id| {
                self.cursor_epoch_transactions.get(transaction_id).copied()
            })
    }

    pub(crate) const fn cursor_epoch_transactions_dropped(&self) -> u64 {
        self.cursor_epoch_transactions_dropped
    }

    fn record_cursor_epoch_event(&mut self, event: PresentationTransactionEvent) {
        match event {
            PresentationTransactionEvent::CursorEpochTransactionBuilt {
                output_id,
                cursor_epoch,
                transaction_id,
                timestamp_ns,
            } => {
                if !self.cursor_epoch_transactions.contains_key(&transaction_id) {
                    if self.cursor_epoch_transaction_order.len() == self.capacity
                        && let Some(evicted) = self.cursor_epoch_transaction_order.pop_front()
                    {
                        self.cursor_epoch_transactions.remove(&evicted);
                        self.cursor_epoch_transactions_dropped =
                            self.cursor_epoch_transactions_dropped.saturating_add(1);
                    }
                    self.cursor_epoch_transaction_order
                        .push_back(transaction_id);
                }
                self.cursor_epoch_transactions.insert(
                    transaction_id,
                    CursorEpochPresentationTrace {
                        output_id,
                        cursor_epoch,
                        transaction_id,
                        transaction_built_at_ns: timestamp_ns,
                        worker_queued_at_ns: None,
                        kms_submit_started_at_ns: None,
                        kms_submit_returned_at_ns: None,
                        pageflip_observed_at_ns: None,
                    },
                );
            }
            PresentationTransactionEvent::WorkerQueued {
                transaction_id,
                timestamp_ns,
            } => {
                if let Some(cursor_transaction) =
                    self.cursor_epoch_transactions.get_mut(&transaction_id)
                {
                    cursor_transaction
                        .worker_queued_at_ns
                        .get_or_insert(timestamp_ns);
                }
            }
            PresentationTransactionEvent::KmsSubmitStarted {
                transaction_id,
                timestamp_ns,
            } => {
                if let Some(cursor_transaction) =
                    self.cursor_epoch_transactions.get_mut(&transaction_id)
                {
                    cursor_transaction
                        .kms_submit_started_at_ns
                        .get_or_insert(timestamp_ns);
                }
            }
            PresentationTransactionEvent::KmsSubmitReturned {
                transaction_id,
                timestamp_ns,
            } => {
                if let Some(cursor_transaction) =
                    self.cursor_epoch_transactions.get_mut(&transaction_id)
                {
                    cursor_transaction
                        .kms_submit_returned_at_ns
                        .get_or_insert(timestamp_ns);
                }
            }
            PresentationTransactionEvent::PageflipPresented {
                transaction_id,
                timestamp_ns,
            } => {
                if let Some(cursor_transaction) =
                    self.cursor_epoch_transactions.get_mut(&transaction_id)
                {
                    cursor_transaction
                        .pageflip_observed_at_ns
                        .get_or_insert(timestamp_ns);
                }
            }
            _ => {}
        }
    }

    #[allow(dead_code)]
    pub(crate) fn summarize(
        &self,
        transaction_id: OutputTransactionId,
    ) -> Option<PresentationTransactionSummary> {
        let mut observed = None;
        let mut submitted = None;
        let mut presented = None;
        for event in self.events.iter().copied() {
            if event.transaction_id() != transaction_id {
                continue;
            }
            match event {
                PresentationTransactionEvent::ContentObserved { timestamp_ns, .. } => {
                    observed.get_or_insert(timestamp_ns);
                }
                PresentationTransactionEvent::KmsSubmitReturned { timestamp_ns, .. } => {
                    submitted.get_or_insert(timestamp_ns);
                }
                PresentationTransactionEvent::PageflipPresented { timestamp_ns, .. } => {
                    presented.get_or_insert(timestamp_ns);
                }
                PresentationTransactionEvent::ImmediatePresented { timestamp_ns, .. } => {
                    presented.get_or_insert(timestamp_ns);
                }
                _ => {}
            }
        }
        (observed.is_some() || submitted.is_some() || presented.is_some()).then_some(
            PresentationTransactionSummary {
                observe_to_submit_ns: observed
                    .zip(submitted)
                    .map(|(start, end)| end.saturating_sub(start)),
                submit_to_pageflip_ns: submitted
                    .zip(presented)
                    .map(|(start, end)| end.saturating_sub(start)),
            },
        )
    }

    #[allow(dead_code)]
    pub(crate) fn events(&self) -> impl Iterator<Item = PresentationTransactionEvent> + '_ {
        self.events.iter().copied()
    }

    pub(crate) fn export_jsonl(&self) -> String {
        let mut output = String::new();
        for event in self.events.iter().copied() {
            append_event_jsonl(&mut output, event);
        }
        output
    }

    pub(crate) fn export_delta(&self, previous: Option<(usize, u64)>) -> TraceExport {
        let current = (self.events.len(), self.dropped);
        let Some((previous_len, previous_dropped)) = previous else {
            return TraceExport::Replace(self.export_jsonl());
        };
        if current == (previous_len, previous_dropped) {
            return TraceExport::Unchanged;
        }
        if previous_dropped == self.dropped && previous_len <= self.events.len() {
            let mut output = String::new();
            for event in self.events.iter().skip(previous_len).copied() {
                append_event_jsonl(&mut output, event);
            }
            TraceExport::Append(output)
        } else {
            TraceExport::Replace(self.export_jsonl())
        }
    }

    pub(crate) fn export_cursor(&self) -> (usize, u64) {
        (self.events.len(), self.dropped)
    }
}

fn append_event_jsonl(output: &mut String, event: PresentationTransactionEvent) {
    match event {
        PresentationTransactionEvent::CursorEpochTransactionBuilt {
            output_id,
            cursor_epoch,
            transaction_id,
            timestamp_ns,
        } => {
            let _ = writeln!(
                output,
                "{{\"event\":\"{}\",\"output_id\":{},\"cursor_epoch\":{},\"transaction_id\":{},\"timestamp_ns\":{}}}",
                event.name(),
                output_id.get(),
                cursor_epoch,
                transaction_id.get(),
                timestamp_ns,
            );
        }
        _ => {
            let _ = writeln!(
                output,
                "{{\"event\":\"{}\",\"transaction_id\":{},\"timestamp_ns\":{}}}",
                event.name(),
                event.transaction_id().get(),
                event.timestamp_ns(),
            );
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TimingSummary {
    pub(crate) count: u64,
    pub(crate) total_ns: u64,
    pub(crate) last_ns: u64,
    pub(crate) max_ns: u64,
    pub(crate) buckets: [u64; 8],
}

impl TimingSummary {
    pub(crate) fn record(&mut self, elapsed_ns: u64) {
        self.count = self.count.saturating_add(1);
        self.total_ns = self.total_ns.saturating_add(elapsed_ns);
        self.last_ns = elapsed_ns;
        self.max_ns = self.max_ns.max(elapsed_ns);
        let bucket = HISTOGRAM_BUCKETS_NS
            .iter()
            .position(|limit| elapsed_ns <= *limit)
            .unwrap_or(HISTOGRAM_BUCKETS_NS.len() - 1);
        self.buckets[bucket] = self.buckets[bucket].saturating_add(1);
    }

    pub(crate) fn percentile_ns(&self, percentile: u8) -> u64 {
        if self.count == 0 {
            return 0;
        }
        let percentile = u64::from(percentile.clamp(1, 100));
        let rank = self.count.saturating_mul(percentile).saturating_add(99) / 100;
        let mut seen = 0u64;
        for (index, count) in self.buckets.iter().copied().enumerate() {
            seen = seen.saturating_add(count);
            if seen >= rank {
                return HISTOGRAM_BUCKETS_NS[index];
            }
        }
        HISTOGRAM_BUCKETS_NS[HISTOGRAM_BUCKETS_NS.len() - 1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oblivion_one::core::OutputId;
    use std::num::NonZeroU64;

    fn event(timestamp_ns: u64) -> PresentationTransactionEvent {
        PresentationTransactionEvent::ContentObserved {
            transaction_id: OutputTransactionId::new(NonZeroU64::new(1).unwrap()),
            timestamp_ns,
        }
    }

    fn transaction_id(value: u64) -> OutputTransactionId {
        OutputTransactionId::new(NonZeroU64::new(value).unwrap())
    }

    fn cursor_epoch_event(
        transaction_id: OutputTransactionId,
        cursor_epoch: u64,
    ) -> PresentationTransactionEvent {
        PresentationTransactionEvent::CursorEpochTransactionBuilt {
            output_id: OutputId::from_raw(1).unwrap(),
            cursor_epoch,
            transaction_id,
            timestamp_ns: cursor_epoch * 10,
        }
    }

    #[test]
    fn cursor_epoch_history_is_bounded_and_tracks_only_cursor_transactions() {
        let mut ring = PresentationTransactionTraceRing::new(2);
        let first = transaction_id(1);
        let second = transaction_id(2);
        let third = transaction_id(3);
        ring.push(cursor_epoch_event(first, 1));
        ring.push(cursor_epoch_event(second, 2));
        ring.push(cursor_epoch_event(third, 3));
        ring.push(PresentationTransactionEvent::KmsSubmitStarted {
            transaction_id: third,
            timestamp_ns: 31,
        });

        let retained = ring.cursor_epoch_transactions().collect::<Vec<_>>();
        assert_eq!(retained.len(), 2);
        assert_eq!(retained[0].transaction_id, second);
        assert_eq!(retained[1].transaction_id, third);
        assert_eq!(retained[1].kms_submit_started_at_ns, Some(31));
        assert_eq!(ring.cursor_epoch_transactions_dropped(), 1);
    }

    #[test]
    fn disabled_trace_does_not_allocate_event_or_cursor_histories() {
        let ring = PresentationTransactionTraceRing::disabled(128);
        assert_eq!(ring.events.capacity(), 0);
        assert_eq!(ring.cursor_epoch_transactions.capacity(), 0);
        assert_eq!(ring.cursor_epoch_transaction_order.capacity(), 0);
    }

    #[test]
    fn unchanged_trace_does_not_rewrite_history() {
        let mut ring = PresentationTransactionTraceRing::new(4);
        ring.push(event(1));
        let cursor = ring.export_cursor();
        assert_eq!(ring.export_delta(Some(cursor)), TraceExport::Unchanged);
    }

    #[test]
    fn new_events_export_as_append_only_jsonl() {
        let mut ring = PresentationTransactionTraceRing::new(4);
        ring.push(event(1));
        let cursor = ring.export_cursor();
        ring.push(event(2));
        let TraceExport::Append(delta) = ring.export_delta(Some(cursor)) else {
            panic!("expected append-only export");
        };
        assert!(delta.contains("\"timestamp_ns\":2"));
        assert!(!delta.contains("\"timestamp_ns\":1"));
    }

    #[test]
    fn multiple_new_events_export_in_chronological_order() {
        let mut ring = PresentationTransactionTraceRing::new(8);
        ring.push(event(1));
        let cursor = ring.export_cursor();
        ring.push(event(2));
        ring.push(event(3));
        ring.push(event(4));

        let TraceExport::Append(delta) = ring.export_delta(Some(cursor)) else {
            panic!("expected append-only export");
        };
        assert_eq!(delta.lines().count(), 3);
        let timestamp_2 = delta.find("\"timestamp_ns\":2").unwrap();
        let timestamp_3 = delta.find("\"timestamp_ns\":3").unwrap();
        let timestamp_4 = delta.find("\"timestamp_ns\":4").unwrap();
        assert!(timestamp_2 < timestamp_3);
        assert!(timestamp_3 < timestamp_4);
        assert!(!delta.contains("\"timestamp_ns\":1"));
    }

    #[test]
    fn large_exported_history_does_not_expand_a_new_event_delta() {
        let mut ring = PresentationTransactionTraceRing::new(1024);
        for timestamp in 0..512 {
            ring.push(event(timestamp));
        }
        let cursor = ring.export_cursor();
        ring.push(event(512));

        let TraceExport::Append(delta) = ring.export_delta(Some(cursor)) else {
            panic!("expected append-only export");
        };
        assert_eq!(delta.lines().count(), 1);
        assert!(delta.contains("\"timestamp_ns\":512"));
        assert!(!delta.contains("\"timestamp_ns\":511"));
    }

    #[test]
    fn ring_overwrite_requires_a_bounded_full_replacement() {
        let mut ring = PresentationTransactionTraceRing::new(1);
        ring.push(event(1));
        let cursor = ring.export_cursor();
        ring.push(event(2));
        let TraceExport::Replace(full) = ring.export_delta(Some(cursor)) else {
            panic!("expected replacement export after a drop");
        };
        assert!(!full.contains("\"timestamp_ns\":1"));
        assert!(full.contains("\"timestamp_ns\":2"));
    }
}
