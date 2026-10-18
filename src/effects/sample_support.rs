use super::render_graph::{
    CompiledFrameGraph, GraphPassId, GraphTextureId, GraphTextureSource, RenderPassKind,
    required_input_region,
};
use super::{EffectInstanceId, EffectRegion};

/// Pass-aware logical source support for one compiled effect instance.
///
/// Regions retain their `EffectRegion` representation so disjoint support is
/// not reduced to a bounding rectangle for semantic decisions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectSampleSupportEnvelope {
    pub instance_id: EffectInstanceId,
    pub presentation_region: EffectRegion,
    pub backdrop_region: EffectRegion,
    pub target_region: EffectRegion,
    /// Conservative unions that coalesced while accumulating pass/source
    /// demand. Lower-level mapping coalescing is not exposed by
    /// `required_input_region`.
    pub known_representation_coalesces: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectSampleSupportError {
    InstanceMissing(EffectInstanceId),
    InstanceAmbiguous(EffectInstanceId),
    FinalPassMissing(EffectInstanceId),
    FinalPassAmbiguous(EffectInstanceId),
    PassOutputMissing(GraphPassId),
    PassInputsMissing(GraphPassId),
    TextureMissing(GraphTextureId),
    TextureLookupAmbiguous(GraphTextureId),
    TextureProducerMissing(GraphTextureId),
    TextureProducerAmbiguous(GraphTextureId),
    ProducerOrderingInvalid {
        consumer: GraphPassId,
        producer: GraphPassId,
    },
    CrossInstanceProducer {
        consumer_instance: EffectInstanceId,
        producer_instance: EffectInstanceId,
        texture: GraphTextureId,
    },
    ProducerKindMismatch {
        pass: GraphPassId,
        texture: GraphTextureId,
    },
    RequiredInputRegionUnavailable {
        pass: GraphPassId,
        input: GraphTextureId,
    },
    RegionNotRepresentable {
        pass: Option<GraphPassId>,
    },
    RegionOutsideTextureDomain {
        pass: GraphPassId,
    },
    UnexpectedExternalSource {
        pass: GraphPassId,
        texture: GraphTextureId,
        source: GraphTextureSource,
    },
    ExternalPrecomputedBackdrop {
        pass: GraphPassId,
        texture: GraphTextureId,
    },
    SampledBackdropOutsideAggregateCapture {
        outside_capture: EffectRegion,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectSampleSupportInstanceResult {
    pub instance_id: EffectInstanceId,
    pub result: Result<EffectSampleSupportEnvelope, EffectSampleSupportError>,
}

/// Analysis results for an exact compiled graph, including diagnostic-only
/// checkpoint pruning candidates.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EffectSampleSupportAnalysis {
    pub instances: Vec<EffectSampleSupportInstanceResult>,
    pub aggregate_checkpoint_dependency_count: usize,
    pub sample_support_checkpoint_dependency_count: usize,
    pub prunable_checkpoint_candidates: usize,
}

/// Derive complete sampled source support for one ordinary compiled instance.
/// This function is independent of frame damage and demand planner policy.
pub fn derive_effect_sample_support(
    graph: &CompiledFrameGraph,
    instance_id: EffectInstanceId,
) -> Result<EffectSampleSupportEnvelope, EffectSampleSupportError> {
    let mut instance_index = None;
    for (index, instance) in graph.instances.iter().enumerate() {
        if instance.id != instance_id {
            continue;
        }
        if instance_index.replace(index).is_some() {
            return Err(EffectSampleSupportError::InstanceAmbiguous(instance_id));
        }
    }
    let Some(instance) = instance_index.and_then(|index| graph.instances.get(index)) else {
        return Err(EffectSampleSupportError::InstanceMissing(instance_id));
    };

    let mut final_pass_index = None;
    for (index, pass) in graph.passes.iter().enumerate() {
        if pass.instance != instance_id
            || !matches!(
                pass.kind,
                RenderPassKind::Composite | RenderPassKind::OutputPostProcess
            )
        {
            continue;
        }
        if final_pass_index.replace(index).is_some() {
            return Err(EffectSampleSupportError::FinalPassAmbiguous(instance_id));
        }
    }
    let final_pass_index =
        final_pass_index.ok_or(EffectSampleSupportError::FinalPassMissing(instance_id))?;
    let final_pass = &graph.passes[final_pass_index];
    let final_output_id = final_pass
        .output
        .ok_or(EffectSampleSupportError::PassOutputMissing(final_pass.id))?;
    let final_output_index = unique_texture_index(graph, final_output_id)?;
    let final_output = &graph.textures[final_output_index];
    if final_output.source != GraphTextureSource::Output {
        return Err(EffectSampleSupportError::UnexpectedExternalSource {
            pass: final_pass.id,
            texture: final_output.id,
            source: final_output.source,
        });
    }

    let presentation_region = instance
        .output_influence_region
        .intersect_rect(final_output.domain);
    ensure_region_representable(&presentation_region, None)?;
    ensure_region_within_domain(&presentation_region, final_output.domain, final_pass.id)?;

    let mut pass_demands = vec![EffectRegion::empty(); graph.passes.len()];
    pass_demands[final_pass_index] = presentation_region.clone();
    let mut backdrop_region = EffectRegion::empty();
    let mut target_region = EffectRegion::empty();
    let mut known_representation_coalesces = 0usize;

    // `required_input_region` maps every actual input independently. The
    // reverse pass walk only resolves the resulting logical region to its
    // ordinary producer; checkpoint dependencies are deliberately ignored.
    for pass_index in (0..graph.passes.len()).rev() {
        let demanded_output = pass_demands[pass_index].clone();
        if demanded_output.is_empty() {
            continue;
        }
        let pass = &graph.passes[pass_index];
        let output_id = pass
            .output
            .ok_or(EffectSampleSupportError::PassOutputMissing(pass.id))?;
        let output = &graph.textures[unique_texture_index(graph, output_id)?];
        ensure_region_representable(&demanded_output, Some(pass.id))?;
        ensure_region_within_domain(&demanded_output, output.domain, pass.id)?;

        match pass.kind {
            RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture => {
                let expected_source = if pass.kind == RenderPassKind::SceneCapture {
                    GraphTextureSource::CapturedScene
                } else {
                    GraphTextureSource::CapturedTarget
                };
                if output.source != expected_source {
                    return Err(EffectSampleSupportError::ProducerKindMismatch {
                        pass: pass.id,
                        texture: output.id,
                    });
                }
                let accumulated = if pass.kind == RenderPassKind::SceneCapture {
                    &mut backdrop_region
                } else {
                    &mut target_region
                };
                let (union, coalesced) = accumulated.union_with_diagnostics(&demanded_output);
                *accumulated = union;
                known_representation_coalesces += usize::from(coalesced);
                ensure_region_representable(accumulated, Some(pass.id))?;
                continue;
            }
            _ => {}
        }

        if pass.inputs.is_empty() {
            return Err(EffectSampleSupportError::PassInputsMissing(pass.id));
        }
        for input_id in &pass.inputs {
            let input = &graph.textures[unique_texture_index(graph, *input_id)?];
            let required = required_input_region(pass, &demanded_output, output, input).ok_or(
                EffectSampleSupportError::RequiredInputRegionUnavailable {
                    pass: pass.id,
                    input: input.id,
                },
            )?;
            ensure_region_representable(&required, Some(pass.id))?;
            ensure_region_within_domain(&required, input.domain, pass.id)?;

            match input.source {
                GraphTextureSource::CapturedScene
                | GraphTextureSource::CapturedTarget
                | GraphTextureSource::Intermediate => {
                    let producer_index = unique_producer_index(graph, input.id)?;
                    let producer = &graph.passes[producer_index];
                    if producer_index >= pass_index {
                        return Err(EffectSampleSupportError::ProducerOrderingInvalid {
                            consumer: pass.id,
                            producer: producer.id,
                        });
                    }
                    if producer.instance != instance_id {
                        return Err(EffectSampleSupportError::CrossInstanceProducer {
                            consumer_instance: instance_id,
                            producer_instance: producer.instance,
                            texture: input.id,
                        });
                    }
                    if (input.source == GraphTextureSource::CapturedScene
                        && producer.kind != RenderPassKind::SceneCapture)
                        || (input.source == GraphTextureSource::CapturedTarget
                            && producer.kind != RenderPassKind::SurfaceCapture)
                        || (input.source == GraphTextureSource::Intermediate
                            && matches!(
                                producer.kind,
                                RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
                            ))
                    {
                        return Err(EffectSampleSupportError::ProducerKindMismatch {
                            pass: producer.id,
                            texture: input.id,
                        });
                    }
                    let (union, coalesced) =
                        pass_demands[producer_index].union_with_diagnostics(&required);
                    pass_demands[producer_index] = union;
                    known_representation_coalesces += usize::from(coalesced);
                    ensure_region_representable(&pass_demands[producer_index], Some(producer.id))?;
                }
                GraphTextureSource::PrecomputedBackdrop => {
                    return Err(EffectSampleSupportError::ExternalPrecomputedBackdrop {
                        pass: pass.id,
                        texture: input.id,
                    });
                }
                source @ (GraphTextureSource::Output | GraphTextureSource::Static(_)) => {
                    return Err(EffectSampleSupportError::UnexpectedExternalSource {
                        pass: pass.id,
                        texture: input.id,
                        source,
                    });
                }
            }
        }
    }

    let outside_capture = backdrop_region.subtract(&instance.capture_region);
    if !outside_capture.is_empty() {
        return Err(
            EffectSampleSupportError::SampledBackdropOutsideAggregateCapture { outside_capture },
        );
    }

    Ok(EffectSampleSupportEnvelope {
        instance_id,
        presentation_region,
        backdrop_region,
        target_region,
        known_representation_coalesces,
    })
}

/// Analyze every compiled instance and compare existing aggregate-capture
/// checkpoint edges with the derived backdrop support. This is diagnostic
/// metadata only; it does not alter the graph or its dependencies.
pub fn analyze_effect_sample_support(graph: &CompiledFrameGraph) -> EffectSampleSupportAnalysis {
    let instances = graph
        .instances
        .iter()
        .map(|instance| EffectSampleSupportInstanceResult {
            instance_id: instance.id,
            result: derive_effect_sample_support(graph, instance.id),
        })
        .collect::<Vec<_>>();

    let mut analysis = EffectSampleSupportAnalysis {
        instances,
        ..EffectSampleSupportAnalysis::default()
    };
    for later in &graph.instances {
        for earlier_id in &later.dependencies {
            analysis.aggregate_checkpoint_dependency_count += 1;
            let Some(earlier) = graph
                .instances
                .iter()
                .find(|instance| instance.id == *earlier_id)
            else {
                continue;
            };
            if !earlier
                .output_influence_region
                .intersects(&later.capture_region)
            {
                continue;
            }
            let Some(later_support) = analysis
                .instances
                .iter()
                .find(|entry| entry.instance_id == later.id)
                .and_then(|entry| entry.result.as_ref().ok())
            else {
                continue;
            };
            if earlier
                .output_influence_region
                .intersects(&later_support.backdrop_region)
            {
                analysis.sample_support_checkpoint_dependency_count += 1;
            } else {
                analysis.prunable_checkpoint_candidates += 1;
            }
        }
    }
    analysis
}

fn ensure_region_representable(
    region: &EffectRegion,
    pass: Option<GraphPassId>,
) -> Result<(), EffectSampleSupportError> {
    if !region.is_empty() && region.bounding_rect().is_none() {
        return Err(EffectSampleSupportError::RegionNotRepresentable { pass });
    }
    Ok(())
}

fn ensure_region_within_domain(
    region: &EffectRegion,
    domain: super::damage::EffectRect,
    pass: GraphPassId,
) -> Result<(), EffectSampleSupportError> {
    if region.subtract(&EffectRegion::from_rect(domain)).is_empty() {
        Ok(())
    } else {
        Err(EffectSampleSupportError::RegionOutsideTextureDomain { pass })
    }
}

fn unique_texture_index(
    graph: &CompiledFrameGraph,
    id: GraphTextureId,
) -> Result<usize, EffectSampleSupportError> {
    let mut result = None;
    for (index, texture) in graph.textures.iter().enumerate() {
        if texture.id != id {
            continue;
        }
        if result.replace(index).is_some() {
            return Err(EffectSampleSupportError::TextureLookupAmbiguous(id));
        }
    }
    result.ok_or(EffectSampleSupportError::TextureMissing(id))
}

fn unique_producer_index(
    graph: &CompiledFrameGraph,
    texture: GraphTextureId,
) -> Result<usize, EffectSampleSupportError> {
    let mut result = None;
    for (index, pass) in graph.passes.iter().enumerate() {
        if pass.output != Some(texture) {
            continue;
        }
        if result.replace(index).is_some() {
            return Err(EffectSampleSupportError::TextureProducerAmbiguous(texture));
        }
    }
    result.ok_or(EffectSampleSupportError::TextureProducerMissing(texture))
}
