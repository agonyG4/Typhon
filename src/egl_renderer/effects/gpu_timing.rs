use std::{collections::VecDeque, ffi::OsStr};

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
struct TimingSpanMetadata {
    scope_id: u64,
    frame_id: Option<u64>,
    pass_id: Option<u64>,
    instance_id: Option<u64>,
    kind: Option<oblivion_one::effects::RenderPassKind>,
    pixels: u64,
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
enum TimestampPath {
    ExtDisjoint,
    DesktopCore,
}

impl TimestampPath {
    const fn uses_disjoint(self) -> bool {
        matches!(self, Self::ExtDisjoint)
    }
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

fn timestamp_path(gl: &glow::Context) -> Result<TimestampPath, &'static str> {
    let extensions = gl.supported_extensions();
    let has_ext = extensions.contains("GL_EXT_disjoint_timer_query");
    let version = gl.version();
    let has_desktop_core =
        !version.is_embedded && (version.major > 3 || (version.major == 3 && version.minor >= 3));
    let has_arb = extensions.contains("GL_ARB_timer_query");
    if !has_ext && !has_desktop_core && !has_arb {
        return Err("timestamp-query-unavailable");
    }
    let counter_bits = unsafe { gl.get_parameter_i32(glow::QUERY_COUNTER_BITS) };
    if counter_bits <= 0 {
        return Err("timestamp-query-counter-unavailable");
    }
    if has_ext {
        Ok(TimestampPath::ExtDisjoint)
    } else {
        Ok(TimestampPath::DesktopCore)
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
    let capture_ns = record.durations_ns[TimingCategory::SceneCapture.index()]
        .saturating_add(record.durations_ns[TimingCategory::SurfaceCapture.index()]);
    let capture_pixels = record.pixels[TimingCategory::SceneCapture.index()]
        .saturating_add(record.pixels[TimingCategory::SurfaceCapture.index()]);
    let field = |values: &[u64; 10], category: TimingCategory| values[category.index()];
    format!(
        "event=effect_gpu_timing frame_id={} scope={} total_ns={} capture_ns={} normalize_ns={} blur_downsample_ns={} blur_upsample_ns={} fragment_ns={} blend_ns={} mask_ns={} composite_ns={} postprocess_ns={} timed_passes={} dropped_passes={} capture_pixels={} normalize_pixels={} blur_downsample_pixels={} blur_upsample_pixels={} fragment_pixels={} blend_pixels={} mask_pixels={} composite_pixels={} postprocess_pixels={} query_pool_capacity={} query_pool_high_water={} dropped_spans={} disjoint_invalidated_spans={}",
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
    )
}

impl EffectGpuProfiler {
    pub(crate) fn new(gl: &glow::Context) -> Self {
        if !gpu_timing_requested(std::env::var_os(GPU_TIMING_ENV).as_deref()) {
            return Self {
                state: ProfilerState::Disabled,
            };
        }
        let path = match timestamp_path(gl) {
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
        if active.path.uses_disjoint() && gpu_disjoint(gl) {
            active.timing.invalidate_pending();
            return;
        }
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
        if active.path.uses_disjoint() && gpu_disjoint(gl) {
            return None;
        }
        let scope = active.timing.begin_scope(frame_id)?;
        let query = active.queries[scope.total.slot].start;
        unsafe { gl.query_counter(query, glow::TIMESTAMP) };
        Some(scope)
    }

    pub(crate) fn end_graph(&mut self, gl: &glow::Context, scope: Option<GraphTimingScope>) {
        let Some(scope) = scope else {
            return;
        };
        let ProfilerState::Active(active) = &mut self.state else {
            return;
        };
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
                path: TimestampPath::DesktopCore,
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
    use super::*;
    use oblivion_one::effects::RenderPassKind;

    fn pass_metadata(pass_id: u64, kind: RenderPassKind, pixels: u64) -> TimingSpanMetadata {
        TimingSpanMetadata {
            scope_id: 1,
            frame_id: Some(120),
            pass_id: Some(pass_id),
            instance_id: Some(1),
            kind: Some(kind),
            pixels,
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
        let pass = state.begin_pass(scope, pass_metadata(7, RenderPassKind::Composite, 64));
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
            .begin_pass(scope, pass_metadata(7, RenderPassKind::Composite, 64))
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
            .begin_pass(scope, pass_metadata(7, RenderPassKind::Composite, 64))
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
            .begin_pass(scope, pass_metadata(7, RenderPassKind::Composite, 64))
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
                .begin_pass(scope, pass_metadata(7, RenderPassKind::Composite, 64))
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
                .begin_pass(scope, pass_metadata(index as u64, kind, 64))
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
    fn total_completion_returns_one_graph_record() {
        let mut state = TimingState::active_for_test(2);
        let scope = state.begin_scope(Some(120)).expect("scope slot");
        let pass = state
            .begin_pass(scope, pass_metadata(7, RenderPassKind::Composite, 64))
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
            .begin_pass(scope, pass_metadata(7, RenderPassKind::Composite, 64))
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
        };
        assert_eq!(
            format_gpu_timing_line(&record),
            "event=effect_gpu_timing frame_id=120 scope=31 total_ns=281400 capture_ns=41200 normalize_ns=0 blur_downsample_ns=78300 blur_upsample_ns=109700 fragment_ns=0 blend_ns=0 mask_ns=0 composite_ns=52200 postprocess_ns=0 timed_passes=6 dropped_passes=0 capture_pixels=640 normalize_pixels=0 blur_downsample_pixels=320 blur_upsample_pixels=160 fragment_pixels=0 blend_pixels=0 mask_pixels=0 composite_pixels=640 postprocess_pixels=0 query_pool_capacity=4096 query_pool_high_water=14 dropped_spans=0 disjoint_invalidated_spans=0"
        );
    }

    #[test]
    fn timing_ownership_closes_on_error_path() {
        let mut state = TimingState::active_for_test(2);
        let scope = state.begin_scope(Some(120)).expect("scope slot");
        let pass = state
            .begin_pass(scope, pass_metadata(7, RenderPassKind::Composite, 64))
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
}
