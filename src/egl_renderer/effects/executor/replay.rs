use super::*;

pub(super) fn graph_texture(
    graph: &CompiledFrameGraph,
    id: GraphTextureId,
) -> RendererResult<&oblivion_one::effects::GraphTexturePlan> {
    graph
        .textures
        .iter()
        .find(|texture| texture.id == id)
        .ok_or_else(|| io::Error::other("effect graph references an unknown texture").into())
}

pub(super) fn full_output_rect(size: (u32, u32)) -> OutputRect {
    OutputRect::new(0, 0, size.0, size.1)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SceneCheckpointRequirement {
    pub(super) capture_pass: GraphPassId,
    pub(super) region: Vec<OutputRect>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SceneReplayWorkMode {
    GlobalBaseline,
    SuffixDemand,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SceneBaselineAuthority {
    ReplayRequired,
    PrecomposedFramebuffer,
}

pub(super) fn should_reconstruct_scene_work(
    authority: SceneBaselineAuthority,
    framebuffer_capture: bool,
    has_extra_scene_work: bool,
) -> bool {
    match authority {
        SceneBaselineAuthority::ReplayRequired => framebuffer_capture || has_extra_scene_work,
        SceneBaselineAuthority::PrecomposedFramebuffer => false,
    }
}

pub(super) fn initial_scene_valid_region(
    authority: SceneBaselineAuthority,
    plan: &SceneReplayWorkPlan,
) -> EffectRegion {
    match authority {
        SceneBaselineAuthority::ReplayRequired => EffectRegion::empty(),
        SceneBaselineAuthority::PrecomposedFramebuffer => {
            output_rects_to_effect_region(&plan.baseline_work)
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct SceneReplayWorkPlan {
    pub(super) presentation_work: Vec<OutputRect>,
    pub(super) baseline_work: Vec<OutputRect>,
    pub(super) extra_scene_work: Vec<OutputRect>,
    pub(super) checkpoint_requirements: Vec<SceneCheckpointRequirement>,
    pub(super) output_size: (u32, u32),
    pub(super) normalization_overflow_fallbacks: usize,
}

impl SceneReplayWorkPlan {
    pub(super) fn new(
        presentation_work: Vec<OutputRect>,
        checkpoint_requirements: Vec<SceneCheckpointRequirement>,
        output_size: (u32, u32),
    ) -> Self {
        let checkpoint_work = checkpoint_requirements
            .iter()
            .flat_map(|requirement| requirement.region.iter().copied());
        let normalized =
            normalize_scene_replay_work(&presentation_work, checkpoint_work, output_size);
        let baseline_work = normalized.rects;
        let normalization_overflow_fallbacks = if normalized.overflow_fallback { 1 } else { 0 };
        let extra_scene_work = extra_scene_work(&baseline_work, &presentation_work);
        Self {
            presentation_work,
            baseline_work,
            extra_scene_work,
            checkpoint_requirements,
            output_size,
            normalization_overflow_fallbacks,
        }
    }
}

pub(super) struct SceneReplayWorkState<'a> {
    pub(super) plan: &'a SceneReplayWorkPlan,
    pub(super) mode: SceneReplayWorkMode,
    pub(super) pending_capture_passes: Vec<GraphPassId>,
    pub(super) active_work: Vec<OutputRect>,
    pub(super) normalization_overflow_fallbacks: usize,
}

impl<'a> SceneReplayWorkState<'a> {
    pub(super) fn new(plan: &'a SceneReplayWorkPlan, mode: SceneReplayWorkMode) -> Self {
        let pending_capture_passes = plan
            .checkpoint_requirements
            .iter()
            .map(|requirement| requirement.capture_pass)
            .collect::<Vec<_>>();
        let mut normalization_overflow_fallbacks = plan.normalization_overflow_fallbacks;
        let active_work = match mode {
            SceneReplayWorkMode::GlobalBaseline => plan.baseline_work.clone(),
            SceneReplayWorkMode::SuffixDemand => {
                let normalized = normalize_scene_replay_work(
                    &plan.presentation_work,
                    plan.checkpoint_requirements
                        .iter()
                        .flat_map(|requirement| requirement.region.iter().copied()),
                    output_size_for_work_plan(plan),
                );
                normalization_overflow_fallbacks = normalization_overflow_fallbacks
                    .saturating_add(if normalized.overflow_fallback { 1 } else { 0 });
                normalized.rects
            }
        };
        Self {
            plan,
            mode,
            pending_capture_passes,
            active_work,
            normalization_overflow_fallbacks,
        }
    }

    pub(super) fn active_work(&self) -> &[OutputRect] {
        &self.active_work
    }

    pub(super) fn pending_checkpoint_requirements(&self) -> usize {
        self.pending_capture_passes.len()
    }

    pub(super) fn mark_capture_satisfied(&mut self, capture_pass: GraphPassId) -> bool {
        let Some(index) = self
            .pending_capture_passes
            .iter()
            .position(|pending| *pending == capture_pass)
        else {
            return false;
        };
        let previous = self.active_work.clone();
        self.pending_capture_passes.remove(index);
        if self.mode == SceneReplayWorkMode::SuffixDemand {
            let normalized = normalize_scene_replay_work(
                &self.plan.presentation_work,
                self.plan
                    .checkpoint_requirements
                    .iter()
                    .filter(|requirement| {
                        self.pending_capture_passes
                            .contains(&requirement.capture_pass)
                    })
                    .flat_map(|requirement| requirement.region.iter().copied()),
                output_size_for_work_plan(self.plan),
            );
            self.normalization_overflow_fallbacks = self
                .normalization_overflow_fallbacks
                .saturating_add(if normalized.overflow_fallback { 1 } else { 0 });
            self.active_work = normalized.rects;
            debug_assert!(
                output_rects_to_effect_region(&self.active_work)
                    .subtract(&output_rects_to_effect_region(&previous))
                    .is_empty(),
                "scene replay work must only shrink after a capture succeeds"
            );
        }
        true
    }

    #[allow(dead_code)]
    pub(super) fn force_baseline(&mut self) {
        self.mode = SceneReplayWorkMode::GlobalBaseline;
        self.active_work = self.plan.baseline_work.clone();
    }

    pub(super) fn work_trace_metrics(&self) -> (usize, u64, usize, u64, usize) {
        (
            self.active_work.len(),
            output_rects_pixels(&self.active_work),
            self.plan.baseline_work.len(),
            output_rects_pixels(&self.plan.baseline_work),
            self.pending_checkpoint_requirements(),
        )
    }
}

pub(super) fn finalize_surface_consumer_trailing_work(
    scene_work_state: &mut SceneReplayWorkState<'_>,
) -> Vec<OutputRect> {
    let pending_checkpoint_requirements = scene_work_state.pending_checkpoint_requirements();
    if pending_checkpoint_requirements != 0 {
        debug_assert_eq!(
            pending_checkpoint_requirements, 0,
            "pending scene checkpoint requirements remain before trailing consumer planning"
        );
        scene_work_state.force_baseline();
    }
    scene_work_state.active_work().to_vec()
}

pub(super) fn output_size_for_work_plan(plan: &SceneReplayWorkPlan) -> (u32, u32) {
    plan.output_size
}

pub(super) struct NormalizedSceneReplayWork {
    pub(super) rects: Vec<OutputRect>,
    pub(super) overflow_fallback: bool,
}

pub(super) fn normalize_scene_replay_work(
    presentation_work: &[OutputRect],
    checkpoint_work: impl IntoIterator<Item = OutputRect>,
    output_size: (u32, u32),
) -> NormalizedSceneReplayWork {
    let mut rects = presentation_work.to_vec();
    rects.extend(checkpoint_work);
    let coalesced = OutputDamage::rects(output_size.0, output_size.1, rects);
    let rects = match coalesced {
        OutputDamage::Empty => Vec::new(),
        OutputDamage::Full => vec![full_output_rect(output_size)],
        OutputDamage::Rects(rects) => rects,
    };
    let (rects, overflow_fallback) = disjoint_output_rects_with_overflow(rects, output_size);
    NormalizedSceneReplayWork {
        rects,
        overflow_fallback,
    }
}

pub(super) fn extra_scene_work(
    scene_work_rects: &[OutputRect],
    repaint_rects: &[OutputRect],
) -> Vec<OutputRect> {
    let mut extra_scene_work = Vec::new();
    for scene_rect in scene_work_rects {
        let mut fragments = vec![*scene_rect];
        for repaint_rect in repaint_rects {
            let mut next = Vec::new();
            for fragment in fragments {
                next.extend(subtract_output_rect(fragment, *repaint_rect));
            }
            fragments = next;
            if fragments.is_empty() {
                break;
            }
        }
        extra_scene_work.extend(fragments);
    }
    extra_scene_work
}

pub(super) fn scene_replay_work_plan(
    repaint_rects: &[OutputRect],
    graph: &CompiledFrameGraph,
    selection: &EffectExecutionSelection,
    output_size: (u32, u32),
    scene_baseline_authority: SceneBaselineAuthority,
    debug_config: EffectDebugConfig,
) -> SceneReplayWorkPlan {
    let presentation_work = repaint_rects.to_vec();
    let mut checkpoint_requirements = Vec::new();
    for pass in &graph.passes {
        if !selection.executed_passes.contains(&pass.id)
            || !is_direct_framebuffer_capture(pass, scene_baseline_authority, debug_config)
            || !matches!(
                pass.kind,
                RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
            )
        {
            continue;
        }
        let Some(output) = pass.output else {
            continue;
        };
        let Some(texture) = graph.textures.iter().find(|texture| texture.id == output) else {
            continue;
        };
        let Some(rect) = clipped_output_rect(texture.domain, output_size) else {
            continue;
        };
        checkpoint_requirements.push(SceneCheckpointRequirement {
            capture_pass: pass.id,
            region: vec![rect],
        });
    }
    SceneReplayWorkPlan::new(presentation_work, checkpoint_requirements, output_size)
}

pub(super) fn disjoint_output_rects_with_overflow(
    rects: Vec<OutputRect>,
    output_size: (u32, u32),
) -> (Vec<OutputRect>, bool) {
    let mut disjoint = Vec::new();
    for source in rects {
        let mut fragments = vec![source];
        for represented in &disjoint {
            let mut next = Vec::new();
            for fragment in fragments {
                next.extend(subtract_output_rect(fragment, *represented));
            }
            fragments = next;
            if fragments.is_empty() {
                break;
            }
        }
        disjoint.extend(fragments);
        if disjoint.len() > MAX_EFFECT_REGION_RECTS {
            return (vec![full_output_rect(output_size)], true);
        }
    }
    (disjoint, false)
}

pub(super) fn subtract_output_rect(source: OutputRect, excluded: OutputRect) -> Vec<OutputRect> {
    let left = i64::from(source.x).max(i64::from(excluded.x));
    let top = i64::from(source.y).max(i64::from(excluded.y));
    let right = (i64::from(source.x) + i64::from(source.width))
        .min(i64::from(excluded.x) + i64::from(excluded.width));
    let bottom = (i64::from(source.y) + i64::from(source.height))
        .min(i64::from(excluded.y) + i64::from(excluded.height));
    if left >= right || top >= bottom {
        return vec![source];
    }

    let mut result = Vec::with_capacity(4);
    let push = |result: &mut Vec<OutputRect>, x: i64, y: i64, right: i64, bottom: i64| {
        if right > x && bottom > y {
            result.push(OutputRect::new(
                i32::try_from(x).expect("scene work x fits i32"),
                i32::try_from(y).expect("scene work y fits i32"),
                u32::try_from(right - x).expect("scene work width fits u32"),
                u32::try_from(bottom - y).expect("scene work height fits u32"),
            ));
        }
    };
    let source_right = i64::from(source.x) + i64::from(source.width);
    let source_bottom = i64::from(source.y) + i64::from(source.height);
    push(
        &mut result,
        i64::from(source.x),
        i64::from(source.y),
        source_right,
        top,
    );
    push(
        &mut result,
        i64::from(source.x),
        bottom,
        source_right,
        source_bottom,
    );
    push(&mut result, i64::from(source.x), top, left, bottom);
    push(&mut result, right, top, source_right, bottom);
    result
}

pub(super) fn clipped_output_rect(
    rect: oblivion_one::effects::EffectRect,
    output_size: (u32, u32),
) -> Option<OutputRect> {
    let left = i64::from(rect.x).clamp(0, i64::from(output_size.0));
    let top = i64::from(rect.y).clamp(0, i64::from(output_size.1));
    let right = i64::from(rect.right()).clamp(0, i64::from(output_size.0));
    let bottom = i64::from(rect.bottom()).clamp(0, i64::from(output_size.1));
    (right > left && bottom > top).then(|| {
        OutputRect::new(
            i32::try_from(left).expect("output width fits i32"),
            i32::try_from(top).expect("output height fits i32"),
            u32::try_from(right - left).expect("output rect width fits u32"),
            u32::try_from(bottom - top).expect("output rect height fits u32"),
        )
    })
}

pub(super) fn output_rects_bounding_box(rects: &[OutputRect]) -> Option<(i32, i32, u32, u32)> {
    let first = rects.first().copied()?;
    let (left, top, right, bottom) = rects.iter().skip(1).fold(
        (
            i64::from(first.x),
            i64::from(first.y),
            i64::from(first.x) + i64::from(first.width),
            i64::from(first.y) + i64::from(first.height),
        ),
        |(left, top, right, bottom), rect| {
            (
                left.min(i64::from(rect.x)),
                top.min(i64::from(rect.y)),
                right.max(i64::from(rect.x) + i64::from(rect.width)),
                bottom.max(i64::from(rect.y) + i64::from(rect.height)),
            )
        },
    );
    Some((
        i32::try_from(left).ok()?,
        i32::try_from(top).ok()?,
        u32::try_from(right - left).ok()?,
        u32::try_from(bottom - top).ok()?,
    ))
}

pub(super) fn effect_capture_output_rects(
    damage: &EffectRegion,
    target_domain: Option<oblivion_one::effects::EffectRect>,
    output_size: (u32, u32),
) -> Vec<OutputRect> {
    let full_output = oblivion_one::effects::EffectRect::new(0, 0, output_size.0, output_size.1)
        .expect("renderer dimensions are valid");
    let rects = if damage.is_empty() || damage.bounding_rect().is_none() {
        std::slice::from_ref(&full_output)
    } else {
        damage.rects()
    };
    rects
        .iter()
        .filter_map(|rect| target_domain.map_or(Some(*rect), |domain| rect.intersect(domain)))
        .map(effect_rect_to_output_rect)
        .collect()
}

pub(super) fn effect_rect_to_output_rect(rect: oblivion_one::effects::EffectRect) -> OutputRect {
    OutputRect::new(rect.x.max(0), rect.y.max(0), rect.width, rect.height)
}

pub(super) fn draw_damage_scissors(
    gl: &glow::Context,
    damage: &EffectRegion,
    target: &oblivion_one::effects::GraphTexturePlan,
    framebuffer_origin: OutputFramebufferOrigin,
    presentation_clip: Option<super::super::super::geometry::EglRect>,
) {
    let mut rects = effect_damage_to_texture_rects(damage, target, framebuffer_origin);
    if let Some(clip) = presentation_clip {
        let clip = output_rect_for_egl_clip(clip);
        rects = clip.map_or_else(Vec::new, |clip| {
            rects
                .into_iter()
                .filter_map(|damage| intersect_output_rect(damage, clip))
                .collect()
        });
    }
    unsafe {
        gl.enable(glow::SCISSOR_TEST);
        for rect in rects {
            gl.scissor(rect.x, rect.y, rect.width as i32, rect.height as i32);
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
        }
    }
}

pub(super) fn effect_damage_to_texture_rects(
    damage: &EffectRegion,
    target: &oblivion_one::effects::GraphTexturePlan,
    framebuffer_origin: OutputFramebufferOrigin,
) -> Vec<OutputRect> {
    if damage.is_empty() {
        return Vec::new();
    }
    let Some(_) = damage.bounding_rect() else {
        return vec![full_output_rect((target.width, target.height))];
    };
    damage
        .rects()
        .iter()
        .filter_map(|rect| effect_rect_to_texture_rect(*rect, target, framebuffer_origin))
        .collect()
}

#[cfg(test)]
pub(super) fn capture_clear_rects(
    damage: &EffectRegion,
    target: &oblivion_one::effects::GraphTexturePlan,
) -> Vec<OutputRect> {
    effect_damage_to_texture_rects(damage, target, OutputFramebufferOrigin::BottomLeft)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CaptureMaterializationPlan {
    pub(super) region: EffectRegion,
    pub(super) output_rects: Vec<OutputRect>,
}

impl CaptureMaterializationPlan {
    pub(super) fn texture_rects(
        &self,
        target: &oblivion_one::effects::GraphTexturePlan,
    ) -> Vec<OutputRect> {
        effect_damage_to_texture_rects(&self.region, target, OutputFramebufferOrigin::BottomLeft)
    }
}

pub(super) fn capture_materialization_plan(
    execution_damage: &EffectRegion,
    target_domain: Option<oblivion_one::effects::EffectRect>,
    output_size: (u32, u32),
) -> CaptureMaterializationPlan {
    let region = target_domain.map_or_else(
        || execution_damage.clone(),
        |domain| execution_damage.intersect_rect(domain),
    );
    let output_rects = effect_capture_output_rects(&region, target_domain, output_size);
    CaptureMaterializationPlan {
        region,
        output_rects,
    }
}

pub(super) fn materialized_target_rects(
    direct_capture: bool,
    persistent_checkpoint: bool,
    materialization: Option<&CaptureMaterializationPlan>,
    output_rects: &[OutputRect],
    target: &oblivion_one::effects::GraphTexturePlan,
) -> Vec<OutputRect> {
    if direct_capture && !persistent_checkpoint {
        vec![full_output_rect((target.width, target.height))]
    } else if let Some(materialization) = materialization {
        materialization.texture_rects(target)
    } else {
        output_rects.to_vec()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CheckpointCausalUnprovenReason {
    NoCacheBaseline,
    ScenePrefixChanged,
    DependencyChanged,
    UnsupportedTopology,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CheckpointCaptureCausalStability {
    pub(crate) ordinary_prefix_unchanged: bool,
    pub(crate) dependency_outputs_unchanged: bool,
    pub(crate) source_unchanged: bool,
    pub(crate) unproven_reason: Option<CheckpointCausalUnprovenReason>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EffectInstanceCausalStability {
    pub(crate) ordinary_prefix_unchanged: bool,
    pub(crate) dependency_outputs_unchanged: bool,
    pub(crate) source_unchanged: bool,
    pub(crate) output_unchanged: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CheckpointCausalStabilityPlan {
    pub(crate) captures: std::collections::HashMap<GraphPassId, CheckpointCaptureCausalStability>,
    pub(crate) instances:
        std::collections::HashMap<EffectInstanceId, EffectInstanceCausalStability>,
}
