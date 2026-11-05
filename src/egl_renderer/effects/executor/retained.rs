//! Retained effect outputs: an instance's last pre-composite texture kept
//! across frames.
//!
//! Stacked blurred windows otherwise re-run capture and every Kawase pass of
//! each window whenever anything above or beside them repaints, even though
//! their backdrop is unchanged. The checkpoint causal stability plan already
//! proves when an instance's output is unchanged since the previous frame;
//! while it holds, the texture the final Composite samples is still current
//! wherever it was rendered before. A frame whose Composite needs only such
//! pixels skips the instance's capture and intermediate passes and composites
//! from the retained texture. Other frames render into the retained texture
//! (unchanged instances) so its valid region grows, or into a fresh one.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

use oblivion_one::effects::{EffectFrameDemand, required_input_region};

use super::*;
use crate::egl_renderer::effects::resources::RetainedEffectOutputIdentity;

#[derive(Clone, Debug)]
pub(crate) struct RetainedOutputCandidate {
    pub(crate) instance: EffectInstanceId,
    pub(crate) composite: GraphPassId,
    /// The Composite's single intermediate input.
    pub(crate) texture: GraphTextureId,
    pub(crate) identity: RetainedEffectOutputIdentity,
    /// The Composite can run from the retained texture alone.
    pub(crate) hit: bool,
    /// A current retained texture exists; misses render into it.
    pub(crate) reuse_texture: bool,
}

/// Retained output decisions for one frame graph, made while planning and
/// consumed by that graph's execution.
#[derive(Clone, Debug)]
pub(crate) struct RetainedOutputPlan {
    frame_serial: u64,
    passes: Vec<GraphPassId>,
    pub(crate) candidates: Vec<RetainedOutputCandidate>,
}

impl RetainedOutputPlan {
    pub(crate) fn candidate_for_composite(
        &self,
        pass: GraphPassId,
    ) -> Option<&RetainedOutputCandidate> {
        self.candidates
            .iter()
            .find(|candidate| candidate.composite == pass)
    }

    pub(crate) fn is_candidate_texture(&self, texture: GraphTextureId) -> bool {
        self.candidates
            .iter()
            .any(|candidate| candidate.texture == texture)
    }

    pub(crate) fn hits(&self) -> usize {
        self.candidates
            .iter()
            .filter(|candidate| candidate.hit)
            .count()
    }
}

/// Instances whose output is a closed chain from one scene capture to one
/// Composite that samples a single intermediate texture of the instance.
pub(crate) fn retained_output_candidates(
    graph: &CompiledFrameGraph,
) -> Vec<RetainedOutputCandidate> {
    let mut candidates = Vec::new();
    for instance in &graph.instances {
        if instance.frame_demand == EffectFrameDemand::Continuous {
            continue;
        }
        let passes = graph
            .passes
            .iter()
            .filter(|pass| pass.instance == instance.id)
            .collect::<Vec<_>>();
        let count = |kind| passes.iter().filter(|pass| pass.kind == kind).count();
        if count(RenderPassKind::SceneCapture) != 1
            || count(RenderPassKind::SurfaceCapture) != 0
            || count(RenderPassKind::Composite) != 1
            || count(RenderPassKind::OutputPostProcess) != 0
        {
            continue;
        }
        let reads_outside_instance = passes.iter().any(|pass| {
            pass.inputs.iter().any(|input| {
                graph
                    .textures
                    .iter()
                    .find(|texture| texture.id == *input)
                    .is_none_or(|texture| {
                        matches!(
                            texture.source,
                            GraphTextureSource::Output
                                | GraphTextureSource::PrecomputedBackdrop
                                | GraphTextureSource::CapturedTarget
                        )
                    })
            })
        });
        if reads_outside_instance {
            continue;
        }
        let Some(composite) = passes
            .iter()
            .find(|pass| pass.kind == RenderPassKind::Composite)
        else {
            continue;
        };
        let [input] = composite.inputs.as_slice() else {
            continue;
        };
        let Some(input_plan) = graph.textures.iter().find(|texture| texture.id == *input) else {
            continue;
        };
        let producers = passes
            .iter()
            .filter(|pass| pass.output == Some(*input))
            .count();
        if input_plan.source != GraphTextureSource::Intermediate || producers != 1 {
            continue;
        }
        candidates.push(RetainedOutputCandidate {
            instance: instance.id,
            composite: composite.id,
            texture: *input,
            identity: RetainedEffectOutputIdentity::new(instance.semantic_signature, input_plan),
            hit: false,
            reuse_texture: false,
        });
    }
    candidates
}

/// The logical input region the Composite samples this frame, computed
/// exactly as the executor derives the Composite's execution region.
fn composite_required_input(
    graph: &CompiledFrameGraph,
    demand: &EffectExecutionDemand,
    composite: &CompiledRenderPass,
) -> Option<EffectRegion> {
    let region = prepare_effect_execution_region(
        graph,
        composite,
        effective_pass_damage(graph, demand, composite),
    )
    .region;
    let output = graph
        .textures
        .iter()
        .find(|texture| Some(texture.id) == composite.output)?;
    let input = graph
        .textures
        .iter()
        .find(|texture| composite.inputs.first() == Some(&texture.id))?;
    required_input_region(composite, &region, output, input)
}

