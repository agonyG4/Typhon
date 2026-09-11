#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct EffectResourceMetrics {
    pub current_bytes: u64,
    pub peak_bytes: u64,
    pub budget_bytes: u64,
    pub cached_key_count: usize,
    pub cached_texture_count: usize,
    pub checked_out_texture_count: usize,
    pub eviction_count: usize,
    pub allocation_count: usize,
    pub reuse_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EffectFailureReason {
    GraphCompile,
    ResourceAllocation,
    ShaderUnavailable,
    Execution,
}

impl EffectFailureReason {
    pub(crate) fn from_error(error: &dyn std::error::Error) -> Self {
        let description = error.to_string();
        if description.contains("shader") {
            Self::ShaderUnavailable
        } else if description.contains("BudgetExceeded")
            || description.contains("framebuffer")
            || description.contains("texture")
        {
            Self::ResourceAllocation
        } else {
            Self::Execution
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::GraphCompile => "graph-compile",
            Self::ResourceAllocation => "resource-allocation",
            Self::ShaderUnavailable => "shader-unavailable",
            Self::Execution => "execution",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct EffectGraphMetrics {
    pub instances: usize,
    pub passes: usize,
    pub textures: usize,
    pub peak_live_textures: usize,
    pub peak_live_bytes: u64,
    pub capture_pixels: u64,
    pub output_pixels: u64,
}

pub(crate) fn graph_metrics(
    graph: &oblivion_one::effects::CompiledFrameGraph,
) -> EffectGraphMetrics {
    let capture_pixels = graph
        .textures
        .iter()
        .filter(|texture| {
            matches!(
                texture.source,
                oblivion_one::effects::GraphTextureSource::CapturedScene
                    | oblivion_one::effects::GraphTextureSource::CapturedTarget
            )
        })
        .map(|texture| u64::from(texture.width).saturating_mul(u64::from(texture.height)))
        .sum();
    let output_pixels = graph.final_damage.rects().iter().fold(0u64, |total, rect| {
        total.saturating_add(u64::from(rect.width).saturating_mul(u64::from(rect.height)))
    });
    EffectGraphMetrics {
        instances: graph.stats.effect_instances,
        passes: graph.stats.passes,
        textures: graph.stats.textures,
        peak_live_textures: graph.stats.peak_live_intermediates,
        peak_live_bytes: super::resources::estimate_graph_peak_bytes(graph).unwrap_or_default(),
        capture_pixels,
        output_pixels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_reasons_have_stable_wire_names() {
        assert_eq!(EffectFailureReason::GraphCompile.as_str(), "graph-compile");
        assert_eq!(
            EffectFailureReason::ResourceAllocation.as_str(),
            "resource-allocation"
        );
        assert_eq!(
            EffectFailureReason::ShaderUnavailable.as_str(),
            "shader-unavailable"
        );
        assert_eq!(EffectFailureReason::Execution.as_str(), "execution");
    }
}
