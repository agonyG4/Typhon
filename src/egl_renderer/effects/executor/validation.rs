use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum EffectExecutionInvariantError {
    MissingPassOutput(GraphPassId),
    UnknownTexture(GraphTextureId),
    UninitializedInputRegion {
        consumer: GraphPassId,
        input: GraphTextureId,
        missing: EffectRegion,
    },
    InvalidCheckpointSource {
        pass: GraphPassId,
        missing: EffectRegion,
    },
    SampledOutputTexture(GraphTextureId),
    MissingTextureResource(GraphTextureId),
    FeedbackTextureAlias {
        input: u64,
        output: u64,
    },
    InvalidTextureDimensions(GraphTextureId),
    InvalidTextureDomain(GraphTextureId),
    CaptureDomainOutsideOutput(GraphTextureId),
    InvalidScissor(GraphPassId),
    InvalidDomainMapping(GraphTextureId),
    InvalidFramebufferBlitTargets,
    PendingSceneCheckpointRequirements(Vec<GraphPassId>),
}

impl std::fmt::Display for EffectExecutionInvariantError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for EffectExecutionInvariantError {}

pub(super) fn validate_effect_pass_resources(
    renderer: &EffectExecutionContext<'_>,
    graph: &CompiledFrameGraph,
    pass: &CompiledRenderPass,
    textures: &std::collections::HashMap<GraphTextureId, GraphTextureBinding>,
    execution_damage: &EffectRegion,
    framebuffer_origin: OutputFramebufferOrigin,
) -> Result<(), EffectExecutionInvariantError> {
    let output = pass
        .output
        .ok_or(EffectExecutionInvariantError::MissingPassOutput(pass.id))?;
    let output_plan = graph
        .textures
        .iter()
        .find(|texture| texture.id == output)
        .ok_or(EffectExecutionInvariantError::UnknownTexture(output))?;
    validate_graph_texture_plan(renderer, output_plan)?;
    validate_domain_mapping(output_plan)?;

    let output_physical =
        if output_plan.source == GraphTextureSource::Output {
            None
        } else {
            let texture = textures.get(&output).ok_or(
                EffectExecutionInvariantError::MissingTextureResource(output),
            )?;
            Some(
                renderer
                    .runtime
                    .effect_resources
                    .physical_texture_id(texture)
                    .ok_or(EffectExecutionInvariantError::MissingTextureResource(
                        output,
                    ))?,
            )
        };

    for input in &pass.inputs {
        let input_plan = graph
            .textures
            .iter()
            .find(|texture| texture.id == *input)
            .ok_or(EffectExecutionInvariantError::UnknownTexture(*input))?;
        validate_graph_texture_plan(renderer, input_plan)?;
        validate_domain_mapping(input_plan)?;
        if input_plan.source == GraphTextureSource::Output {
            return Err(EffectExecutionInvariantError::SampledOutputTexture(*input));
        }
        let input_texture =
            textures
                .get(input)
                .ok_or(EffectExecutionInvariantError::MissingTextureResource(
                    *input,
                ))?;
        let input_physical = renderer
            .runtime
            .effect_resources
            .physical_texture_id(input_texture)
            .ok_or(EffectExecutionInvariantError::MissingTextureResource(
                *input,
            ))?;
        validate_no_texture_feedback(output_physical, std::iter::once(input_physical))?;
    }

    if matches!(
        pass.kind,
        RenderPassKind::DualKawaseDownsample | RenderPassKind::DualKawaseUpsample
    ) {
        let input = pass
            .inputs
            .first()
            .and_then(|input| graph.textures.iter().find(|texture| texture.id == *input))
            .ok_or(EffectExecutionInvariantError::UnknownTexture(
                pass.inputs.first().copied().unwrap_or(output),
            ))?;
        if input.width == 0
            || input.height == 0
            || output_plan.width == 0
            || output_plan.height == 0
        {
            return Err(EffectExecutionInvariantError::InvalidTextureDimensions(
                output,
            ));
        }
    }

    if !execution_damage.is_empty() {
        if let Some(bounding_box) = execution_damage.bounding_rect()
            && effect_rect_to_texture_rect(bounding_box, output_plan, framebuffer_origin).is_none()
        {
            return Err(EffectExecutionInvariantError::InvalidScissor(pass.id));
        }
        for rect in execution_damage.rects() {
            let Some(scissor) = effect_rect_to_texture_rect(*rect, output_plan, framebuffer_origin)
            else {
                continue;
            };
            let right = i64::from(scissor.x).saturating_add(i64::from(scissor.width));
            let bottom = i64::from(scissor.y).saturating_add(i64::from(scissor.height));
            if scissor.x < 0
                || scissor.y < 0
                || scissor.width == 0
                || scissor.height == 0
                || right > i64::from(output_plan.width)
                || bottom > i64::from(output_plan.height)
            {
                return Err(EffectExecutionInvariantError::InvalidScissor(pass.id));
            }
        }
    }

    Ok(())
}

