use super::transaction::{DirectScanoutCandidateKey, OutputTransactionId};
use oblivion_one::core::OutputId;
use std::{
    collections::{HashMap, VecDeque},
    fmt::Write as _,
    fs::File,
    io::Read as _,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT_TRACE_SOURCE_ID: AtomicU64 = AtomicU64::new(1);
static TRACE_SOURCE_NONCE: OnceLock<u128> = OnceLock::new();

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PresentationTransactionTraceRecord {
    pub(crate) sequence: u128,
    pub(crate) process_id: u32,
    pub(crate) source_id: u128,
    pub(crate) event: PresentationTransactionEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TraceExportRecord {
    Event(PresentationTransactionTraceRecord),
    LostRange {
        process_id: u32,
        source_id: u128,
        from_sequence: u128,
        to_sequence: u128,
    },
}

#[derive(Debug)]
pub(crate) struct PresentationTransactionTraceRing {
    capacity: usize,
    events: VecDeque<PresentationTransactionTraceRecord>,
    process_id: u32,
    source_id: u128,
    next_sequence: u128,
    sequence_exhausted: bool,
    dropped: u64,
    enabled: bool,
    cursor_epoch_transactions: HashMap<OutputTransactionId, CursorEpochPresentationTrace>,
    cursor_epoch_transaction_order: VecDeque<OutputTransactionId>,
    cursor_epoch_transactions_dropped: u64,
}

#[derive(Debug, Clone, Copy)]
struct TraceDeltaPlan {
    first_retained: u128,
    offset: usize,
    event_count: usize,
    lost_from: Option<u128>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TraceExport {
    Unchanged,
    Append(Vec<TraceExportRecord>),
}

impl PresentationTransactionTraceRing {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            events: VecDeque::with_capacity(capacity),
            process_id: std::process::id(),
            source_id: next_trace_source_id(),
            next_sequence: 1,
            sequence_exhausted: false,
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
            process_id: std::process::id(),
            source_id: 0,
            next_sequence: 1,
            sequence_exhausted: false,
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
        if self.sequence_exhausted {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        let record = PresentationTransactionTraceRecord {
            sequence: self.next_sequence,
            process_id: self.process_id,
            source_id: self.source_id,
            event,
        };
        if let Some(next_sequence) = self.next_sequence.checked_add(1) {
            self.next_sequence = next_sequence;
        } else {
            self.sequence_exhausted = true;
        }
        self.record_cursor_epoch_event(event);
        if self.events.len() == self.capacity {
            self.events.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.events.push_back(record);
    }

    pub(crate) const fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) const fn source_identity(&self) -> (u32, u128) {
        (self.process_id, self.source_id)
    }

    pub(crate) fn len(&self) -> usize {
        self.events.len()
    }

    #[cfg(test)]
    pub(crate) fn retained_event_capacity(&self) -> usize {
        self.events.capacity()
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
        for record in self.events.iter().copied() {
            let event = record.event;
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
        self.events.iter().map(|record| record.event)
    }

    fn delta_plan(&self, previous: Option<u128>) -> Option<TraceDeltaPlan> {
        let Some(first_retained) = self.events.front().map(|record| record.sequence) else {
            return None;
        };
        let Some(last_retained) = self.events.back().map(|record| record.sequence) else {
            return None;
        };
        if previous.is_some_and(|sequence| sequence >= last_retained) {
            return None;
        }

        let first_requested = match previous {
            Some(sequence) => sequence.checked_add(1)?,
            None => 1,
        };
        let lost_from = (first_requested < first_retained).then_some(first_requested);
        let first_exported = first_requested.max(first_retained);
        let event_count = usize::try_from(last_retained - first_exported + 1)
            .unwrap_or(self.events.len())
            .min(self.events.len());

        Some(TraceDeltaPlan {
            first_retained,
            offset: usize::try_from(first_exported - first_retained).unwrap_or(usize::MAX),
            event_count,
            lost_from,
        })
    }

    pub(crate) fn append_delta_records(
        &self,
        previous: Option<u128>,
        records: &mut Vec<TraceExportRecord>,
    ) -> Option<u128> {
        let plan = self.delta_plan(previous)?;
        let loss_records = usize::from(plan.lost_from.is_some());
        let available = records.capacity().saturating_sub(records.len());
        if available <= loss_records {
            return None;
        }
        let event_count = plan.event_count.min(available - loss_records);
        if event_count == 0 {
            return None;
        }
        if let Some(from_sequence) = plan.lost_from {
            records.push(TraceExportRecord::LostRange {
                process_id: self.process_id,
                source_id: self.source_id,
                from_sequence,
                to_sequence: plan.first_retained - 1,
            });
        }
        for offset in plan.offset..plan.offset + event_count {
            if let Some(record) = self.events.get(offset) {
                records.push(TraceExportRecord::Event(*record));
            }
        }
        records.last().and_then(|record| match record {
            TraceExportRecord::Event(record) => Some(record.sequence),
            TraceExportRecord::LostRange { .. } => None,
        })
    }

    #[cfg(test)]
    pub(crate) fn export_delta(&self, previous: Option<u128>) -> TraceExport {
        let Some(plan) = self.delta_plan(previous) else {
            return TraceExport::Unchanged;
        };
        let mut records =
            Vec::with_capacity(plan.event_count + usize::from(plan.lost_from.is_some()));
        let sequence = self.append_delta_records(previous, &mut records);
        debug_assert_eq!(sequence, self.latest_sequence());
        TraceExport::Append(records)
    }

    pub(crate) fn latest_sequence(&self) -> Option<u128> {
        self.events.back().map(|record| record.sequence)
    }

    #[cfg(test)]
    pub(crate) fn export_cursor(&self) -> Option<u128> {
        self.latest_sequence()
    }

    #[cfg(test)]
    pub(crate) fn export_jsonl(&self) -> String {
        let records = match self.export_delta(None) {
            TraceExport::Unchanged => return String::new(),
            TraceExport::Append(records) => records,
        };
        let mut output = String::new();
        for record in records {
            append_trace_record_jsonl(&mut output, record);
        }
        output
    }
}

pub(crate) fn append_trace_record_jsonl(output: &mut String, record: TraceExportRecord) {
    match record {
        TraceExportRecord::LostRange {
            process_id,
            source_id,
            from_sequence,
            to_sequence,
        } => {
            let _ = writeln!(
                output,
                "{{\"schema_version\":1,\"record\":\"loss\",\"source\":\"native_reactor\",\"process_id\":{},\"source_id\":{},\"from_sequence\":{},\"to_sequence\":{},\"reason\":\"retention_overwrite\"}}",
                process_id, source_id, from_sequence, to_sequence,
            );
        }
        TraceExportRecord::Event(record) => append_event_jsonl(output, record),
    }
}

fn append_event_jsonl(output: &mut String, record: PresentationTransactionTraceRecord) {
    let event = record.event;
    match event {
        PresentationTransactionEvent::CursorEpochTransactionBuilt {
            output_id,
            cursor_epoch,
            transaction_id,
            timestamp_ns,
        } => {
            let _ = writeln!(
                output,
                "{{\"schema_version\":1,\"record\":\"event\",\"source\":\"native_reactor\",\"process_id\":{},\"source_id\":{},\"sequence\":{},\"event\":\"{}\",\"output_id\":{},\"cursor_epoch\":{},\"transaction_id\":{},\"timestamp_clock\":\"CLOCK_MONOTONIC\",\"timestamp_ns\":{}}}",
                record.process_id,
                record.source_id,
                record.sequence,
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
                "{{\"schema_version\":1,\"record\":\"event\",\"source\":\"native_reactor\",\"process_id\":{},\"source_id\":{},\"sequence\":{},\"event\":\"{}\",\"transaction_id\":{},\"timestamp_clock\":\"CLOCK_MONOTONIC\",\"timestamp_ns\":{}}}",
                record.process_id,
                record.source_id,
                record.sequence,
                event.name(),
                event.transaction_id().get(),
                event.timestamp_ns(),
            );
        }
    }
}

fn next_trace_source_id() -> u128 {
    let sequence = NEXT_TRACE_SOURCE_ID.fetch_add(1, Ordering::Relaxed);
    let nonce = *TRACE_SOURCE_NONCE.get_or_init(random_trace_source_nonce);
    let source_id = nonce ^ ((u128::from(std::process::id()) << 64) | u128::from(sequence));
    source_id.max(1)
}

fn random_trace_source_nonce() -> u128 {
    let mut bytes = [0; 16];
    if File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .is_ok()
    {
        let source_id = u128::from_ne_bytes(bytes);
        if source_id != 0 {
            return source_id;
        }
    }

    let monotonic_seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    (u128::from(std::process::id()) << 96) ^ (monotonic_seed << 32)
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
        assert_eq!(ring.source_identity().1, 0);
    }

    #[test]
    fn enabled_trace_sources_have_distinct_identities() {
        let first = PresentationTransactionTraceRing::new(8);
        let second = PresentationTransactionTraceRing::new(8);
        assert_ne!(first.source_identity(), second.source_identity());
    }

    #[test]
    fn unchanged_trace_does_not_rewrite_history() {
        let mut ring = PresentationTransactionTraceRing::new(4);
        ring.push(event(1));
        let cursor = ring.export_cursor();
        assert_eq!(ring.export_delta(cursor), TraceExport::Unchanged);
    }

    #[test]
    fn new_events_export_as_append_only_jsonl() {
        let mut ring = PresentationTransactionTraceRing::new(4);
        ring.push(event(1));
        let cursor = ring.export_cursor();
        ring.push(event(2));
        let TraceExport::Append(delta) = ring.export_delta(cursor) else {
            panic!("expected append-only export");
        };
        assert_eq!(delta.len(), 1);
        let mut serialized = String::new();
        append_trace_record_jsonl(&mut serialized, delta[0]);
        assert!(serialized.contains("\"timestamp_ns\":2"));
        assert!(serialized.contains("\"timestamp_clock\":\"CLOCK_MONOTONIC\""));
        assert!(!serialized.contains("\"timestamp_ns\":1"));
    }

    #[test]
    fn multiple_new_events_export_in_chronological_order() {
        let mut ring = PresentationTransactionTraceRing::new(8);
        ring.push(event(1));
        let cursor = ring.export_cursor();
        ring.push(event(2));
        ring.push(event(3));
        ring.push(event(4));

        let TraceExport::Append(delta) = ring.export_delta(cursor) else {
            panic!("expected append-only export");
        };
        assert_eq!(delta.len(), 3);
        let serialized = delta
            .iter()
            .copied()
            .map(|record| {
                let mut line = String::new();
                append_trace_record_jsonl(&mut line, record);
                line
            })
            .collect::<String>();
        let timestamp_2 = serialized.find("\"timestamp_ns\":2").unwrap();
        let timestamp_3 = serialized.find("\"timestamp_ns\":3").unwrap();
        let timestamp_4 = serialized.find("\"timestamp_ns\":4").unwrap();
        assert!(timestamp_2 < timestamp_3);
        assert!(timestamp_3 < timestamp_4);
        assert!(!serialized.contains("\"timestamp_ns\":1"));
    }

    #[test]
    fn large_exported_history_does_not_expand_a_new_event_delta() {
        let mut ring = PresentationTransactionTraceRing::new(1024);
        for timestamp in 0..512 {
            ring.push(event(timestamp));
        }
        let cursor = ring.export_cursor();
        ring.push(event(512));

        let TraceExport::Append(delta) = ring.export_delta(cursor) else {
            panic!("expected append-only export");
        };
        assert_eq!(delta.len(), 1);
        let mut serialized = String::new();
        append_trace_record_jsonl(&mut serialized, delta[0]);
        assert!(serialized.contains("\"timestamp_ns\":512"));
        assert!(!serialized.contains("\"timestamp_ns\":511"));
    }

    #[test]
    fn ring_overwrite_exports_only_new_events_and_explicit_loss_ranges() {
        let mut ring = PresentationTransactionTraceRing::new(1);
        ring.push(event(1));
        let cursor = ring.export_cursor();
        ring.push(event(2));
        ring.push(event(3));
        let TraceExport::Append(delta) = ring.export_delta(cursor) else {
            panic!("expected an incremental export after a ring overwrite");
        };

        assert_eq!(delta.len(), 2);
        assert!(delta.capacity() <= 2);
        let mut serialized = String::new();
        for record in delta {
            append_trace_record_jsonl(&mut serialized, record);
        }
        assert_eq!(serialized.lines().count(), 2);
        assert!(serialized.contains("\"record\":\"loss\""));
        assert!(serialized.contains("\"from_sequence\":2"));
        assert!(serialized.contains("\"to_sequence\":2"));
        assert!(serialized.contains("\"sequence\":3"));
        assert!(serialized.contains("\"timestamp_ns\":3"));
        assert!(!serialized.contains("\"timestamp_ns\":1"));
        assert!(!serialized.contains("\"timestamp_ns\":2"));
    }

    #[test]
    fn restarted_export_keeps_sequence_identity_and_reports_evicted_prefix() {
        let mut ring = PresentationTransactionTraceRing::new(2);
        ring.push(event(1));
        ring.push(event(2));
        ring.push(event(3));

        let TraceExport::Append(records) = ring.export_delta(None) else {
            panic!("a new exporter should receive retained records");
        };
        assert_eq!(records.len(), 3);
        let mut serialized = String::new();
        for record in records {
            append_trace_record_jsonl(&mut serialized, record);
        }
        assert!(serialized.contains("\"from_sequence\":1"));
        assert!(serialized.contains("\"to_sequence\":1"));
        assert!(serialized.contains("\"sequence\":2"));
        assert!(serialized.contains("\"sequence\":3"));
        assert!(serialized.contains("\"source\":\"native_reactor\""));
        assert!(serialized.contains("\"schema_version\":1"));
    }

    #[test]
    fn sequence_remains_ordered_across_u64_boundary() {
        let mut ring = PresentationTransactionTraceRing::new(4);
        ring.next_sequence = u64::MAX as u128;
        ring.push(event(1));
        ring.push(event(2));

        let TraceExport::Append(records) = ring.export_delta(Some(u64::MAX as u128 - 1)) else {
            panic!("expected both records after the cursor");
        };
        assert_eq!(records.len(), 2);
        assert!(
            matches!(records[0], TraceExportRecord::Event(record) if record.sequence == u64::MAX as u128)
        );
        assert!(
            matches!(records[1], TraceExportRecord::Event(record) if record.sequence == u64::MAX as u128 + 1)
        );
    }

    #[test]
    fn repeated_ring_wraps_keep_export_cost_proportional_to_new_records() {
        const CAPACITY: usize = 64;
        let mut ring = PresentationTransactionTraceRing::new(CAPACITY);
        for timestamp in 1..=CAPACITY as u64 {
            ring.push(event(timestamp));
        }
        let mut cursor = ring.export_cursor();

        for timestamp in (CAPACITY as u64 + 1)..=(CAPACITY as u64 * 11) {
            ring.push(event(timestamp));
        }
        let TraceExport::Append(backlog) = ring.export_delta(cursor) else {
            panic!("expected the retained backlog after repeated wraps");
        };
        assert_eq!(backlog.len(), CAPACITY + 1);
        assert!(matches!(
            backlog[0],
            TraceExportRecord::LostRange {
                from_sequence: 65,
                to_sequence: 640,
                ..
            }
        ));
        cursor = ring.export_cursor();

        ring.push(event(CAPACITY as u64 * 11 + 1));
        let TraceExport::Append(delta) = ring.export_delta(cursor) else {
            panic!("expected one new event after the ring was saturated");
        };
        assert_eq!(delta.len(), 1);
        assert!(matches!(
            delta[0],
            TraceExportRecord::Event(record)
                if record.sequence == CAPACITY as u128 * 11 + 1
        ));
    }
}
