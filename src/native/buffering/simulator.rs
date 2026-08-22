use super::{
    O1CreditDemandController, PipelineServiceEstimate, PresentationOpportunity,
    PresentationOpportunityId,
};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimulatedO1Config {
    pub refresh_interval_ns: u64,
    pub render_service_ns: u64,
    pub dispatch_service_ns: u64,
    pub apply_guard_ns: u64,
    pub apply_delay_ns: u64,
    pub frames: u32,
    pub worker_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SimulatedO1EventKind {
    VisualWorkArrived,
    FrameCallbackProgress,
    RenderStarted,
    RenderCompleted,
    FenceReady,
    WorkerWake,
    SubmitStarted,
    SubmitReturned,
    PageFlip,
    OutputGenerationChanged,
    RenderFailed,
    CommitTimingConstraintChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SimulatedO1Event {
    pub at_ns: i64,
    pub order: u64,
    pub frame: u32,
    pub generation: u64,
    pub kind: SimulatedO1EventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SimulatedFrame {
    generation: u64,
    predecessor_ns: i64,
    target_ns: i64,
    deadline_ns: i64,
    overlap_required_ns: u64,
    service_ns: u64,
    render_started_ns: Option<i64>,
    render_ready_ns: Option<i64>,
    submit_returned_ns: Option<i64>,
    admitted: bool,
    render_ahead: bool,
    submitted: bool,
    target_hit: bool,
    render_requested: bool,
    callback_scheduled: bool,
    transport_scheduled: bool,
    pageflip_scheduled: bool,
    actual_pageflip_ns: Option<i64>,
    invalidated: bool,
    terminalized: bool,
    used_extra_credit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimulatedO1State {
    pub now_ns: i64,
    pub output_generation: u64,
    pub desired_credit: u8,
    pub owned_future_primary_depth: u8,
    pub rendering: Option<u32>,
    pub worker_queued: Option<u32>,
    pub kernel_submitted: Option<u32>,
    pub prepared: Option<u32>,
    pub visual_work_pending: bool,
    pub armed_target: Option<PresentationOpportunity>,
    pub worker_enabled: bool,
}

impl SimulatedO1State {
    fn new(worker_enabled: bool) -> Self {
        Self {
            now_ns: 0,
            output_generation: 1,
            desired_credit: 1,
            owned_future_primary_depth: 0,
            rendering: None,
            worker_queued: None,
            kernel_submitted: None,
            prepared: None,
            visual_work_pending: false,
            armed_target: None,
            worker_enabled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SimulatedO1Result {
    pub target_hits: u32,
    pub render_readiness_misses: u32,
    pub dispatch_misses: u32,
    pub apply_guard_misses: u32,
    pub max_future_primary_depth: u8,
    pub credit_one_observations: u32,
    pub credit_two_observations: u32,
    pub target_mutations: u32,
    pub intentional_queue_latency_ns: u64,
    pub credit_grants: u64,
    pub credit_revokes: u64,
    pub desired_credit_one_observations: u64,
    pub desired_credit_two_observations: u64,
    pub owned_depth_one_observations: u64,
    pub owned_depth_two_observations: u64,
    pub drain_events: u64,
    pub refill_suppressed_while_draining: u64,
    pub credit2_useful_hits: u64,
    pub credit2_unnecessary_hits: u64,
    pub credit2_ineffective_misses: u64,
    pub credit2_granted_not_consumed: u64,
    pub kms_dispatch_misses: u32,
    pub kms_apply_misses: u32,
    pub submitted_frames: u32,
    pub terminalized_submitted_frames: u32,
    pub submitted_frame_liveness_violations: u32,
    pub later_refresh_pageflips: u32,
    pub max_rendering_owners: u8,
    pub max_worker_queued_owners: u8,
    pub max_kernel_submitted_owners: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct SimulatedO1EventModel {
    config: SimulatedO1Config,
}

impl SimulatedO1EventModel {
    pub const fn new(config: SimulatedO1Config) -> Self {
        Self { config }
    }

    pub fn run(self, render_services_ns: &[u64]) -> SimulatedO1Result {
        self.run_with_events(render_services_ns, &[])
    }

    pub fn run_with_events(
        self,
        render_services_ns: &[u64],
        extra_events: &[SimulatedO1Event],
    ) -> SimulatedO1Result {
        let refresh = self.config.refresh_interval_ns.max(1) as i64;
        let mut queue = BinaryHeap::new();
        let mut frames = vec![None; self.config.frames as usize];
        let mut order = 0;
        for frame in 0..self.config.frames {
            let service = render_services_ns
                .get(frame as usize)
                .copied()
                .unwrap_or(self.config.render_service_ns);
            let predecessor_ns = refresh.saturating_add(refresh.saturating_mul(i64::from(frame)));
            let target_ns = predecessor_ns.saturating_add(refresh);
            let total = service
                .saturating_add(self.config.dispatch_service_ns)
                .saturating_add(self.config.apply_guard_ns);
            let ahead_start = target_ns.saturating_sub(total as i64);
            let arrival = predecessor_ns.min(ahead_start);
            let estimate = PipelineServiceEstimate::new(
                0,
                service,
                self.config.dispatch_service_ns,
                self.config.apply_guard_ns,
            );
            let overlap = estimate.overlap_required_ns(
                super::super::presentation_deadline::MonotonicTimestampNs::new(
                    predecessor_ns.max(0) as u64,
                ),
                super::super::presentation_deadline::MonotonicTimestampNs::new(
                    target_ns.max(0) as u64
                ),
            );
            frames[frame as usize] = Some(SimulatedFrame {
                generation: 1,
                predecessor_ns,
                target_ns,
                deadline_ns: target_ns.saturating_sub(self.config.apply_guard_ns as i64),
                overlap_required_ns: overlap,
                service_ns: service,
                render_started_ns: None,
                render_ready_ns: None,
                submit_returned_ns: None,
                admitted: false,
                render_ahead: false,
                submitted: false,
                target_hit: false,
                render_requested: false,
                callback_scheduled: false,
                transport_scheduled: false,
                pageflip_scheduled: false,
                actual_pageflip_ns: None,
                invalidated: false,
                terminalized: false,
                used_extra_credit: false,
            });
            push_event(
                &mut queue,
                &mut order,
                SimulatedO1Event {
                    at_ns: arrival,
                    order: 0,
                    frame,
                    generation: 1,
                    kind: SimulatedO1EventKind::VisualWorkArrived,
                },
            );
        }
        for event in extra_events.iter().copied() {
            push_event(&mut queue, &mut order, event);
        }

        let mut state = SimulatedO1State::new(self.config.worker_enabled);
        let mut demand = O1CreditDemandController::new();
        let mut result = SimulatedO1Result::default();
        while let Some(Reverse(event)) = queue.pop() {
            state.now_ns = state.now_ns.max(event.at_ns);
            if frames
                .get(event.frame as usize)
                .and_then(Option::as_ref)
                .is_none_or(|frame| frame.invalidated)
            {
                continue;
            }
            if event.kind != SimulatedO1EventKind::OutputGenerationChanged
                && event.generation != state.output_generation
            {
                continue;
            }

            match event.kind {
                SimulatedO1EventKind::VisualWorkArrived => {
                    state.visual_work_pending = true;
                    let opportunity =
                        PresentationOpportunityId::new(event.generation, u64::from(event.frame));
                    let overlap_required_ns = frames[event.frame as usize]
                        .as_ref()
                        .expect("visual event frame exists")
                        .overlap_required_ns;
                    let before = demand.effective();
                    demand.observe_opportunity(opportunity, overlap_required_ns);
                    record_desired_observation(&mut result, demand.effective());
                    if before == 2
                        && demand.effective() == 1
                        && state.owned_future_primary_depth > 0
                    {
                        result.refill_suppressed_while_draining =
                            result.refill_suppressed_while_draining.saturating_add(1);
                    }
                    result.credit_one_observations = result
                        .credit_one_observations
                        .saturating_add(u32::from(demand.effective() == 1));
                    result.credit_two_observations = result
                        .credit_two_observations
                        .saturating_add(u32::from(demand.effective() == 2));
                    let frame = frames[event.frame as usize]
                        .as_mut()
                        .expect("visual event frame exists");
                    frame.render_requested = true;
                    frame.admitted = demand.effective() > state.owned_future_primary_depth;
                    if before == 1 && demand.effective() == 2 {
                        let next_target = PresentationOpportunity::fixed_vsync(
                            opportunity,
                            super::super::presentation_deadline::MonotonicTimestampNs::new(
                                frame.target_ns.max(0) as u64,
                            ),
                            std::time::Duration::from_nanos(refresh as u64),
                        );
                        if state.armed_target.is_some_and(|armed| armed != next_target) {
                            result.target_mutations = result.target_mutations.saturating_add(1);
                        } else {
                            state.armed_target = Some(next_target);
                        }
                    }
                    queue_callback_progress(
                        &mut queue,
                        &mut order,
                        &mut frames,
                        event.frame,
                        event.generation,
                        event.at_ns,
                    );
                }
                SimulatedO1EventKind::FrameCallbackProgress => {
                    if let Some(frame) = frames[event.frame as usize].as_mut() {
                        frame.callback_scheduled = false;
                    }
                    push_event(
                        &mut queue,
                        &mut order,
                        SimulatedO1Event {
                            at_ns: event.at_ns,
                            order: 0,
                            frame: event.frame,
                            generation: event.generation,
                            kind: SimulatedO1EventKind::RenderStarted,
                        },
                    );
                }
                SimulatedO1EventKind::RenderStarted => {
                    let can_start = state.rendering.is_none()
                        && state.prepared.is_none()
                        && state.owned_future_primary_depth < 2
                        && demand.effective() > state.owned_future_primary_depth;
                    if !can_start {
                        if let Some(frame) = frames[event.frame as usize].as_mut() {
                            frame.render_requested = true;
                            frame.admitted = false;
                        }
                        continue;
                    }
                    let frame = frames[event.frame as usize]
                        .as_mut()
                        .expect("render event frame exists");
                    frame.render_requested = false;
                    frame.admitted = true;
                    frame.render_ahead = demand.effective() == 2
                        && state.owned_future_primary_depth >= 1;
                    frame.used_extra_credit = frame.render_ahead;
                    frame.render_started_ns = Some(event.at_ns);
                    state.rendering = Some(event.frame);
                    state.visual_work_pending = false;
                    state.owned_future_primary_depth =
                        state.owned_future_primary_depth.saturating_add(1);
                    result.max_future_primary_depth = result
                        .max_future_primary_depth
                        .max(state.owned_future_primary_depth);
                    observe_depth(&mut result, state.owned_future_primary_depth);
                    push_event(
                        &mut queue,
                        &mut order,
                        SimulatedO1Event {
                            at_ns: event.at_ns.saturating_add(frame.service_ns as i64),
                            order: 0,
                            frame: event.frame,
                            generation: event.generation,
                            kind: SimulatedO1EventKind::RenderCompleted,
                        },
                    );
                }
                SimulatedO1EventKind::RenderCompleted => {
                    if state.rendering != Some(event.frame) {
                        continue;
                    }
                    state.rendering = None;
                    state.prepared = Some(event.frame);
                    let frame = frames[event.frame as usize]
                        .as_mut()
                        .expect("render event frame exists");
                    frame.render_ready_ns = Some(event.at_ns);
                    push_event(
                        &mut queue,
                        &mut order,
                        SimulatedO1Event {
                            at_ns: event.at_ns,
                            order: 0,
                            frame: event.frame,
                            generation: event.generation,
                            kind: SimulatedO1EventKind::FenceReady,
                        },
                    );
                    if event.at_ns > frame.deadline_ns {
                        result.render_readiness_misses =
                            result.render_readiness_misses.saturating_add(1);
                        demand.observe_render_readiness_miss();
                    }
                }
                SimulatedO1EventKind::FenceReady => {
                    if state.prepared == Some(event.frame) {
                        queue_transport_progress(
                            &mut queue,
                            &mut order,
                            &mut frames,
                            &state,
                            event.frame,
                            event.generation,
                            event.at_ns,
                            self.config.worker_enabled,
                            self.config.dispatch_service_ns,
                        );
                    }
                }
                SimulatedO1EventKind::WorkerWake => {
                    if let Some(frame) = frames[event.frame as usize].as_mut() {
                        frame.transport_scheduled = false;
                    }
                    if state.prepared == Some(event.frame) && state.worker_queued.is_none() {
                        state.prepared = None;
                        state.worker_queued = Some(event.frame);
                        queue_transport_progress(
                            &mut queue,
                            &mut order,
                            &mut frames,
                            &state,
                            event.frame,
                            event.generation,
                            event.at_ns,
                            self.config.worker_enabled,
                            self.config.dispatch_service_ns,
                        );
                    }
                }
                SimulatedO1EventKind::SubmitStarted => {
                    if let Some(frame) = frames[event.frame as usize].as_mut() {
                        frame.transport_scheduled = false;
                    }
                    let owns_transport = if self.config.worker_enabled {
                        state.worker_queued == Some(event.frame)
                    } else {
                        state.prepared == Some(event.frame)
                    };
                    if owns_transport && state.kernel_submitted.is_none() {
                        if let Some(frame) = frames[event.frame as usize].as_mut() {
                            frame.transport_scheduled = true;
                        }
                        push_event(
                            &mut queue,
                            &mut order,
                            SimulatedO1Event {
                                at_ns: event.at_ns.saturating_add(if self.config.worker_enabled {
                                    0
                                } else {
                                    self.config.dispatch_service_ns as i64
                                }),
                                order: 0,
                                frame: event.frame,
                                generation: event.generation,
                                kind: SimulatedO1EventKind::SubmitReturned,
                            },
                        );
                    }
                }
                SimulatedO1EventKind::SubmitReturned => {
                    if let Some(frame) = frames[event.frame as usize].as_mut() {
                        frame.transport_scheduled = false;
                    }
                    let owns_transport = if self.config.worker_enabled {
                        state.worker_queued == Some(event.frame)
                    } else {
                        state.prepared == Some(event.frame)
                    };
                    if !owns_transport || state.kernel_submitted.is_some() {
                        continue;
                    }
                    let frame = frames[event.frame as usize]
                        .as_mut()
                        .expect("submit event frame exists");
                    frame.submitted = true;
                    frame.submit_returned_ns = Some(event.at_ns);
                    frame.pageflip_scheduled = true;
                    if state.worker_queued == Some(event.frame) {
                        state.worker_queued = None;
                    }
                    if state.prepared == Some(event.frame) {
                        state.prepared = None;
                    }
                    state.worker_queued = state.worker_queued.filter(|id| *id != event.frame);
                    state.kernel_submitted = Some(event.frame);
                    result.submitted_frames = result.submitted_frames.saturating_add(1);
                    if event.at_ns > frame.deadline_ns {
                        result.dispatch_misses = result.dispatch_misses.saturating_add(1);
                        result.kms_dispatch_misses = result.kms_dispatch_misses.saturating_add(1);
                    }
                    let earliest_apply = event
                        .at_ns
                        .saturating_add(self.config.apply_delay_ns as i64);
                    let pageflip_at = first_refresh_at_or_after(earliest_apply, refresh);
                    push_event(
                        &mut queue,
                        &mut order,
                        SimulatedO1Event {
                            at_ns: pageflip_at,
                            order: 0,
                            frame: event.frame,
                            generation: event.generation,
                            kind: SimulatedO1EventKind::PageFlip,
                        },
                    );
                    schedule_waiting_render(
                        &mut queue,
                        &mut order,
                        &mut frames,
                        &state,
                        event.at_ns,
                    );
                }
                SimulatedO1EventKind::PageFlip => {
                    if state.kernel_submitted != Some(event.frame)
                        || !frames[event.frame as usize]
                            .as_ref()
                            .is_some_and(|frame| frame.submitted && frame.pageflip_scheduled)
                    {
                        continue;
                    }
                    let frame = frames[event.frame as usize]
                        .as_mut()
                        .expect("pageflip event frame exists");
                    frame.pageflip_scheduled = false;
                    frame.actual_pageflip_ns = Some(event.at_ns);
                    state.kernel_submitted = None;
                    if state
                        .armed_target
                        .is_some_and(|target| target.id().sequence() == u64::from(event.frame))
                    {
                        state.armed_target = None;
                    }
                    let previous_depth = state.owned_future_primary_depth;
                    state.owned_future_primary_depth =
                        state.owned_future_primary_depth.saturating_sub(1);
                    if previous_depth == 2 && state.owned_future_primary_depth == 1 {
                        result.drain_events = result.drain_events.saturating_add(1);
                    }
                    observe_depth(&mut result, state.owned_future_primary_depth);
                    if event.at_ns > frame.target_ns {
                        result.later_refresh_pageflips =
                            result.later_refresh_pageflips.saturating_add(1);
                    }
                    let hit = frame.render_ready_ns.is_some_and(|ready| {
                        ready <= frame.deadline_ns
                            && frame
                                .submit_returned_ns
                                .is_some_and(|submitted| submitted <= frame.deadline_ns)
                            && self.config.apply_delay_ns <= self.config.apply_guard_ns
                    });
                    frame.target_hit = hit;
                    if hit {
                        result.target_hits = result.target_hits.saturating_add(1);
                    } else if self.config.apply_delay_ns > self.config.apply_guard_ns {
                        result.apply_guard_misses = result.apply_guard_misses.saturating_add(1);
                        result.kms_apply_misses = result.kms_apply_misses.saturating_add(1);
                    }
                    if frame.used_extra_credit {
                        if hit && frame.overlap_required_ns > 0 {
                            result.credit2_useful_hits =
                                result.credit2_useful_hits.saturating_add(1);
                        } else if hit {
                            result.credit2_unnecessary_hits =
                                result.credit2_unnecessary_hits.saturating_add(1);
                        } else {
                            result.credit2_ineffective_misses =
                                result.credit2_ineffective_misses.saturating_add(1);
                        }
                    }
                    if !frame.terminalized {
                        frame.terminalized = true;
                        result.terminalized_submitted_frames = result
                            .terminalized_submitted_frames
                            .saturating_add(1);
                    }
                    if let Some(next_frame) = state.worker_queued.or(state.prepared) {
                        queue_transport_progress(
                            &mut queue,
                            &mut order,
                            &mut frames,
                            &state,
                            next_frame,
                            state.output_generation,
                            event.at_ns,
                            self.config.worker_enabled,
                            self.config.dispatch_service_ns,
                        );
                    }
                    schedule_waiting_render(
                        &mut queue,
                        &mut order,
                        &mut frames,
                        &state,
                        event.at_ns,
                    );
                }
                SimulatedO1EventKind::OutputGenerationChanged => {
                    let old_generation = state.output_generation;
                    state.output_generation = state.output_generation.saturating_add(1);
                    state.armed_target = None;
                    state.rendering = None;
                    state.prepared = None;
                    state.worker_queued = None;
                    state.kernel_submitted = None;
                    state.owned_future_primary_depth = 0;
                    for frame in frames.iter_mut().flatten() {
                        if frame.generation == old_generation && !frame.invalidated {
                            frame.invalidated = true;
                            frame.render_requested = false;
                            if frame.submitted && !frame.terminalized {
                                frame.terminalized = true;
                                result.terminalized_submitted_frames = result
                                    .terminalized_submitted_frames
                                    .saturating_add(1);
                            }
                        }
                    }
                }
                SimulatedO1EventKind::RenderFailed => {
                    let frame = frames[event.frame as usize]
                        .as_mut()
                        .expect("render failure frame exists");
                    if state.rendering == Some(event.frame) {
                        state.rendering = None;
                        state.owned_future_primary_depth =
                            state.owned_future_primary_depth.saturating_sub(1);
                    }
                    if state.prepared == Some(event.frame) {
                        state.prepared = None;
                    }
                    if state.worker_queued == Some(event.frame) {
                        state.worker_queued = None;
                    }
                    if state.kernel_submitted == Some(event.frame) {
                        state.kernel_submitted = None;
                    }
                    frame.admitted = false;
                    frame.invalidated = true;
                    frame.render_requested = false;
                    if frame.submitted && !frame.terminalized {
                        frame.terminalized = true;
                        result.terminalized_submitted_frames = result
                            .terminalized_submitted_frames
                            .saturating_add(1);
                    }
                }
                SimulatedO1EventKind::CommitTimingConstraintChanged => {}
            }
            state.owned_future_primary_depth = physical_future_depth(&state);
            state.visual_work_pending = frames
                .iter()
                .flatten()
                .any(|frame| frame.render_requested && !frame.invalidated);
            record_lane_bounds(&state, &mut result);
            state.desired_credit = demand.effective();
        }

        result.credit_grants = demand.grants();
        result.credit_revokes = demand.revokes();
        result.credit2_granted_not_consumed = result
            .credit2_granted_not_consumed
            .saturating_add(result.credit_grants.saturating_sub(
                frames
                    .iter()
                    .flatten()
                    .filter(|frame| frame.used_extra_credit)
                    .count() as u64,
            ));
        result.submitted_frame_liveness_violations = result
            .submitted_frames
            .saturating_sub(result.terminalized_submitted_frames);
        result
    }
}

pub fn simulate_o1(config: SimulatedO1Config) -> SimulatedO1Result {
    let services = vec![config.render_service_ns; config.frames as usize];
    simulate_o1_with_render_services(config, &services)
}

pub fn simulate_o1_with_render_services(
    config: SimulatedO1Config,
    render_services_ns: &[u64],
) -> SimulatedO1Result {
    SimulatedO1EventModel::new(config).run(render_services_ns)
}

fn queue_callback_progress(
    queue: &mut BinaryHeap<Reverse<SimulatedO1Event>>,
    order: &mut u64,
    frames: &mut [Option<SimulatedFrame>],
    frame_id: u32,
    generation: u64,
    at_ns: i64,
) {
    let Some(frame) = frames.get_mut(frame_id as usize).and_then(Option::as_mut) else {
        return;
    };
    if frame.invalidated || frame.callback_scheduled {
        return;
    }
    frame.callback_scheduled = true;
    push_event(
        queue,
        order,
        SimulatedO1Event {
            at_ns,
            order: 0,
            frame: frame_id,
            generation,
            kind: SimulatedO1EventKind::FrameCallbackProgress,
        },
    );
}

fn queue_transport_progress(
    queue: &mut BinaryHeap<Reverse<SimulatedO1Event>>,
    order: &mut u64,
    frames: &mut [Option<SimulatedFrame>],
    state: &SimulatedO1State,
    frame_id: u32,
    generation: u64,
    at_ns: i64,
    worker_enabled: bool,
    dispatch_service_ns: u64,
) {
    let Some(frame) = frames.get_mut(frame_id as usize).and_then(Option::as_mut) else {
        return;
    };
    if frame.invalidated || frame.transport_scheduled {
        return;
    }
    let (kind, scheduled_at) = if worker_enabled {
        if state.worker_queued == Some(frame_id) {
            (SimulatedO1EventKind::SubmitStarted, at_ns)
        } else if state.prepared == Some(frame_id) && state.worker_queued.is_none() {
            (
                SimulatedO1EventKind::WorkerWake,
                at_ns.saturating_add(dispatch_service_ns as i64),
            )
        } else {
            return;
        }
    } else if state.prepared == Some(frame_id) {
        (SimulatedO1EventKind::SubmitStarted, at_ns)
    } else {
        return;
    };
    frame.transport_scheduled = true;
    push_event(
        queue,
        order,
        SimulatedO1Event {
            at_ns: scheduled_at,
            order: 0,
            frame: frame_id,
            generation,
            kind,
        },
    );
}

fn schedule_waiting_render(
    queue: &mut BinaryHeap<Reverse<SimulatedO1Event>>,
    order: &mut u64,
    frames: &mut [Option<SimulatedFrame>],
    state: &SimulatedO1State,
    at_ns: i64,
) {
    if state.rendering.is_some()
        || state.prepared.is_some()
        || state.owned_future_primary_depth >= 2
    {
        return;
    }
    let Some(frame_id) = frames
        .iter()
        .flatten()
        .find(|frame| {
            frame.generation == state.output_generation
                && frame.render_requested
                && !frame.invalidated
                && !frame.submitted
        })
        .and_then(|frame| frames.iter().position(|candidate| candidate.as_ref() == Some(frame)))
        .and_then(|index| u32::try_from(index).ok())
    else {
        return;
    };
    queue_callback_progress(
        queue,
        order,
        frames,
        frame_id,
        state.output_generation,
        at_ns,
    );
}

fn first_refresh_at_or_after(at_ns: i64, refresh_ns: i64) -> i64 {
    let refresh_ns = refresh_ns.max(1);
    if at_ns <= refresh_ns {
        return refresh_ns;
    }
    let intervals = (at_ns.saturating_add(refresh_ns - 1) / refresh_ns).max(1);
    intervals.saturating_mul(refresh_ns)
}

fn physical_future_depth(state: &SimulatedO1State) -> u8 {
    u8::from(state.rendering.is_some())
        .saturating_add(u8::from(state.prepared.is_some()))
        .saturating_add(u8::from(state.worker_queued.is_some()))
        .saturating_add(u8::from(state.kernel_submitted.is_some()))
}

fn record_lane_bounds(state: &SimulatedO1State, result: &mut SimulatedO1Result) {
    result.max_rendering_owners = result
        .max_rendering_owners
        .max(u8::from(state.rendering.is_some()));
    result.max_worker_queued_owners = result
        .max_worker_queued_owners
        .max(u8::from(state.worker_queued.is_some()));
    result.max_kernel_submitted_owners = result
        .max_kernel_submitted_owners
        .max(u8::from(state.kernel_submitted.is_some()));
}

fn push_event(
    queue: &mut BinaryHeap<Reverse<SimulatedO1Event>>,
    order: &mut u64,
    mut event: SimulatedO1Event,
) {
    event.order = event_priority(event.kind)
        .saturating_mul(1_000_000)
        .saturating_add(*order);
    *order = order.saturating_add(1);
    queue.push(Reverse(event));
}

const fn event_priority(kind: SimulatedO1EventKind) -> u64 {
    match kind {
        SimulatedO1EventKind::OutputGenerationChanged => 0,
        SimulatedO1EventKind::VisualWorkArrived => 1,
        SimulatedO1EventKind::FrameCallbackProgress => 2,
        SimulatedO1EventKind::RenderStarted => 3,
        SimulatedO1EventKind::RenderCompleted => 4,
        SimulatedO1EventKind::FenceReady => 5,
        SimulatedO1EventKind::WorkerWake => 6,
        SimulatedO1EventKind::SubmitStarted => 7,
        SimulatedO1EventKind::SubmitReturned => 8,
        SimulatedO1EventKind::PageFlip => 9,
        SimulatedO1EventKind::RenderFailed => 10,
        SimulatedO1EventKind::CommitTimingConstraintChanged => 11,
    }
}

fn record_desired_observation(result: &mut SimulatedO1Result, desired: u8) {
    if desired == 1 {
        result.desired_credit_one_observations =
            result.desired_credit_one_observations.saturating_add(1);
    } else {
        result.desired_credit_two_observations =
            result.desired_credit_two_observations.saturating_add(1);
    }
}

fn observe_depth(result: &mut SimulatedO1Result, depth: u8) {
    match depth {
        1 => {
            result.owned_depth_one_observations =
                result.owned_depth_one_observations.saturating_add(1)
        }
        2 => {
            result.owned_depth_two_observations =
                result.owned_depth_two_observations.saturating_add(1)
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SimulatedO1Config, SimulatedO1Event, SimulatedO1EventKind, SimulatedO1EventModel,
        simulate_o1, simulate_o1_with_render_services,
    };

    fn config(render_service_ns: u64) -> SimulatedO1Config {
        SimulatedO1Config {
            refresh_interval_ns: 6_060_606,
            render_service_ns,
            dispatch_service_ns: 300_000,
            apply_guard_ns: 500_000,
            apply_delay_ns: 500_000,
            frames: 120,
            worker_enabled: true,
        }
    }

    #[test]
    fn low_load_stays_at_one_credit_across_refresh_rates() {
        for refresh in [60, 120, 165, 240] {
            let mut test_config = config(500_000);
            test_config.refresh_interval_ns = 1_000_000_000 / refresh;
            test_config.frames = 48;
            let result = simulate_o1(test_config);

            assert_eq!(result.credit_two_observations, 0);
            assert_eq!(result.max_future_primary_depth, 1);
            assert_eq!(result.target_hits, test_config.frames);
        }
    }

    #[test]
    fn sustained_render_pressure_consumes_useful_extra_credit() {
        let result = simulate_o1(config(5_500_000));

        assert_eq!(result.render_readiness_misses, 0);
        assert_eq!(result.dispatch_misses, 0);
        assert_eq!(result.apply_guard_misses, 0);
        assert_eq!(result.max_future_primary_depth, 2);
        assert!(result.credit2_useful_hits > 0);
        assert_eq!(result.target_mutations, 0);
    }

    #[test]
    fn transient_pressure_revokes_and_drains_depth_two() {
        let test_config = config(3_000_000);
        let mut services = vec![3_000_000; test_config.frames as usize];
        services[0] = 12_000_000;
        let result = simulate_o1_with_render_services(test_config, &services);

        assert!(result.credit_two_observations > 0);
        assert!(result.credit_one_observations > 0);
        assert!(result.credit_revokes > 0);
        assert!(result.drain_events > 0);
        assert!(result.refill_suppressed_while_draining > 0);
        assert!(result.owned_depth_two_observations > 0);
    }

    #[test]
    fn kms_only_misses_do_not_grant_render_credit() {
        let mut test_config = config(500_000);
        test_config.apply_delay_ns = 1_500_000;
        let result = simulate_o1(test_config);

        assert_eq!(result.credit_grants, 0);
        assert_eq!(result.kms_dispatch_misses, 0);
        assert_eq!(result.kms_apply_misses, test_config.frames);
    }

    #[test]
    fn worker_transport_does_not_change_low_load_opportunity_results() {
        let worker = simulate_o1(config(500_000));
        let mut synchronous_config = config(500_000);
        synchronous_config.worker_enabled = false;
        let synchronous = simulate_o1(synchronous_config);

        assert_eq!(worker.target_hits, synchronous.target_hits);
        assert_eq!(worker.render_readiness_misses, synchronous.render_readiness_misses);
        assert_eq!(worker.dispatch_misses, synchronous.dispatch_misses);
        assert_eq!(worker.later_refresh_pageflips, synchronous.later_refresh_pageflips);
        assert_eq!(worker.max_future_primary_depth, synchronous.max_future_primary_depth);
        assert_eq!(worker.submitted_frames, synchronous.submitted_frames);
        assert_eq!(
            worker.terminalized_submitted_frames,
            synchronous.terminalized_submitted_frames
        );
        assert_eq!(
            worker.submitted_frame_liveness_violations,
            synchronous.submitted_frame_liveness_violations
        );
        assert_eq!(worker.max_kernel_submitted_owners, synchronous.max_kernel_submitted_owners);
    }

    #[test]
    fn credit_two_usefulness_is_classified_from_admission_overlap() {
        let mut transient = config(500_000);
        transient.refresh_interval_ns = 6_060_606;
        transient.frames = 24;
        let mut services = vec![500_000; transient.frames as usize];
        services[0] = 6_000_000;
        let unnecessary = simulate_o1_with_render_services(transient, &services);
        assert!(unnecessary.credit2_unnecessary_hits > 0);

        let mut ineffective_config = config(7_500_000);
        ineffective_config.apply_delay_ns = 1_500_000;
        let ineffective = simulate_o1(ineffective_config);
        assert!(ineffective.credit2_ineffective_misses > 0);

        let mut unconsumed_config = config(7_500_000);
        unconsumed_config.frames = 1;
        let unconsumed = simulate_o1(unconsumed_config);
        assert_eq!(unconsumed.credit_grants, 1);
        assert_eq!(unconsumed.credit2_granted_not_consumed, 1);
    }

    #[test]
    fn bounded_parameter_sweep_preserves_o1_invariants() {
        for hz in [60_u64, 120, 144, 165, 240] {
            let refresh = 1_000_000_000 / hz;
            for service in [
                200_000,
                refresh / 3,
                refresh.saturating_sub(750_000),
                refresh + 750_000,
            ] {
                for dispatch in [100_000, refresh / 16, refresh / 4] {
                    let mut test_config = config(service);
                    test_config.refresh_interval_ns = refresh;
                    test_config.dispatch_service_ns = dispatch;
                    test_config.apply_guard_ns = refresh / 16;
                    test_config.apply_delay_ns = test_config.apply_guard_ns;
                    test_config.frames = 24;
                    let mut services = vec![service; test_config.frames as usize];
                    for position in [0, test_config.frames as usize / 2] {
                        services[position] = refresh + service;
                    }
                    let result = simulate_o1_with_render_services(test_config, &services);

                    assert!(result.max_future_primary_depth <= 2);
                    assert_eq!(result.target_mutations, 0);
                    assert!(result.drain_events <= u64::from(test_config.frames));
                }
            }
        }
    }

    #[test]
    fn generation_change_invalidates_old_armed_target_without_mutation() {
        let test_config = config(7_500_000);
        let event = SimulatedO1Event {
            at_ns: 4_000_000,
            order: 0,
            frame: 0,
            generation: 1,
            kind: SimulatedO1EventKind::OutputGenerationChanged,
        };
        let result = SimulatedO1EventModel::new(test_config)
            .run_with_events(&[test_config.render_service_ns; 120], &[event]);

        assert_eq!(result.target_mutations, 0);
    }

    #[test]
    fn submitted_frames_present_on_a_later_refresh_instead_of_disappearing() {
        let mut test_config = config(7_500_000);
        test_config.refresh_interval_ns = 1_000_000;
        test_config.apply_delay_ns = 100_000;
        test_config.frames = 4;
        let result = simulate_o1(test_config);

        assert!(result.later_refresh_pageflips > 0);
        assert_eq!(result.submitted_frame_liveness_violations, 0);
        assert_eq!(result.submitted_frames, result.terminalized_submitted_frames);
    }

    #[test]
    fn physical_lane_owners_are_identity_bounded() {
        let mut test_config = config(7_500_000);
        test_config.frames = 32;
        let result = simulate_o1(test_config);

        assert!(result.max_rendering_owners <= 1);
        assert!(result.max_worker_queued_owners <= 1);
        assert!(result.max_kernel_submitted_owners <= 1);
        assert!(result.max_future_primary_depth <= 2);
        assert_eq!(result.submitted_frame_liveness_violations, 0);
    }

    #[test]
    fn generation_change_terminalizes_an_already_submitted_frame() {
        let mut test_config = config(500_000);
        test_config.frames = 4;
        let generation_change = SimulatedO1Event {
            at_ns: 7_000_000,
            order: 0,
            frame: 0,
            generation: 1,
            kind: SimulatedO1EventKind::OutputGenerationChanged,
        };
        let services = vec![test_config.render_service_ns; test_config.frames as usize];
        let result = SimulatedO1EventModel::new(test_config)
            .run_with_events(&services, &[generation_change]);

        assert!(result.submitted_frames > 0);
        assert_eq!(result.submitted_frames, result.terminalized_submitted_frames);
        assert_eq!(result.submitted_frame_liveness_violations, 0);
    }
}
