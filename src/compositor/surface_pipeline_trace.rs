#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_trace_retains_no_records() {
        let mut trace = SurfacePipelineTrace::new(false, 4);
        trace.record(SurfacePipelineRecord::commit_captured(
            7,
            SurfaceCommitSequence(11),
        ));
        assert_eq!(trace.len(), 0);
    }

    #[test]
    fn enabled_trace_retains_the_first_event_before_the_ring_has_entries() {
        let mut trace = SurfacePipelineTrace::new(true, 4);
        assert!(trace.enabled());
        trace.record(SurfacePipelineRecord::commit_captured(
            7,
            SurfaceCommitSequence(11),
        ));
        assert_eq!(trace.len(), 1);
    }

    #[test]
    fn trace_is_bounded_and_preserves_newest_lineage() {
        let mut trace = SurfacePipelineTrace::new(true, 2);
        trace.record(SurfacePipelineRecord::commit_captured(
            7,
            SurfaceCommitSequence(1),
        ));
        trace.record(SurfacePipelineRecord::transaction_queued(
            7,
            SurfaceCommitSequence(1),
            9,
        ));
        trace.record(SurfacePipelineRecord::scene_sampled(
            7,
            SurfaceCommitSequence(1),
            12,
        ));

        let records = trace.records().collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].kind, SurfacePipelineEvent::TransactionQueued);
        assert_eq!(records[1].kind, SurfacePipelineEvent::SceneSampled);
        assert_eq!(records[1].surface_id, 7);
        assert_eq!(records[1].commit_sequence, SurfaceCommitSequence(1));
        assert_eq!(records[1].output_frame_id, Some(12));
    }

    #[test]
    fn commit_to_frame_lineage_keeps_numeric_identity() {
        let mut trace = SurfacePipelineTrace::new(true, 16);
        trace.record(SurfacePipelineRecord::commit_captured(
            7,
            SurfaceCommitSequence(41),
        ));
        trace.record(SurfacePipelineRecord::transaction_queued(
            7,
            SurfaceCommitSequence(41),
            99,
        ));
        trace.record(SurfacePipelineRecord::scene_sampled(
            7,
            SurfaceCommitSequence(41),
            100,
        ));
        trace.record(SurfacePipelineRecord::output_presented(
            7,
            SurfaceCommitSequence(41),
            100,
            101,
        ));

        let records = trace.records().collect::<Vec<_>>();
        assert_eq!(records[0].kind, SurfacePipelineEvent::CommitCaptured);
        assert_eq!(records[1].surface_tree_transaction_id, Some(99));
        assert_eq!(records[2].output_frame_id, Some(100));
        assert_eq!(records[3].pageflip_token, Some(101));
        assert!(
            records.iter().all(|record| record.surface_id == 7
                && record.commit_sequence == SurfaceCommitSequence(41))
        );
    }

    #[test]
    fn blocked_and_terminal_lineage_events_keep_commit_identity() {
        let mut trace = SurfacePipelineTrace::new(true, 32);
        for kind in [
            SurfacePipelineEvent::AcquirePending,
            SurfacePipelineEvent::AcquireReady,
            SurfacePipelineEvent::FifoWaitBlocked,
            SurfacePipelineEvent::FifoWaitReleased,
            SurfacePipelineEvent::CommitTimingBlocked,
            SurfacePipelineEvent::CommitTimingReleased,
            SurfacePipelineEvent::CommitDiscarded,
            SurfacePipelineEvent::CommitSuperseded,
            SurfacePipelineEvent::TransactionAbandoned,
            SurfacePipelineEvent::SurfaceDetached,
        ] {
            trace.record(SurfacePipelineRecord::for_event(
                kind,
                9,
                SurfaceCommitSequence(17),
            ));
        }

        let records = trace.records().collect::<Vec<_>>();
        assert_eq!(records.len(), 10);
        assert!(records.iter().all(|record| {
            record.surface_id == 9 && record.commit_sequence == SurfaceCommitSequence(17)
        }));
        assert_eq!(records[2].kind, SurfacePipelineEvent::FifoWaitBlocked);
        assert_eq!(records[4].kind, SurfacePipelineEvent::CommitTimingBlocked);
        assert_eq!(records[6].kind, SurfacePipelineEvent::CommitDiscarded);
        assert_eq!(records[8].kind, SurfacePipelineEvent::TransactionAbandoned);
    }
}
use std::collections::VecDeque;

use super::{CompositorState, SurfaceCommitSequence};

