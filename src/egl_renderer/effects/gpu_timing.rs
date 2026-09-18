use std::{
    collections::VecDeque,
    ffi::{OsStr, c_void},
};

use glow::HasContext;
use oblivion_one::effects::{MAX_EFFECT_INSTANCES_PER_OUTPUT, MAX_GRAPH_PASSES, RenderPassKind};

const MAX_COLLECTION_PER_CALL: usize = 64;
const GPU_TIMING_ENV: &str = "TYPHON_EFFECT_GPU_TIMING";
const GPU_DISJOINT_EXT: u32 = 0x8fbb;
const EXPECTED_IN_FLIGHT_GRAPH_SCOPES: usize = 2;
const CURRENT_BUILTIN_BLUR_PASSES: usize = 2;
const CURRENT_PASSES_PER_BLUR_INSTANCE: usize = 1 + CURRENT_BUILTIN_BLUR_PASSES * 2 + 1;
const TIMING_SPAN_POOL_CAPACITY: usize = 2048;
const TIMING_QUERY_OBJECT_CAPACITY: usize = TIMING_SPAN_POOL_CAPACITY * 2;

const _: () = assert!(
    TIMING_SPAN_POOL_CAPACITY
        >= EXPECTED_IN_FLIGHT_GRAPH_SCOPES
            * MAX_EFFECT_INSTANCES_PER_OUTPUT
            * CURRENT_PASSES_PER_BLUR_INSTANCE
);
const _: () = assert!(TIMING_SPAN_POOL_CAPACITY <= MAX_GRAPH_PASSES);

