use std::{ffi::c_void, time::Instant};

use glow::HasContext;

use super::super::program::create_capture_copy_program;
use super::super::*;
use super::gpu_timing::EffectGpuProfiler;
use super::resources::EffectResourceBudgetConfig;
use super::{
    ShaderProgramCache, builtin_shader_program_count, shader_cache_capacity_for_custom_shaders,
};

/// Persistent effect execution state. The renderer supplies the GL context so
/// this owner never hides context lifetime or makes GL calls globally.
pub(in crate::egl_renderer) struct EffectRuntime {
    pub(in crate::egl_renderer) capture_program: GlProgram,
    pub(in crate::egl_renderer) capture_copy_program: GlProgram,
    capture_uniform_locations: HashMap<String, Option<glow::UniformLocation>>,
    capture_copy_uniform_locations: HashMap<String, Option<glow::UniformLocation>>,
    pub(in crate::egl_renderer) effect_resources: EffectGlResourceCache,
    pub(in crate::egl_renderer) effect_registry: EffectRegistry,
    pub(in crate::egl_renderer) effect_registry_generation: u64,
    pub(in crate::egl_renderer) failed_effect_generation: Option<u64>,
    pub(in crate::egl_renderer) effect_shaders: ShaderProgramCache,
    effect_quad: Option<(GlVertexArray, GlBuffer)>,
    pub(in crate::egl_renderer) effect_trace: EffectExecutionTrace,
    pub(in crate::egl_renderer) effect_gpu_profiler: EffectGpuProfiler,
    effect_clock_start: Instant,
    pub(in crate::egl_renderer) effect_time_seconds: f32,
    pub(in crate::egl_renderer) effect_delta_seconds: f32,
    pub(in crate::egl_renderer) effect_output_scale: f32,
    pub(in crate::egl_renderer) capture_in_progress: bool,
    #[cfg(test)]
    pub(in crate::egl_renderer) fail_next_dependency_free_replay_capture: bool,
}

/// Programs allocated before scene geometry, preserving renderer initialization
/// order. Consumed once when the persistent runtime is assembled.
pub(in crate::egl_renderer) struct EffectRuntimePrograms {
    capture: GlProgram,
    copy: GlProgram,
}

impl EffectRuntimePrograms {
    pub(in crate::egl_renderer) fn initialize_uniforms(&self, gl: &glow::Context) {
        unsafe {
            gl.use_program(Some(self.capture));
            if let Some(location) = gl.get_uniform_location(self.capture, "u_texture") {
                gl.uniform_1_i32(Some(&location), 0);
            }
            if let Some(location) = gl.get_uniform_location(self.capture, "u_opacity") {
                gl.uniform_1_f32(Some(&location), 1.0);
            }
            gl.use_program(Some(self.copy));
            if let Some(location) = gl.get_uniform_location(self.copy, "u_output_texture") {
                gl.uniform_1_i32(Some(&location), 0);
            }
        }
    }
}

/// The screenshot path restores only transient effect state. GPU pools,
/// programs, shader caches, profilers, and registries remain authoritative.
#[derive(Clone, Copy)]
pub(in crate::egl_renderer) struct EffectRuntimeCaptureSnapshot {
    failed_effect_generation: Option<u64>,
    effect_trace: EffectExecutionTrace,
    effect_time_seconds: f32,
    effect_delta_seconds: f32,
    effect_output_scale: f32,
    capture_in_progress: bool,
}

impl EffectRuntimeCaptureSnapshot {
    pub(in crate::egl_renderer) fn take(runtime: &EffectRuntime) -> Self {
        Self {
            failed_effect_generation: runtime.failed_effect_generation,
            effect_trace: runtime.effect_trace,
            effect_time_seconds: runtime.effect_time_seconds,
            effect_delta_seconds: runtime.effect_delta_seconds,
            effect_output_scale: runtime.effect_output_scale,
            capture_in_progress: runtime.capture_in_progress,
        }
    }

    pub(in crate::egl_renderer) fn restore(self, runtime: &mut EffectRuntime) {
        runtime.failed_effect_generation = self.failed_effect_generation;
        runtime.effect_trace = self.effect_trace;
        runtime.effect_time_seconds = self.effect_time_seconds;
        runtime.effect_delta_seconds = self.effect_delta_seconds;
        runtime.effect_output_scale = self.effect_output_scale;
        runtime.capture_in_progress = self.capture_in_progress;
    }
}

impl EffectRuntime {
    pub(in crate::egl_renderer) fn create_programs(
        gl: &glow::Context,
    ) -> RendererResult<EffectRuntimePrograms> {
        Ok(EffectRuntimePrograms {
            capture: program::create_capture_program(gl)?,
            copy: create_capture_copy_program(gl)?,
        })
    }

    pub(in crate::egl_renderer) fn new(
        gl: &glow::Context,
        egl: &EglInstance,
        programs: EffectRuntimePrograms,
    ) -> RendererResult<Self> {
        let mut effect_shaders = ShaderProgramCache::new(builtin_shader_program_count())
            .expect("built-in shader cache capacity is non-zero");
        effect_shaders.prewarm_builtins(gl)?;
        let effect_gpu_profiler = EffectGpuProfiler::new(gl, |name| {
            egl.get_proc_address(name)
                .map(|symbol| symbol as *const c_void)
        });
        let effect_resources =
            EffectGlResourceCache::with_budget_config(EffectResourceBudgetConfig::from_env())?;

        Ok(Self {
            capture_program: programs.capture,
            capture_copy_program: programs.copy,
            capture_uniform_locations: HashMap::new(),
            capture_copy_uniform_locations: HashMap::new(),
            effect_resources,
            effect_registry: EffectRegistry::with_builtin_background_blur(),
            effect_registry_generation: 1,
            failed_effect_generation: None,
            effect_shaders,
            effect_quad: None,
            effect_trace: EffectExecutionTrace::new(None, None, None, None),
            effect_gpu_profiler,
            effect_clock_start: Instant::now(),
            effect_time_seconds: 0.0,
            effect_delta_seconds: 0.0,
            effect_output_scale: 1.0,
            capture_in_progress: false,
            #[cfg(test)]
            fail_next_dependency_free_replay_capture: false,
        })
    }

