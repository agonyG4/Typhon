use std::{
    collections::{HashMap, VecDeque, hash_map::Entry},
    io,
    ops::Deref,
};

use glow::HasContext;
use oblivion_one::compositor::{EffectAnchorScope, VisualGroupId};
use oblivion_one::effects::{
    CompiledFrameGraph, EffectInstanceId, EffectWorkingSpace, GraphTextureId, GraphTexturePlan,
    GraphTextureSource, RenderPassKind,
};

use super::super::CheckpointCausalState;
use super::metrics::EffectResourceMetrics;

pub const DEFAULT_EFFECT_RESOURCE_BUDGET_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EffectTextureFormat {
    Rgba8,
    Rgba16Float,
}

impl EffectTextureFormat {
    const fn bytes_per_pixel(self) -> u64 {
        match self {
            Self::Rgba8 => 4,
            Self::Rgba16Float => 8,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EffectTextureFilter {
    Nearest,
    Linear,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EffectTextureKey {
    pub width: u32,
    pub height: u32,
    pub format: EffectTextureFormat,
    pub filter: EffectTextureFilter,
    pub working_space: EffectWorkingSpace,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CheckpointDependencySemanticIdentity {
    instance: EffectInstanceId,
    semantic_signature: u64,
    composition: CheckpointCompositionIdentity,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum CheckpointAnchorIdentity {
    BeforeSurface(u32),
    ReplaceSurface(u32),
    AfterSurface(u32),
    OutputPostProcess,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct CheckpointCompositionIdentity {
    anchor: CheckpointAnchorIdentity,
    anchor_scope: EffectAnchorScope,
    visual_group: Option<VisualGroupId>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct CheckpointCaptureCacheKey {
    consumer: EffectInstanceId,
    consumer_semantic_signature: u64,
    consumer_composition: CheckpointCompositionIdentity,
    capture_domain: (i32, i32, u32, u32),
    width: u32,
    height: u32,
    format: EffectTextureFormat,
    working_space: EffectWorkingSpace,
    dependencies: Vec<CheckpointDependencySemanticIdentity>,
}

fn checkpoint_composition_identity(
    pass: &oblivion_one::effects::CompiledRenderPass,
) -> CheckpointCompositionIdentity {
    let anchor = match pass.anchor {
        oblivion_one::compositor::EffectAnchor::BeforeSurface(surface) => {
            CheckpointAnchorIdentity::BeforeSurface(surface)
        }
        oblivion_one::compositor::EffectAnchor::ReplaceSurface(surface) => {
            CheckpointAnchorIdentity::ReplaceSurface(surface)
        }
        oblivion_one::compositor::EffectAnchor::AfterSurface(surface) => {
            CheckpointAnchorIdentity::AfterSurface(surface)
        }
        oblivion_one::compositor::EffectAnchor::OutputPostProcess => {
            CheckpointAnchorIdentity::OutputPostProcess
        }
    };
    let visual_group = match pass.anchor_scope {
        EffectAnchorScope::Surface => None,
        EffectAnchorScope::VisualGroup => pass.visual_group,
    };
    CheckpointCompositionIdentity {
        anchor,
        anchor_scope: pass.anchor_scope,
        visual_group,
    }
}

/// Build a cross-frame identity from compositor-owned effect semantics and
/// capture texture layout. Frame-local graph IDs are resolved to producer
/// instances and never become part of the returned key.
pub(crate) fn checkpoint_capture_cache_key(
    graph: &CompiledFrameGraph,
    pass: &oblivion_one::effects::CompiledRenderPass,
) -> Option<CheckpointCaptureCacheKey> {
    if pass.kind != RenderPassKind::SceneCapture || pass.checkpoint_dependencies.is_empty() {
        return None;
    }
    let consumer = graph
        .instances
        .iter()
        .find(|instance| instance.id == pass.instance)?;
    let output = pass.output?;
    let capture_texture = graph.textures.iter().find(|texture| texture.id == output)?;
    let capture_key = texture_key(capture_texture);
    let mut dependencies = Vec::with_capacity(pass.checkpoint_dependencies.len());
    for dependency in &pass.checkpoint_dependencies {
        let producer = graph
            .passes
            .iter()
            .find(|candidate| candidate.id == *dependency)?;
        let producer_instance = graph
            .instances
            .iter()
            .find(|instance| instance.id == producer.instance)?;
        dependencies.push(CheckpointDependencySemanticIdentity {
            instance: producer_instance.id,
            semantic_signature: producer_instance.semantic_signature,
            composition: checkpoint_composition_identity(producer),
        });
    }
    Some(CheckpointCaptureCacheKey {
        consumer: consumer.id,
        consumer_semantic_signature: consumer.semantic_signature,
        consumer_composition: checkpoint_composition_identity(pass),
        capture_domain: (
            capture_texture.domain.x,
            capture_texture.domain.y,
            capture_texture.domain.width,
            capture_texture.domain.height,
        ),
        width: capture_texture.width,
        height: capture_texture.height,
        format: capture_key.format,
        working_space: capture_texture.working_space,
        dependencies,
    })
}

impl EffectTextureKey {
    pub const fn new(
        width: u32,
        height: u32,
        format: EffectTextureFormat,
        filter: EffectTextureFilter,
        working_space: EffectWorkingSpace,
    ) -> Self {
        Self {
            width,
            height,
            format,
            filter,
            working_space,
        }
    }

    pub fn estimated_bytes(self) -> Result<u64, EffectResourceError> {
        if self.width == 0 || self.height == 0 {
            return Err(EffectResourceError::InvalidDimensions);
        }
        u64::from(self.width)
            .checked_mul(u64::from(self.height))
            .and_then(|pixels| pixels.checked_mul(self.format.bytes_per_pixel()))
            .ok_or(EffectResourceError::SizeOverflow)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectResourceError {
    InvalidBudget,
    InvalidDimensions,
    SizeOverflow,
    TextureIdOverflow,
    InvalidGraphLifetime,
    BudgetExceeded {
        requested_bytes: u64,
        current_bytes: u64,
        budget_bytes: u64,
    },
    UnknownTexture(u64),
    TextureAlreadyReturned(u64),
}

impl std::fmt::Display for EffectResourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for EffectResourceError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PooledEffectTexture {
    pub id: u64,
    pub key: EffectTextureKey,
    pub bytes: u64,
    last_used: u64,
    checked_out: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GraphTextureBindingOwnership {
    FrameTransient,
    CheckpointCache,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GraphTextureBinding {
    texture: PooledEffectTexture,
    ownership: GraphTextureBindingOwnership,
}

impl GraphTextureBinding {
    pub(crate) fn transient(texture: PooledEffectTexture) -> Self {
        Self {
            texture,
            ownership: GraphTextureBindingOwnership::FrameTransient,
        }
    }

    pub(crate) fn checkpoint_cache(texture: PooledEffectTexture) -> Self {
        Self {
            texture,
            ownership: GraphTextureBindingOwnership::CheckpointCache,
        }
    }

    fn into_transient(self) -> Option<PooledEffectTexture> {
        (self.ownership == GraphTextureBindingOwnership::FrameTransient).then_some(self.texture)
    }

    pub(crate) fn is_checkpoint_cache(&self) -> bool {
        self.ownership == GraphTextureBindingOwnership::CheckpointCache
    }
}

impl Deref for GraphTextureBinding {
    type Target = PooledEffectTexture;

    fn deref(&self) -> &Self::Target {
        &self.texture
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CheckpointCacheCompatibility {
    pub(crate) output_size: (u32, u32),
    pub(crate) framebuffer_origin_top_left: bool,
    pub(crate) effect_registry_generation: u64,
}

#[derive(Debug)]
struct CachedCheckpointCapture {
    texture: PooledEffectTexture,
    last_populated_frame_serial: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CheckpointCacheAdmissionStats {
    pub(crate) candidates_total: usize,
    pub(crate) candidates_considered: usize,
    pub(crate) resident_candidates: usize,
    pub(crate) newly_admitted_candidates: usize,
    pub(crate) skipped_entry_limit: usize,
    pub(crate) skipped_graph_peak_unknown: usize,
    pub(crate) skipped_graph_pressure: usize,
    pub(crate) skipped_budget: usize,
    pub(crate) skipped_size: usize,
    pub(crate) skipped_allocation: usize,
    pub(crate) skipped_budget_bytes: u64,
    pub(crate) graph_peak_known: bool,
    pub(crate) graph_peak_bytes: u64,
    pub(crate) base_checked_out_bytes: u64,
    pub(crate) budget_bytes: u64,
    pub(crate) additional_budget_needed_for_all_candidates_bytes: u64,
    pub(crate) additional_budget_needed_known: bool,
    pub(crate) cache_cleared_for_graph_pressure: bool,
}

pub(crate) struct PreparedCheckpointCaptures {
    pub(crate) bindings: HashMap<GraphTextureId, GraphTextureBinding>,
    pub(crate) admission: CheckpointCacheAdmissionStats,
}

#[derive(Debug)]
struct CheckpointCacheCausalBaseline {
    frame_serial: u64,
    state: CheckpointCausalState,
}

const MAX_CHECKPOINT_CACHE_ENTRIES: usize = 32;

#[derive(Debug)]
pub struct EffectResourcePool {
    textures: HashMap<EffectTextureKey, Vec<PooledEffectTexture>>,
    current_bytes: u64,
    peak_bytes: u64,
    budget_bytes: u64,
    next_id: u64,
    clock: u64,
    pending_evicted_ids: VecDeque<u64>,
    eviction_count: usize,
    allocation_count: usize,
    reuse_count: usize,
}

impl EffectResourcePool {
    pub fn new() -> Self {
        Self::with_budget(DEFAULT_EFFECT_RESOURCE_BUDGET_BYTES)
            .expect("stable default effect resource budget is non-zero")
    }

    pub fn with_budget(budget_bytes: u64) -> Result<Self, EffectResourceError> {
        if budget_bytes == 0 {
            return Err(EffectResourceError::InvalidBudget);
        }
        Ok(Self {
            textures: HashMap::new(),
            current_bytes: 0,
            peak_bytes: 0,
            budget_bytes,
            next_id: 1,
            clock: 0,
            pending_evicted_ids: VecDeque::new(),
            eviction_count: 0,
            allocation_count: 0,
            reuse_count: 0,
        })
    }

    pub fn checkout(
        &mut self,
        key: EffectTextureKey,
    ) -> Result<PooledEffectTexture, EffectResourceError> {
        let bytes = key.estimated_bytes()?;
        self.clock = self.clock.saturating_add(1);
        if let Some(texture) = self
            .textures
            .get_mut(&key)
            .and_then(|textures| textures.iter_mut().find(|texture| !texture.checked_out))
        {
            texture.checked_out = true;
            texture.last_used = self.clock;
            self.reuse_count = self.reuse_count.saturating_add(1);
            return Ok(texture.clone());
        }

        self.evict_until_fits(bytes);
        let new_current = self
            .current_bytes
            .checked_add(bytes)
            .ok_or(EffectResourceError::SizeOverflow)?;
        if new_current > self.budget_bytes {
            return Err(EffectResourceError::BudgetExceeded {
                requested_bytes: bytes,
                current_bytes: self.current_bytes,
                budget_bytes: self.budget_bytes,
            });
        }
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(EffectResourceError::TextureIdOverflow)?;
        let texture = PooledEffectTexture {
            id,
            key,
            bytes,
            last_used: self.clock,
            checked_out: true,
        };
        self.textures.entry(key).or_default().push(texture.clone());
        self.current_bytes = new_current;
        self.peak_bytes = self.peak_bytes.max(new_current);
        self.allocation_count = self.allocation_count.saturating_add(1);
        Ok(texture)
    }

    pub fn return_texture(
        &mut self,
        texture: PooledEffectTexture,
    ) -> Result<(), EffectResourceError> {
        let Some(cached) = self
            .textures
            .get_mut(&texture.key)
            .and_then(|textures| textures.iter_mut().find(|cached| cached.id == texture.id))
        else {
            return Err(EffectResourceError::UnknownTexture(texture.id));
        };
        if !cached.checked_out {
            return Err(EffectResourceError::TextureAlreadyReturned(texture.id));
        }
        cached.checked_out = false;
        cached.last_used = self.clock;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn cleanup_size_history(&mut self) -> Vec<u64> {
        let mut removed = Vec::new();
        self.textures.retain(|_, textures| {
            textures.retain(|texture| {
                if texture.checked_out {
                    true
                } else {
                    removed.push(texture.id);
                    false
                }
            });
            !textures.is_empty()
        });
        self.recalculate_current_bytes();
        removed
    }

    #[allow(dead_code)]
    pub fn budget_bytes(&self) -> u64 {
        self.budget_bytes
    }

    #[allow(dead_code)]
    pub fn current_bytes(&self) -> u64 {
        self.current_bytes
    }

    fn checked_out_bytes(&self) -> u64 {
        self.textures
            .values()
            .flatten()
            .filter(|texture| texture.checked_out)
            .map(|texture| texture.bytes)
            .fold(0, u64::saturating_add)
    }

    #[allow(dead_code)]
    pub fn peak_bytes(&self) -> u64 {
        self.peak_bytes
    }

    pub fn cached_key_count(&self) -> usize {
        self.textures.len()
    }

    #[allow(dead_code)]
    pub fn is_checked_out(&self, id: u64) -> bool {
        self.textures
            .values()
            .flatten()
            .any(|texture| texture.id == id && texture.checked_out)
    }

    #[allow(dead_code)]
    pub fn evicted_texture_ids(&self) -> Vec<u64> {
        self.pending_evicted_ids.iter().copied().collect()
    }

    pub fn drain_evicted_texture_ids(&mut self) -> Vec<u64> {
        self.pending_evicted_ids.drain(..).collect()
    }

    pub(crate) fn metrics(&self) -> EffectResourceMetrics {
        let cached_texture_count = self.textures.values().map(Vec::len).sum();
        let checked_out_texture_count = self
            .textures
            .values()
            .flatten()
            .filter(|texture| texture.checked_out)
            .count();
        EffectResourceMetrics {
            current_bytes: self.current_bytes,
            peak_bytes: self.peak_bytes,
            budget_bytes: self.budget_bytes,
            cached_key_count: self.cached_key_count(),
            cached_texture_count,
            checked_out_texture_count,
            eviction_count: self.eviction_count,
            allocation_count: self.allocation_count,
            reuse_count: self.reuse_count,
        }
    }

    fn evict_until_fits(&mut self, requested_bytes: u64) {
        while self.current_bytes.saturating_add(requested_bytes) > self.budget_bytes {
            let mut candidates = self
                .textures
                .values()
                .flatten()
                .filter(|texture| !texture.checked_out)
                .map(|texture| (texture.last_used, texture.id))
                .collect::<Vec<_>>();
            candidates.sort_unstable();
            let Some((_, id)) = candidates.first().copied() else {
                break;
            };
            if self.remove_texture(id) {
                self.pending_evicted_ids.push_back(id);
                self.eviction_count = self.eviction_count.saturating_add(1);
            } else {
                break;
            }
        }
    }

    fn remove_texture(&mut self, id: u64) -> bool {
        let mut removed = false;
        self.textures.retain(|_, textures| {
            let before = textures.len();
            textures.retain(|texture| texture.id != id);
            removed |= before != textures.len();
            !textures.is_empty()
        });
        if removed {
            self.recalculate_current_bytes();
        }
        removed
    }

    fn recalculate_current_bytes(&mut self) {
        self.current_bytes = self
            .textures
            .values()
            .flatten()
            .map(|texture| texture.bytes)
            .sum();
    }
}

impl Default for EffectResourcePool {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
pub(crate) struct EffectGlResourceCache {
    pool: EffectResourcePool,
    gl_textures: HashMap<u64, glow::Texture>,
    scratch_fbo: Option<glow::Framebuffer>,
    lifecycle_composition_fbo: Option<glow::Framebuffer>,
    checkpoint_captures: HashMap<CheckpointCaptureCacheKey, CachedCheckpointCapture>,
    checkpoint_compatibility: Option<CheckpointCacheCompatibility>,
    checkpoint_frame_serial: u64,
    checkpoint_causal_baseline: Option<CheckpointCacheCausalBaseline>,
}

#[allow(dead_code)]
impl EffectGlResourceCache {
    pub(crate) fn new() -> Self {
        Self {
            pool: EffectResourcePool::new(),
            gl_textures: HashMap::new(),
            scratch_fbo: None,
            lifecycle_composition_fbo: None,
            checkpoint_captures: HashMap::new(),
            checkpoint_compatibility: None,
            checkpoint_frame_serial: 0,
            checkpoint_causal_baseline: None,
        }
    }

    pub(crate) fn with_budget(budget_bytes: u64) -> Result<Self, EffectResourceError> {
        Ok(Self {
            pool: EffectResourcePool::with_budget(budget_bytes)?,
            gl_textures: HashMap::new(),
            scratch_fbo: None,
            lifecycle_composition_fbo: None,
            checkpoint_captures: HashMap::new(),
            checkpoint_compatibility: None,
            checkpoint_frame_serial: 0,
            checkpoint_causal_baseline: None,
        })
    }

    pub(crate) fn acquire(
        &mut self,
        gl: &glow::Context,
        key: EffectTextureKey,
    ) -> Result<PooledEffectTexture, Box<dyn std::error::Error>> {
        let texture = self.pool.checkout(key)?;
        for id in self.pool.drain_evicted_texture_ids() {
            if let Some(gl_texture) = self.gl_textures.remove(&id) {
                unsafe { gl.delete_texture(gl_texture) };
            }
        }
        if let Entry::Vacant(entry) = self.gl_textures.entry(texture.id) {
            let gl_texture = match unsafe { gl.create_texture() } {
                Ok(texture) => texture,
                Err(error) => {
                    let _ = self.pool.return_texture(texture);
                    return Err(io::Error::other(error).into());
                }
            };
            unsafe {
                gl.bind_texture(glow::TEXTURE_2D, Some(gl_texture));
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    match key.filter {
                        EffectTextureFilter::Nearest => glow::NEAREST as i32,
                        EffectTextureFilter::Linear => glow::LINEAR as i32,
                    },
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    match key.filter {
                        EffectTextureFilter::Nearest => glow::NEAREST as i32,
                        EffectTextureFilter::Linear => glow::LINEAR as i32,
                    },
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_S,
                    glow::CLAMP_TO_EDGE as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_T,
                    glow::CLAMP_TO_EDGE as i32,
                );
                let (internal_format, format, data_type) = match key.format {
                    EffectTextureFormat::Rgba8 => {
                        (glow::RGBA8 as i32, glow::RGBA, glow::UNSIGNED_BYTE)
                    }
                    EffectTextureFormat::Rgba16Float => {
                        (glow::RGBA16F as i32, glow::RGBA, glow::HALF_FLOAT)
                    }
                };
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    internal_format,
                    key.width as i32,
                    key.height as i32,
                    0,
                    format,
                    data_type,
                    glow::PixelUnpackData::Slice(None),
                );
            }
            entry.insert(gl_texture);
        }
        Ok(texture)
    }

    pub(crate) fn prepare_graph(
        &mut self,
        gl: &glow::Context,
        graph: &CompiledFrameGraph,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut live = HashMap::new();
        for pass in &graph.passes {
            for texture_id in pass.inputs.iter().copied().chain(pass.output) {
                if graph_texture_is_output(graph, texture_id) || live.contains_key(&texture_id) {
                    continue;
                }
                let texture = self.acquire_plan(gl, graph_texture_plan(graph, texture_id)?)?;
                live.insert(texture_id, GraphTextureBinding::transient(texture));
            }
            release_dead_graph_textures(self, graph, pass.id, &mut live)?;
        }
        self.release_graph(live)
    }

    pub(crate) fn acquire_plan(
        &mut self,
        gl: &glow::Context,
        plan: &GraphTexturePlan,
    ) -> Result<PooledEffectTexture, Box<dyn std::error::Error>> {
        self.acquire(gl, texture_key(plan))
    }

    pub(crate) fn begin_checkpoint_frame(&mut self) -> u64 {
        if self.checkpoint_frame_serial == u64::MAX {
            self.checkpoint_frame_serial = 1;
            self.invalidate_checkpoint_capture_contents();
        } else {
            self.checkpoint_frame_serial += 1;
        }
        self.checkpoint_frame_serial
    }

    pub(crate) fn checkpoint_frame_serial(&self) -> u64 {
        self.checkpoint_frame_serial
    }

    pub(crate) fn checkpoint_causal_state_for_frame(
        &self,
        frame_serial: u64,
    ) -> Option<&CheckpointCausalState> {
        self.checkpoint_causal_baseline
            .as_ref()
            .filter(|baseline| baseline.frame_serial.checked_add(1) == Some(frame_serial))
            .map(|baseline| &baseline.state)
    }

    pub(crate) fn promote_checkpoint_causal_state(
        &mut self,
        frame_serial: u64,
        state: CheckpointCausalState,
    ) -> bool {
        if frame_serial != self.checkpoint_frame_serial {
            self.invalidate_checkpoint_causal_state();
            return false;
        }
        self.checkpoint_causal_baseline = Some(CheckpointCacheCausalBaseline {
            frame_serial,
            state,
        });
        true
    }

    pub(crate) fn invalidate_checkpoint_causal_state(&mut self) {
        self.checkpoint_causal_baseline = None;
    }

    pub(crate) fn checkpoint_cache_stats(&self) -> (usize, u64) {
        let entries = self.checkpoint_captures.len();
        let bytes = self
            .checkpoint_captures
            .values()
            .map(|capture| capture.texture.bytes)
            .fold(0, u64::saturating_add);
        (entries, bytes)
    }

    pub(crate) fn prepare_checkpoint_captures(
        &mut self,
        gl: &glow::Context,
        compatibility: CheckpointCacheCompatibility,
        graph_peak_bytes: Option<u64>,
        candidates: &[(CheckpointCaptureCacheKey, GraphTexturePlan)],
    ) -> PreparedCheckpointCaptures {
        let candidates_considered = candidates.len().min(MAX_CHECKPOINT_CACHE_ENTRIES);
        let mut admission = CheckpointCacheAdmissionStats {
            candidates_total: candidates.len(),
            candidates_considered,
            skipped_entry_limit: candidates.len().saturating_sub(candidates_considered),
            graph_peak_known: graph_peak_bytes.is_some(),
            graph_peak_bytes: graph_peak_bytes.unwrap_or_default(),
            budget_bytes: self.pool.budget_bytes(),
            ..Default::default()
        };
        self.update_checkpoint_compatibility(compatibility);
        self.retain_checkpoint_captures(
            candidates
                .iter()
                .take(candidates_considered)
                .map(|(key, _)| key),
        );

        let Some(graph_peak_bytes) = graph_peak_bytes else {
            self.clear_checkpoint_capture_cache();
            admission.skipped_graph_peak_unknown = candidates_considered;
            admission.base_checked_out_bytes = self.pool.checked_out_bytes();
            return PreparedCheckpointCaptures {
                bindings: HashMap::new(),
                admission,
            };
        };
        if self
            .pool
            .checked_out_bytes()
            .saturating_add(graph_peak_bytes)
            > self.pool.budget_bytes()
        {
            admission.cache_cleared_for_graph_pressure = true;
            self.clear_checkpoint_capture_cache();
        }

        let base_checked_out_bytes = self.pool.checked_out_bytes();
        admission.base_checked_out_bytes = base_checked_out_bytes;
        let mut required_bytes_for_all_candidates =
            base_checked_out_bytes.checked_add(graph_peak_bytes);
        for (key, plan) in candidates.iter().take(candidates_considered) {
            if self.checkpoint_captures.contains_key(key) {
                continue;
            }
            match texture_key(plan).estimated_bytes() {
                Ok(bytes) => {
                    required_bytes_for_all_candidates = required_bytes_for_all_candidates
                        .and_then(|required| required.checked_add(bytes));
                }
                Err(_) => required_bytes_for_all_candidates = None,
            }
        }
        if let Some(required_bytes) = required_bytes_for_all_candidates {
            admission.additional_budget_needed_for_all_candidates_bytes =
                required_bytes.saturating_sub(admission.budget_bytes);
            admission.additional_budget_needed_known = true;
        }

        if base_checked_out_bytes.saturating_add(graph_peak_bytes) > self.pool.budget_bytes() {
            admission.skipped_graph_pressure = candidates_considered;
            return PreparedCheckpointCaptures {
                bindings: HashMap::new(),
                admission,
            };
        }

        let mut bindings = HashMap::new();
        for (key, plan) in candidates.iter().take(candidates_considered) {
            let was_resident = self.checkpoint_captures.contains_key(key);
            if !was_resident {
                let bytes = match texture_key(plan).estimated_bytes() {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        admission.skipped_size = admission.skipped_size.saturating_add(1);
                        continue;
                    }
                };
                if self
                    .pool
                    .checked_out_bytes()
                    .saturating_add(graph_peak_bytes)
                    .saturating_add(bytes)
                    > self.pool.budget_bytes()
                {
                    admission.skipped_budget = admission.skipped_budget.saturating_add(1);
                    admission.skipped_budget_bytes =
                        admission.skipped_budget_bytes.saturating_add(bytes);
                    continue;
                }
                let texture = match self.acquire_plan(gl, plan) {
                    Ok(texture) => texture,
                    Err(_) => {
                        admission.skipped_allocation =
                            admission.skipped_allocation.saturating_add(1);
                        continue;
                    }
                };
                self.checkpoint_captures.insert(
                    key.clone(),
                    CachedCheckpointCapture {
                        texture,
                        last_populated_frame_serial: None,
                    },
                );
                admission.newly_admitted_candidates =
                    admission.newly_admitted_candidates.saturating_add(1);
            } else {
                admission.resident_candidates = admission.resident_candidates.saturating_add(1);
            }
            if let Some(cached) = self.checkpoint_captures.get(key) {
                bindings.insert(
                    plan.id,
                    GraphTextureBinding::checkpoint_cache(cached.texture.clone()),
                );
            }
        }
        PreparedCheckpointCaptures {
            bindings,
            admission,
        }
    }

    fn update_checkpoint_compatibility(
        &mut self,
        compatibility: CheckpointCacheCompatibility,
    ) -> bool {
        if self.checkpoint_compatibility == Some(compatibility) {
            return false;
        }
        self.clear_checkpoint_capture_cache();
        self.checkpoint_compatibility = Some(compatibility);
        true
    }

    fn retain_checkpoint_captures<'a>(
        &mut self,
        live_keys: impl IntoIterator<Item = &'a CheckpointCaptureCacheKey>,
    ) {
        let live_keys = live_keys
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        let stale_keys = self
            .checkpoint_captures
            .keys()
            .filter(|key| !live_keys.contains(key))
            .cloned()
            .collect::<Vec<_>>();
        for key in stale_keys {
            self.evict_checkpoint_capture(&key);
        }
    }

    pub(crate) fn checkpoint_capture_texture(
        &self,
        key: &CheckpointCaptureCacheKey,
    ) -> Option<PooledEffectTexture> {
        self.checkpoint_captures
            .get(key)
            .map(|cached| cached.texture.clone())
    }

    pub(crate) fn checkpoint_capture_needs_full_refresh(
        &self,
        key: &CheckpointCaptureCacheKey,
        frame_serial: u64,
    ) -> bool {
        !self.checkpoint_captures.get(key).is_some_and(|cached| {
            cached
                .last_populated_frame_serial
                .and_then(|serial| serial.checked_add(1))
                == Some(frame_serial)
        })
    }

    pub(crate) fn mark_checkpoint_capture_populated(
        &mut self,
        key: &CheckpointCaptureCacheKey,
        frame_serial: u64,
    ) {
        if let Some(cached) = self.checkpoint_captures.get_mut(key) {
            cached.last_populated_frame_serial = Some(frame_serial);
        }
    }

    pub(crate) fn invalidate_checkpoint_capture(&mut self, key: &CheckpointCaptureCacheKey) {
        if let Some(cached) = self.checkpoint_captures.get_mut(key) {
            cached.last_populated_frame_serial = None;
        }
    }

    pub(crate) fn invalidate_checkpoint_capture_contents(&mut self) {
        self.invalidate_checkpoint_causal_state();
        for cached in self.checkpoint_captures.values_mut() {
            cached.last_populated_frame_serial = None;
        }
    }

    pub(crate) fn clear_checkpoint_capture_cache(&mut self) {
        self.invalidate_checkpoint_causal_state();
        let entries = std::mem::take(&mut self.checkpoint_captures);
        for cached in entries.into_values() {
            let _ = self.release(cached.texture);
        }
    }

    fn evict_checkpoint_capture(&mut self, key: &CheckpointCaptureCacheKey) {
        if let Some(cached) = self.checkpoint_captures.remove(key) {
            let _ = self.release(cached.texture);
        }
    }

    pub(crate) fn acquire_graph(
        &mut self,
        gl: &glow::Context,
        graph: &CompiledFrameGraph,
    ) -> Result<HashMap<GraphTextureId, GraphTextureBinding>, Box<dyn std::error::Error>> {
        let mut checked_out = HashMap::new();
        for texture in &graph.textures {
            if texture.source == GraphTextureSource::Output {
                continue;
            }
            match self.acquire(gl, texture_key(texture)) {
                Ok(realized) => {
                    checked_out.insert(texture.id, GraphTextureBinding::transient(realized));
                }
                Err(error) => {
                    let _ = self.release_graph(checked_out);
                    return Err(error);
                }
            }
        }
        Ok(checked_out)
    }

    pub(crate) fn release_graph(
        &mut self,
        textures: HashMap<GraphTextureId, GraphTextureBinding>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut first_error = None;
        for texture in textures.into_values() {
            if let Some(texture) = texture.into_transient()
                && let Err(error) = self.release(texture)
            {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), |error| Err(error.into()))
    }

    pub(crate) fn texture(&self, texture: &PooledEffectTexture) -> Option<glow::Texture> {
        self.gl_textures.get(&texture.id).copied()
    }

    pub(crate) fn physical_texture_id(&self, texture: &PooledEffectTexture) -> Option<u64> {
        self.gl_textures
            .contains_key(&texture.id)
            .then_some(texture.id)
    }

    pub(crate) fn scratch_framebuffer_identity(&self) -> Option<String> {
        self.scratch_fbo
            .as_ref()
            .map(|framebuffer| format!("{framebuffer:?}"))
    }

    pub(crate) fn bind_render_target(
        &mut self,
        gl: &glow::Context,
        texture: &PooledEffectTexture,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.bind_render_target_to(gl, texture, glow::FRAMEBUFFER)
            .map(|_| ())
    }

    pub(crate) fn bind_draw_target(
        &mut self,
        gl: &glow::Context,
        texture: &PooledEffectTexture,
    ) -> Result<glow::Framebuffer, Box<dyn std::error::Error>> {
        self.bind_render_target_to(gl, texture, glow::DRAW_FRAMEBUFFER)
    }

    pub(crate) fn bind_lifecycle_composition_target(
        &mut self,
        gl: &glow::Context,
        texture: &PooledEffectTexture,
    ) -> Result<glow::Framebuffer, Box<dyn std::error::Error>> {
        let framebuffer = if let Some(framebuffer) = self.lifecycle_composition_fbo {
            framebuffer
        } else {
            let framebuffer = unsafe { gl.create_framebuffer().map_err(io::Error::other)? };
            self.lifecycle_composition_fbo = Some(framebuffer);
            framebuffer
        };
        self.attach_render_target_to_framebuffer(gl, texture, glow::FRAMEBUFFER, framebuffer)
    }

    pub(crate) fn bind_read_target(
        &mut self,
        gl: &glow::Context,
        texture: &PooledEffectTexture,
    ) -> Result<glow::Framebuffer, Box<dyn std::error::Error>> {
        self.bind_render_target_to(gl, texture, glow::READ_FRAMEBUFFER)
    }

    fn bind_render_target_to(
        &mut self,
        gl: &glow::Context,
        texture: &PooledEffectTexture,
        framebuffer_target: u32,
    ) -> Result<glow::Framebuffer, Box<dyn std::error::Error>> {
        let framebuffer = if let Some(framebuffer) = self.scratch_fbo {
            framebuffer
        } else {
            let framebuffer = unsafe { gl.create_framebuffer().map_err(io::Error::other)? };
            self.scratch_fbo = Some(framebuffer);
            framebuffer
        };
        self.attach_render_target_to_framebuffer(gl, texture, framebuffer_target, framebuffer)
    }

    fn attach_render_target_to_framebuffer(
        &self,
        gl: &glow::Context,
        texture: &PooledEffectTexture,
        framebuffer_target: u32,
        framebuffer: glow::Framebuffer,
    ) -> Result<glow::Framebuffer, Box<dyn std::error::Error>> {
        let gl_texture = self
            .texture(texture)
            .ok_or_else(|| io::Error::other("effect texture was not realized"))?;
        unsafe {
            gl.bind_framebuffer(framebuffer_target, Some(framebuffer));
            gl.framebuffer_texture_2d(
                framebuffer_target,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(gl_texture),
                0,
            );
            if gl.check_framebuffer_status(framebuffer_target) != glow::FRAMEBUFFER_COMPLETE {
                gl.bind_framebuffer(framebuffer_target, None);
                return Err(io::Error::other("effect framebuffer is incomplete").into());
            }
        }
        Ok(framebuffer)
    }

    pub(crate) fn unbind_render_target(&self, gl: &glow::Context) {
        unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, None) };
    }

    pub(crate) fn release(
        &mut self,
        texture: PooledEffectTexture,
    ) -> Result<(), EffectResourceError> {
        self.pool.return_texture(texture)
    }

    pub(crate) fn metrics(&self) -> EffectResourceMetrics {
        self.pool.metrics()
    }

    pub(crate) fn cleanup_size_history(&mut self, gl: &glow::Context) {
        for id in self.pool.cleanup_size_history() {
            if let Some(texture) = self.gl_textures.remove(&id) {
                unsafe { gl.delete_texture(texture) };
            }
        }
    }

    pub(crate) fn destroy(&mut self, gl: &glow::Context) {
        self.clear_checkpoint_capture_cache();
        for (_, texture) in self.gl_textures.drain() {
            unsafe { gl.delete_texture(texture) };
        }
        if let Some(framebuffer) = self.scratch_fbo.take() {
            unsafe { gl.delete_framebuffer(framebuffer) };
        }
        if let Some(framebuffer) = self.lifecycle_composition_fbo.take() {
            unsafe { gl.delete_framebuffer(framebuffer) };
        }
        self.pool.cleanup_size_history();
    }

    #[cfg(test)]
    pub(crate) fn poison_cached_textures(&mut self, gl: &glow::Context, color: [f32; 4]) -> usize {
        let cached = self
            .pool
            .textures
            .values()
            .flatten()
            .filter(|texture| !texture.checked_out)
            .cloned()
            .collect::<Vec<_>>();
        let cached_count = cached.len();
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.clear_color(color[0], color[1], color[2], color[3]);
        }
        for texture in cached {
            self.bind_render_target(gl, &texture)
                .expect("cached effect texture is a valid poison target");
            unsafe {
                gl.clear(glow::COLOR_BUFFER_BIT);
            }
        }
        self.unbind_render_target(gl);
        cached_count
    }
}

fn texture_key(texture: &GraphTexturePlan) -> EffectTextureKey {
    let format = if texture.source == GraphTextureSource::Intermediate {
        EffectTextureFormat::Rgba16Float
    } else {
        EffectTextureFormat::Rgba8
    };
    let filter = if matches!(texture.source, GraphTextureSource::Static(_)) {
        EffectTextureFilter::Nearest
    } else {
        EffectTextureFilter::Linear
    };
    EffectTextureKey::new(
        texture.width,
        texture.height,
        format,
        filter,
        texture.working_space,
    )
}

fn graph_texture_plan(
    graph: &CompiledFrameGraph,
    id: GraphTextureId,
) -> Result<&GraphTexturePlan, Box<dyn std::error::Error>> {
    graph
        .textures
        .iter()
        .find(|texture| texture.id == id)
        .ok_or_else(|| io::Error::other("effect graph references an unknown texture").into())
}

fn graph_texture_is_output(graph: &CompiledFrameGraph, id: GraphTextureId) -> bool {
    graph
        .textures
        .iter()
        .find(|texture| texture.id == id)
        .is_some_and(|texture| texture.source == GraphTextureSource::Output)
}

pub(crate) fn release_dead_graph_textures(
    cache: &mut EffectGlResourceCache,
    graph: &CompiledFrameGraph,
    pass: oblivion_one::effects::GraphPassId,
    live: &mut HashMap<GraphTextureId, GraphTextureBinding>,
) -> Result<(), Box<dyn std::error::Error>> {
    let dead = graph
        .textures
        .iter()
        .filter(|texture| texture.last_use == Some(pass))
        .map(|texture| texture.id)
        .collect::<Vec<_>>();
    for id in dead {
        if let Some(texture) = live.remove(&id) {
            if let Some(texture) = texture.into_transient() {
                cache.release(texture)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn estimate_graph_peak_bytes(
    graph: &CompiledFrameGraph,
) -> Result<u64, EffectResourceError> {
    if graph.passes.is_empty() {
        return Ok(0);
    }

    let max_pass_id = graph
        .passes
        .iter()
        .map(|pass| usize::from(pass.id.get()))
        .max()
        .unwrap_or(0);
    let mut position_by_id = vec![None; max_pass_id.saturating_add(1)];
    for (position, pass) in graph.passes.iter().enumerate() {
        let id = usize::from(pass.id.get());
        let Some(mapped_position) = position_by_id.get_mut(id) else {
            return Err(EffectResourceError::InvalidGraphLifetime);
        };
        if mapped_position.is_some() {
            return Err(EffectResourceError::InvalidGraphLifetime);
        }
        *mapped_position = Some(position);
    }

    let mut delta = vec![0_i128; graph.passes.len().saturating_add(1)];
    for texture in &graph.textures {
        if texture.source == GraphTextureSource::Output {
            continue;
        }
        let (Some(first), Some(last)) = (texture.first_use, texture.last_use) else {
            continue;
        };
        let first = position_by_id
            .get(usize::from(first.get()))
            .copied()
            .flatten()
            .ok_or(EffectResourceError::InvalidGraphLifetime)?;
        let last = position_by_id
            .get(usize::from(last.get()))
            .copied()
            .flatten()
            .ok_or(EffectResourceError::InvalidGraphLifetime)?;
        if first > last {
            return Err(EffectResourceError::InvalidGraphLifetime);
        }
        let bytes = i128::from(texture_key(texture).estimated_bytes()?);
        delta[first] = delta[first]
            .checked_add(bytes)
            .ok_or(EffectResourceError::SizeOverflow)?;
        delta[last + 1] = delta[last + 1]
            .checked_sub(bytes)
            .ok_or(EffectResourceError::SizeOverflow)?;
    }

    let mut live = 0_i128;
    let mut peak = 0_i128;
    for change in delta.into_iter().take(graph.passes.len()) {
        live = live
            .checked_add(change)
            .ok_or(EffectResourceError::SizeOverflow)?;
        if live < 0 {
            return Err(EffectResourceError::InvalidGraphLifetime);
        }
        peak = peak.max(live);
    }
    u64::try_from(peak).map_err(|_| EffectResourceError::SizeOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oblivion_one::effects::{
        CompiledRenderPass, EffectInstanceId, EffectRegion, EffectWorkingSpace, RenderPassKind,
    };

    fn backdrop_stack_graph(generation: u64, source_damage: EffectRegion) -> CompiledFrameGraph {
        use oblivion_one::{
            compositor::{
                EffectAnchor, EffectAnchorScope, EffectSceneOrder, ResolvedEffectInstance,
                ResolvedEffectScene,
            },
            effects::{
                DualKawaseBlurSpec, EffectAlphaMode, EffectFailurePolicy, EffectFrameDemand,
                EffectNode, EffectNodeId, EffectOutsets, EffectParameterBlock, EffectProgram,
                EffectProgramId, EffectSource, EffectWorkingSpace, FrameExecutionPlan,
                compile_frame_execution_plan, validate_effect_program,
            },
        };

        let source = EffectNodeId::new(1).unwrap();
        let blur = EffectNodeId::new(2).unwrap();
        let program = EffectProgram {
            id: EffectProgramId::new(1).unwrap(),
            nodes: vec![
                EffectNode::source(source, EffectSource::Backdrop),
                EffectNode::dual_kawase(
                    blur,
                    source,
                    DualKawaseBlurSpec::new(4.0, 2, 1.0).unwrap(),
                ),
            ],
            output: blur,
            working_space: EffectWorkingSpace::LinearSrgb,
            alpha_mode: EffectAlphaMode::Opaque,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        };
        let mut registry = oblivion_one::effects::EffectRegistry::empty();
        registry
            .insert(validate_effect_program(program).unwrap())
            .unwrap();
        let region = EffectRegion::from_rect(
            oblivion_one::effects::EffectRect::new(100, 80, 320, 180).unwrap(),
        );
        let instances = (1..=3)
            .map(|id| {
                let anchor = EffectAnchor::BeforeSurface(1);
                ResolvedEffectInstance {
                    id: EffectInstanceId::new(id).unwrap(),
                    program: EffectProgramId::new(1).unwrap(),
                    anchor,
                    target_bounds: region.bounding_rect().unwrap(),
                    region: region.clone(),
                    parameter_block: EffectParameterBlock::default(),
                    signature: 100 + id,
                    frame_demand: EffectFrameDemand::OnDamage,
                    visual_group: None,
                    anchor_scope: EffectAnchorScope::VisualGroup,
                    scene_order: EffectSceneOrder::for_anchor(anchor),
                }
            })
            .collect();
        let scene = ResolvedEffectScene::new(generation, instances);
        let FrameExecutionPlan::EffectGraph(graph) = compile_frame_execution_plan(
            &scene,
            &source_damage,
            oblivion_one::effects::EffectRect::new(0, 0, 1920, 1080).unwrap(),
            &registry,
        )
        .unwrap() else {
            panic!("three backdrop instances must compile to a graph");
        };
        graph
    }

    fn key(width: u32, height: u32) -> EffectTextureKey {
        EffectTextureKey::new(
            width,
            height,
            EffectTextureFormat::Rgba8,
            EffectTextureFilter::Linear,
            EffectWorkingSpace::LinearSrgb,
        )
    }

    fn resource_cache_with_checkpoint(
        cache_key: CheckpointCaptureCacheKey,
        last_populated_frame_serial: Option<u64>,
    ) -> EffectGlResourceCache {
        let mut cache = EffectGlResourceCache::with_budget(1024 * 1024).unwrap();
        let texture = cache.pool.checkout(key(8, 8)).unwrap();
        cache.checkpoint_captures.insert(
            cache_key,
            CachedCheckpointCapture {
                texture,
                last_populated_frame_serial,
            },
        );
        cache
    }

    fn first_checkpoint_candidate(
        graph: &CompiledFrameGraph,
    ) -> (CheckpointCaptureCacheKey, GraphTexturePlan) {
        let pass = graph
            .passes
            .iter()
            .find(|pass| {
                pass.kind == RenderPassKind::SceneCapture
                    && !pass.checkpoint_dependencies.is_empty()
            })
            .unwrap();
        let key = checkpoint_capture_cache_key(graph, pass).unwrap();
        let plan = graph
            .textures
            .iter()
            .find(|texture| Some(texture.id) == pass.output)
            .unwrap()
            .clone();
        (key, plan)
    }

    fn assert_checkpoint_admission_partition(admission: CheckpointCacheAdmissionStats) {
        assert_eq!(
            admission.candidates_considered + admission.skipped_entry_limit,
            admission.candidates_total
        );
        assert_eq!(
            admission.resident_candidates
                + admission.newly_admitted_candidates
                + admission.skipped_graph_peak_unknown
                + admission.skipped_graph_pressure
                + admission.skipped_budget
                + admission.skipped_size
                + admission.skipped_allocation,
            admission.candidates_considered
        );
    }

    fn sample_checkpoint_key() -> CheckpointCaptureCacheKey {
        let graph = backdrop_stack_graph(1, EffectRegion::empty());
        let checkpoint = graph
            .passes
            .iter()
            .find(|pass| {
                pass.kind == RenderPassKind::SceneCapture
                    && pass.instance == EffectInstanceId::new(3).unwrap()
                    && !pass.checkpoint_dependencies.is_empty()
            })
            .unwrap();
        checkpoint_capture_cache_key(&graph, checkpoint).unwrap()
    }

    fn stacked_checkpoint(
        graph: &CompiledFrameGraph,
        consumer: EffectInstanceId,
    ) -> &CompiledRenderPass {
        graph
            .passes
            .iter()
            .find(|pass| {
                pass.kind == RenderPassKind::SceneCapture
                    && pass.instance == consumer
                    && !pass.checkpoint_dependencies.is_empty()
            })
            .expect("third stacked checkpoint")
    }

    fn checkpoint_key_with_composition(
        mut graph: CompiledFrameGraph,
        consumer: EffectInstanceId,
        anchor: oblivion_one::compositor::EffectAnchor,
        anchor_scope: oblivion_one::compositor::EffectAnchorScope,
        visual_group: Option<oblivion_one::compositor::VisualGroupId>,
    ) -> CheckpointCaptureCacheKey {
        let pass = graph
            .passes
            .iter_mut()
            .find(|pass| pass.kind == RenderPassKind::SceneCapture && pass.instance == consumer)
            .expect("checkpoint capture");
        pass.anchor = anchor;
        pass.anchor_scope = anchor_scope;
        pass.visual_group = visual_group;
        checkpoint_capture_cache_key(&graph, stacked_checkpoint(&graph, consumer))
            .expect("eligible checkpoint capture")
    }

    #[test]
    fn checkpoint_capture_key_ignores_scene_generation_damage_and_pass_ids() {
        let first = backdrop_stack_graph(
            1,
            EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(101, 81, 2, 3).unwrap()),
        );
        let mut second = backdrop_stack_graph(
            9,
            EffectRegion::from_rect(
                oblivion_one::effects::EffectRect::new(139, 113, 3, 2).unwrap(),
            ),
        );
        let consumer = EffectInstanceId::new(3).unwrap();
        let first_key = checkpoint_capture_cache_key(&first, stacked_checkpoint(&first, consumer))
            .expect("eligible third checkpoint");

        for pass in &mut second.passes {
            pass.id = oblivion_one::effects::GraphPassId::new(pass.id.get() + 100).unwrap();
            for dependency in &mut pass.checkpoint_dependencies {
                *dependency =
                    oblivion_one::effects::GraphPassId::new(dependency.get() + 100).unwrap();
            }
        }
        for texture in &mut second.textures {
            texture.first_use = texture
                .first_use
                .map(|id| oblivion_one::effects::GraphPassId::new(id.get() + 100).unwrap());
            texture.last_use = texture
                .last_use
                .map(|id| oblivion_one::effects::GraphPassId::new(id.get() + 100).unwrap());
        }
        let second_key =
            checkpoint_capture_cache_key(&second, stacked_checkpoint(&second, consumer))
                .expect("renumbered eligible third checkpoint");

        assert_ne!(first.final_damage, second.final_damage);
        assert_eq!(first_key, second_key);
    }

    #[test]
    fn checkpoint_capture_key_changes_with_consumer_anchor_scope() {
        let mut surface_graph = backdrop_stack_graph(1, EffectRegion::empty());
        let consumer = EffectInstanceId::new(3).unwrap();
        let visual_group = oblivion_one::compositor::VisualGroupId::new(7).unwrap();
        let surface_pass = surface_graph
            .passes
            .iter_mut()
            .find(|pass| pass.kind == RenderPassKind::SceneCapture && pass.instance == consumer)
            .expect("third checkpoint capture");
        surface_pass.anchor_scope = oblivion_one::compositor::EffectAnchorScope::Surface;
        surface_pass.visual_group = Some(visual_group);
        let surface_key = checkpoint_capture_cache_key(
            &surface_graph,
            stacked_checkpoint(&surface_graph, consumer),
        )
        .expect("surface-scoped checkpoint");

        let mut visual_group_graph = surface_graph.clone();
        visual_group_graph
            .passes
            .iter_mut()
            .find(|pass| pass.kind == RenderPassKind::SceneCapture && pass.instance == consumer)
            .expect("third checkpoint capture")
            .anchor_scope = oblivion_one::compositor::EffectAnchorScope::VisualGroup;
        let visual_group_key = checkpoint_capture_cache_key(
            &visual_group_graph,
            stacked_checkpoint(&visual_group_graph, consumer),
        )
        .expect("visual-group-scoped checkpoint");

        assert_eq!(
            surface_key.consumer_semantic_signature,
            visual_group_key.consumer_semantic_signature
        );
        assert_ne!(surface_key, visual_group_key);
    }

    #[test]
    fn checkpoint_capture_key_preserves_anchor_identity() {
        use oblivion_one::compositor::{EffectAnchor, EffectAnchorScope};

        let graph = backdrop_stack_graph(1, EffectRegion::empty());
        let consumer = EffectInstanceId::new(3).unwrap();
        let visual_group = oblivion_one::compositor::VisualGroupId::new(7);
        let before_10 = checkpoint_key_with_composition(
            graph.clone(),
            consumer,
            EffectAnchor::BeforeSurface(10),
            EffectAnchorScope::Surface,
            visual_group,
        );
        let before_11 = checkpoint_key_with_composition(
            graph.clone(),
            consumer,
            EffectAnchor::BeforeSurface(11),
            EffectAnchorScope::Surface,
            visual_group,
        );
        let replace_10 = checkpoint_key_with_composition(
            graph.clone(),
            consumer,
            EffectAnchor::ReplaceSurface(10),
            EffectAnchorScope::Surface,
            visual_group,
        );
        let after_10 = checkpoint_key_with_composition(
            graph.clone(),
            consumer,
            EffectAnchor::AfterSurface(10),
            EffectAnchorScope::Surface,
            visual_group,
        );
        let output_post_process = checkpoint_key_with_composition(
            graph,
            consumer,
            EffectAnchor::OutputPostProcess,
            EffectAnchorScope::Surface,
            visual_group,
        );

        assert_ne!(before_10, before_11);
        assert_ne!(before_10, replace_10);
        assert_ne!(before_10, after_10);
        assert_ne!(after_10, output_post_process);
    }

    #[test]
    fn checkpoint_capture_key_preserves_visual_group_identity() {
        use oblivion_one::compositor::{EffectAnchor, EffectAnchorScope};

        let graph = backdrop_stack_graph(1, EffectRegion::empty());
        let consumer = EffectInstanceId::new(3).unwrap();
        let anchor = EffectAnchor::BeforeSurface(10);
        let group_7 = oblivion_one::compositor::VisualGroupId::new(7);
        let group_8 = oblivion_one::compositor::VisualGroupId::new(8);
        let none = checkpoint_key_with_composition(
            graph.clone(),
            consumer,
            anchor,
            EffectAnchorScope::VisualGroup,
            None,
        );
        let some_7 = checkpoint_key_with_composition(
            graph.clone(),
            consumer,
            anchor,
            EffectAnchorScope::VisualGroup,
            group_7,
        );
        let some_8 = checkpoint_key_with_composition(
            graph,
            consumer,
            anchor,
            EffectAnchorScope::VisualGroup,
            group_8,
        );

        assert_ne!(none, some_7);
        assert_ne!(some_7, some_8);
    }

    #[test]
    fn checkpoint_capture_key_ignores_visual_group_for_surface_scope() {
        use oblivion_one::compositor::{EffectAnchor, EffectAnchorScope};

        let graph = backdrop_stack_graph(1, EffectRegion::empty());
        let consumer = EffectInstanceId::new(3).unwrap();
        let anchor = EffectAnchor::BeforeSurface(10);
        let group_7 = oblivion_one::compositor::VisualGroupId::new(7);
        let group_8 = oblivion_one::compositor::VisualGroupId::new(8);
        let surface_7 = checkpoint_key_with_composition(
            graph.clone(),
            consumer,
            anchor,
            EffectAnchorScope::Surface,
            group_7,
        );
        let surface_8 = checkpoint_key_with_composition(
            graph,
            consumer,
            anchor,
            EffectAnchorScope::Surface,
            group_8,
        );

        assert_eq!(surface_7, surface_8);
    }

    #[test]
    fn checkpoint_capture_key_tracks_dependency_composition_identity() {
        use oblivion_one::compositor::{EffectAnchor, EffectAnchorScope};

        let mut surface_graph = backdrop_stack_graph(1, EffectRegion::empty());
        let consumer = EffectInstanceId::new(3).unwrap();
        let dependency_id = stacked_checkpoint(&surface_graph, consumer).checkpoint_dependencies[0];
        let group_7 = oblivion_one::compositor::VisualGroupId::new(7).unwrap();
        let group_8 = oblivion_one::compositor::VisualGroupId::new(8).unwrap();
        let dependency = surface_graph
            .passes
            .iter_mut()
            .find(|pass| pass.id == dependency_id)
            .expect("checkpoint dependency producer");
        dependency.anchor = EffectAnchor::BeforeSurface(10);
        dependency.anchor_scope = EffectAnchorScope::Surface;
        dependency.visual_group = Some(group_7);
        let base = checkpoint_capture_cache_key(
            &surface_graph,
            stacked_checkpoint(&surface_graph, consumer),
        )
        .expect("consumer checkpoint key");

        let mut visual_group_graph = surface_graph.clone();
        visual_group_graph
            .passes
            .iter_mut()
            .find(|pass| pass.id == dependency_id)
            .expect("checkpoint dependency producer")
            .anchor_scope = EffectAnchorScope::VisualGroup;
        let changed_scope = checkpoint_capture_cache_key(
            &visual_group_graph,
            stacked_checkpoint(&visual_group_graph, consumer),
        )
        .expect("consumer checkpoint key with visual-group dependency");

        let mut other_group_graph = visual_group_graph.clone();
        other_group_graph
            .passes
            .iter_mut()
            .find(|pass| pass.id == dependency_id)
            .expect("checkpoint dependency producer")
            .visual_group = Some(group_8);
        let changed_group = checkpoint_capture_cache_key(
            &other_group_graph,
            stacked_checkpoint(&other_group_graph, consumer),
        )
        .expect("consumer checkpoint key with another dependency group");

        assert_eq!(
            base.consumer_semantic_signature,
            changed_scope.consumer_semantic_signature
        );
        assert_eq!(
            base.consumer_semantic_signature,
            changed_group.consumer_semantic_signature
        );
        assert_ne!(base, changed_scope);
        assert_ne!(changed_scope, changed_group);
    }

    #[test]
    fn checkpoint_capture_key_changes_with_semantics_and_texture_layout() {
        let graph = backdrop_stack_graph(1, EffectRegion::empty());
        let consumer = EffectInstanceId::new(3).unwrap();
        let checkpoint = graph
            .passes
            .iter()
            .find(|pass| {
                pass.kind == RenderPassKind::SceneCapture
                    && pass.instance == consumer
                    && !pass.checkpoint_dependencies.is_empty()
            })
            .expect("third stacked checkpoint");
        let base = checkpoint_capture_cache_key(&graph, checkpoint).unwrap();

        let changed_consumer_signature = {
            let mut graph = graph.clone();
            graph
                .instances
                .iter_mut()
                .find(|instance| instance.id == consumer)
                .unwrap()
                .semantic_signature += 1;
            checkpoint_capture_cache_key(
                &graph,
                graph
                    .passes
                    .iter()
                    .find(|pass| {
                        pass.instance == consumer && pass.kind == RenderPassKind::SceneCapture
                    })
                    .unwrap(),
            )
            .unwrap()
        };
        assert_ne!(base, changed_consumer_signature);

        let changed_domain = {
            let mut graph = graph.clone();
            let output = checkpoint.output.unwrap();
            let texture = graph
                .textures
                .iter_mut()
                .find(|texture| texture.id == output)
                .unwrap();
            texture.domain = oblivion_one::effects::EffectRect::new(
                texture.domain.x + 1,
                texture.domain.y,
                texture.domain.width,
                texture.domain.height,
            )
            .unwrap();
            checkpoint_capture_cache_key(
                &graph,
                graph
                    .passes
                    .iter()
                    .find(|pass| {
                        pass.instance == consumer && pass.kind == RenderPassKind::SceneCapture
                    })
                    .unwrap(),
            )
            .unwrap()
        };
        assert_ne!(base, changed_domain);

        let changed_width = {
            let mut graph = graph.clone();
            let output = checkpoint.output.unwrap();
            graph
                .textures
                .iter_mut()
                .find(|texture| texture.id == output)
                .unwrap()
                .width += 1;
            checkpoint_capture_cache_key(
                &graph,
                graph
                    .passes
                    .iter()
                    .find(|pass| {
                        pass.instance == consumer && pass.kind == RenderPassKind::SceneCapture
                    })
                    .unwrap(),
            )
            .unwrap()
        };
        assert_ne!(base, changed_width);

        let changed_height = {
            let mut graph = graph.clone();
            let output = checkpoint.output.unwrap();
            graph
                .textures
                .iter_mut()
                .find(|texture| texture.id == output)
                .unwrap()
                .height += 1;
            checkpoint_capture_cache_key(
                &graph,
                graph
                    .passes
                    .iter()
                    .find(|pass| {
                        pass.instance == consumer && pass.kind == RenderPassKind::SceneCapture
                    })
                    .unwrap(),
            )
            .unwrap()
        };
        assert_ne!(base, changed_height);

        let changed_format = {
            let mut graph = graph.clone();
            let output = checkpoint.output.unwrap();
            graph
                .textures
                .iter_mut()
                .find(|texture| texture.id == output)
                .unwrap()
                .source = GraphTextureSource::Intermediate;
            checkpoint_capture_cache_key(
                &graph,
                graph
                    .passes
                    .iter()
                    .find(|pass| {
                        pass.instance == consumer && pass.kind == RenderPassKind::SceneCapture
                    })
                    .unwrap(),
            )
            .unwrap()
        };
        assert_ne!(base, changed_format);

        let changed_working_space = {
            let mut graph = graph.clone();
            let output = checkpoint.output.unwrap();
            let texture = graph
                .textures
                .iter_mut()
                .find(|texture| texture.id == output)
                .unwrap();
            texture.working_space = match texture.working_space {
                EffectWorkingSpace::LinearSrgb => EffectWorkingSpace::OutputEncodedSrgb,
                EffectWorkingSpace::OutputEncodedSrgb => EffectWorkingSpace::LinearSrgb,
            };
            checkpoint_capture_cache_key(
                &graph,
                graph
                    .passes
                    .iter()
                    .find(|pass| {
                        pass.instance == consumer && pass.kind == RenderPassKind::SceneCapture
                    })
                    .unwrap(),
            )
            .unwrap()
        };
        assert_ne!(base, changed_working_space);

        let changed_dependency_signature = {
            let mut graph = graph.clone();
            let dependency_id = graph
                .passes
                .iter()
                .find(|pass| pass.id == checkpoint.checkpoint_dependencies[0])
                .unwrap()
                .instance;
            graph
                .instances
                .iter_mut()
                .find(|instance| instance.id == dependency_id)
                .unwrap()
                .semantic_signature += 1;
            checkpoint_capture_cache_key(
                &graph,
                graph
                    .passes
                    .iter()
                    .find(|pass| {
                        pass.instance == consumer && pass.kind == RenderPassKind::SceneCapture
                    })
                    .unwrap(),
            )
            .unwrap()
        };
        assert_ne!(base, changed_dependency_signature);

        let changed_dependency_identity = {
            let mut graph = graph.clone();
            let dependency_pass_id = checkpoint.checkpoint_dependencies[0];
            let old_dependency_id = graph
                .passes
                .iter()
                .find(|pass| pass.id == dependency_pass_id)
                .unwrap()
                .instance;
            let new_dependency_id = EffectInstanceId::new(99).unwrap();
            graph
                .passes
                .iter_mut()
                .filter(|pass| pass.instance == old_dependency_id)
                .for_each(|pass| pass.instance = new_dependency_id);
            graph
                .instances
                .iter_mut()
                .find(|instance| instance.id == old_dependency_id)
                .unwrap()
                .id = new_dependency_id;
            checkpoint_capture_cache_key(
                &graph,
                graph
                    .passes
                    .iter()
                    .find(|pass| {
                        pass.instance == consumer && pass.kind == RenderPassKind::SceneCapture
                    })
                    .unwrap(),
            )
            .unwrap()
        };
        assert_ne!(base, changed_dependency_identity);

        let changed_dependency_order = {
            let mut graph = graph.clone();
            let target = graph
                .passes
                .iter_mut()
                .find(|pass| pass.instance == consumer && pass.kind == RenderPassKind::SceneCapture)
                .unwrap();
            target.checkpoint_dependencies.reverse();
            checkpoint_capture_cache_key(
                &graph,
                graph
                    .passes
                    .iter()
                    .find(|pass| {
                        pass.instance == consumer && pass.kind == RenderPassKind::SceneCapture
                    })
                    .unwrap(),
            )
            .unwrap()
        };
        assert_ne!(base, changed_dependency_order);
    }

    #[test]
    fn checkpoint_cache_requires_the_immediately_preceding_frame() {
        let key = sample_checkpoint_key();
        let mut cache = resource_cache_with_checkpoint(key.clone(), None);

        assert!(cache.checkpoint_capture_needs_full_refresh(&key, 10));
        cache.mark_checkpoint_capture_populated(&key, 10);
        assert!(!cache.checkpoint_capture_needs_full_refresh(&key, 11));
        assert!(cache.checkpoint_capture_needs_full_refresh(&key, 12));

        cache.mark_checkpoint_capture_populated(&key, 12);
        cache.invalidate_checkpoint_capture_contents();
        assert!(cache.checkpoint_capture_needs_full_refresh(&key, 13));
    }

    #[test]
    fn checkpoint_compatibility_changes_release_cached_contents() {
        let base = CheckpointCacheCompatibility {
            output_size: (1920, 1080),
            framebuffer_origin_top_left: false,
            effect_registry_generation: 4,
        };
        let variants = [
            CheckpointCacheCompatibility {
                output_size: (2560, 1440),
                ..base
            },
            CheckpointCacheCompatibility {
                framebuffer_origin_top_left: true,
                ..base
            },
            CheckpointCacheCompatibility {
                effect_registry_generation: 5,
                ..base
            },
        ];
        for changed in variants {
            let key = sample_checkpoint_key();
            let mut cache = resource_cache_with_checkpoint(key, Some(9));
            cache.checkpoint_compatibility = Some(base);
            assert!(cache.update_checkpoint_compatibility(changed));
            assert!(cache.checkpoint_captures.is_empty());
            assert_eq!(cache.pool.checked_out_bytes(), 0);
        }
    }

    #[test]
    fn removed_checkpoint_lease_returns_to_pool_once_and_can_be_reused() {
        let cache_key = sample_checkpoint_key();
        let mut cache = resource_cache_with_checkpoint(cache_key.clone(), Some(2));
        let texture_id = cache.checkpoint_captures[&cache_key].texture.id;

        cache.retain_checkpoint_captures(std::iter::empty());
        assert!(cache.checkpoint_captures.is_empty());
        assert_eq!(cache.pool.checked_out_bytes(), 0);
        cache.clear_checkpoint_capture_cache();
        assert_eq!(cache.pool.checked_out_bytes(), 0);

        let reused = cache.pool.checkout(key(8, 8)).unwrap();
        assert_eq!(reused.id, texture_id);
        cache.pool.return_texture(reused.clone()).unwrap();
        assert_eq!(
            cache.pool.return_texture(reused),
            Err(EffectResourceError::TextureAlreadyReturned(texture_id))
        );
    }

    #[test]
    fn renderer_destroy_releases_persistent_checkpoint_leases() {
        let cache_key = sample_checkpoint_key();
        let mut cache = resource_cache_with_checkpoint(cache_key, Some(1));
        assert_eq!(cache.pool.checked_out_bytes(), 8 * 8 * 4);

        let gl = unsafe { glow::Context::from_loader_function(|_| std::ptr::null()) };
        cache.destroy(&gl);

        assert!(cache.checkpoint_captures.is_empty());
        assert_eq!(cache.pool.checked_out_bytes(), 0);
        assert_eq!(cache.pool.current_bytes(), 0);
    }

    #[test]
    fn frame_local_release_keeps_a_cache_owned_texture_checked_out() {
        let cache_key = sample_checkpoint_key();
        let mut cache = resource_cache_with_checkpoint(cache_key.clone(), Some(1));
        let persistent = cache.checkpoint_captures[&cache_key].texture.clone();
        let texture_id = persistent.id;
        let graph_texture = GraphTextureId::new(7).unwrap();
        let borrowed = HashMap::from([(
            graph_texture,
            GraphTextureBinding::checkpoint_cache(persistent),
        )]);

        cache.release_graph(borrowed).unwrap();
        assert!(cache.pool.is_checked_out(texture_id));
        assert_eq!(cache.pool.checked_out_bytes(), 8 * 8 * 4);
        cache.clear_checkpoint_capture_cache();
        assert!(!cache.pool.is_checked_out(texture_id));
        assert_eq!(cache.pool.checked_out_bytes(), 0);
    }

    #[test]
    fn checkpoint_budget_pressure_evicts_cache_and_leaves_room_for_graph_work() {
        let graph = backdrop_stack_graph(1, EffectRegion::empty());
        let pass = graph
            .passes
            .iter()
            .find(|pass| {
                pass.kind == RenderPassKind::SceneCapture
                    && !pass.checkpoint_dependencies.is_empty()
            })
            .unwrap();
        let cache_key = checkpoint_capture_cache_key(&graph, pass).unwrap();
        let plan = graph
            .textures
            .iter()
            .find(|texture| texture.id == pass.output.unwrap())
            .unwrap()
            .clone();
        let graph_peak = estimate_graph_peak_bytes(&graph).unwrap();
        let checkpoint_bytes = texture_key(&plan).estimated_bytes().unwrap();
        let budget = graph_peak
            .saturating_add(checkpoint_bytes)
            .saturating_sub(1);
        assert!(budget >= checkpoint_bytes);

        let mut cache = EffectGlResourceCache::with_budget(budget).unwrap();
        let lease = cache.pool.checkout(texture_key(&plan)).unwrap();
        cache.checkpoint_captures.insert(
            cache_key.clone(),
            CachedCheckpointCapture {
                texture: lease.clone(),
                last_populated_frame_serial: Some(1),
            },
        );
        let compatibility = CheckpointCacheCompatibility {
            output_size: (1920, 1080),
            framebuffer_origin_top_left: false,
            effect_registry_generation: 1,
        };
        cache.checkpoint_compatibility = Some(compatibility);
        let gl = unsafe { glow::Context::from_loader_function(|_| std::ptr::null()) };

        let prepared = cache.prepare_checkpoint_captures(
            &gl,
            compatibility,
            Some(graph_peak),
            &[(cache_key, plan.clone())],
        );

        assert!(
            prepared.bindings.is_empty(),
            "cache lease must be bypassed under pressure"
        );
        assert_eq!(prepared.admission.candidates_total, 1);
        assert_eq!(prepared.admission.candidates_considered, 1);
        assert_eq!(prepared.admission.resident_candidates, 0);
        assert_eq!(prepared.admission.newly_admitted_candidates, 0);
        assert_eq!(prepared.admission.skipped_graph_peak_unknown, 0);
        assert_eq!(prepared.admission.skipped_graph_pressure, 0);
        assert_eq!(prepared.admission.skipped_budget, 1);
        assert_eq!(prepared.admission.skipped_budget_bytes, checkpoint_bytes);
        assert_eq!(prepared.admission.skipped_size, 0);
        assert_eq!(prepared.admission.skipped_allocation, 0);
        assert!(prepared.admission.graph_peak_known);
        assert_eq!(prepared.admission.graph_peak_bytes, graph_peak);
        assert_eq!(prepared.admission.base_checked_out_bytes, 0);
        assert_eq!(prepared.admission.budget_bytes, budget);
        assert_eq!(
            prepared
                .admission
                .additional_budget_needed_for_all_candidates_bytes,
            1
        );
        assert!(prepared.admission.additional_budget_needed_known);
        assert!(prepared.admission.cache_cleared_for_graph_pressure);
        assert_checkpoint_admission_partition(prepared.admission);
        assert!(cache.checkpoint_captures.is_empty());
        assert_eq!(cache.pool.checked_out_bytes(), 0);
        assert!(cache.pool.budget_bytes() >= graph_peak);
        let transient = cache.pool.checkout(texture_key(&plan)).unwrap();
        assert_eq!(
            transient.id, lease.id,
            "the released cache lease is reusable"
        );
        cache.pool.return_texture(transient).unwrap();
    }

    #[test]
    fn checkpoint_graph_pressure_clear_reports_post_clear_reservation() {
        let graph = backdrop_stack_graph(1, EffectRegion::empty());
        let (cache_key, plan) = first_checkpoint_candidate(&graph);
        let graph_peak = estimate_graph_peak_bytes(&graph).unwrap();
        let budget = graph_peak.saturating_sub(1).max(1);
        assert!(
            budget >= key(8, 8).estimated_bytes().unwrap() + key(1, 1).estimated_bytes().unwrap()
        );
        let compatibility = CheckpointCacheCompatibility {
            output_size: (1920, 1080),
            framebuffer_origin_top_left: false,
            effect_registry_generation: 1,
        };
        let mut cache = EffectGlResourceCache::with_budget(budget).unwrap();
        let checkpoint_lease = cache.pool.checkout(key(8, 8)).unwrap();
        cache.checkpoint_captures.insert(
            cache_key.clone(),
            CachedCheckpointCapture {
                texture: checkpoint_lease.clone(),
                last_populated_frame_serial: Some(1),
            },
        );
        let mandatory_lease = cache.pool.checkout(key(1, 1)).unwrap();
        let initial_checked_out = checkpoint_lease.bytes + mandatory_lease.bytes;
        assert!(initial_checked_out.saturating_add(graph_peak) > budget);
        cache.checkpoint_compatibility = Some(compatibility);
        let gl = unsafe { glow::Context::from_loader_function(|_| std::ptr::null()) };

        let prepared = cache.prepare_checkpoint_captures(
            &gl,
            compatibility,
            Some(graph_peak),
            &[(cache_key, plan)],
        );

        assert!(prepared.bindings.is_empty());
        assert!(prepared.admission.cache_cleared_for_graph_pressure);
        assert_eq!(
            prepared.admission.base_checked_out_bytes,
            mandatory_lease.bytes
        );
        assert_eq!(cache.pool.checked_out_bytes(), mandatory_lease.bytes);
        assert!(cache.checkpoint_captures.is_empty());
        assert_eq!(prepared.admission.graph_peak_bytes, graph_peak);
        assert_eq!(prepared.admission.budget_bytes, budget);
        assert_eq!(prepared.admission.skipped_graph_pressure, 1);
        assert_eq!(prepared.admission.skipped_budget, 0);
        assert_eq!(prepared.admission.skipped_size, 0);
        assert_eq!(prepared.admission.skipped_allocation, 0);
        assert_checkpoint_admission_partition(prepared.admission);
        assert!(cache.pool.is_checked_out(mandatory_lease.id));
        cache.pool.return_texture(mandatory_lease).unwrap();
    }

    #[test]
    fn checkpoint_unknown_graph_peak_clears_cache_and_has_separate_attribution() {
        let graph = backdrop_stack_graph(1, EffectRegion::empty());
        let (cache_key, plan) = first_checkpoint_candidate(&graph);
        let compatibility = CheckpointCacheCompatibility {
            output_size: (1920, 1080),
            framebuffer_origin_top_left: false,
            effect_registry_generation: 1,
        };
        let mut cache = resource_cache_with_checkpoint(cache_key.clone(), Some(1));
        cache.checkpoint_compatibility = Some(compatibility);
        let gl = unsafe { glow::Context::from_loader_function(|_| std::ptr::null()) };

        let prepared =
            cache.prepare_checkpoint_captures(&gl, compatibility, None, &[(cache_key, plan)]);

        assert!(prepared.bindings.is_empty());
        assert!(cache.checkpoint_captures.is_empty());
        assert_eq!(cache.pool.checked_out_bytes(), 0);
        assert_eq!(prepared.admission.skipped_graph_peak_unknown, 1);
        assert_eq!(prepared.admission.skipped_graph_pressure, 0);
        assert_eq!(prepared.admission.skipped_budget, 0);
        assert!(!prepared.admission.graph_peak_known);
        assert_eq!(prepared.admission.graph_peak_bytes, 0);
        assert_eq!(prepared.admission.base_checked_out_bytes, 0);
        assert!(!prepared.admission.additional_budget_needed_known);
        assert_checkpoint_admission_partition(prepared.admission);
    }

    #[test]
    fn checkpoint_size_overflow_is_attributed_without_binding() {
        let graph = backdrop_stack_graph(1, EffectRegion::empty());
        let (cache_key, mut plan) = first_checkpoint_candidate(&graph);
        plan.width = u32::MAX;
        plan.height = u32::MAX;
        let mut cache = EffectGlResourceCache::with_budget(1024).unwrap();
        let compatibility = CheckpointCacheCompatibility {
            output_size: (1920, 1080),
            framebuffer_origin_top_left: false,
            effect_registry_generation: 1,
        };
        let gl = unsafe { glow::Context::from_loader_function(|_| std::ptr::null()) };

        let prepared =
            cache.prepare_checkpoint_captures(&gl, compatibility, Some(0), &[(cache_key, plan)]);

        assert!(prepared.bindings.is_empty());
        assert_eq!(prepared.admission.skipped_size, 1);
        assert_eq!(prepared.admission.skipped_budget, 0);
        assert_eq!(prepared.admission.skipped_allocation, 0);
        assert!(!prepared.admission.additional_budget_needed_known);
        assert_checkpoint_admission_partition(prepared.admission);
    }

    #[test]
    fn checkpoint_entry_limit_reports_ignored_candidates_without_reordering() {
        let graph = backdrop_stack_graph(1, EffectRegion::empty());
        let (cache_key, plan) = first_checkpoint_candidate(&graph);
        let bytes = texture_key(&plan).estimated_bytes().unwrap();
        let candidates = (0..=MAX_CHECKPOINT_CACHE_ENTRIES)
            .map(|candidate_index| {
                let mut candidate_key = cache_key.clone();
                candidate_key.consumer_semantic_signature = candidate_key
                    .consumer_semantic_signature
                    .wrapping_add(candidate_index as u64);
                (candidate_key, plan.clone())
            })
            .collect::<Vec<_>>();
        let mut cache = EffectGlResourceCache::with_budget(1).unwrap();
        let compatibility = CheckpointCacheCompatibility {
            output_size: (1920, 1080),
            framebuffer_origin_top_left: false,
            effect_registry_generation: 1,
        };
        let gl = unsafe { glow::Context::from_loader_function(|_| std::ptr::null()) };

        let prepared = cache.prepare_checkpoint_captures(&gl, compatibility, Some(0), &candidates);

        assert!(prepared.bindings.is_empty());
        assert_eq!(
            prepared.admission.candidates_total,
            MAX_CHECKPOINT_CACHE_ENTRIES + 1
        );
        assert_eq!(
            prepared.admission.candidates_considered,
            MAX_CHECKPOINT_CACHE_ENTRIES
        );
        assert_eq!(prepared.admission.skipped_entry_limit, 1);
        assert_eq!(
            prepared.admission.skipped_budget,
            MAX_CHECKPOINT_CACHE_ENTRIES
        );
        assert_eq!(
            prepared.admission.skipped_budget_bytes,
            bytes.saturating_mul(MAX_CHECKPOINT_CACHE_ENTRIES as u64)
        );
        assert_checkpoint_admission_partition(prepared.admission);
    }

    #[test]
    fn checkpoint_resident_candidate_keeps_its_physical_binding() {
        let graph = backdrop_stack_graph(1, EffectRegion::empty());
        let (cache_key, plan) = first_checkpoint_candidate(&graph);
        let graph_peak = estimate_graph_peak_bytes(&graph).unwrap();
        let checkpoint_bytes = texture_key(&plan).estimated_bytes().unwrap();
        let compatibility = CheckpointCacheCompatibility {
            output_size: (1920, 1080),
            framebuffer_origin_top_left: false,
            effect_registry_generation: 1,
        };
        let mut cache = EffectGlResourceCache::with_budget(graph_peak + checkpoint_bytes).unwrap();
        let lease = cache.pool.checkout(texture_key(&plan)).unwrap();
        let lease_id = lease.id;
        cache.checkpoint_captures.insert(
            cache_key.clone(),
            CachedCheckpointCapture {
                texture: lease,
                last_populated_frame_serial: Some(1),
            },
        );
        cache.checkpoint_compatibility = Some(compatibility);
        let allocations_before = cache.pool.metrics().allocation_count;
        let gl = unsafe { glow::Context::from_loader_function(|_| std::ptr::null()) };

        let prepared = cache.prepare_checkpoint_captures(
            &gl,
            compatibility,
            Some(graph_peak),
            &[(cache_key, plan.clone())],
        );

        assert_eq!(prepared.admission.resident_candidates, 1);
        assert_eq!(prepared.admission.newly_admitted_candidates, 0);
        assert_eq!(prepared.admission.skipped_budget, 0);
        assert_eq!(prepared.bindings[&plan.id].texture.id, lease_id);
        assert_eq!(cache.pool.metrics().allocation_count, allocations_before);
        assert_eq!(prepared.admission.base_checked_out_bytes, checkpoint_bytes);
        assert_checkpoint_admission_partition(prepared.admission);
    }

    #[test]
    fn checkpoint_new_admission_reports_the_successful_candidate() {
        let graph = backdrop_stack_graph(1, EffectRegion::empty());
        let (cache_key, plan) = first_checkpoint_candidate(&graph);
        let checkpoint_bytes = texture_key(&plan).estimated_bytes().unwrap();
        let compatibility = CheckpointCacheCompatibility {
            output_size: (1920, 1080),
            framebuffer_origin_top_left: false,
            effect_registry_generation: 1,
        };
        let mut cache = EffectGlResourceCache::with_budget(checkpoint_bytes).unwrap();
        let texture_id = 77;
        cache.pool.next_id = texture_id;
        cache.gl_textures.insert(
            texture_id,
            glow::NativeTexture(std::num::NonZeroU32::new(1).unwrap()),
        );
        let gl = unsafe { glow::Context::from_loader_function(|_| std::ptr::null()) };

        let prepared = cache.prepare_checkpoint_captures(
            &gl,
            compatibility,
            Some(0),
            &[(cache_key, plan.clone())],
        );

        assert_eq!(prepared.admission.candidates_total, 1);
        assert_eq!(prepared.admission.resident_candidates, 0);
        assert_eq!(prepared.admission.newly_admitted_candidates, 1);
        assert_eq!(prepared.admission.skipped_budget, 0);
        assert_eq!(prepared.admission.skipped_allocation, 0);
        assert!(prepared.admission.additional_budget_needed_known);
        assert_eq!(
            prepared
                .admission
                .additional_budget_needed_for_all_candidates_bytes,
            0
        );
        assert_eq!(prepared.bindings[&plan.id].texture.id, texture_id);
        assert_checkpoint_admission_partition(prepared.admission);
    }

    #[test]
    fn checkpoint_allocation_failure_is_not_reported_as_budget_pressure() {
        let graph = backdrop_stack_graph(1, EffectRegion::empty());
        let (cache_key, plan) = first_checkpoint_candidate(&graph);
        let checkpoint_bytes = texture_key(&plan).estimated_bytes().unwrap();
        let mut cache = EffectGlResourceCache::with_budget(checkpoint_bytes).unwrap();
        cache.pool.next_id = u64::MAX;
        let compatibility = CheckpointCacheCompatibility {
            output_size: (1920, 1080),
            framebuffer_origin_top_left: false,
            effect_registry_generation: 1,
        };
        let gl = unsafe { glow::Context::from_loader_function(|_| std::ptr::null()) };

        let prepared =
            cache.prepare_checkpoint_captures(&gl, compatibility, Some(0), &[(cache_key, plan)]);

        assert!(prepared.bindings.is_empty());
        assert_eq!(prepared.admission.skipped_allocation, 1);
        assert_eq!(prepared.admission.skipped_budget, 0);
        assert_eq!(prepared.admission.skipped_size, 0);
        assert_eq!(prepared.admission.newly_admitted_candidates, 0);
        assert_checkpoint_admission_partition(prepared.admission);
    }

    #[test]
    fn texture_key_includes_all_allocation_dimensions() {
        let base = key(64, 64);
        assert_ne!(base, key(32, 64));
        assert_ne!(
            base,
            EffectTextureKey::new(
                64,
                64,
                EffectTextureFormat::Rgba16Float,
                EffectTextureFilter::Linear,
                EffectWorkingSpace::LinearSrgb,
            )
        );
        assert_ne!(
            base,
            EffectTextureKey::new(
                64,
                64,
                EffectTextureFormat::Rgba8,
                EffectTextureFilter::Nearest,
                EffectWorkingSpace::LinearSrgb,
            )
        );
    }

    #[test]
    fn byte_estimation_is_checked() {
        assert_eq!(key(10, 20).estimated_bytes().unwrap(), 800);
        assert!(
            EffectTextureKey::new(
                u32::MAX,
                u32::MAX,
                EffectTextureFormat::Rgba16Float,
                EffectTextureFilter::Linear,
                EffectWorkingSpace::LinearSrgb,
            )
            .estimated_bytes()
            .is_err()
        );
    }

    #[test]
    fn budget_rejects_live_allocation_without_eviction() {
        let mut pool = EffectResourcePool::with_budget(1024).unwrap();
        let first = pool.checkout(key(16, 16)).unwrap();
        let second = pool.checkout(key(16, 16));
        assert!(matches!(
            second,
            Err(EffectResourceError::BudgetExceeded { .. })
        ));
        pool.return_texture(first).unwrap();
    }

    #[test]
    fn idle_eviction_order_is_deterministic() {
        let mut pool = EffectResourcePool::with_budget(2048).unwrap();
        let old = pool.checkout(key(16, 16)).unwrap();
        pool.return_texture(old.clone()).unwrap();
        let newer = pool.checkout(key(16, 16)).unwrap();
        pool.return_texture(newer.clone()).unwrap();
        let replacement = pool.checkout(key(32, 16)).unwrap();
        assert_eq!(pool.evicted_texture_ids(), vec![old.id]);
        pool.return_texture(replacement).unwrap();
    }

    #[test]
    fn eviction_notifications_are_consumable_without_losing_cumulative_stats() {
        let mut pool = EffectResourcePool::with_budget(2048).unwrap();
        let old = pool.checkout(key(16, 16)).unwrap();
        pool.return_texture(old.clone()).unwrap();
        let replacement = pool.checkout(key(32, 16)).unwrap();

        assert_eq!(pool.drain_evicted_texture_ids(), vec![old.id]);
        assert!(pool.drain_evicted_texture_ids().is_empty());
        assert_eq!(pool.metrics().eviction_count, 1);

        pool.return_texture(replacement).unwrap();
    }

    #[test]
    fn eviction_notifications_stay_consumable_during_repeated_churn() {
        let mut pool = EffectResourcePool::with_budget(2048).unwrap();
        let keys = [key(16, 16), key(32, 16)];
        let initial = pool.checkout(keys[0]).unwrap();
        pool.return_texture(initial).unwrap();

        const CHURN_COUNT: usize = 512;
        for index in 0..CHURN_COUNT {
            let texture = pool.checkout(keys[(index + 1) % keys.len()]).unwrap();
            assert_eq!(pool.drain_evicted_texture_ids().len(), 1);
            assert!(pool.evicted_texture_ids().is_empty());
            assert_eq!(pool.metrics().eviction_count, index + 1);
            pool.return_texture(texture).unwrap();
        }

        for _ in 0..16 {
            let texture = pool.checkout(keys[0]).unwrap();
            assert!(pool.drain_evicted_texture_ids().is_empty());
            pool.return_texture(texture).unwrap();
        }
        assert_eq!(pool.metrics().eviction_count, CHURN_COUNT);
    }

    #[test]
    fn failed_oversized_acquisition_retains_eviction_notification() {
        let mut pool = EffectResourcePool::with_budget(2048).unwrap();
        let idle = pool.checkout(key(16, 16)).unwrap();
        pool.return_texture(idle.clone()).unwrap();

        assert!(matches!(
            pool.checkout(key(64, 64)),
            Err(EffectResourceError::BudgetExceeded { .. })
        ));
        assert_eq!(pool.evicted_texture_ids(), vec![idle.id]);
        assert_eq!(pool.metrics().eviction_count, 1);

        assert_eq!(pool.drain_evicted_texture_ids(), vec![idle.id]);
        assert!(pool.drain_evicted_texture_ids().is_empty());
        assert_eq!(pool.metrics().eviction_count, 1);
    }

    #[test]
    fn size_history_cleanup_removes_idle_entries_but_keeps_live_textures() {
        let mut pool = EffectResourcePool::with_budget(16 * 1024).unwrap();
        let idle = pool.checkout(key(8, 8)).unwrap();
        pool.return_texture(idle).unwrap();
        let live = pool.checkout(key(16, 16)).unwrap();
        pool.cleanup_size_history();
        assert_eq!(pool.cached_key_count(), 1);
        assert!(pool.is_checked_out(live.id));
        pool.return_texture(live).unwrap();
    }

    #[test]
    fn compatible_lifetimes_alias_a_single_physical_target() {
        let mut pool = EffectResourcePool::new();
        let full = EffectTextureKey::new(
            1920,
            1080,
            EffectTextureFormat::Rgba16Float,
            EffectTextureFilter::Linear,
            EffectWorkingSpace::LinearSrgb,
        );
        let first = pool.checkout(full).unwrap();
        pool.return_texture(first.clone()).unwrap();
        let second = pool.checkout(full).unwrap();
        assert_eq!(first.id, second.id);
        assert!(pool.peak_bytes() <= DEFAULT_EFFECT_RESOURCE_BUDGET_BYTES);
        pool.return_texture(second).unwrap();
    }

    #[test]
    fn same_key_concurrent_checkouts_have_distinct_physical_ids() {
        let mut pool = EffectResourcePool::new();
        let key = key(32, 32);
        let first = pool.checkout(key).unwrap();
        let second = pool.checkout(key).unwrap();

        assert_ne!(first.id, second.id);

        pool.return_texture(first).unwrap();
        pool.return_texture(second).unwrap();
    }

    #[test]
    fn same_key_reuse_is_allowed_only_after_return() {
        let mut pool = EffectResourcePool::new();
        let key = key(32, 32);
        let first = pool.checkout(key).unwrap();
        pool.return_texture(first.clone()).unwrap();

        let reused = pool.checkout(key).unwrap();
        assert_eq!(reused.id, first.id);
        pool.return_texture(reused).unwrap();
    }

    fn fullscreen_blur_graph(width: u32, height: u32) -> CompiledFrameGraph {
        let effect = EffectInstanceId::new(1).unwrap();
        let anchor = oblivion_one::compositor::EffectAnchor::OutputPostProcess;
        let ids = (1..=6)
            .map(GraphTextureId::new)
            .collect::<Option<Vec<_>>>()
            .unwrap();
        let pass_ids = (1..=6)
            .map(oblivion_one::effects::GraphPassId::new)
            .collect::<Option<Vec<_>>>()
            .unwrap();
        let mut textures = vec![GraphTexturePlan {
            id: ids[0],
            source: GraphTextureSource::CapturedScene,
            width,
            height,
            domain: oblivion_one::effects::EffectRect::new(0, 0, width, height).unwrap(),
            working_space: EffectWorkingSpace::OutputEncodedSrgb,
            origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
            first_use: Some(pass_ids[0]),
            last_use: Some(pass_ids[1]),
        }];
        for (index, (texture_width, texture_height, first, last)) in [
            (width / 2, height / 2, 1, 2),
            (width / 4, height / 4, 2, 3),
            (width / 2, height / 2, 3, 4),
            (width, height, 4, 5),
        ]
        .into_iter()
        .enumerate()
        {
            textures.push(GraphTexturePlan {
                id: ids[index + 1],
                source: GraphTextureSource::Intermediate,
                width: texture_width,
                height: texture_height,
                domain: oblivion_one::effects::EffectRect::new(0, 0, width, height).unwrap(),
                working_space: EffectWorkingSpace::LinearSrgb,
                origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
                first_use: Some(pass_ids[first]),
                last_use: Some(pass_ids[last]),
            });
        }
        textures.push(GraphTexturePlan {
            id: ids[5],
            source: GraphTextureSource::Output,
            width,
            height,
            domain: oblivion_one::effects::EffectRect::new(0, 0, width, height).unwrap(),
            working_space: EffectWorkingSpace::OutputEncodedSrgb,
            origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
            first_use: Some(pass_ids[5]),
            last_use: Some(pass_ids[5]),
        });
        let input_output = [
            (vec![], ids[0]),
            (vec![ids[0]], ids[1]),
            (vec![ids[1]], ids[2]),
            (vec![ids[2]], ids[3]),
            (vec![ids[3]], ids[4]),
            (vec![ids[4]], ids[5]),
        ];
        let passes = input_output
            .into_iter()
            .zip(pass_ids)
            .map(|((inputs, output), id)| CompiledRenderPass {
                id,
                kind: RenderPassKind::Composite,
                inputs,
                output: Some(output),
                damage: EffectRegion::empty(),
                instance: effect,
                anchor,
                blur_radius: None,
                stage: None,
                fused_stages: Vec::new(),
                parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
                alpha_mode: oblivion_one::effects::EffectAlphaMode::Preserve,
                encode_output: false,
                color_conversion: oblivion_one::effects::EffectColorConversion::None,
                checkpoint_dependencies: Vec::new(),
                visual_group: None,
                anchor_scope: oblivion_one::compositor::EffectAnchorScope::VisualGroup,
                visible_clip_fallback: None,
            })
            .collect();
        CompiledFrameGraph {
            passes,
            textures,
            instances: Vec::new(),
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        }
    }

    #[test]
    fn fullscreen_blur_peak_bytes_scale_with_output_size() {
        let peak_1080p = estimate_graph_peak_bytes(&fullscreen_blur_graph(1920, 1080)).unwrap();
        let peak_1440p = estimate_graph_peak_bytes(&fullscreen_blur_graph(2560, 1440)).unwrap();
        let peak_4k = estimate_graph_peak_bytes(&fullscreen_blur_graph(3840, 2160)).unwrap();

        assert_eq!(peak_1080p, 20_736_000);
        assert_eq!(peak_1440p, 36_864_000);
        assert_eq!(peak_4k, 82_944_000);
        assert_eq!(peak_1440p, peak_1080p * 16 / 9);
        assert_eq!(peak_4k, peak_1080p * 4);
        assert_eq!(
            DEFAULT_EFFECT_RESOURCE_BUDGET_BYTES - peak_1080p,
            46_372_864
        );
        assert!(peak_4k > DEFAULT_EFFECT_RESOURCE_BUDGET_BYTES);
    }

    #[test]
    fn pool_reuse_reduces_allocations_without_changing_graph_demand() {
        let graph = fullscreen_blur_graph(1920, 1080);
        let demand = estimate_graph_peak_bytes(&graph).unwrap();
        assert_eq!(
            graph
                .textures
                .iter()
                .filter(|texture| texture.width == 960 && texture.height == 540)
                .count(),
            2
        );
        let mut pool = EffectResourcePool::new();
        let reusable = key(960, 540);
        let first = pool.checkout(reusable).unwrap();
        pool.return_texture(first).unwrap();
        let second = pool.checkout(reusable).unwrap();
        pool.return_texture(second).unwrap();

        assert_eq!(pool.metrics().allocation_count, 1);
        assert_eq!(pool.metrics().reuse_count, 1);
        assert_eq!(demand, 20_736_000);
    }

    #[test]
    fn fullscreen_1080p_blur_liveness_stays_inside_the_default_budget() {
        let effect = EffectInstanceId::new(1).unwrap();
        let anchor = oblivion_one::compositor::EffectAnchor::OutputPostProcess;
        let ids = (1..=6)
            .map(GraphTextureId::new)
            .collect::<Option<Vec<_>>>()
            .unwrap();
        let pass_ids = (1..=6)
            .map(oblivion_one::effects::GraphPassId::new)
            .collect::<Option<Vec<_>>>()
            .unwrap();
        let mut textures = Vec::new();
        textures.push(GraphTexturePlan {
            id: ids[0],
            source: GraphTextureSource::CapturedScene,
            width: 1920,
            height: 1080,
            domain: oblivion_one::effects::EffectRect::new(0, 0, 1920, 1080).unwrap(),
            working_space: EffectWorkingSpace::OutputEncodedSrgb,
            origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
            first_use: Some(pass_ids[0]),
            last_use: Some(pass_ids[1]),
        });
        for (index, (width, height, first, last)) in [
            (960, 540, 1, 2),
            (480, 270, 2, 3),
            (960, 540, 3, 4),
            (1920, 1080, 4, 5),
        ]
        .into_iter()
        .enumerate()
        {
            textures.push(GraphTexturePlan {
                id: ids[index + 1],
                source: GraphTextureSource::Intermediate,
                width,
                height,
                domain: oblivion_one::effects::EffectRect::new(0, 0, 1920, 1080).unwrap(),
                working_space: EffectWorkingSpace::LinearSrgb,
                origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
                first_use: Some(pass_ids[first]),
                last_use: Some(pass_ids[last]),
            });
        }
        textures.push(GraphTexturePlan {
            id: ids[5],
            source: GraphTextureSource::Output,
            width: 1920,
            height: 1080,
            domain: oblivion_one::effects::EffectRect::new(0, 0, 1920, 1080).unwrap(),
            working_space: EffectWorkingSpace::OutputEncodedSrgb,
            origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
            first_use: Some(pass_ids[5]),
            last_use: Some(pass_ids[5]),
        });
        let input_output = [
            (vec![], ids[0]),
            (vec![ids[0]], ids[1]),
            (vec![ids[1]], ids[2]),
            (vec![ids[2]], ids[3]),
            (vec![ids[3]], ids[4]),
            (vec![ids[4]], ids[5]),
        ];
        let passes = input_output
            .into_iter()
            .zip(pass_ids)
            .map(|((inputs, output), id)| CompiledRenderPass {
                id,
                kind: RenderPassKind::Composite,
                inputs,
                output: Some(output),
                damage: EffectRegion::empty(),
                instance: effect,
                anchor,
                blur_radius: None,
                stage: None,
                fused_stages: Vec::new(),
                parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
                alpha_mode: oblivion_one::effects::EffectAlphaMode::Preserve,
                encode_output: false,
                color_conversion: oblivion_one::effects::EffectColorConversion::None,
                checkpoint_dependencies: Vec::new(),
                visual_group: None,
                anchor_scope: oblivion_one::compositor::EffectAnchorScope::VisualGroup,
                visible_clip_fallback: None,
            })
            .collect();
        let graph = CompiledFrameGraph {
            passes,
            textures,
            instances: Vec::new(),
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        assert!(estimate_graph_peak_bytes(&graph).unwrap() < DEFAULT_EFFECT_RESOURCE_BUDGET_BYTES);
    }

    #[test]
    fn resource_metrics_distinguish_allocations_from_reuses() {
        let mut pool = EffectResourcePool::new();
        let texture = pool.checkout(key(16, 16)).unwrap();
        let live_metrics = pool.metrics();
        assert_eq!(live_metrics.allocation_count, 1);
        assert_eq!(live_metrics.reuse_count, 0);
        assert_eq!(live_metrics.current_bytes, 16 * 16 * 4);
        assert_eq!(live_metrics.peak_bytes, live_metrics.current_bytes);
        assert_eq!(
            live_metrics.budget_bytes,
            DEFAULT_EFFECT_RESOURCE_BUDGET_BYTES
        );
        assert_eq!(live_metrics.cached_key_count, 1);
        assert_eq!(live_metrics.cached_texture_count, 1);
        assert_eq!(live_metrics.checked_out_texture_count, 1);
        pool.return_texture(texture).unwrap();
        let reused = pool.checkout(key(16, 16)).unwrap();
        let reused_metrics = pool.metrics();
        assert_eq!(reused_metrics.allocation_count, 1);
        assert_eq!(reused_metrics.reuse_count, 1);
        assert_eq!(reused_metrics.current_bytes, live_metrics.current_bytes);
        assert_eq!(reused_metrics.peak_bytes, live_metrics.peak_bytes);
        pool.return_texture(reused).unwrap();
    }

    #[test]
    fn cleanup_size_history_removes_idle_dimensions_but_preserves_live_textures() {
        let mut pool = EffectResourcePool::new();
        let old = pool.checkout(key(64, 64)).unwrap();
        pool.return_texture(old.clone()).unwrap();
        let live = pool.checkout(key(32, 32)).unwrap();
        let removed = pool.cleanup_size_history();
        assert_eq!(removed, vec![old.id]);
        assert_eq!(pool.cached_key_count(), 1);
        assert!(pool.is_checked_out(live.id));
        pool.return_texture(live).unwrap();
    }
}