#[cfg(any(debug_assertions, test))]
pub(super) fn validate_current_frame_input_regions(
    graph: &CompiledFrameGraph,
    pass: &CompiledRenderPass,
    execution_damage: &EffectRegion,
    valid_regions: &std::collections::HashMap<GraphTextureId, EffectRegion>,
) -> Result<(), EffectExecutionInvariantError> {
    let output = pass
        .output
        .ok_or(EffectExecutionInvariantError::MissingPassOutput(pass.id))?;
    let output_plan = graph
        .textures
        .iter()
        .find(|texture| texture.id == output)
        .ok_or(EffectExecutionInvariantError::UnknownTexture(output))?;

    for input_id in &pass.inputs {
        let input_plan = graph
            .textures
            .iter()
            .find(|texture| texture.id == *input_id)
            .ok_or(EffectExecutionInvariantError::UnknownTexture(*input_id))?;
        if matches!(input_plan.source, GraphTextureSource::Static(_)) {
            continue;
        }
        let required = oblivion_one::effects::required_input_region(
            pass,
            execution_damage,
            output_plan,
            input_plan,
        )
        .ok_or(EffectExecutionInvariantError::UninitializedInputRegion {
            consumer: pass.id,
            input: *input_id,
            missing: EffectRegion::from_rect(input_plan.domain),
        })?;
        let valid = valid_regions
            .get(input_id)
            .cloned()
            .unwrap_or_else(EffectRegion::empty);
        let missing = required.subtract(&valid);
        if !missing.is_empty() {
            return Err(EffectExecutionInvariantError::UninitializedInputRegion {
                consumer: pass.id,
                input: *input_id,
                missing,
            });
        }
    }
    Ok(())
}

#[cfg(any(debug_assertions, test))]
pub(super) fn record_current_frame_output_region(
    renderer: &EffectExecutionContext<'_>,
    graph: &CompiledFrameGraph,
    pass: &CompiledRenderPass,
    execution_damage: &EffectRegion,
    scene_baseline_authority: SceneBaselineAuthority,
    debug_config: EffectDebugConfig,
) -> Result<EffectRegion, EffectExecutionInvariantError> {
    let output = pass
        .output
        .ok_or(EffectExecutionInvariantError::MissingPassOutput(pass.id))?;
    let output_plan = graph
        .textures
        .iter()
        .find(|texture| texture.id == output)
        .ok_or(EffectExecutionInvariantError::UnknownTexture(output))?;
    if matches!(
        pass.kind,
        RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
    ) {
        if is_direct_framebuffer_capture(pass, scene_baseline_authority, debug_config) {
            return Ok(EffectRegion::from_rect(output_plan.domain));
        }
        return Ok(capture_materialization_plan(
            execution_damage,
            Some(output_plan.domain),
            renderer.scene.current_size,
        )
        .region);
    }
    Ok(execution_damage.intersect_rect(output_plan.domain))
}

pub(super) fn validate_domain_mapping(
    texture: &oblivion_one::effects::GraphTexturePlan,
) -> Result<(), EffectExecutionInvariantError> {
    let domain_width = i128::from(texture.domain.width);
    let domain_height = i128::from(texture.domain.height);
    if domain_width
        .checked_mul(i128::from(texture.width))
        .is_none()
        || domain_height
            .checked_mul(i128::from(texture.height))
            .is_none()
        || domain_width * i128::from(texture.width) > i128::from(i64::MAX)
        || domain_height * i128::from(texture.height) > i128::from(i64::MAX)
    {
        return Err(EffectExecutionInvariantError::InvalidDomainMapping(
            texture.id,
        ));
    }
    Ok(())
}

pub(super) fn validate_no_texture_feedback(
    output_physical: Option<u64>,
    input_physical_ids: impl Iterator<Item = u64>,
) -> Result<(), EffectExecutionInvariantError> {
    let Some(output_physical) = output_physical else {
        return Ok(());
    };
    for input_physical in input_physical_ids {
        if input_physical == output_physical {
            return Err(EffectExecutionInvariantError::FeedbackTextureAlias {
                input: input_physical,
                output: output_physical,
            });
        }
    }
    Ok(())
}

pub(super) fn validate_graph_texture_plan(
    renderer: &EffectExecutionContext<'_>,
    texture: &oblivion_one::effects::GraphTexturePlan,
) -> Result<(), EffectExecutionInvariantError> {
    let domain = texture.domain;
    if texture.width == 0 || texture.height == 0 || domain.width == 0 || domain.height == 0 {
        return Err(EffectExecutionInvariantError::InvalidTextureDimensions(
            texture.id,
        ));
    }
    let Some(domain_right) = i64::from(domain.x).checked_add(i64::from(domain.width)) else {
        return Err(EffectExecutionInvariantError::InvalidTextureDomain(
            texture.id,
        ));
    };
    let Some(domain_bottom) = i64::from(domain.y).checked_add(i64::from(domain.height)) else {
        return Err(EffectExecutionInvariantError::InvalidTextureDomain(
            texture.id,
        ));
    };
    if domain_right > i64::from(i32::MAX) || domain_bottom > i64::from(i32::MAX) {
        return Err(EffectExecutionInvariantError::InvalidTextureDomain(
            texture.id,
        ));
    }
    if matches!(
        texture.source,
        GraphTextureSource::CapturedScene
            | GraphTextureSource::CapturedTarget
            | GraphTextureSource::Output
    ) && (domain.x < 0
        || domain.y < 0
        || i64::from(domain.right()) > i64::from(renderer.scene.current_size.0)
        || i64::from(domain.bottom()) > i64::from(renderer.scene.current_size.1))
    {
        return Err(EffectExecutionInvariantError::CaptureDomainOutsideOutput(
            texture.id,
        ));
    }
    Ok(())
}