    pub(in crate::egl_renderer) fn capture_uniform_location(
        &mut self,
        gl: &glow::Context,
        name: &str,
    ) -> Option<glow::UniformLocation> {
        if let Some(location) = self.capture_uniform_locations.get(name) {
            return *location;
        }
        let location = unsafe { gl.get_uniform_location(self.capture_program, name) };
        self.capture_uniform_locations
            .insert(name.to_owned(), location);
        location
    }

    pub(in crate::egl_renderer) fn capture_copy_uniform_location(
        &mut self,
        gl: &glow::Context,
        name: &str,
    ) -> Option<glow::UniformLocation> {
        if let Some(location) = self.capture_copy_uniform_locations.get(name) {
            return *location;
        }
        let location = unsafe { gl.get_uniform_location(self.capture_copy_program, name) };
        self.capture_copy_uniform_locations
            .insert(name.to_owned(), location);
        location
    }

    pub(in crate::egl_renderer) fn ensure_effect_quad(
        &mut self,
        gl: &glow::Context,
    ) -> RendererResult<(GlVertexArray, GlBuffer)> {
        if let Some(quad) = self.effect_quad {
            return Ok(quad);
        }
        let vertex_array = unsafe { gl.create_vertex_array().map_err(io::Error::other)? };
        let vertex_buffer = unsafe { gl.create_buffer().map_err(io::Error::other)? };
        let vertices: [f32; 24] = [
            -1.0, -1.0, 0.0, 1.0, 1.0, -1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0, -1.0, -1.0, 0.0, 1.0,
            1.0, 1.0, 1.0, 0.0, -1.0, 1.0, 0.0, 0.0,
        ];
        unsafe {
            gl.bind_vertex_array(Some(vertex_array));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(&vertices),
                glow::STATIC_DRAW,
            );
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 16, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 16, 8);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_vertex_array(None);
        }
        self.effect_quad = Some((vertex_array, vertex_buffer));
        Ok((vertex_array, vertex_buffer))
    }

    pub(in crate::egl_renderer) fn destroy_persistent_resources(&mut self, gl: &glow::Context) {
        self.effect_gpu_profiler.destroy(gl);
        self.effect_shaders.clear(gl);
        self.effect_resources.destroy(gl);
        if let Some((vertex_array, vertex_buffer)) = self.effect_quad.take() {
            unsafe {
                gl.delete_buffer(vertex_buffer);
                gl.delete_vertex_array(vertex_array);
            }
        }
    }

    pub(in crate::egl_renderer) fn destroy_programs(&mut self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.capture_program);
            gl.delete_program(self.capture_copy_program);
        }
    }

    pub(in crate::egl_renderer) fn effect_clock_elapsed_seconds(&self) -> f32 {
        self.effect_clock_start.elapsed().as_secs_f32()
    }
}

impl EffectRuntime {
    pub(in crate::egl_renderer) fn set_registry(&mut self, registry: EffectRegistry) {
        self.effect_registry = registry;
        self.effect_registry_generation = self.effect_registry_generation.saturating_add(1);
        self.failed_effect_generation = None;
    }

    pub(in crate::egl_renderer) fn publish_registry_generation(
        &mut self,
        gl: &glow::Context,
        generation: EffectRegistryGeneration,
    ) -> Result<(), RegistryReloadError> {
        let capacity = shader_cache_capacity_for_custom_shaders(generation.shaders.len())
            .expect("validated shader generation size fits cache capacity");
        let mut next_shaders =
            ShaderProgramCache::new(capacity).expect("shader cache capacity is non-zero");
        let compile_result = (|| {
            next_shaders.prewarm_builtins(gl).map_err(|error| {
                RegistryReloadError::ShaderCompile {
                    module: oblivion_one::effects::ShaderModuleId::new(
                        oblivion_one::effects::INTERNAL_EFFECT_SHADER_MODULE_DOWNSAMPLE,
                    )
                    .expect("builtin shader ids are non-zero"),
                    log: error.to_string(),
                }
            })?;
            for shader in generation.shaders.values() {
                next_shaders
                    .prewarm_trusted_custom(gl, shader)
                    .map_err(|error| RegistryReloadError::ShaderCompile {
                        module: shader.module,
                        log: error.to_string(),
                    })?;
            }
            Ok::<(), RegistryReloadError>(())
        })();
        if let Err(error) = compile_result {
            next_shaders.clear(gl);
            return Err(error);
        }
        self.effect_shaders.clear(gl);
        self.effect_shaders = next_shaders;
        self.effect_registry = generation.registry;
        self.effect_registry_generation = generation.generation;
        self.failed_effect_generation = None;
        Ok(())
    }

    pub(in crate::egl_renderer) fn publish_material_generation(
        &mut self,
        generation: &EffectRegistryGeneration,
    ) {
        self.effect_registry = generation.registry.clone();
        self.effect_registry_generation = generation.generation;
        self.failed_effect_generation = None;
    }
}