fn gpu_timing_requested(value: Option<&OsStr>) -> bool {
    value == Some(OsStr::new("1"))
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProfilerStateKind {
    Disabled,
    Unsupported,
    Active,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SlotPhase {
    Free,
    Open,
    Pending,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SpanToken {
    slot: usize,
    generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GraphTimingScope {
    scope_id: u64,
    total: SpanToken,
    frame_id: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CaptureTimingMode {
    Replay,
    FramebufferBlit,
}

impl CaptureTimingMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Replay => "replay",
            Self::FramebufferBlit => "framebuffer_blit",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CaptureTimingMetadata {
    pub(crate) mode: CaptureTimingMode,
    pub(crate) checkpoint_count: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CaptureExecutionTimingSummary {
    pub(crate) capture_execution_pixels: u64,
    pub(crate) scene_capture_execution_pixels: u64,
    pub(crate) surface_capture_execution_pixels: u64,
    pub(crate) replay_capture_execution_pixels: u64,
    pub(crate) framebuffer_capture_execution_pixels: u64,
    pub(crate) checkpoint_capture_execution_pixels: u64,
    pub(crate) replay_capture_passes: usize,
    pub(crate) framebuffer_capture_passes: usize,
    pub(crate) checkpoint_capture_passes: usize,
    pub(crate) replay_capture_commands: usize,
    pub(crate) checkpoint_dependency_edges: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TimingSpanMetadata {
    scope_id: u64,
    frame_id: Option<u64>,
    pass_id: Option<u64>,
    instance_id: Option<u64>,
    kind: Option<oblivion_one::effects::RenderPassKind>,
    pixels: u64,
    capture: Option<CaptureTimingMetadata>,
    is_total: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PollOutcome {
    Empty,
    NotReady,
    Ready {
        duration_ns: Option<u64>,
        record: Option<GpuTimingRecord>,
    },
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimingCategory {
    SceneCapture,
    SurfaceCapture,
    NormalizeInput,
    DualKawaseDownsample,
    DualKawaseUpsample,
    Fragment,
    Blend,
    Mask,
    Composite,
    OutputPostProcess,
}

impl TimingCategory {
    const fn index(self) -> usize {
        match self {
            Self::SceneCapture => 0,
            Self::SurfaceCapture => 1,
            Self::NormalizeInput => 2,
            Self::DualKawaseDownsample => 3,
            Self::DualKawaseUpsample => 4,
            Self::Fragment => 5,
            Self::Blend => 6,
            Self::Mask => 7,
            Self::Composite => 8,
            Self::OutputPostProcess => 9,
        }
    }

    fn from_render_pass_kind(kind: oblivion_one::effects::RenderPassKind) -> Self {
        match kind {
            RenderPassKind::SceneCapture => Self::SceneCapture,
            RenderPassKind::SurfaceCapture => Self::SurfaceCapture,
            RenderPassKind::NormalizeInput => Self::NormalizeInput,
            RenderPassKind::DualKawaseDownsample => Self::DualKawaseDownsample,
            RenderPassKind::DualKawaseUpsample => Self::DualKawaseUpsample,
            RenderPassKind::Fragment => Self::Fragment,
            RenderPassKind::Blend => Self::Blend,
            RenderPassKind::Mask => Self::Mask,
            RenderPassKind::Composite => Self::Composite,
            RenderPassKind::OutputPostProcess => Self::OutputPostProcess,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MaxCapturePassTiming {
    duration_ns: u64,
    pass_id: u64,
    instance_id: u64,
    kind: RenderPassKind,
    mode: CaptureTimingMode,
    effect_pixels: u64,
    checkpoint_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GpuTimingRecord {
    frame_id: Option<u64>,
    scope_id: u64,
    total_ns: u64,
    durations_ns: [u64; 10],
    pixels: [u64; 10],
    timed_passes: usize,
    dropped_passes: usize,
    query_pool_capacity: usize,
    query_pool_high_water: usize,
    dropped_spans: usize,
    disjoint_invalidated_spans: usize,
    scene_capture_ns: u64,
    surface_capture_ns: u64,
    replay_capture_ns: u64,
    framebuffer_capture_ns: u64,
    checkpoint_capture_ns: u64,
    scene_capture_passes: usize,
    surface_capture_passes: usize,
    replay_capture_passes: usize,
    framebuffer_capture_passes: usize,
    checkpoint_capture_passes: usize,
    scene_capture_pixels: u64,
    surface_capture_pixels: u64,
    replay_capture_pixels: u64,
    framebuffer_capture_pixels: u64,
    checkpoint_capture_pixels: u64,
    capture_execution_pixels: u64,
    scene_capture_execution_pixels: u64,
    surface_capture_execution_pixels: u64,
    replay_capture_execution_pixels: u64,
    framebuffer_capture_execution_pixels: u64,
    checkpoint_capture_execution_pixels: u64,
    replay_capture_execution_passes: usize,
    framebuffer_capture_execution_passes: usize,
    checkpoint_capture_execution_passes: usize,
    replay_capture_commands: usize,
    checkpoint_dependency_edges: usize,
    capture_execution_summary_available: bool,
    max_capture_pass: Option<MaxCapturePassTiming>,
}

impl GpuTimingRecord {
    fn capture_ns(&self) -> u64 {
        self.durations_ns[TimingCategory::SceneCapture.index()]
            .saturating_add(self.durations_ns[TimingCategory::SurfaceCapture.index()])
    }
}

#[derive(Clone, Copy, Debug)]
struct SlotState {
    phase: SlotPhase,
    generation: u64,
    metadata: Option<TimingSpanMetadata>,
}

#[derive(Clone, Copy, Debug)]
struct PendingSpan {
    token: SpanToken,
    metadata: TimingSpanMetadata,
}

#[derive(Clone, Copy, Debug)]
struct GraphAggregate {
    scope_id: u64,
    frame_id: Option<u64>,
    durations_ns: [u64; 10],
    pixels: [u64; 10],
    timed_passes: usize,
    dropped_passes: usize,
    scene_capture_ns: u64,
    surface_capture_ns: u64,
    replay_capture_ns: u64,
    framebuffer_capture_ns: u64,
    checkpoint_capture_ns: u64,
    scene_capture_passes: usize,
    surface_capture_passes: usize,
    replay_capture_passes: usize,
    framebuffer_capture_passes: usize,
    checkpoint_capture_passes: usize,
    scene_capture_pixels: u64,
    surface_capture_pixels: u64,
    replay_capture_pixels: u64,
    framebuffer_capture_pixels: u64,
    checkpoint_capture_pixels: u64,
    capture_execution: Option<CaptureExecutionTimingSummary>,
    max_capture_pass: Option<MaxCapturePassTiming>,
}

#[derive(Debug)]
struct TimingState {
    enabled: bool,
    slots: Vec<SlotState>,
    free_slots: Vec<usize>,
    pending: VecDeque<PendingSpan>,
    aggregates: Vec<GraphAggregate>,
    next_scope_id: u64,
    dropped_spans: usize,
    high_water: usize,
    disjoint_invalidated_spans: usize,
    #[cfg(test)]
    read_count: usize,
}

impl TimingState {
    #[cfg(test)]
    fn disabled() -> Self {
        Self {
            enabled: false,
            slots: Vec::new(),
            free_slots: Vec::new(),
            pending: VecDeque::new(),
            aggregates: Vec::new(),
            next_scope_id: 1,
            dropped_spans: 0,
            high_water: 0,
            disjoint_invalidated_spans: 0,
            #[cfg(test)]
            read_count: 0,
        }
    }

    #[cfg(test)]
    fn active_for_test(capacity: usize) -> Self {
        Self::active(capacity)
    }

    fn active(capacity: usize) -> Self {
        let mut free_slots = (0..capacity).collect::<Vec<_>>();
        free_slots.reverse();
        Self {
            enabled: true,
            slots: vec![
                SlotState {
                    phase: SlotPhase::Free,
                    generation: 0,
                    metadata: None,
                };
                capacity
            ],
            free_slots,
            pending: VecDeque::new(),
            aggregates: Vec::with_capacity(capacity),
            next_scope_id: 1,
            dropped_spans: 0,
            high_water: 0,
            disjoint_invalidated_spans: 0,
            #[cfg(test)]
            read_count: 0,
        }
    }

    fn begin_scope(&mut self, frame_id: Option<u64>) -> Option<GraphTimingScope> {
        if !self.enabled {
            return None;
        }
        let scope_id = self.next_scope_id;
        self.next_scope_id = self.next_scope_id.saturating_add(1);
        let token = self.allocate_span(TimingSpanMetadata {
            scope_id,
            frame_id,
            pass_id: None,
            instance_id: None,
            kind: None,
            pixels: 0,
            capture: None,
            is_total: true,
        });
        let Some(token) = token else {
            return None;
        };
        self.aggregates.push(GraphAggregate {
            scope_id,
            frame_id,
            durations_ns: [0; 10],
            pixels: [0; 10],
            timed_passes: 0,
            dropped_passes: 0,
            scene_capture_ns: 0,
            surface_capture_ns: 0,
            replay_capture_ns: 0,
            framebuffer_capture_ns: 0,
            checkpoint_capture_ns: 0,
            scene_capture_passes: 0,
            surface_capture_passes: 0,
            replay_capture_passes: 0,
            framebuffer_capture_passes: 0,
            checkpoint_capture_passes: 0,
            scene_capture_pixels: 0,
            surface_capture_pixels: 0,
            replay_capture_pixels: 0,
            framebuffer_capture_pixels: 0,
            checkpoint_capture_pixels: 0,
            capture_execution: None,
            max_capture_pass: None,
        });
        Some(GraphTimingScope {
            scope_id,
            total: token,
            frame_id,
        })
    }

    fn begin_pass(
        &mut self,
        scope: GraphTimingScope,
        metadata: TimingSpanMetadata,
    ) -> Option<SpanToken> {
        if !self.enabled || !self.has_scope(scope.scope_id) {
            return None;
        }
        let token = self.allocate_span(TimingSpanMetadata {
            scope_id: scope.scope_id,
            ..metadata
        });
        if token.is_none()
            && let Some(aggregate) = self
                .aggregates
                .iter_mut()
                .find(|aggregate| aggregate.scope_id == scope.scope_id)
        {
            aggregate.dropped_passes = aggregate.dropped_passes.saturating_add(1);
        }
        token.map(|token| {
            debug_assert!(!metadata.is_total);
            token
        })
    }

    fn allocate_span(&mut self, metadata: TimingSpanMetadata) -> Option<SpanToken> {
        let Some(slot) = self.free_slots.pop() else {
            self.dropped_spans = self.dropped_spans.saturating_add(1);
            return None;
        };
        let used_slots = self.slots.len().saturating_sub(self.free_slots.len());
        let state = &mut self.slots[slot];
        state.generation = state.generation.saturating_add(1).max(1);
        state.phase = SlotPhase::Open;
        state.metadata = Some(metadata);
        self.high_water = self.high_water.max(used_slots);
        Some(SpanToken {
            slot,
            generation: state.generation,
        })
    }

    fn finish(&mut self, token: SpanToken) -> bool {
        let Some(state) = self.slots.get_mut(token.slot) else {
            return false;
        };
        if state.generation != token.generation || state.phase != SlotPhase::Open {
            return false;
        }
        let Some(metadata) = state.metadata.take() else {
            return false;
        };
        state.phase = SlotPhase::Pending;
        self.pending.push_back(PendingSpan { token, metadata });
        true
    }

    fn has_scope(&self, scope_id: u64) -> bool {
        self.aggregates
            .iter()
            .any(|aggregate| aggregate.scope_id == scope_id)
    }

    fn attach_capture_execution_summary(
        &mut self,
        scope_id: u64,
        summary: CaptureExecutionTimingSummary,
    ) {
        if let Some(aggregate) = self
            .aggregates
            .iter_mut()
            .find(|aggregate| aggregate.scope_id == scope_id)
        {
            aggregate.capture_execution = Some(summary);
        }
    }

    #[cfg(test)]
    fn query_slots(&self) -> usize {
        self.slots.len()
    }

    #[cfg(test)]
    fn pending_span_count(&self) -> usize {
        self.pending.len()
    }

    #[cfg(test)]
    fn read_count_for_test(&self) -> usize {
        self.read_count
    }

    #[cfg(test)]
    fn dropped_spans(&self) -> usize {
        self.dropped_spans
    }

    #[cfg(test)]
    fn free_slot_count(&self) -> usize {
        self.free_slots.len()
    }

    #[cfg(test)]
    fn disjoint_invalidated_span_count(&self) -> usize {
        self.disjoint_invalidated_spans
    }

    fn pending_query_slots(&self) -> Option<(usize, usize)> {
        self.pending.front().map(|pending| {
            let slot = pending.token.slot;
            (slot, slot)
        })
    }

    #[cfg(test)]
    fn aggregate_count_for_test(&self) -> usize {
        self.aggregates.len()
    }

    fn poll_front(&mut self, available: bool, timestamps: Option<(u64, u64)>) -> PollOutcome {
        if self.pending.is_empty() {
            return PollOutcome::Empty;
        }
        if !available {
            return PollOutcome::NotReady;
        }
        #[cfg(test)]
        {
            self.read_count = self.read_count.saturating_add(1);
        }
        let pending = self
            .pending
            .pop_front()
            .expect("pending span checked above");
        self.recycle_pending_slot(pending.token);
        let Some((start_ns, end_ns)) = timestamps else {
            self.invalidate_resolved_span(pending.metadata);
            return PollOutcome::Invalid;
        };
        let valid = end_ns >= start_ns;
        if !valid {
            self.invalidate_resolved_span(pending.metadata);
            return PollOutcome::Invalid;
        }
        let duration_ns = end_ns - start_ns;
        let record = if pending.metadata.is_total {
            self.finish_total(pending.metadata, duration_ns)
        } else {
            self.finish_pass(pending.metadata, duration_ns);
            None
        };
        PollOutcome::Ready {
            duration_ns: Some(duration_ns),
            record,
        }
    }

    fn recycle_pending_slot(&mut self, token: SpanToken) {
        let state = &mut self.slots[token.slot];
        debug_assert_eq!(state.generation, token.generation);
        debug_assert_eq!(state.phase, SlotPhase::Pending);
        debug_assert!(state.metadata.is_none());
        state.phase = SlotPhase::Free;
        self.free_slots.push(token.slot);
    }

    fn invalidate_resolved_span(&mut self, metadata: TimingSpanMetadata) {
        if !metadata.is_total
            && let Some(aggregate) = self
                .aggregates
                .iter_mut()
                .find(|aggregate| aggregate.scope_id == metadata.scope_id)
        {
            aggregate.dropped_passes = aggregate.dropped_passes.saturating_add(1);
        }
        if metadata.is_total {
            self.aggregates
                .retain(|aggregate| aggregate.scope_id != metadata.scope_id);
        }
    }

    fn finish_pass(&mut self, metadata: TimingSpanMetadata, duration_ns: u64) {
        let Some(kind) = metadata.kind else {
            return;
        };
        let Some(aggregate) = self
            .aggregates
            .iter_mut()
            .find(|aggregate| aggregate.scope_id == metadata.scope_id)
        else {
            return;
        };
        let category = TimingCategory::from_render_pass_kind(kind);
        let index = category.index();
        aggregate.durations_ns[index] = aggregate.durations_ns[index].saturating_add(duration_ns);
        aggregate.pixels[index] = aggregate.pixels[index].saturating_add(metadata.pixels);
        aggregate.timed_passes = aggregate.timed_passes.saturating_add(1);

        let is_capture = matches!(
            kind,
            RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
        );
        let Some(capture) = metadata.capture.filter(|_| is_capture) else {
            return;
        };
        match kind {
            RenderPassKind::SceneCapture => {
                aggregate.scene_capture_ns = aggregate.scene_capture_ns.saturating_add(duration_ns);
                aggregate.scene_capture_passes = aggregate.scene_capture_passes.saturating_add(1);
                aggregate.scene_capture_pixels = aggregate
                    .scene_capture_pixels
                    .saturating_add(metadata.pixels);
            }
            RenderPassKind::SurfaceCapture => {
                aggregate.surface_capture_ns =
                    aggregate.surface_capture_ns.saturating_add(duration_ns);
                aggregate.surface_capture_passes =
                    aggregate.surface_capture_passes.saturating_add(1);
                aggregate.surface_capture_pixels = aggregate
                    .surface_capture_pixels
                    .saturating_add(metadata.pixels);
            }
            _ => unreachable!("capture metadata filtered to capture pass kinds"),
        }
        match capture.mode {
            CaptureTimingMode::Replay => {
                aggregate.replay_capture_ns =
                    aggregate.replay_capture_ns.saturating_add(duration_ns);
                aggregate.replay_capture_passes = aggregate.replay_capture_passes.saturating_add(1);
                aggregate.replay_capture_pixels = aggregate
                    .replay_capture_pixels
                    .saturating_add(metadata.pixels);
            }
            CaptureTimingMode::FramebufferBlit => {
                aggregate.framebuffer_capture_ns =
                    aggregate.framebuffer_capture_ns.saturating_add(duration_ns);
                aggregate.framebuffer_capture_passes =
                    aggregate.framebuffer_capture_passes.saturating_add(1);
                aggregate.framebuffer_capture_pixels = aggregate
                    .framebuffer_capture_pixels
                    .saturating_add(metadata.pixels);
            }
        }
        if capture.checkpoint_count > 0 {
            aggregate.checkpoint_capture_ns =
                aggregate.checkpoint_capture_ns.saturating_add(duration_ns);
            aggregate.checkpoint_capture_passes =
                aggregate.checkpoint_capture_passes.saturating_add(1);
            aggregate.checkpoint_capture_pixels = aggregate
                .checkpoint_capture_pixels
                .saturating_add(metadata.pixels);
        }
        if let (Some(pass_id), Some(instance_id)) = (metadata.pass_id, metadata.instance_id) {
            let candidate = MaxCapturePassTiming {
                duration_ns,
                pass_id,
                instance_id,
                kind,
                mode: capture.mode,
                effect_pixels: metadata.pixels,
                checkpoint_count: capture.checkpoint_count,
            };
            if aggregate
                .max_capture_pass
                .is_none_or(|current| candidate.duration_ns > current.duration_ns)
            {
                aggregate.max_capture_pass = Some(candidate);
            }
        }
    }

    fn finish_total(
        &mut self,
        metadata: TimingSpanMetadata,
        total_ns: u64,
    ) -> Option<GpuTimingRecord> {
        let index = self
            .aggregates
            .iter()
            .position(|aggregate| aggregate.scope_id == metadata.scope_id)?;
        let aggregate = self.aggregates.remove(index);
        Some(GpuTimingRecord {
            frame_id: aggregate.frame_id,
            scope_id: aggregate.scope_id,
            total_ns,
            durations_ns: aggregate.durations_ns,
            pixels: aggregate.pixels,
            timed_passes: aggregate.timed_passes,
            dropped_passes: aggregate.dropped_passes,
            query_pool_capacity: self.slots.len().saturating_mul(2),
            query_pool_high_water: self.high_water.saturating_mul(2),
            dropped_spans: self.dropped_spans,
            disjoint_invalidated_spans: self.disjoint_invalidated_spans,
            scene_capture_ns: aggregate.scene_capture_ns,
            surface_capture_ns: aggregate.surface_capture_ns,
            replay_capture_ns: aggregate.replay_capture_ns,
            framebuffer_capture_ns: aggregate.framebuffer_capture_ns,
            checkpoint_capture_ns: aggregate.checkpoint_capture_ns,
            scene_capture_passes: aggregate.scene_capture_passes,
            surface_capture_passes: aggregate.surface_capture_passes,
            replay_capture_passes: aggregate.replay_capture_passes,
            framebuffer_capture_passes: aggregate.framebuffer_capture_passes,
            checkpoint_capture_passes: aggregate.checkpoint_capture_passes,
            scene_capture_pixels: aggregate.scene_capture_pixels,
            surface_capture_pixels: aggregate.surface_capture_pixels,
            replay_capture_pixels: aggregate.replay_capture_pixels,
            framebuffer_capture_pixels: aggregate.framebuffer_capture_pixels,
            checkpoint_capture_pixels: aggregate.checkpoint_capture_pixels,
            capture_execution_pixels: aggregate
                .capture_execution
                .map_or(0, |summary| summary.capture_execution_pixels),
            scene_capture_execution_pixels: aggregate
                .capture_execution
                .map_or(0, |summary| summary.scene_capture_execution_pixels),
            surface_capture_execution_pixels: aggregate
                .capture_execution
                .map_or(0, |summary| summary.surface_capture_execution_pixels),
            replay_capture_execution_pixels: aggregate
                .capture_execution
                .map_or(0, |summary| summary.replay_capture_execution_pixels),
            framebuffer_capture_execution_pixels: aggregate
                .capture_execution
                .map_or(0, |summary| summary.framebuffer_capture_execution_pixels),
            checkpoint_capture_execution_pixels: aggregate
                .capture_execution
                .map_or(0, |summary| summary.checkpoint_capture_execution_pixels),
            replay_capture_execution_passes: aggregate
                .capture_execution
                .map_or(0, |summary| summary.replay_capture_passes),
            framebuffer_capture_execution_passes: aggregate
                .capture_execution
                .map_or(0, |summary| summary.framebuffer_capture_passes),
            checkpoint_capture_execution_passes: aggregate
                .capture_execution
                .map_or(0, |summary| summary.checkpoint_capture_passes),
            replay_capture_commands: aggregate
                .capture_execution
                .map_or(0, |summary| summary.replay_capture_commands),
            checkpoint_dependency_edges: aggregate
                .capture_execution
                .map_or(0, |summary| summary.checkpoint_dependency_edges),
            capture_execution_summary_available: aggregate.capture_execution.is_some(),
            max_capture_pass: aggregate.max_capture_pass,
        })
    }

    #[cfg(test)]
    fn collect_ready_for_test(&mut self, available_count: usize) -> usize {
        let mut collected = 0;
        while collected < available_count && collected < MAX_COLLECTION_PER_CALL {
            if !matches!(
                self.poll_front(true, Some((100, 140))),
                PollOutcome::Ready { .. } | PollOutcome::Invalid
            ) {
                break;
            }
            collected += 1;
        }
        collected
    }

    #[cfg(test)]
    fn invalidate_pending_for_test(&mut self) -> usize {
        self.invalidate_pending()
    }

    fn invalidate_pending(&mut self) -> usize {
        let count = self.pending.len();
        self.pending.clear();
        for state in &mut self.slots {
            if state.phase == SlotPhase::Pending {
                state.phase = SlotPhase::Free;
                state.metadata = None;
            }
        }
        self.free_slots.clear();
        for (slot, state) in self.slots.iter().enumerate() {
            if state.phase == SlotPhase::Free {
                self.free_slots.push(slot);
            }
        }
        self.aggregates.clear();
        self.disjoint_invalidated_spans = self.disjoint_invalidated_spans.saturating_add(count);
        count
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CollectionAction {
    StopUnavailable,
    InvalidateDisjoint,
    ReadResults,
}

fn collection_action(available: bool, disjoint: bool, uses_disjoint: bool) -> CollectionAction {
    if !available {
        CollectionAction::StopUnavailable
    } else if uses_disjoint && disjoint {
        CollectionAction::InvalidateDisjoint
    } else {
        CollectionAction::ReadResults
    }
}

fn observe_disjoint(timing: &mut TimingState, disjoint: bool) -> bool {
    if disjoint {
        timing.invalidate_pending();
        true
    } else {
        false
    }
}

#[derive(Clone, Copy)]
enum TimestampPath {
    ExtDisjoint(QueryTargetFunction),
    DesktopCore(QueryTargetFunction),
}

impl TimestampPath {
    fn uses_disjoint(self) -> bool {
        match self {
            Self::ExtDisjoint(query_target) => {
                query_target.target == GL_TIMESTAMP_EXT
                    && query_target.pname == GL_QUERY_COUNTER_BITS_EXT
            }
            Self::DesktopCore(query_target) => {
                debug_assert_eq!(query_target.target, glow::TIMESTAMP);
                false
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TimestampQueryPath {
    Ext,
    Core,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TimestampCapabilityInfo {
    embedded: bool,
    major: u32,
    minor: u32,
    has_ext: bool,
    has_arb: bool,
}

type GetQueryiv = unsafe extern "system" fn(u32, u32, *mut i32);

const GL_TIMESTAMP_EXT: u32 = 0x8e28;
const GL_QUERY_COUNTER_BITS_EXT: u32 = 0x8864;

#[cfg(test)]
unsafe extern "system" fn test_queryiv(_target: u32, _pname: u32, _params: *mut i32) {}

#[derive(Clone, Copy)]
struct QueryTargetFunction {
    function: GetQueryiv,
    target: u32,
    pname: u32,
}

#[derive(Clone, Copy, Default)]
struct QueryTargetFunctions {
    core: Option<QueryTargetFunction>,
    ext: Option<QueryTargetFunction>,
}

impl QueryTargetFunctions {
    fn load_with(mut load: impl FnMut(&str) -> Option<*const c_void>) -> Self {
        Self {
            core: load_queryiv(&mut load, "glGetQueryiv").map(|function| QueryTargetFunction {
                function,
                target: glow::TIMESTAMP,
                pname: glow::QUERY_COUNTER_BITS,
            }),
            ext: load_queryiv(&mut load, "glGetQueryivEXT").map(|function| QueryTargetFunction {
                function,
                target: GL_TIMESTAMP_EXT,
                pname: GL_QUERY_COUNTER_BITS_EXT,
            }),
        }
    }

    fn function_for(self, path: TimestampQueryPath) -> Option<QueryTargetFunction> {
        match path {
            TimestampQueryPath::Core => self.core,
            TimestampQueryPath::Ext => self.ext,
        }
    }

    fn counter_bits(self, path: TimestampQueryPath) -> Result<i32, &'static str> {
        let Some(queryiv) = self.function_for(path) else {
            return Err("timestamp-query-counter-function-unavailable");
        };
        let mut counter_bits = 0;
        // SAFETY: the pointer was loaded by its exact EGL symbol name and the
        // arguments use the target/pname pair specified by the selected path.
        unsafe { (queryiv.function)(queryiv.target, queryiv.pname, &mut counter_bits) };
        Ok(counter_bits)
    }
}

fn load_queryiv(
    load: &mut impl FnMut(&str) -> Option<*const c_void>,
    name: &str,
) -> Option<GetQueryiv> {
    let symbol = load(name).filter(|symbol| !symbol.is_null())?;
    // SAFETY: the EGL proc-address loader returned the address for this exact
    // GL entry point, whose ABI and signature are fixed by GLES/OpenGL.
    Some(unsafe { std::mem::transmute::<*const c_void, GetQueryiv>(symbol) })
}

fn select_timestamp_path(
    info: TimestampCapabilityInfo,
    functions: QueryTargetFunctions,
) -> Result<TimestampQueryPath, &'static str> {
    let desktop_core_or_arb =
        !info.embedded && (info.major > 3 || (info.major == 3 && info.minor >= 3) || info.has_arb);
    let path = if desktop_core_or_arb && functions.core.is_some() {
        TimestampQueryPath::Core
    } else if info.has_ext && functions.ext.is_some() {
        TimestampQueryPath::Ext
    } else if desktop_core_or_arb || info.has_ext {
        return Err("timestamp-query-counter-function-unavailable");
    } else {
        return Err("timestamp-query-unavailable");
    };
    if functions.counter_bits(path)? <= 0 {
        return Err("timestamp-query-counter-unavailable");
    }
    Ok(path)
}

#[derive(Clone, Copy, Debug)]
struct QueryPair {
    start: glow::Query,
    end: glow::Query,
}

struct ActiveProfiler {
    path: TimestampPath,
    queries: Vec<QueryPair>,
    timing: TimingState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PassTimingSpan {
    token: SpanToken,
}

fn timestamp_path(
    gl: &glow::Context,
    functions: QueryTargetFunctions,
) -> Result<TimestampPath, &'static str> {
    let path = select_timestamp_path(timestamp_capability_info(gl), functions)?;
    let query_target = functions
        .function_for(path)
        .ok_or("timestamp-query-counter-function-unavailable")?;
    Ok(match path {
        TimestampQueryPath::Ext => TimestampPath::ExtDisjoint(query_target),
        TimestampQueryPath::Core => TimestampPath::DesktopCore(query_target),
    })
}

fn timestamp_capability_info(gl: &glow::Context) -> TimestampCapabilityInfo {
    let extensions = gl.supported_extensions();
    let version = gl.version();
    TimestampCapabilityInfo {
        embedded: version.is_embedded,
        major: version.major,
        minor: version.minor,
        has_ext: extensions.contains("GL_EXT_disjoint_timer_query"),
        has_arb: extensions.contains("GL_ARB_timer_query"),
    }
}

fn gpu_disjoint(gl: &glow::Context) -> bool {
    unsafe { gl.get_parameter_i32(GPU_DISJOINT_EXT) != 0 }
}

fn delete_query_pairs(gl: &glow::Context, queries: &mut Vec<QueryPair>) {
    for pair in queries.drain(..) {
        unsafe {
            gl.delete_query(pair.start);
            gl.delete_query(pair.end);
        }
    }
}

fn create_query_pairs(gl: &glow::Context) -> Result<Vec<QueryPair>, ()> {
    let mut queries = Vec::with_capacity(TIMING_QUERY_OBJECT_CAPACITY / 2);
    for _ in 0..TIMING_SPAN_POOL_CAPACITY {
        let start = match unsafe { gl.create_query() } {
            Ok(start) => start,
            Err(_) => {
                delete_query_pairs(gl, &mut queries);
                return Err(());
            }
        };
        let end = match unsafe { gl.create_query() } {
            Ok(end) => end,
            Err(_) => {
                unsafe { gl.delete_query(start) };
                delete_query_pairs(gl, &mut queries);
                return Err(());
            }
        };
        queries.push(QueryPair { start, end });
    }
    Ok(queries)
}

pub(crate) struct EffectGpuProfiler {
    state: ProfilerState,
}

enum ProfilerState {
    Disabled,
    Unsupported,
    Active(ActiveProfiler),
}

fn format_gpu_timing_line(record: &GpuTimingRecord) -> String {
    let capture_ns = record.capture_ns();
    let capture_pixels = record.pixels[TimingCategory::SceneCapture.index()]
        .saturating_add(record.pixels[TimingCategory::SurfaceCapture.index()]);
    let field = |values: &[u64; 10], category: TimingCategory| values[category.index()];
    let max_capture_pass_id = record.max_capture_pass.map_or(0, |pass| pass.pass_id);
    let max_capture_instance_id = record.max_capture_pass.map_or(0, |pass| pass.instance_id);
    let max_capture_kind = record
        .max_capture_pass
        .map_or("none", |pass| match pass.kind {
            RenderPassKind::SceneCapture => "scene",
            RenderPassKind::SurfaceCapture => "surface",
            _ => "none",
        });
    let max_capture_mode = record
        .max_capture_pass
        .map_or("none", |pass| pass.mode.as_str());
    let max_capture_pass_ns = record.max_capture_pass.map_or(0, |pass| pass.duration_ns);
    let max_capture_pixels = record.max_capture_pass.map_or(0, |pass| pass.effect_pixels);
    let max_capture_checkpoint_count = record
        .max_capture_pass
        .map_or(0, |pass| pass.checkpoint_count);
    format!(
        "event=effect_gpu_timing frame_id={} scope={} total_ns={} capture_ns={} normalize_ns={} blur_downsample_ns={} blur_upsample_ns={} fragment_ns={} blend_ns={} mask_ns={} composite_ns={} postprocess_ns={} timed_passes={} dropped_passes={} capture_pixels={} normalize_pixels={} blur_downsample_pixels={} blur_upsample_pixels={} fragment_pixels={} blend_pixels={} mask_pixels={} composite_pixels={} postprocess_pixels={} query_pool_capacity={} query_pool_high_water={} dropped_spans={} disjoint_invalidated_spans={} scene_capture_ns={} surface_capture_ns={} replay_capture_ns={} framebuffer_capture_ns={} checkpoint_capture_ns={} scene_capture_passes={} surface_capture_passes={} replay_capture_passes={} framebuffer_capture_passes={} checkpoint_capture_passes={} scene_capture_pixels={} surface_capture_pixels={} replay_capture_pixels={} framebuffer_capture_pixels={} checkpoint_capture_pixels={} capture_execution_summary_available={} capture_execution_pixels={} scene_capture_execution_pixels={} surface_capture_execution_pixels={} replay_capture_execution_pixels={} framebuffer_capture_execution_pixels={} checkpoint_capture_execution_pixels={} replay_capture_execution_passes={} framebuffer_capture_execution_passes={} checkpoint_capture_execution_passes={} replay_capture_commands={} checkpoint_dependency_edges={} max_capture_pass_ns={} max_capture_pass_id={} max_capture_instance_id={} max_capture_kind={} max_capture_mode={} max_capture_pixels={} max_capture_checkpoint_count={}",
        record
            .frame_id
            .map_or_else(|| "unknown".to_owned(), |id| id.to_string()),
        record.scope_id,
        record.total_ns,
        capture_ns,
        field(&record.durations_ns, TimingCategory::NormalizeInput),
        field(&record.durations_ns, TimingCategory::DualKawaseDownsample),
        field(&record.durations_ns, TimingCategory::DualKawaseUpsample),
        field(&record.durations_ns, TimingCategory::Fragment),
        field(&record.durations_ns, TimingCategory::Blend),
        field(&record.durations_ns, TimingCategory::Mask),
        field(&record.durations_ns, TimingCategory::Composite),
        field(&record.durations_ns, TimingCategory::OutputPostProcess),
        record.timed_passes,
        record.dropped_passes,
        capture_pixels,
        field(&record.pixels, TimingCategory::NormalizeInput),
        field(&record.pixels, TimingCategory::DualKawaseDownsample),
        field(&record.pixels, TimingCategory::DualKawaseUpsample),
        field(&record.pixels, TimingCategory::Fragment),
        field(&record.pixels, TimingCategory::Blend),
        field(&record.pixels, TimingCategory::Mask),
        field(&record.pixels, TimingCategory::Composite),
        field(&record.pixels, TimingCategory::OutputPostProcess),
        record.query_pool_capacity,
        record.query_pool_high_water,
        record.dropped_spans,
        record.disjoint_invalidated_spans,
        record.scene_capture_ns,
        record.surface_capture_ns,
        record.replay_capture_ns,
        record.framebuffer_capture_ns,
        record.checkpoint_capture_ns,
        record.scene_capture_passes,
        record.surface_capture_passes,
        record.replay_capture_passes,
        record.framebuffer_capture_passes,
        record.checkpoint_capture_passes,
        record.scene_capture_pixels,
        record.surface_capture_pixels,
        record.replay_capture_pixels,
        record.framebuffer_capture_pixels,
        record.checkpoint_capture_pixels,
        usize::from(record.capture_execution_summary_available),
        record.capture_execution_pixels,
        record.scene_capture_execution_pixels,
        record.surface_capture_execution_pixels,
        record.replay_capture_execution_pixels,
        record.framebuffer_capture_execution_pixels,
        record.checkpoint_capture_execution_pixels,
        record.replay_capture_execution_passes,
        record.framebuffer_capture_execution_passes,
        record.checkpoint_capture_execution_passes,
        record.replay_capture_commands,
        record.checkpoint_dependency_edges,
        max_capture_pass_ns,
        max_capture_pass_id,
        max_capture_instance_id,
        max_capture_kind,
        max_capture_mode,
        max_capture_pixels,
        max_capture_checkpoint_count,
    )
}

impl EffectGpuProfiler {
    pub(crate) fn new(
        gl: &glow::Context,
        load_with: impl FnMut(&str) -> Option<*const c_void>,
    ) -> Self {
        if !gpu_timing_requested(std::env::var_os(GPU_TIMING_ENV).as_deref()) {
            return Self {
                state: ProfilerState::Disabled,
            };
        }
        let functions = QueryTargetFunctions::load_with(load_with);
        let path = match timestamp_path(gl, functions) {
            Ok(path) => path,
            Err(reason) => {
                eprintln!("typhon effect: event=effect_gpu_timing_unsupported reason={reason}");
                return Self {
                    state: ProfilerState::Unsupported,
                };
            }
        };
        let queries = match create_query_pairs(gl) {
            Ok(queries) => queries,
            Err(()) => {
                eprintln!(
                    "typhon effect: event=effect_gpu_timing_unsupported reason=query-pool-create-failed"
                );
                return Self {
                    state: ProfilerState::Unsupported,
                };
            }
        };
        Self {
            state: ProfilerState::Active(ActiveProfiler {
                path,
                queries,
                timing: TimingState::active(TIMING_SPAN_POOL_CAPACITY),
            }),
        }
    }

    pub(crate) fn collect(&mut self, gl: &glow::Context) {
        let ProfilerState::Active(active) = &mut self.state else {
            return;
        };
        for _ in 0..MAX_COLLECTION_PER_CALL {
            let Some((_, end_slot)) = active.timing.pending_query_slots() else {
                break;
            };
            let pair = active.queries[end_slot];
            let available =
                unsafe { gl.get_query_parameter_u32(pair.end, glow::QUERY_RESULT_AVAILABLE) } != 0;
            if !available {
                break;
            }
            let disjoint = active.path.uses_disjoint() && gpu_disjoint(gl);
            match collection_action(true, disjoint, active.path.uses_disjoint()) {
                CollectionAction::InvalidateDisjoint => {
                    observe_disjoint(&mut active.timing, true);
                    return;
                }
                CollectionAction::ReadResults => {}
                CollectionAction::StopUnavailable => unreachable!("availability was checked"),
            }
            let start_ns = unsafe { gl.get_query_parameter_u64(pair.start, glow::QUERY_RESULT) };
            let end_ns = unsafe { gl.get_query_parameter_u64(pair.end, glow::QUERY_RESULT) };
            if let PollOutcome::Ready {
                record: Some(record),
                ..
            } = active.timing.poll_front(true, Some((start_ns, end_ns)))
            {
                eprintln!("typhon effect: {}", format_gpu_timing_line(&record));
            }
        }
    }

    pub(crate) fn begin_graph(
        &mut self,
        gl: &glow::Context,
        frame_id: Option<u64>,
    ) -> Option<GraphTimingScope> {
        let ProfilerState::Active(active) = &mut self.state else {
            return None;
        };
        if active.path.uses_disjoint() && observe_disjoint(&mut active.timing, gpu_disjoint(gl)) {
            return None;
        }
        let scope = active.timing.begin_scope(frame_id)?;
        let query = active.queries[scope.total.slot].start;
        unsafe { gl.query_counter(query, glow::TIMESTAMP) };
        Some(scope)
    }

    pub(crate) fn end_graph(
        &mut self,
        gl: &glow::Context,
        scope: Option<GraphTimingScope>,
        capture_execution: Option<CaptureExecutionTimingSummary>,
    ) {
        let Some(scope) = scope else {
            return;
        };
        let ProfilerState::Active(active) = &mut self.state else {
            return;
        };
        if let Some(summary) = capture_execution {
            active
                .timing
                .attach_capture_execution_summary(scope.scope_id, summary);
        }
        if active.timing.finish(scope.total) {
            let query = active.queries[scope.total.slot].end;
            unsafe { gl.query_counter(query, glow::TIMESTAMP) };
        }
    }

    pub(crate) fn begin_pass(
        &mut self,
        gl: &glow::Context,
        scope: GraphTimingScope,
        pass_id: u64,
        instance_id: u64,
        kind: RenderPassKind,
        pixels: u64,
        capture: Option<CaptureTimingMetadata>,
    ) -> Option<PassTimingSpan> {
        let ProfilerState::Active(active) = &mut self.state else {
            return None;
        };
        let metadata = TimingSpanMetadata {
            scope_id: scope.scope_id,
            frame_id: scope.frame_id,
            pass_id: Some(pass_id),
            instance_id: Some(instance_id),
            kind: Some(kind),
            pixels,
            capture,
            is_total: false,
        };
        let token = active.timing.begin_pass(scope, metadata)?;
        let query = active.queries[token.slot].start;
        unsafe { gl.query_counter(query, glow::TIMESTAMP) };
        Some(PassTimingSpan { token })
    }

    pub(crate) fn end_pass(&mut self, gl: &glow::Context, span: Option<PassTimingSpan>) {
        let Some(span) = span else {
            return;
        };
        let ProfilerState::Active(active) = &mut self.state else {
            return;
        };
        if active.timing.finish(span.token) {
            let query = active.queries[span.token.slot].end;
            unsafe { gl.query_counter(query, glow::TIMESTAMP) };
        }
    }

    pub(crate) fn destroy(&mut self, gl: &glow::Context) {
        let state = std::mem::replace(&mut self.state, ProfilerState::Disabled);
        if let ProfilerState::Active(mut active) = state {
            delete_query_pairs(gl, &mut active.queries);
        }
    }
}

#[cfg(test)]
impl EffectGpuProfiler {
    fn active_for_test(capacity: usize) -> Self {
        Self {
            state: ProfilerState::Active(ActiveProfiler {
                path: TimestampPath::DesktopCore(QueryTargetFunction {
                    function: test_queryiv,
                    target: glow::TIMESTAMP,
                    pname: glow::QUERY_COUNTER_BITS,
                }),
                queries: Vec::new(),
                timing: TimingState::active_for_test(capacity),
            }),
        }
    }

    fn unsupported_for_test(_reason: &'static str) -> Self {
        Self {
            state: ProfilerState::Unsupported,
        }
    }

    fn state_kind(&self) -> ProfilerStateKind {
        match &self.state {
            ProfilerState::Disabled => ProfilerStateKind::Disabled,
            ProfilerState::Unsupported => ProfilerStateKind::Unsupported,
            ProfilerState::Active(_) => ProfilerStateKind::Active,
        }
    }

    fn pending_span_count_for_test(&self) -> usize {
        match &self.state {
            ProfilerState::Active(active) => active.timing.pending.len(),
            ProfilerState::Disabled | ProfilerState::Unsupported => 0,
        }
    }

    fn allocated_query_count_for_test(&self) -> usize {
        match &self.state {
            ProfilerState::Active(active) => active.timing.query_slots().saturating_mul(2),
            ProfilerState::Disabled | ProfilerState::Unsupported => 0,
        }
    }

    fn destroy_for_test(&mut self) -> usize {
        let state = std::mem::replace(&mut self.state, ProfilerState::Disabled);
        match state {
            ProfilerState::Active(active) => active.timing.query_slots().saturating_mul(2),
            ProfilerState::Disabled | ProfilerState::Unsupported => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicI32, AtomicUsize, Ordering},
    };

    use super::*;
    use oblivion_one::effects::RenderPassKind;

    static CORE_QUERYIV_CALLS: AtomicUsize = AtomicUsize::new(0);
    static CORE_QUERYIV_TARGET: AtomicI32 = AtomicI32::new(0);
    static CORE_QUERYIV_PNAME: AtomicI32 = AtomicI32::new(0);
    static EXT_QUERYIV_CALLS: AtomicUsize = AtomicUsize::new(0);
    static EXT_QUERYIV_TARGET: AtomicI32 = AtomicI32::new(0);
    static EXT_QUERYIV_PNAME: AtomicI32 = AtomicI32::new(0);
    static QUERYIV_TEST_LOCK: Mutex<()> = Mutex::new(());

    unsafe extern "system" fn fake_core_get_queryiv(target: u32, pname: u32, params: *mut i32) {
        CORE_QUERYIV_CALLS.fetch_add(1, Ordering::Relaxed);
        CORE_QUERYIV_TARGET.store(target as i32, Ordering::Relaxed);
        CORE_QUERYIV_PNAME.store(pname as i32, Ordering::Relaxed);
        unsafe { *params = 64 };
    }

    unsafe extern "system" fn fake_ext_get_queryiv(target: u32, pname: u32, params: *mut i32) {
        EXT_QUERYIV_CALLS.fetch_add(1, Ordering::Relaxed);
        EXT_QUERYIV_TARGET.store(target as i32, Ordering::Relaxed);
        EXT_QUERYIV_PNAME.store(pname as i32, Ordering::Relaxed);
        unsafe { *params = 64 };
    }

    fn reset_queryiv_calls() {
        CORE_QUERYIV_CALLS.store(0, Ordering::Relaxed);
        CORE_QUERYIV_TARGET.store(0, Ordering::Relaxed);
        CORE_QUERYIV_PNAME.store(0, Ordering::Relaxed);
        EXT_QUERYIV_CALLS.store(0, Ordering::Relaxed);
        EXT_QUERYIV_TARGET.store(0, Ordering::Relaxed);
        EXT_QUERYIV_PNAME.store(0, Ordering::Relaxed);
    }

    fn capture_metadata(mode: CaptureTimingMode, checkpoint_count: usize) -> CaptureTimingMetadata {
        CaptureTimingMetadata {
            mode,
            checkpoint_count,
        }
    }

    fn pass_metadata(
        pass_id: u64,
        kind: RenderPassKind,
        pixels: u64,
        capture: Option<CaptureTimingMetadata>,
    ) -> TimingSpanMetadata {
        TimingSpanMetadata {
            scope_id: 1,
            frame_id: Some(120),
            pass_id: Some(pass_id),
            instance_id: Some(1),
            kind: Some(kind),
            pixels,
            capture,
            is_total: false,
        }
    }

    #[test]
    fn disabled_timing_does_not_allocate_or_issue_query_work() {
        let mut state = TimingState::disabled();
        assert!(state.begin_scope(Some(120)).is_none());
        assert_eq!(state.query_slots(), 0);
    }

    #[test]
    fn unsupported_timing_degrades_without_pending_work() {
        let profiler = EffectGpuProfiler::unsupported_for_test("timestamp-query-unavailable");
        assert_eq!(profiler.state_kind(), ProfilerStateKind::Unsupported);
        assert_eq!(profiler.pending_span_count_for_test(), 0);
    }

    #[test]
    fn unavailable_end_query_is_not_read() {
        let mut state = TimingState::active_for_test(2);
        let scope = state.begin_scope(Some(120)).expect("scope slot");
        let pass = state.begin_pass(scope, pass_metadata(7, RenderPassKind::Composite, 64, None));
        state.finish(pass.expect("pass slot"));
        assert_eq!(
            state.poll_front(false, Some((100, 140))),
            PollOutcome::NotReady
        );
        assert_eq!(state.read_count_for_test(), 0);
        assert_eq!(state.pending_span_count(), 1);
    }

    #[test]
    fn ready_timestamps_publish_end_minus_start_and_recycle_pair() {
        let mut state = TimingState::active_for_test(2);
        let scope = state.begin_scope(Some(120)).expect("scope slot");
        let pass = state
            .begin_pass(scope, pass_metadata(7, RenderPassKind::Composite, 64, None))
            .expect("pass slot");
        let free_before = state.free_slot_count();
        assert!(state.finish(pass));
        assert_eq!(
            state.poll_front(true, Some((100, 140))),
            PollOutcome::Ready {
                duration_ns: Some(40),
                record: None,
            }
        );
        assert_eq!(state.free_slot_count(), free_before + 1);
    }

    #[test]
    fn end_before_start_is_discarded_and_recycled() {
        let mut state = TimingState::active_for_test(2);
        let scope = state.begin_scope(Some(120)).expect("scope slot");
        let pass = state
            .begin_pass(scope, pass_metadata(7, RenderPassKind::Composite, 64, None))
            .expect("pass slot");
        assert!(state.finish(pass));
        assert_eq!(
            state.poll_front(true, Some((140, 100))),
            PollOutcome::Invalid
        );
        assert_eq!(state.pending_span_count(), 0);
        assert_eq!(state.free_slot_count(), 1);
    }

    #[test]
    fn double_finish_or_recycle_cannot_return_a_slot_twice() {
        let mut state = TimingState::active_for_test(2);
        let scope = state.begin_scope(Some(120)).expect("scope slot");
        let pass = state
            .begin_pass(scope, pass_metadata(7, RenderPassKind::Composite, 64, None))
            .expect("pass slot");
        assert!(state.finish(pass));
        assert!(!state.finish(pass));
        assert_eq!(
            state.poll_front(true, Some((100, 140))),
            PollOutcome::Ready {
                duration_ns: Some(40),
                record: None
            }
        );
        let free_after_recycle = state.free_slot_count();
        assert_eq!(state.poll_front(true, Some((100, 140))), PollOutcome::Empty);
        assert_eq!(state.free_slot_count(), free_after_recycle);
    }

    #[test]
    fn pool_exhaustion_drops_only_the_new_timing_span() {
        let mut state = TimingState::active_for_test(1);
        let scope = state.begin_scope(Some(120)).expect("total slot");
        assert!(
            state
                .begin_pass(scope, pass_metadata(7, RenderPassKind::Composite, 64, None),)
                .is_none()
        );
        assert_eq!(state.dropped_spans(), 1);
        assert!(state.finish(scope.total));
        assert!(matches!(
            state.poll_front(true, Some((100, 140))),
            PollOutcome::Ready {
                duration_ns: Some(40),
                record: Some(_)
            }
        ));
    }

    #[test]
    fn collection_bound_stops_before_a_large_ready_backlog() {
        let mut state = TimingState::active_for_test(65);
        for frame_id in 0..65 {
            let scope = state.begin_scope(Some(frame_id)).expect("scope slot");
            assert!(state.finish(scope.total));
        }
        assert_eq!(state.collect_ready_for_test(65), 64);
        assert_eq!(state.pending_span_count(), 1);
    }

    #[test]
    fn multiple_frames_preserve_source_frame_ids() {
        let mut state = TimingState::active_for_test(4);
        let first = state.begin_scope(Some(120)).expect("first scope");
        let second = state.begin_scope(Some(121)).expect("second scope");
        assert!(state.finish(first.total));
        assert!(state.finish(second.total));
        let first_record = match state.poll_front(true, Some((100, 140))) {
            PollOutcome::Ready {
                record: Some(record),
                ..
            } => record,
            outcome => panic!("unexpected first outcome: {outcome:?}"),
        };
        let second_record = match state.poll_front(true, Some((200, 250))) {
            PollOutcome::Ready {
                record: Some(record),
                ..
            } => record,
            outcome => panic!("unexpected second outcome: {outcome:?}"),
        };
        assert_eq!(first_record.frame_id, Some(120));
        assert_eq!(second_record.frame_id, Some(121));
    }

    #[test]
    fn multiple_scopes_in_one_frame_remain_distinct() {
        let mut state = TimingState::active_for_test(4);
        let first = state.begin_scope(Some(120)).expect("first scope");
        let second = state.begin_scope(Some(120)).expect("second scope");
        assert!(second.scope_id > first.scope_id);
    }

    #[test]
    fn pass_categories_accumulate_in_correct_buckets() {
        let kinds = [
            RenderPassKind::SceneCapture,
            RenderPassKind::SurfaceCapture,
            RenderPassKind::NormalizeInput,
            RenderPassKind::DualKawaseDownsample,
            RenderPassKind::DualKawaseUpsample,
            RenderPassKind::Fragment,
            RenderPassKind::Blend,
            RenderPassKind::Mask,
            RenderPassKind::Composite,
            RenderPassKind::OutputPostProcess,
        ];
        let mut state = TimingState::active_for_test(kinds.len() + 1);
        let scope = state.begin_scope(Some(120)).expect("scope slot");
        for (index, kind) in kinds.into_iter().enumerate() {
            let pass = state
                .begin_pass(scope, pass_metadata(index as u64, kind, 64, None))
                .expect("pass slot");
            assert!(state.finish(pass));
        }
        for _ in 0..kinds.len() {
            assert!(matches!(
                state.poll_front(true, Some((100, 101))),
                PollOutcome::Ready {
                    duration_ns: Some(1),
                    record: None
                }
            ));
        }
        assert!(state.finish(scope.total));
        let record = match state.poll_front(true, Some((200, 300))) {
            PollOutcome::Ready {
                record: Some(record),
                ..
            } => record,
            outcome => panic!("unexpected total outcome: {outcome:?}"),
        };
        assert_eq!(record.timed_passes, kinds.len());
        assert!(record.durations_ns.iter().all(|duration| *duration == 1));
        assert!(record.pixels.iter().all(|pixels| *pixels == 64));
    }

    #[test]
    fn capture_kind_aggregation_preserves_legacy_capture_totals() {
        let mut state = TimingState::active_for_test(3);
        let scope = state.begin_scope(Some(120)).expect("scope slot");
        let scene = state
            .begin_pass(
                scope,
                pass_metadata(
                    7,
                    RenderPassKind::SceneCapture,
                    100,
                    Some(capture_metadata(CaptureTimingMode::Replay, 0)),
                ),
            )
            .expect("scene capture slot");
        assert!(state.finish(scene));
        assert!(matches!(
            state.poll_front(true, Some((100, 112))),
            PollOutcome::Ready {
                duration_ns: Some(12),
                record: None
            }
        ));
        let surface = state
            .begin_pass(
                scope,
                pass_metadata(
                    8,
                    RenderPassKind::SurfaceCapture,
                    200,
                    Some(capture_metadata(CaptureTimingMode::FramebufferBlit, 0)),
                ),
            )
            .expect("surface capture slot");
        assert!(state.finish(surface));
        assert!(matches!(
            state.poll_front(true, Some((100, 130))),
            PollOutcome::Ready {
                duration_ns: Some(30),
                record: None
            }
        ));
        assert!(state.finish(scope.total));
        let record = match state.poll_front(true, Some((200, 260))) {
            PollOutcome::Ready {
                record: Some(record),
                ..
            } => record,
            outcome => panic!("unexpected outcome: {outcome:?}"),
        };

        assert_eq!(record.capture_ns(), 42);
        assert_eq!(
            record.capture_ns(),
            record.scene_capture_ns + record.surface_capture_ns
        );
        assert_eq!(
            record.capture_ns(),
            record.replay_capture_ns + record.framebuffer_capture_ns
        );
        assert!(record.checkpoint_capture_ns <= record.framebuffer_capture_ns);
        assert_eq!(record.scene_capture_ns, 12);
        assert_eq!(record.surface_capture_ns, 30);
        assert_eq!(record.scene_capture_passes, 1);
        assert_eq!(record.surface_capture_passes, 1);
        assert_eq!(record.scene_capture_pixels, 100);
        assert_eq!(record.surface_capture_pixels, 200);
    }

    #[test]
    fn capture_execution_mode_aggregation_routes_distinct_samples() {
        let mut state = TimingState::active_for_test(4);
        let scope = state.begin_scope(Some(120)).expect("scope slot");
        for (pass_id, mode, duration, pixels) in [
            (7, CaptureTimingMode::Replay, 11, 70),
            (8, CaptureTimingMode::FramebufferBlit, 23, 90),
        ] {
            let pass = state
                .begin_pass(
                    scope,
                    pass_metadata(
                        pass_id,
                        RenderPassKind::SceneCapture,
                        pixels,
                        Some(capture_metadata(mode, 0)),
                    ),
                )
                .expect("capture slot");
            assert!(state.finish(pass));
            assert!(matches!(
                state.poll_front(true, Some((100, 100 + duration))),
                PollOutcome::Ready {
                    duration_ns: Some(_),
                    record: None
                }
            ));
        }
        assert!(state.finish(scope.total));
        let record = match state.poll_front(true, Some((200, 300))) {
            PollOutcome::Ready {
                record: Some(record),
                ..
            } => record,
            outcome => panic!("unexpected outcome: {outcome:?}"),
        };

        assert_eq!(record.replay_capture_ns, 11);
        assert_eq!(record.framebuffer_capture_ns, 23);
        assert_eq!(record.replay_capture_passes, 1);
        assert_eq!(record.framebuffer_capture_passes, 1);
        assert_eq!(record.replay_capture_pixels, 70);
        assert_eq!(record.framebuffer_capture_pixels, 90);
    }

    #[test]
    fn checkpoint_capture_aggregation_uses_dependency_count() {
        let mut state = TimingState::active_for_test(3);
        let scope = state.begin_scope(Some(120)).expect("scope slot");
        for (pass_id, checkpoint_count, duration, pixels) in [(7, 2, 17, 700), (8, 0, 19, 900)] {
            let pass = state
                .begin_pass(
                    scope,
                    pass_metadata(
                        pass_id,
                        RenderPassKind::SceneCapture,
                        pixels,
                        Some(capture_metadata(
                            CaptureTimingMode::FramebufferBlit,
                            checkpoint_count,
                        )),
                    ),
                )
                .expect("capture slot");
            assert!(state.finish(pass));
            assert!(matches!(
                state.poll_front(true, Some((100, 100 + duration))),
                PollOutcome::Ready {
                    duration_ns: Some(_),
                    record: None
                }
            ));
        }
        assert!(state.finish(scope.total));
        let record = match state.poll_front(true, Some((200, 300))) {
            PollOutcome::Ready {
                record: Some(record),
                ..
            } => record,
            outcome => panic!("unexpected outcome: {outcome:?}"),
        };

        assert_eq!(record.checkpoint_capture_ns, 17);
        assert_eq!(record.checkpoint_capture_passes, 1);
        assert_eq!(record.checkpoint_capture_pixels, 700);
        assert_eq!(record.framebuffer_capture_ns, 36);
    }

    #[test]
    fn max_capture_pass_keeps_the_longest_valid_capture_identity() {
        let mut state = TimingState::active_for_test(4);
        let scope = state.begin_scope(Some(120)).expect("scope slot");
        for (pass_id, instance_id, kind, mode, duration, pixels, checkpoint_count) in [
            (
                7,
                11,
                RenderPassKind::SceneCapture,
                CaptureTimingMode::Replay,
                9,
                70,
                0,
            ),
            (
                8,
                12,
                RenderPassKind::SurfaceCapture,
                CaptureTimingMode::FramebufferBlit,
                31,
                90,
                3,
            ),
        ] {
            let mut metadata = pass_metadata(
                pass_id,
                kind,
                pixels,
                Some(capture_metadata(mode, checkpoint_count)),
            );
            metadata.instance_id = Some(instance_id);
            let pass = state.begin_pass(scope, metadata).expect("capture slot");
            assert!(state.finish(pass));
            assert!(matches!(
                state.poll_front(true, Some((100, 100 + duration))),
                PollOutcome::Ready {
                    duration_ns: Some(_),
                    record: None
                }
            ));
        }
        let non_capture = state
            .begin_pass(
                scope,
                pass_metadata(9, RenderPassKind::Composite, 900, None),
            )
            .expect("non-capture slot");
        assert!(state.finish(non_capture));
        assert!(matches!(
            state.poll_front(true, Some((100, 1_000))),
            PollOutcome::Ready {
                duration_ns: Some(900),
                record: None
            }
        ));
        assert!(state.finish(scope.total));
        let record = match state.poll_front(true, Some((200, 300))) {
            PollOutcome::Ready {
                record: Some(record),
                ..
            } => record,
            outcome => panic!("unexpected outcome: {outcome:?}"),
        };
        let max = record.max_capture_pass.expect("max capture pass");

        assert_eq!(max.duration_ns, 31);
        assert_eq!(max.pass_id, 8);
        assert_eq!(max.instance_id, 12);
        assert_eq!(max.kind, RenderPassKind::SurfaceCapture);
        assert_eq!(max.mode, CaptureTimingMode::FramebufferBlit);
        assert_eq!(max.effect_pixels, 90);
        assert_eq!(max.checkpoint_count, 3);
    }

    #[test]
    fn invalid_capture_span_does_not_contribute_to_attribution() {
        let mut state = TimingState::active_for_test(2);
        let scope = state.begin_scope(Some(120)).expect("scope slot");
        let pass = state
            .begin_pass(
                scope,
                pass_metadata(
                    7,
                    RenderPassKind::SceneCapture,
                    700,
                    Some(capture_metadata(CaptureTimingMode::FramebufferBlit, 2)),
                ),
            )
            .expect("capture slot");
        assert!(state.finish(pass));
        assert_eq!(
            state.poll_front(true, Some((200, 100))),
            PollOutcome::Invalid
        );
        assert!(state.finish(scope.total));
        let record = match state.poll_front(true, Some((200, 300))) {
            PollOutcome::Ready {
                record: Some(record),
                ..
            } => record,
            outcome => panic!("unexpected outcome: {outcome:?}"),
        };

        assert_eq!(record.capture_ns(), 0);
        assert_eq!(record.framebuffer_capture_passes, 0);
        assert_eq!(record.checkpoint_capture_ns, 0);
        assert!(record.max_capture_pass.is_none());
        assert_eq!(record.dropped_passes, 1);
    }

    #[test]
    fn execution_summaries_follow_scope_id_not_frame_id() {
        let mut state = TimingState::active_for_test(4);
        let first = state.begin_scope(Some(120)).expect("first scope");
        let second = state.begin_scope(Some(120)).expect("second scope");
        state.attach_capture_execution_summary(
            first.scope_id,
            CaptureExecutionTimingSummary {
                capture_execution_pixels: 111,
                ..Default::default()
            },
        );
        state.attach_capture_execution_summary(
            second.scope_id,
            CaptureExecutionTimingSummary {
                capture_execution_pixels: 222,
                ..Default::default()
            },
        );
        assert!(state.finish(first.total));
        assert!(state.finish(second.total));

        let first_record = match state.poll_front(true, Some((100, 140))) {
            PollOutcome::Ready {
                record: Some(record),
                ..
            } => record,
            outcome => panic!("unexpected first outcome: {outcome:?}"),
        };
        let second_record = match state.poll_front(true, Some((200, 240))) {
            PollOutcome::Ready {
                record: Some(record),
                ..
            } => record,
            outcome => panic!("unexpected second outcome: {outcome:?}"),
        };

        assert_eq!(first_record.capture_execution_pixels, 111);
        assert_eq!(second_record.capture_execution_pixels, 222);
    }

    #[test]
    fn attribution_does_not_change_query_pool_capacity() {
        assert_eq!(TIMING_QUERY_OBJECT_CAPACITY, TIMING_SPAN_POOL_CAPACITY * 2);
        let profiler = EffectGpuProfiler::active_for_test(TIMING_SPAN_POOL_CAPACITY);
        assert_eq!(
            profiler.allocated_query_count_for_test(),
            TIMING_QUERY_OBJECT_CAPACITY
        );
    }

    #[test]
    fn total_completion_returns_one_graph_record() {
        let mut state = TimingState::active_for_test(2);
        let scope = state.begin_scope(Some(120)).expect("scope slot");
        let pass = state
            .begin_pass(scope, pass_metadata(7, RenderPassKind::Composite, 64, None))
            .expect("pass slot");
        assert!(state.finish(pass));
        assert!(matches!(
            state.poll_front(true, Some((100, 140))),
            PollOutcome::Ready { record: None, .. }
        ));
        assert!(state.finish(scope.total));
        assert!(matches!(
            state.poll_front(true, Some((200, 260))),
            PollOutcome::Ready {
                record: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn disjoint_invalidates_all_pending_measurements() {
        let mut state = TimingState::active_for_test(3);
        let scope = state.begin_scope(Some(120)).expect("scope slot");
        let pass = state
            .begin_pass(scope, pass_metadata(7, RenderPassKind::Composite, 64, None))
            .expect("pass slot");
        assert!(state.finish(pass));
        assert!(state.finish(scope.total));
        assert_eq!(state.invalidate_pending_for_test(), 2);
        assert_eq!(state.disjoint_invalidated_span_count(), 2);
        assert_eq!(state.pending_span_count(), 0);
        assert_eq!(state.free_slot_count(), 3);
    }

    #[test]
    fn formatting_is_stable_and_integer_nanoseconds() {
        let record = GpuTimingRecord {
            frame_id: Some(120),
            scope_id: 31,
            total_ns: 281_400,
            durations_ns: [41_200, 0, 0, 78_300, 109_700, 0, 0, 0, 52_200, 0],
            pixels: [640, 0, 0, 320, 160, 0, 0, 0, 640, 0],
            timed_passes: 6,
            dropped_passes: 0,
            query_pool_capacity: 4_096,
            query_pool_high_water: 14,
            dropped_spans: 0,
            disjoint_invalidated_spans: 0,
            scene_capture_ns: 0,
            surface_capture_ns: 0,
            replay_capture_ns: 0,
            framebuffer_capture_ns: 0,
            checkpoint_capture_ns: 0,
            scene_capture_passes: 0,
            surface_capture_passes: 0,
            replay_capture_passes: 0,
            framebuffer_capture_passes: 0,
            checkpoint_capture_passes: 0,
            scene_capture_pixels: 0,
            surface_capture_pixels: 0,
            replay_capture_pixels: 0,
            framebuffer_capture_pixels: 0,
            checkpoint_capture_pixels: 0,
            capture_execution_pixels: 0,
            scene_capture_execution_pixels: 0,
            surface_capture_execution_pixels: 0,
            replay_capture_execution_pixels: 0,
            framebuffer_capture_execution_pixels: 0,
            checkpoint_capture_execution_pixels: 0,
            replay_capture_execution_passes: 0,
            framebuffer_capture_execution_passes: 0,
            checkpoint_capture_execution_passes: 0,
            replay_capture_commands: 0,
            checkpoint_dependency_edges: 0,
            capture_execution_summary_available: false,
            max_capture_pass: None,
        };
        assert_eq!(
            format_gpu_timing_line(&record),
            "event=effect_gpu_timing frame_id=120 scope=31 total_ns=281400 capture_ns=41200 normalize_ns=0 blur_downsample_ns=78300 blur_upsample_ns=109700 fragment_ns=0 blend_ns=0 mask_ns=0 composite_ns=52200 postprocess_ns=0 timed_passes=6 dropped_passes=0 capture_pixels=640 normalize_pixels=0 blur_downsample_pixels=320 blur_upsample_pixels=160 fragment_pixels=0 blend_pixels=0 mask_pixels=0 composite_pixels=640 postprocess_pixels=0 query_pool_capacity=4096 query_pool_high_water=14 dropped_spans=0 disjoint_invalidated_spans=0 scene_capture_ns=0 surface_capture_ns=0 replay_capture_ns=0 framebuffer_capture_ns=0 checkpoint_capture_ns=0 scene_capture_passes=0 surface_capture_passes=0 replay_capture_passes=0 framebuffer_capture_passes=0 checkpoint_capture_passes=0 scene_capture_pixels=0 surface_capture_pixels=0 replay_capture_pixels=0 framebuffer_capture_pixels=0 checkpoint_capture_pixels=0 capture_execution_summary_available=0 capture_execution_pixels=0 scene_capture_execution_pixels=0 surface_capture_execution_pixels=0 replay_capture_execution_pixels=0 framebuffer_capture_execution_pixels=0 checkpoint_capture_execution_pixels=0 replay_capture_execution_passes=0 framebuffer_capture_execution_passes=0 checkpoint_capture_execution_passes=0 replay_capture_commands=0 checkpoint_dependency_edges=0 max_capture_pass_ns=0 max_capture_pass_id=0 max_capture_instance_id=0 max_capture_kind=none max_capture_mode=none max_capture_pixels=0 max_capture_checkpoint_count=0"
        );
    }

    #[test]
    fn timing_ownership_closes_on_error_path() {
        let mut state = TimingState::active_for_test(2);
        let scope = state.begin_scope(Some(120)).expect("scope slot");
        let pass = state
            .begin_pass(scope, pass_metadata(7, RenderPassKind::Composite, 64, None))
            .expect("pass slot");
        let free_before = state.free_slot_count();
        let simulated_error = true;
        if simulated_error {
            assert!(state.finish(pass));
        }
        assert_eq!(state.pending_span_count(), 1);
        assert_eq!(state.free_slot_count(), free_before);
    }

    #[test]
    fn teardown_deletes_each_query_object_once() {
        let mut profiler = EffectGpuProfiler::active_for_test(3);
        assert_eq!(profiler.allocated_query_count_for_test(), 6);
        assert_eq!(profiler.destroy_for_test(), 6);
        assert_eq!(profiler.destroy_for_test(), 0);
    }

    #[test]
    fn gpu_timing_gate_requires_exact_one() {
        assert!(!gpu_timing_requested(None));
        assert!(!gpu_timing_requested(Some(OsStr::new(""))));
        assert!(!gpu_timing_requested(Some(OsStr::new("0"))));
        assert!(!gpu_timing_requested(Some(OsStr::new("true"))));
        assert!(gpu_timing_requested(Some(OsStr::new("1"))));
    }

    #[test]
    fn query_counter_bits_uses_query_target_api() {
        let _lock = QUERYIV_TEST_LOCK.lock().expect("queryiv test lock");
        reset_queryiv_calls();
        let functions = QueryTargetFunctions::load_with(|name| {
            (name == "glGetQueryiv").then_some(fake_core_get_queryiv as *const c_void)
        });

        assert_eq!(functions.counter_bits(TimestampQueryPath::Core), Ok(64));
        assert_eq!(CORE_QUERYIV_CALLS.load(Ordering::Relaxed), 1);
        assert_eq!(
            CORE_QUERYIV_TARGET.load(Ordering::Relaxed) as u32,
            glow::TIMESTAMP
        );
        assert_eq!(
            CORE_QUERYIV_PNAME.load(Ordering::Relaxed) as u32,
            glow::QUERY_COUNTER_BITS
        );
        assert_eq!(EXT_QUERYIV_CALLS.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn missing_query_target_function_degrades_without_gl_work() {
        let _lock = QUERYIV_TEST_LOCK.lock().expect("queryiv test lock");
        reset_queryiv_calls();
        let functions = QueryTargetFunctions::load_with(|_| None);

        assert_eq!(
            select_timestamp_path(
                TimestampCapabilityInfo {
                    embedded: true,
                    major: 3,
                    minor: 0,
                    has_ext: true,
                    has_arb: false,
                },
                functions,
            ),
            Err("timestamp-query-counter-function-unavailable")
        );
        assert_eq!(CORE_QUERYIV_CALLS.load(Ordering::Relaxed), 0);
        assert_eq!(EXT_QUERYIV_CALLS.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn ext_capability_uses_ext_query_target_when_core_is_unavailable() {
        let _lock = QUERYIV_TEST_LOCK.lock().expect("queryiv test lock");
        reset_queryiv_calls();
        let functions = QueryTargetFunctions::load_with(|name| {
            (name == "glGetQueryivEXT").then_some(fake_ext_get_queryiv as *const c_void)
        });

        assert_eq!(
            select_timestamp_path(
                TimestampCapabilityInfo {
                    embedded: false,
                    major: 3,
                    minor: 3,
                    has_ext: true,
                    has_arb: false,
                },
                functions,
            ),
            Ok(TimestampQueryPath::Ext)
        );
        assert_eq!(CORE_QUERYIV_CALLS.load(Ordering::Relaxed), 0);
        assert_eq!(EXT_QUERYIV_CALLS.load(Ordering::Relaxed), 1);
        assert_eq!(
            EXT_QUERYIV_TARGET.load(Ordering::Relaxed) as u32,
            GL_TIMESTAMP_EXT
        );
        assert_eq!(
            EXT_QUERYIV_PNAME.load(Ordering::Relaxed) as u32,
            GL_QUERY_COUNTER_BITS_EXT
        );
    }

    #[test]
    fn desktop_core_capability_uses_core_query_target_function() {
        let _lock = QUERYIV_TEST_LOCK.lock().expect("queryiv test lock");
        reset_queryiv_calls();
        let functions = QueryTargetFunctions::load_with(|name| {
            (name == "glGetQueryiv").then_some(fake_core_get_queryiv as *const c_void)
        });

        assert_eq!(
            select_timestamp_path(
                TimestampCapabilityInfo {
                    embedded: false,
                    major: 3,
                    minor: 3,
                    has_ext: false,
                    has_arb: false,
                },
                functions,
            ),
            Ok(TimestampQueryPath::Core)
        );
        assert_eq!(CORE_QUERYIV_CALLS.load(Ordering::Relaxed), 1);
        assert_eq!(EXT_QUERYIV_CALLS.load(Ordering::Relaxed), 0);

        reset_queryiv_calls();
        assert_eq!(
            select_timestamp_path(
                TimestampCapabilityInfo {
                    embedded: false,
                    major: 3,
                    minor: 2,
                    has_ext: false,
                    has_arb: true,
                },
                functions,
            ),
            Ok(TimestampQueryPath::Core)
        );
        assert_eq!(CORE_QUERYIV_CALLS.load(Ordering::Relaxed), 1);
        assert_eq!(EXT_QUERYIV_CALLS.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn disjoint_before_new_graph_invalidates_pending_measurements() {
        let mut state = TimingState::active_for_test(2);
        let scope = state.begin_scope(Some(120)).expect("scope slot");
        assert!(state.finish(scope.total));

        assert!(observe_disjoint(&mut state, true));
        assert_eq!(state.pending_span_count(), 0);
        assert_eq!(state.free_slot_count(), 2);
        assert_eq!(state.aggregate_count_for_test(), 0);
        assert_eq!(state.disjoint_invalidated_span_count(), 1);
        assert_eq!(state.poll_front(true, Some((100, 140))), PollOutcome::Empty);
        assert!(observe_disjoint(&mut state, true));
        assert_eq!(state.free_slot_count(), 2);
        assert_eq!(state.disjoint_invalidated_span_count(), 1);
    }

    #[test]
    fn available_result_requires_disjoint_validation_before_read() {
        let mut state = TimingState::active_for_test(1);
        let scope = state.begin_scope(Some(120)).expect("scope slot");
        assert!(state.finish(scope.total));

        assert_eq!(
            collection_action(true, true, true),
            CollectionAction::InvalidateDisjoint
        );
        if collection_action(true, true, true) == CollectionAction::InvalidateDisjoint {
            observe_disjoint(&mut state, true);
        }
        assert_eq!(state.read_count_for_test(), 0);
        assert_eq!(state.pending_span_count(), 0);
    }
}
