use super::transaction::PresentationTransactionId;
use std::{collections::VecDeque, fmt::Write as _};

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
        transaction_id: PresentationTransactionId,
        timestamp_ns: u64,
    },
    AcquireReady {
        transaction_id: PresentationTransactionId,
        timestamp_ns: u64,
    },
    TransactionBuilt {
        transaction_id: PresentationTransactionId,
        timestamp_ns: u64,
    },
    RenderStarted {
        transaction_id: PresentationTransactionId,
        timestamp_ns: u64,
    },
    RenderCompleted {
        transaction_id: PresentationTransactionId,
        timestamp_ns: u64,
    },
    TestOnlyStarted {
        transaction_id: PresentationTransactionId,
        timestamp_ns: u64,
    },
    TestOnlyCompleted {
        transaction_id: PresentationTransactionId,
        timestamp_ns: u64,
    },
    KmsSubmitStarted {
        transaction_id: PresentationTransactionId,
        timestamp_ns: u64,
    },
    KmsSubmitReturned {
        transaction_id: PresentationTransactionId,
        timestamp_ns: u64,
    },
    PageflipPresented {
        transaction_id: PresentationTransactionId,
        timestamp_ns: u64,
    },
    Superseded {
        transaction_id: PresentationTransactionId,
        timestamp_ns: u64,
    },
    SameBufferSuppressed {
        transaction_id: PresentationTransactionId,
        timestamp_ns: u64,
    },
    Rejected {
        transaction_id: PresentationTransactionId,
        timestamp_ns: u64,
    },
    FeedbackCompleted {
        transaction_id: PresentationTransactionId,
        timestamp_ns: u64,
    },
    BufferReleased {
        transaction_id: PresentationTransactionId,
        timestamp_ns: u64,
    },
}

impl PresentationTransactionEvent {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::ContentObserved { .. } => "content_observed",
            Self::AcquireReady { .. } => "acquire_ready",
            Self::TransactionBuilt { .. } => "transaction_built",
            Self::RenderStarted { .. } => "render_started",
            Self::RenderCompleted { .. } => "render_completed",
            Self::TestOnlyStarted { .. } => "test_only_started",
            Self::TestOnlyCompleted { .. } => "test_only_completed",
            Self::KmsSubmitStarted { .. } => "kms_submit_started",
            Self::KmsSubmitReturned { .. } => "kms_submit_returned",
            Self::PageflipPresented { .. } => "pageflip_presented",
            Self::Superseded { .. } => "superseded",
            Self::SameBufferSuppressed { .. } => "same_buffer_suppressed",
            Self::Rejected { .. } => "rejected",
            Self::FeedbackCompleted { .. } => "feedback_completed",
            Self::BufferReleased { .. } => "buffer_released",
        }
    }

    #[allow(dead_code)]
    pub(crate) const fn transaction_id(self) -> PresentationTransactionId {
        match self {
            Self::ContentObserved { transaction_id, .. }
            | Self::AcquireReady { transaction_id, .. }
            | Self::TransactionBuilt { transaction_id, .. }
            | Self::RenderStarted { transaction_id, .. }
            | Self::RenderCompleted { transaction_id, .. }
            | Self::TestOnlyStarted { transaction_id, .. }
            | Self::TestOnlyCompleted { transaction_id, .. }
            | Self::KmsSubmitStarted { transaction_id, .. }
            | Self::KmsSubmitReturned { transaction_id, .. }
            | Self::PageflipPresented { transaction_id, .. }
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
            | Self::RenderStarted { timestamp_ns, .. }
            | Self::RenderCompleted { timestamp_ns, .. }
            | Self::TestOnlyStarted { timestamp_ns, .. }
            | Self::TestOnlyCompleted { timestamp_ns, .. }
            | Self::KmsSubmitStarted { timestamp_ns, .. }
            | Self::KmsSubmitReturned { timestamp_ns, .. }
            | Self::PageflipPresented { timestamp_ns, .. }
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
}

impl PresentationTransactionTraceRing {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity,
            events: VecDeque::with_capacity(capacity),
            dropped: 0,
            enabled: true,
        }
    }

    pub(crate) fn disabled(capacity: usize) -> Self {
        let mut ring = Self::new(capacity);
        ring.enabled = false;
        ring
    }

    pub(crate) fn from_env() -> Self {
        let enabled = std::env::var("OBLIVION_ONE_PRESENTATION_TRACE")
            .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "on"));
        let capacity = std::env::var("OBLIVION_ONE_PRESENTATION_TRACE_CAPACITY")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(4096)
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
        if self.events.len() == self.capacity {
            self.events.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.events.push_back(event);
    }

    pub(crate) fn len(&self) -> usize {
        self.events.len()
    }

    pub(crate) const fn dropped(&self) -> u64 {
        self.dropped
    }

    #[allow(dead_code)]
    pub(crate) fn summarize(
        &self,
        transaction_id: PresentationTransactionId,
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
            let _ = writeln!(
                output,
                "{{\"event\":\"{}\",\"transaction_id\":{},\"timestamp_ns\":{}}}",
                event.name(),
                event.transaction_id().get(),
                event.timestamp_ns(),
            );
        }
        output
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TimingSummary {
    pub(crate) count: u64,
    pub(crate) max_ns: u64,
    pub(crate) buckets: [u64; 8],
}

impl TimingSummary {
    pub(crate) fn record(&mut self, elapsed_ns: u64) {
        self.count = self.count.saturating_add(1);
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
