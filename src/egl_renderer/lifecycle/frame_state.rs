use super::super::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct LifecycleOrderedSample {
    pub(super) painter_order: LifecyclePainterOrder,
    pub(super) sample: LifecycleFrameSample,
}

#[derive(Default)]
pub(super) struct LifecycleFrameState {
    pub(super) samples: Vec<LifecycleFrameSample>,
    pub(super) ordered_samples: Vec<LifecycleOrderedSample>,
    pub(super) evidence: LifecycleRenderEvidence,
    pub(super) fallbacks: LifecycleRenderFallbacks,
}

impl LifecycleFrameState {
    pub(super) fn sample_for_identity(
        &self,
        presentation_identity: oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
    ) -> Option<LifecycleFrameSample> {
        self.samples
            .iter()
            .find(|sample| sample.presentation_identity == presentation_identity)
            .copied()
    }

    pub(super) fn begin(&mut self, snapshot: &LifecycleSceneSample) {
        self.samples = LifecycleFrameSnapshot::from_sample(snapshot).samples;
        self.ordered_samples = snapshot
            .samples
            .iter()
            .zip(self.samples.iter().copied())
            .map(|(source, sample)| LifecycleOrderedSample {
                painter_order: source.painter_order,
                sample,
            })
            .collect();
        self.ordered_samples
            .sort_by_key(|ordered| ordered.painter_order);
        self.evidence.consumed.clear();
        self.fallbacks.failed.clear();
    }

    pub(super) fn record_fallback(
        &mut self,
        sample: LifecycleFrameSample,
        reason: LifecycleRenderFallbackReason,
    ) {
        self.fallbacks.record(LifecycleRenderFallbackEntry {
            window_id: sample.window_id,
            root_surface_id: sample.root_surface_id,
            presentation_identity: sample.presentation_identity,
            payload_id: sample.payload_id,
            effect: sample.effect,
            reason,
        });
    }

    pub(super) fn visible_samples(
        &self,
        output_scale: f64,
        output_size: (u32, u32),
    ) -> Vec<LifecycleFrameSample> {
        visible_lifecycle_samples(&self.samples, output_scale, output_size.0, output_size.1)
            .collect()
    }

    pub(super) fn record_missing_evidence_fallbacks(
        &mut self,
        output_scale: f64,
        output_size: (u32, u32),
    ) {
        let missing = self
            .visible_samples(output_scale, output_size)
            .into_iter()
            .filter(|sample| {
                !self.evidence.contains(
                    sample.presentation_identity,
                    sample.payload_id,
                    sample.root_surface_id,
                )
            })
            .collect::<Vec<_>>();
        for sample in missing {
            self.record_fallback(
                sample,
                LifecycleRenderFallbackReason::NoConsumedRepresentation,
            );
        }
    }

    pub(super) fn damage_for_snapshot(
        snapshot: &LifecycleSceneSample,
        output_size: (u32, u32),
        output_scale: f64,
    ) -> OutputDamage {
        let rects: Vec<OutputRect> = snapshot
            .samples
            .iter()
            .filter_map(|sample| {
                let footprint = lifecycle_visual_transition_bounds(
                    sample.effect,
                    sample.visual_group,
                    sample.progress,
                )?;
                let left = footprint.x();
                let top = footprint.y();
                let right = left + footprint.width();
                let bottom = top + footprint.height();
                Some(OutputRect::new(
                    (left * output_scale).floor() as i32,
                    (top * output_scale).floor() as i32,
                    ((right - left) * output_scale).ceil().max(1.0) as u32,
                    ((bottom - top) * output_scale).ceil().max(1.0) as u32,
                ))
            })
            .collect();
        OutputDamage::rects(output_size.0, output_size.1, rects)
    }
}

pub(super) struct LifecycleCaptureFrameSnapshot {
    evidence: LifecycleRenderEvidence,
    fallbacks: LifecycleRenderFallbacks,
    samples: Vec<LifecycleFrameSample>,
    ordered_samples: Vec<LifecycleOrderedSample>,
}

impl LifecycleCaptureFrameSnapshot {
    pub(super) fn take(frame: &LifecycleFrameState) -> Self {
        Self {
            evidence: frame.evidence.clone(),
            fallbacks: frame.fallbacks.clone(),
            samples: frame.samples.clone(),
            ordered_samples: frame.ordered_samples.clone(),
        }
    }

    pub(super) fn restore(self, frame: &mut LifecycleFrameState) {
        frame.evidence = self.evidence;
        frame.fallbacks = self.fallbacks;
        frame.samples = self.samples;
        frame.ordered_samples = self.ordered_samples;
    }
}

pub(in crate::egl_renderer) struct LifecycleCaptureSnapshot {
    pub(super) frame: LifecycleCaptureFrameSnapshot,
    pub(super) visual_sources: HashMap<
        oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
        LifecycleVisualSource,
    >,
}
