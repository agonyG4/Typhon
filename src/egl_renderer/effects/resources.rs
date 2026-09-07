use std::{collections::HashMap, io};

use glow::HasContext;
use oblivion_one::effects::{CompiledFrameGraph, EffectWorkingSpace, GraphTextureSource};

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

#[derive(Debug)]
pub struct EffectResourcePool {
    textures: HashMap<EffectTextureKey, Vec<PooledEffectTexture>>,
    current_bytes: u64,
    peak_bytes: u64,
    budget_bytes: u64,
    next_id: u64,
    clock: u64,
    evicted_ids: Vec<u64>,
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
            evicted_ids: Vec::new(),
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
    pub fn cleanup_size_history(&mut self) {
        self.textures.retain(|_, textures| {
            textures.retain(|texture| texture.checked_out);
            !textures.is_empty()
        });
        self.recalculate_current_bytes();
    }

    #[allow(dead_code)]
    pub fn budget_bytes(&self) -> u64 {
        self.budget_bytes
    }

    #[allow(dead_code)]
    pub fn current_bytes(&self) -> u64 {
        self.current_bytes
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

    pub fn evicted_texture_ids(&self) -> Vec<u64> {
        self.evicted_ids.clone()
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
            eviction_count: self.evicted_ids.len(),
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
                self.evicted_ids.push(id);
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
}

#[allow(dead_code)]
impl EffectGlResourceCache {
    pub(crate) fn new() -> Self {
        Self {
            pool: EffectResourcePool::new(),
            gl_textures: HashMap::new(),
            scratch_fbo: None,
        }
    }

    pub(crate) fn with_budget(budget_bytes: u64) -> Result<Self, EffectResourceError> {
        Ok(Self {
            pool: EffectResourcePool::with_budget(budget_bytes)?,
            gl_textures: HashMap::new(),
            scratch_fbo: None,
        })
    }

    pub(crate) fn acquire(
        &mut self,
        gl: &glow::Context,
        key: EffectTextureKey,
    ) -> Result<PooledEffectTexture, Box<dyn std::error::Error>> {
        let evictions_before = self.pool.evicted_texture_ids().len();
        let texture = self.pool.checkout(key)?;
        for id in self
            .pool
            .evicted_texture_ids()
            .iter()
            .skip(evictions_before)
        {
            if let Some(gl_texture) = self.gl_textures.remove(id) {
                unsafe { gl.delete_texture(gl_texture) };
            }
        }
        if !self.gl_textures.contains_key(&texture.id) {
            let gl_texture = unsafe { gl.create_texture().map_err(io::Error::other)? };
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
            self.gl_textures.insert(texture.id, gl_texture);
        }
        Ok(texture)
    }

    pub(crate) fn prepare_graph(
        &mut self,
        gl: &glow::Context,
        graph: &CompiledFrameGraph,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut checked_out = Vec::new();
        for texture in &graph.textures {
            if texture.source == GraphTextureSource::Output {
                continue;
            }
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
            let key = EffectTextureKey::new(
                texture.width,
                texture.height,
                format,
                filter,
                EffectWorkingSpace::LinearSrgb,
            );
            match self.acquire(gl, key) {
                Ok(texture) => checked_out.push(texture),
                Err(error) => {
                    for texture in checked_out {
                        let _ = self.release(texture);
                    }
                    return Err(error);
                }
            }
        }
        for texture in checked_out {
            self.release(texture)?;
        }
        Ok(())
    }

    pub(crate) fn texture(&self, texture: &PooledEffectTexture) -> Option<glow::Texture> {
        self.gl_textures.get(&texture.id).copied()
    }

    pub(crate) fn bind_render_target(
        &mut self,
        gl: &glow::Context,
        texture: &PooledEffectTexture,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let gl_texture = self
            .texture(texture)
            .ok_or_else(|| io::Error::other("effect texture was not realized"))?;
        let framebuffer = if let Some(framebuffer) = self.scratch_fbo {
            framebuffer
        } else {
            let framebuffer = unsafe { gl.create_framebuffer().map_err(io::Error::other)? };
            self.scratch_fbo = Some(framebuffer);
            framebuffer
        };
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(gl_texture),
                0,
            );
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                return Err(io::Error::other("effect framebuffer is incomplete").into());
            }
        }
        Ok(())
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use oblivion_one::effects::EffectWorkingSpace;

    fn key(width: u32, height: u32) -> EffectTextureKey {
        EffectTextureKey::new(
            width,
            height,
            EffectTextureFormat::Rgba8,
            EffectTextureFilter::Linear,
            EffectWorkingSpace::LinearSrgb,
        )
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
}