pub(crate) const SURFACE_PIPELINE_TRACE_CAPACITY: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfacePipelineEvent {
    CommitCaptured,
    AcquirePending,
    AcquireReady,
    FifoBarrierSet,
    FifoWaitBlocked,
    FifoWaitReleased,
    CommitTimingCaptured,
    CommitTimingBlocked,
    CommitTimingReleased,
    TransactionQueued,
    TransactionPromoted,
    SceneResolved,
    SceneSampled,
    FrameBatchBuilt,
    FrameCallbacksDeferred,
    FrameCallbacksAdmitted,
    OutputFrameRendered,
    OutputFrameSubmitted,
    OutputFramePresented,
    PresentationFeedbackCompleted,
    BufferReleaseCompleted,
    CommitDiscarded,
    CommitSuperseded,
    TransactionAbandoned,
    SurfaceDetached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SurfacePipelineRecord {
    pub(crate) timestamp_ns: u64,
    pub(crate) kind: SurfacePipelineEvent,
    pub(crate) surface_id: u32,
    pub(crate) surface_generation: Option<u64>,
    pub(crate) commit_sequence: SurfaceCommitSequence,
    pub(crate) buffer_id: Option<u64>,
    pub(crate) buffer_generation: Option<u64>,
    pub(crate) xwayland_generation: Option<u64>,
    pub(crate) association_serial: Option<u64>,
    pub(crate) frame_batch_id: Option<u64>,
    pub(crate) surface_tree_transaction_id: Option<u64>,
    pub(crate) output_transaction_id: Option<u64>,
    pub(crate) output_frame_id: Option<u64>,
    pub(crate) pageflip_token: Option<u64>,
}

impl SurfacePipelineRecord {
    fn base(surface_id: u32, commit_sequence: SurfaceCommitSequence) -> Self {
        Self {
            timestamp_ns: super::protocol_error_trace::protocol_error_timestamp_ns(),
            kind: SurfacePipelineEvent::CommitCaptured,
            surface_id,
            surface_generation: None,
            commit_sequence,
            buffer_id: None,
            buffer_generation: None,
            xwayland_generation: None,
            association_serial: None,
            frame_batch_id: None,
            surface_tree_transaction_id: None,
            output_transaction_id: None,
            output_frame_id: None,
            pageflip_token: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn commit_captured(surface_id: u32, commit_sequence: SurfaceCommitSequence) -> Self {
        Self::base(surface_id, commit_sequence)
    }

    pub(crate) fn for_event(
        kind: SurfacePipelineEvent,
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
    ) -> Self {
        Self {
            kind,
            ..Self::base(surface_id, commit_sequence)
        }
    }

    #[cfg(test)]
    pub(crate) fn transaction_queued(
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
        transaction_id: u64,
    ) -> Self {
        Self {
            kind: SurfacePipelineEvent::TransactionQueued,
            surface_tree_transaction_id: Some(transaction_id),
            ..Self::base(surface_id, commit_sequence)
        }
    }

    #[cfg(test)]
    pub(crate) fn scene_sampled(
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
        output_frame_id: u64,
    ) -> Self {
        Self {
            kind: SurfacePipelineEvent::SceneSampled,
            output_frame_id: Some(output_frame_id),
            ..Self::base(surface_id, commit_sequence)
        }
    }

    #[cfg(test)]
    pub(crate) fn output_presented(
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
        output_frame_id: u64,
        pageflip_token: u64,
    ) -> Self {
        Self {
            kind: SurfacePipelineEvent::OutputFramePresented,
            output_frame_id: Some(output_frame_id),
            pageflip_token: Some(pageflip_token),
            ..Self::base(surface_id, commit_sequence)
        }
    }
}

impl CompositorState {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::compositor) fn trace_surface_pipeline_event(
        &mut self,
        kind: SurfacePipelineEvent,
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
        buffer_id: Option<u64>,
        frame_batch_id: Option<u64>,
        surface_tree_transaction_id: Option<u64>,
        output_transaction_id: Option<u64>,
        output_frame_id: Option<u64>,
        pageflip_token: Option<u64>,
    ) {
        if !self.surface_pipeline_trace.enabled() {
            return;
        }
        let mut record = SurfacePipelineRecord::for_event(kind, surface_id, commit_sequence);
        record.surface_generation = self
            .surface_presentation_generations
            .get(&surface_id)
            .copied();
        record.buffer_id = buffer_id;
        record.frame_batch_id = frame_batch_id;
        record.surface_tree_transaction_id = surface_tree_transaction_id;
        record.output_transaction_id = output_transaction_id;
        record.output_frame_id = output_frame_id;
        record.pageflip_token = pageflip_token;

        if let Some(xwayland_state) = self.xwayland.surface_states.get(&surface_id) {
            record.xwayland_generation = Some(xwayland_state.generation.get());
            record.association_serial = self
                .xwayland
                .associations
                .serial_for_surface(surface_id)
                .map(|(_, serial)| serial.get());
        }

        self.surface_pipeline_trace.record(record);
    }

    pub(in crate::compositor) fn surface_pipeline_trace_enabled(&self) -> bool {
        self.surface_pipeline_trace.enabled()
    }

    pub(in crate::compositor) fn trace_surface_pipeline_active_surfaces(
        &mut self,
        kind: SurfacePipelineEvent,
        frame_batch_id: Option<u64>,
        output_transaction_id: Option<u64>,
        output_frame_id: Option<u64>,
        pageflip_token: Option<u64>,
    ) {
        if !self.surface_pipeline_trace.enabled() {
            return;
        }
        for index in 0..self.active_scene_surfaces().len() {
            let (surface_id, commit_sequence, buffer_id) = {
                let surface = &self.active_scene_surfaces()[index];
                (
                    surface.surface_id,
                    surface.commit_sequence,
                    Some(surface.buffer_id().get()),
                )
            };
            self.trace_surface_pipeline_event(
                kind,
                surface_id,
                commit_sequence,
                buffer_id,
                frame_batch_id,
                None,
                output_transaction_id,
                output_frame_id,
                pageflip_token,
            );
        }
    }

    pub(in crate::compositor) fn trace_surface_pipeline_buffer_release(
        &mut self,
        buffer_id: u64,
        frame_batch_id: Option<u64>,
        output_frame_id: Option<u64>,
    ) {
        if !self.surface_pipeline_trace.enabled() {
            return;
        }
        for index in 0..self.active_scene_surfaces().len() {
            let (surface_id, commit_sequence, matches_buffer) = {
                let surface = &self.active_scene_surfaces()[index];
                (
                    surface.surface_id,
                    surface.commit_sequence,
                    surface.buffer_id().get() == buffer_id,
                )
            };
            if matches_buffer {
                self.trace_surface_pipeline_event(
                    SurfacePipelineEvent::BufferReleaseCompleted,
                    surface_id,
                    commit_sequence,
                    Some(buffer_id),
                    frame_batch_id,
                    None,
                    None,
                    output_frame_id,
                    None,
                );
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct SurfacePipelineTrace {
    records: Option<VecDeque<SurfacePipelineRecord>>,
    capacity: usize,
}

impl SurfacePipelineTrace {
    pub(crate) fn new(enabled: bool, capacity: usize) -> Self {
        Self {
            records: enabled.then(|| VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    pub(crate) fn from_env() -> Self {
        let enabled = std::env::var("TYPHON_SURFACE_PIPELINE_TRACE")
            .ok()
            .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "on"));
        Self::new(enabled, SURFACE_PIPELINE_TRACE_CAPACITY)
    }

    pub(crate) fn record(&mut self, record: SurfacePipelineRecord) {
        let Some(records) = self.records.as_mut() else {
            return;
        };
        if self.capacity == 0 {
            return;
        }
        if records.len() == self.capacity {
            records.pop_front();
        }
        records.push_back(record);
    }

    pub(crate) fn enabled(&self) -> bool {
        self.records.is_some()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.records.as_ref().map_or(0, VecDeque::len)
    }

    pub(crate) fn records(&self) -> impl Iterator<Item = &SurfacePipelineRecord> {
        self.records.iter().flat_map(|records| records.iter())
    }

    pub(crate) fn dump(&self) {
        for record in self.records() {
            eprintln!(
                "typhon_surface_pipeline timestamp_ns={} event={:?} surface_id={} surface_generation={:?} commit_sequence={} buffer_id={:?} buffer_generation={:?} xwayland_generation={:?} association_serial={:?} frame_batch_id={:?} surface_tree_transaction_id={:?} output_transaction_id={:?} output_frame_id={:?} pageflip_token={:?}",
                record.timestamp_ns,
                record.kind,
                record.surface_id,
                record.surface_generation,
                record.commit_sequence.get(),
                record.buffer_id,
                record.buffer_generation,
                record.xwayland_generation,
                record.association_serial,
                record.frame_batch_id,
                record.surface_tree_transaction_id,
                record.output_transaction_id,
                record.output_frame_id,
                record.pageflip_token,
            );
        }
    }
}

impl Default for SurfacePipelineTrace {
    fn default() -> Self {
        Self::from_env()
    }
}