/// Refreshes the retained output cache for `graph`, marks the instances whose
/// Composite can run from retained pixels, and removes their capture and
/// intermediate passes from `selection`.
pub(crate) fn plan_retained_effect_outputs(
    scene: &SceneRenderState,
    runtime: &mut EffectRuntime,
    graph: &CompiledFrameGraph,
    demand: &EffectExecutionDemand,
    selection: &mut EffectExecutionSelection,
) {
    runtime.retained_output_plan = None;
    if RETAINED_DISABLED.load(Relaxed) {
        runtime.effect_resources.clear_retained_effect_outputs();
        return;
    }
    if runtime.capture_in_progress {
        return;
    }
    let stability = checkpoint_causal_stability_plan_for_owners(scene, runtime, graph);
    let mut candidates = retained_output_candidates(graph);
    let unchanged = |instance: EffectInstanceId| {
        stability
            .instances
            .get(&instance)
            .is_some_and(|stability| stability.output_unchanged)
    };
    runtime
        .effect_resources
        .refresh_retained_effect_outputs(|instance, identity| {
            unchanged(instance)
                && candidates.iter().any(|candidate| {
                    candidate.instance == instance && candidate.identity == *identity
                })
        });

    for candidate in &mut candidates {
        if !selection.executed_passes.contains(&candidate.composite) {
            continue;
        }
        let Some((_, valid)) = runtime
            .effect_resources
            .retained_effect_output(candidate.instance)
        else {
            continue;
        };
        candidate.reuse_texture = true;
        let Some(composite) = graph
            .passes
            .iter()
            .find(|pass| pass.id == candidate.composite)
        else {
            continue;
        };
        candidate.hit = composite_required_input(graph, demand, composite)
            .is_some_and(|required| required.subtract(valid).is_empty());
    }

    let hit_instances = candidates
        .iter()
        .filter(|candidate| candidate.hit)
        .map(|candidate| (candidate.instance, candidate.composite))
        .collect::<Vec<_>>();
    if !hit_instances.is_empty() {
        let skipped = |pass: &CompiledRenderPass| {
            hit_instances
                .iter()
                .any(|(instance, composite)| pass.instance == *instance && pass.id != *composite)
        };
        selection.executed_passes.retain(|id| {
            graph
                .passes
                .iter()
                .find(|pass| pass.id == *id)
                .is_none_or(|pass| !skipped(pass))
        });
        let mut acquired = Vec::new();
        for pass in &graph.passes {
            if !selection.executed_passes.contains(&pass.id) {
                continue;
            }
            for texture_id in pass.inputs.iter().copied().chain(pass.output) {
                if selection.acquired_texture_ids.contains(&texture_id)
                    && !acquired.contains(&texture_id)
                {
                    acquired.push(texture_id);
                }
            }
        }
        selection.acquired_texture_ids = acquired;
    }

    runtime.retained_output_plan = Some(RetainedOutputPlan {
        frame_serial: runtime.effect_resources.checkpoint_frame_serial(),
        passes: graph.passes.iter().map(|pass| pass.id).collect(),
        candidates,
    });
}

/// The plan made for `graph` this frame, if any. The plan is not consumed:
/// a lifecycle baseline executes the same graph and selection again in the
/// same frame, and every execution must bind the retained textures of hits.
pub(super) fn retained_output_plan_for_graph(
    runtime: &EffectRuntime,
    graph: &CompiledFrameGraph,
) -> Option<RetainedOutputPlan> {
    let plan = runtime.retained_output_plan.as_ref()?;
    (!runtime.capture_in_progress
        && plan.frame_serial == runtime.effect_resources.checkpoint_frame_serial()
        && plan.passes.len() == graph.passes.len()
        && plan
            .passes
            .iter()
            .zip(&graph.passes)
            .all(|(id, pass)| *id == pass.id))
    .then(|| plan.clone())
}

static RETAINED_DISABLED: AtomicBool = AtomicBool::new(false);
static RETAINED_FRAMES: AtomicU64 = AtomicU64::new(0);
static RETAINED_HITS: AtomicU64 = AtomicU64::new(0);
static RETAINED_CANDIDATES: AtomicU64 = AtomicU64::new(0);

/// Runtime switch for retained outputs (`astreactl effects retained on|off`).
pub(crate) fn set_retained_effect_outputs_enabled(enabled: bool) {
    RETAINED_DISABLED.store(!enabled, Relaxed);
}

/// Whether retained outputs are enabled and the cumulative executed-graph,
/// hit and candidate counts.
pub(crate) fn retained_effect_output_counters() -> (bool, u64, u64, u64) {
    (
        !RETAINED_DISABLED.load(Relaxed),
        RETAINED_FRAMES.load(Relaxed),
        RETAINED_HITS.load(Relaxed),
        RETAINED_CANDIDATES.load(Relaxed),
    )
}

/// Counts the plan's hits; with `TYPHON_EFFECT_RETAINED_TRACE=1` also logs
/// the hit rate every 300 executed graphs.
pub(super) fn retained_trace(plan: &RetainedOutputPlan, runtime: &EffectRuntime) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    static LOGGED: AtomicU64 = AtomicU64::new(0);
    let frames = RETAINED_FRAMES.fetch_add(1, Relaxed) + 1;
    let hits = RETAINED_HITS.fetch_add(plan.hits() as u64, Relaxed) + plan.hits() as u64;
    let candidates = RETAINED_CANDIDATES.fetch_add(plan.candidates.len() as u64, Relaxed)
        + plan.candidates.len() as u64;
    if !*ENABLED.get_or_init(|| {
        std::env::var_os("TYPHON_EFFECT_RETAINED_TRACE").as_deref()
            == Some(std::ffi::OsStr::new("1"))
    }) || frames % 300 != 0
    {
        return;
    }
    let (entries, bytes) = runtime.effect_resources.retained_effect_output_stats();
    let previous = LOGGED.swap(frames, Relaxed);
    eprintln!(
        "typhon effect: event=retained_outputs frames={frames} since={previous} hits={hits} candidates={candidates} entries={entries} bytes={bytes}",
    );
}
