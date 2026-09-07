use std::io;

use oblivion_one::effects::CompiledFrameGraph;

mod metrics;
mod resources;
mod shader_cache;

pub(crate) use resources::EffectGlResourceCache;
pub(crate) use shader_cache::ShaderProgramCache;

pub(crate) fn execute_semantic_graph(graph: &CompiledFrameGraph) -> super::RendererResult<()> {
    if graph.passes.is_empty() {
        return Err(io::Error::other("effect graph contains no render passes").into());
    }
    Ok(())
}
