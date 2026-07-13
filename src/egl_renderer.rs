use std::{collections::HashMap, error::Error, ffi::c_void, io, ptr};

use glow::HasContext;
use khronos_egl as egl;
use oblivion_one::{
    compositor::{
        self, DesktopVisualState, RenderableSurface, SurfaceDamageRect, cursor_texture_pixels,
        cursor_texture_size,
    },
    render_backend::{
        buffer::{DmabufImageKey, WeakBufferIdentity},
        egl_gles::{EGL_LINUX_DMA_BUF_EXT, EglGlesDmabufImportAttributes},
    },
};

mod damage;
pub(crate) mod dmabuf;
mod geometry;
pub(crate) mod native_fence;
mod program;

pub(crate) use damage::{BufferAge, OutputDamage, OutputRect, RepaintMode, software_buffer_age};
use damage::{
    ClientCursorDamageState, EglOutputDamage, EglOutputDamageTracker,
    EglPartialRepaintCapabilities, EglPresentedDamageState, FullRepaintReason,
    PartialRepaintPlanner, RenderExecution, RepaintPlan,
};
use geometry::{
    EglDrawCommand, EglDrawLayer, EglRect, EglTexturedVertex, EglUvRect, MIN_VERTEX_BUFFER_BYTES,
    SurfaceSampling, VERTEX_STRIDE, push_draw_command, push_draw_command_with_uv,
    surface_sampling_for_plan,
};
use program::create_texture_program;

pub(crate) type RendererResult<T> = Result<T, Box<dyn Error>>;
pub(crate) type EglInstance = egl::DynamicInstance<egl::EGL1_5>;
type GlTexture = <glow::Context as HasContext>::Texture;
type GlProgram = <glow::Context as HasContext>::Program;
type GlBuffer = <glow::Context as HasContext>::Buffer;
type GlVertexArray = <glow::Context as HasContext>::VertexArray;
pub(crate) type GlEglImageTargetTexture2DOes = unsafe extern "system" fn(u32, *mut c_void);
pub(crate) type EglSwapBuffersWithDamage = unsafe extern "system" fn(
    egl::EGLDisplay,
    egl::EGLSurface,
    *const egl::Int,
    egl::Int,
) -> egl::Boolean;
const MAX_CACHED_DMABUF_RESOURCES_PER_SURFACE: usize = 4;
const EGL_BUFFER_AGE_EXT: egl::Int = 0x313d;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeEglConfigCandidate {
    pub config_id: egl::Int,
    pub native_visual_id: u32,
    pub surface_type: egl::Int,
    pub renderable_type: egl::Int,
    pub red_size: egl::Int,
    pub green_size: egl::Int,
    pub blue_size: egl::Int,
    pub alpha_size: egl::Int,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GlesSceneFrameStats {
    pub scene_rebuilt: bool,
    pub shm_upload_bytes: usize,
    pub dmabuf_imports: usize,
    pub dmabuf_reuses: usize,
    pub dmabuf_import_failures: usize,
    pub dmabuf_cache_entries: usize,
    pub dmabuf_cache_peak_entries: usize,
    pub dmabuf_cache_evictions: usize,
    pub repaint_mode: RepaintMode,
    pub buffer_age: Option<u32>,
    pub current_damage_rects: usize,
    pub current_damage_pixels: u64,
    pub repair_damage_rects: usize,
    pub repair_damage_pixels: u64,
    pub scissor_passes: usize,
    pub draw_command_replays: usize,
    pub history_depth: usize,
    pub fallback_reason: Option<FullRepaintReason>,
    pub partial_repaint_enabled: bool,
    pub contradictory_empty_damage: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameSkipReason {
    NoLogicalDamage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EglFrameOutcome {
    Skipped {
        reason: FrameSkipReason,
        stats: GlesSceneFrameStats,
    },
    Rendered {
        commit: EglSceneFrameCommit,
        stats: GlesSceneFrameStats,
    },
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // The explicit Atomic runtime consumes this after bootstrap reordering.
pub(crate) struct EglOutputRenderTarget {
    pub(crate) framebuffer: glow::Framebuffer,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) buffer_age: BufferAge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EglSceneFrameCommit {
    repaint_plan: RepaintPlan,
    damage_state: EglPresentedDamageState,
    scene_key: EglSceneCacheKey,
}

impl EglSceneFrameCommit {
    pub(crate) const fn repaint_plan(&self) -> &RepaintPlan {
        &self.repaint_plan
    }

    #[cfg(test)]
    pub(crate) fn empty_for_test() -> Self {
        Self {
            repaint_plan: RepaintPlan {
                current_damage: OutputDamage::Empty,
                repair_damage: OutputDamage::Empty,
                buffer_age: None,
                mode: RepaintMode::Skip,
                fallback_reason: None,
            },
            damage_state: EglPresentedDamageState::empty_for_test(),
            scene_key: EglSceneCacheKey {
                width: 1,
                height: 1,
                content_generation: 0,
                output_scale_key: 0,
                surface_signature_hash: 0,
            },
        }
    }
}

pub struct EglSceneDrawRequest<'a> {
    pub width: u32,
    pub height: u32,
    pub surfaces: &'a [RenderableSurface],
    pub external_overlay_surface_ids: Vec<u32>,
    pub content_generation: u64,
    pub visual_state: DesktopVisualState,
    pub output_scale: f64,
    pub client_cursor: Option<compositor::ClientCursorRenderState<'a>>,
    pub(crate) current_damage: Option<OutputDamage>,
}

pub(crate) struct GlesSceneRenderer {
    gl: glow::Context,
    program: GlProgram,
    vertex_array: GlVertexArray,
    vertex_buffer: GlBuffer,
    vertex_buffer_capacity: usize,
    current_size: (u32, u32),
    texture_upload_rgba: Vec<u8>,
    vertices: Vec<EglTexturedVertex>,
    commands: Vec<EglDrawCommand>,
    cursor_vertices: Vec<EglTexturedVertex>,
    cursor_commands: Vec<EglDrawCommand>,
    scene_cache_key: Option<EglSceneCacheKey>,
    presented_scene_key: Option<EglSceneCacheKey>,
    wallpaper_resource: Option<EglImageResource>,
    cursor_resource: Option<EglImageResource>,
    surface_resources: HashMap<u32, EglSurfaceResource>,
    dmabuf_resource_cache: HashMap<DmabufImageKey, CachedDmabufResource<EglImageResource>>,
    dmabuf_cache_peak_entries: usize,
    active_surface_ids: Vec<u32>,
    failed_surface_generations: HashMap<u32, u64>,
    frame_resources: HashMap<compositor::ServerFrameColor, EglImageResource>,
    egl_image_target_texture_2d: Option<GlEglImageTargetTexture2DOes>,
    damage_tracker: EglOutputDamageTracker,
    repaint_planner: PartialRepaintPlanner,
    frame_stats: GlesSceneFrameStats,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GlesRendererInfo {
    pub vendor: String,
    pub renderer: String,
    pub version: String,
}

impl GlesSceneRenderer {
    pub(crate) fn new_current(
        egl: &EglInstance,
        width: u32,
        height: u32,
        egl_image_target_texture_2d: Option<GlEglImageTargetTexture2DOes>,
        partial_repaint_capabilities: EglPartialRepaintCapabilities,
    ) -> RendererResult<Self> {
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name)
                    .map(|symbol| symbol as *const c_void)
                    .unwrap_or(ptr::null())
            })
        };
        let program = create_texture_program(&gl)?;
        let vertex_array = unsafe { gl.create_vertex_array().map_err(io::Error::other)? };
        let vertex_buffer = unsafe { gl.create_buffer().map_err(io::Error::other)? };
        unsafe {
            gl.bind_vertex_array(Some(vertex_array));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
            gl.buffer_data_size(
                glow::ARRAY_BUFFER,
                MIN_VERTEX_BUFFER_BYTES as i32,
                glow::DYNAMIC_DRAW,
            );
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, VERTEX_STRIDE, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, VERTEX_STRIDE, 8);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_vertex_array(None);
            gl.use_program(Some(program));
            if let Some(location) = gl.get_uniform_location(program, "u_texture") {
                gl.uniform_1_i32(Some(&location), 0);
            }
            gl.enable(glow::BLEND);
            gl.blend_func_separate(
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
                glow::ONE,
                glow::ONE_MINUS_SRC_ALPHA,
            );
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.viewport(0, 0, width as i32, height as i32);
        }

        Ok(Self {
            gl,
            program,
            vertex_array,
            vertex_buffer,
            vertex_buffer_capacity: MIN_VERTEX_BUFFER_BYTES,
            current_size: (width, height),
            texture_upload_rgba: Vec::new(),
            vertices: Vec::new(),
            commands: Vec::new(),
            cursor_vertices: Vec::new(),
            cursor_commands: Vec::new(),
            scene_cache_key: None,
            presented_scene_key: None,
            wallpaper_resource: None,
            cursor_resource: None,
            surface_resources: HashMap::new(),
            dmabuf_resource_cache: HashMap::new(),
            dmabuf_cache_peak_entries: 0,
            active_surface_ids: Vec::new(),
            failed_surface_generations: HashMap::new(),
            frame_resources: HashMap::new(),
            egl_image_target_texture_2d,
            damage_tracker: EglOutputDamageTracker::default(),
            repaint_planner: PartialRepaintPlanner::new(
                (width, height),
                partial_repaint_capabilities,
            ),
            frame_stats: GlesSceneFrameStats::default(),
        })
    }

    pub(crate) const fn last_frame_stats(&self) -> GlesSceneFrameStats {
        self.frame_stats
    }

    pub(crate) fn renderer_info(&self) -> GlesRendererInfo {
        GlesRendererInfo {
            vendor: unsafe { self.gl.get_parameter_string(glow::VENDOR) },
            renderer: unsafe { self.gl.get_parameter_string(glow::RENDERER) },
            version: unsafe { self.gl.get_parameter_string(glow::VERSION) },
        }
    }

    pub(crate) fn draw_scene(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        egl_surface: egl::Surface,
        request: EglSceneDrawRequest<'_>,
    ) -> RendererResult<EglFrameOutcome> {
        let buffer_age = query_egl_buffer_age(
            egl,
            egl_display,
            egl_surface,
            self.repaint_planner.capabilities().buffer_age,
        );
        self.draw_scene_with_buffer_age(egl, egl_display, request, buffer_age)
    }

    #[allow(dead_code)] // Wired by the explicit Atomic runtime integration task.
    pub(crate) fn draw_scene_to_target(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        target: EglOutputRenderTarget,
        mut request: EglSceneDrawRequest<'_>,
    ) -> RendererResult<EglFrameOutcome> {
        request.width = target.width;
        request.height = target.height;
        unsafe {
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, Some(target.framebuffer));
            self.gl
                .viewport(0, 0, target.width as i32, target.height as i32);
        }
        let result = self.draw_scene_with_buffer_age(egl, egl_display, request, target.buffer_age);
        unsafe {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        result
    }

    fn draw_scene_with_buffer_age(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        request: EglSceneDrawRequest<'_>,
        buffer_age: BufferAge,
    ) -> RendererResult<EglFrameOutcome> {
        let EglSceneDrawRequest {
            width,
            height,
            surfaces,
            external_overlay_surface_ids,
            content_generation,
            visual_state,
            output_scale,
            client_cursor,
            current_damage,
        } = request;
        let width = width.max(1);
        let height = height.max(1);
        let output_scale_key = compositor::output_scale_key(output_scale);
        let mut scaled_visual_state =
            compositor::scale_desktop_visual_state(visual_state, output_scale);
        if client_cursor.is_some() {
            scaled_visual_state.cursor = None;
        }
        self.frame_stats = GlesSceneFrameStats::default();
        self.ensure_output_size(egl, egl_display, width, height)?;
        self.ensure_wallpaper_resource(egl, egl_display, width, height)?;
        self.ensure_frame_resources()?;
        if scaled_visual_state.cursor.is_some() {
            self.ensure_cursor_resource(egl, egl_display)?;
        }
        self.sync_surface_resources(
            egl,
            egl_display,
            surfaces,
            client_cursor.map(|cursor| cursor.surface),
        )?;

        let (base_surfaces, overlay_surfaces) =
            split_external_overlay_surfaces(surfaces, &external_overlay_surface_ids);
        let scene_surfaces = if external_overlay_surface_ids.is_empty() {
            surfaces
        } else {
            base_surfaces.as_slice()
        };
        let surface_signatures = egl_scene_surface_signatures(surfaces);
        let candidate_scene_key = EglSceneCacheKey::new(
            width,
            height,
            content_generation,
            output_scale_key,
            &surface_signatures,
        );
        let scene_changed = self.presented_scene_key != Some(candidate_scene_key);
        let commands_changed = !self.scene_cache_is_current(
            width,
            height,
            content_generation,
            output_scale_key,
            &surface_signatures,
        );
        let client_cursor_damage = client_cursor.map(|cursor| {
            ClientCursorDamageState::new(
                compositor::scale_logical_coordinate(
                    cursor.logical_x.saturating_add(cursor.surface.x),
                    output_scale,
                ),
                compositor::scale_logical_coordinate(
                    cursor.logical_y.saturating_add(cursor.surface.y),
                    output_scale,
                ),
                compositor::scale_logical_extent(cursor.surface.width, output_scale),
                compositor::scale_logical_extent(cursor.surface.height, output_scale),
                cursor.surface.generation,
                width,
                height,
            )
        });
        let mut output_damage = self.damage_tracker.damage_for_frame(
            width,
            height,
            scene_changed,
            current_damage,
            scaled_visual_state,
            client_cursor_damage,
        );
        if scene_changed && output_damage == OutputDamage::Empty {
            self.frame_stats.contradictory_empty_damage = true;
            output_damage = OutputDamage::Full;
        }
        let damage_state = EglOutputDamageTracker::candidate_state(
            width,
            height,
            scaled_visual_state,
            client_cursor_damage,
        );

        if commands_changed {
            self.frame_stats.scene_rebuilt = true;
            self.rebuild_scene_commands(
                width,
                height,
                scene_surfaces,
                content_generation,
                output_scale,
                output_scale_key,
                &surface_signatures,
            );
        }
        self.rebuild_overlay_commands(
            width,
            height,
            scaled_visual_state,
            &overlay_surfaces,
            client_cursor,
            output_scale,
        );
        let plan = self.repaint_planner.plan(output_damage, buffer_age);
        if plan.mode == RepaintMode::Skip {
            self.record_repaint_stats(&plan);
            return Ok(EglFrameOutcome::Skipped {
                reason: FrameSkipReason::NoLogicalDamage,
                stats: self.frame_stats,
            });
        }
        if let Err(error) = self.draw_textured_layers(&plan) {
            self.repaint_planner.invalidate();
            return Err(error);
        }
        self.record_repaint_stats(&plan);
        Ok(EglFrameOutcome::Rendered {
            commit: EglSceneFrameCommit {
                repaint_plan: plan,
                damage_state,
                scene_key: candidate_scene_key,
            },
            stats: self.frame_stats,
        })
    }

    pub(crate) fn commit_presented(&mut self, frame: EglSceneFrameCommit) {
        self.repaint_planner.commit_presented(&frame.repaint_plan);
        self.damage_tracker.commit_presented(frame.damage_state);
        self.presented_scene_key = Some(frame.scene_key);
        self.frame_stats.history_depth = self.repaint_planner.history_depth();
    }

    pub(crate) fn discard_rendered(&mut self, frame: EglSceneFrameCommit) {
        self.repaint_planner.discard_rendered(&frame.repaint_plan);
    }

    pub(crate) fn frame_swap_failed(&mut self) {
        self.repaint_planner.swap_failed();
        self.frame_stats.history_depth = 0;
    }

    fn record_repaint_stats(&mut self, plan: &RepaintPlan) {
        let (width, height) = self.current_size;
        self.frame_stats.repaint_mode = plan.mode;
        self.frame_stats.buffer_age = plan.buffer_age;
        self.frame_stats.current_damage_rects = plan.current_damage.rect_count();
        self.frame_stats.current_damage_pixels = plan
            .current_damage
            .pixels(width, height)
            .unwrap_or(u64::MAX);
        self.frame_stats.repair_damage_rects = plan.repair_damage.rect_count();
        self.frame_stats.repair_damage_pixels =
            plan.repair_damage.pixels(width, height).unwrap_or(u64::MAX);
        self.frame_stats.fallback_reason = plan.fallback_reason;
        self.frame_stats.partial_repaint_enabled = self.repaint_planner.partial_enabled();
        self.frame_stats.history_depth = self.repaint_planner.history_depth();
    }

    fn ensure_output_size(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        width: u32,
        height: u32,
    ) -> RendererResult<()> {
        if self.current_size == (width, height) {
            return Ok(());
        }

        self.current_size = (width, height);
        self.repaint_planner.resize((width, height));
        if let Some(resource) = self.wallpaper_resource.take() {
            destroy_image_resource(&self.gl, egl, egl_display, resource);
        }
        self.scene_cache_key = None;
        unsafe {
            self.gl.viewport(0, 0, width as i32, height as i32);
        }
        Ok(())
    }

    fn ensure_wallpaper_resource(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        width: u32,
        height: u32,
    ) -> RendererResult<()> {
        if self
            .wallpaper_resource
            .as_ref()
            .is_some_and(|resource| resource.size == (width, height))
        {
            return Ok(());
        }

        let mut pixels = vec![compositor::OUTPUT_BACKGROUND; width as usize * height as usize];
        compositor::draw_wallpaper(&mut pixels, width, height);
        let mut resource = create_uploaded_resource(&self.gl, width, height)?;
        write_argb_pixels_to_resource(
            &self.gl,
            &resource,
            SurfaceDamageRect::full(width, height),
            &pixels,
            &mut self.texture_upload_rgba,
        );
        if let Some(old) = self.wallpaper_resource.take() {
            destroy_image_resource(&self.gl, egl, egl_display, old);
        }
        resource.generation = 1;
        self.wallpaper_resource = Some(resource);
        Ok(())
    }

    fn ensure_cursor_resource(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
    ) -> RendererResult<()> {
        let (width, height) = cursor_texture_size();
        if width == 0 || height == 0 {
            return Ok(());
        }
        if self
            .cursor_resource
            .as_ref()
            .is_some_and(|resource| resource.size == (width, height))
        {
            return Ok(());
        }

        let pixels = cursor_texture_pixels();
        let mut resource = create_uploaded_resource(&self.gl, width, height)?;
        write_argb_pixels_to_resource(
            &self.gl,
            &resource,
            SurfaceDamageRect::full(width, height),
            &pixels,
            &mut self.texture_upload_rgba,
        );
        if let Some(old) = self.cursor_resource.take() {
            destroy_image_resource(&self.gl, egl, egl_display, old);
        }
        resource.generation = 1;
        self.cursor_resource = Some(resource);
        Ok(())
    }

    fn ensure_frame_resources(&mut self) -> RendererResult<()> {
        for color in compositor::ServerFrameColor::ALL {
            if self.frame_resources.contains_key(&color) {
                continue;
            }

            let mut resource = create_uploaded_resource(&self.gl, 1, 1)?;
            write_argb_pixels_to_resource(
                &self.gl,
                &resource,
                SurfaceDamageRect::full(1, 1),
                &[color.pixel()],
                &mut self.texture_upload_rgba,
            );
            resource.generation = 1;
            self.frame_resources.insert(color, resource);
        }
        Ok(())
    }

    fn sync_surface_resources(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        surfaces: &[RenderableSurface],
        client_cursor: Option<&RenderableSurface>,
    ) -> RendererResult<()> {
        self.evict_dead_cached_dmabufs(egl, egl_display);
        self.active_surface_ids.clear();
        self.active_surface_ids
            .extend(surfaces.iter().map(|surface| surface.surface_id));
        self.active_surface_ids
            .extend(client_cursor.map(|surface| surface.surface_id));
        self.active_surface_ids.sort_unstable();
        self.active_surface_ids.dedup();

        let stale_ids = self
            .surface_resources
            .keys()
            .copied()
            .filter(|id| self.active_surface_ids.binary_search(id).is_err())
            .collect::<Vec<_>>();
        for surface_id in stale_ids {
            if let Some(resource) = self.surface_resources.remove(&surface_id) {
                destroy_surface_resource(&self.gl, egl, egl_display, resource);
            }
            self.destroy_cached_dmabufs_for_surface(egl, egl_display, surface_id);
            self.failed_surface_generations.remove(&surface_id);
        }

        for surface in surfaces.iter().chain(client_cursor) {
            let update = self
                .surface_resources
                .get(&surface.surface_id)
                .map_or(EglSurfaceResourceUpdate::Recreate, |resource| {
                    resource.update_for(surface)
                });
            match update {
                EglSurfaceResourceUpdate::Reuse => continue,
                EglSurfaceResourceUpdate::ReuseDmabuf => {
                    if let Some(resource) = self.surface_resources.get_mut(&surface.surface_id) {
                        resource.image.generation = surface.generation;
                    }
                    self.frame_stats.dmabuf_reuses =
                        self.frame_stats.dmabuf_reuses.saturating_add(1);
                    continue;
                }
                EglSurfaceResourceUpdate::UploadDamage => {
                    if let Some(resource) = self.surface_resources.get_mut(&surface.surface_id) {
                        self.frame_stats.shm_upload_bytes = self
                            .frame_stats
                            .shm_upload_bytes
                            .saturating_add(resource.write_shm_damage(
                                &self.gl,
                                surface,
                                &mut self.texture_upload_rgba,
                            ));
                    }
                    continue;
                }
                EglSurfaceResourceUpdate::Recreate if surface.dmabuf_handle().is_some() => {
                    self.switch_dmabuf_surface_resource(egl, egl_display, surface)?;
                    continue;
                }
                EglSurfaceResourceUpdate::Recreate => {}
                EglSurfaceResourceUpdate::UnsupportedBuffer => {
                    if let Some(resource) = self.surface_resources.remove(&surface.surface_id) {
                        destroy_surface_resource(&self.gl, egl, egl_display, resource);
                    }
                    self.destroy_cached_dmabufs_for_surface(egl, egl_display, surface.surface_id);
                    continue;
                }
            }

            if let Some(old) = self.surface_resources.remove(&surface.surface_id) {
                destroy_surface_resource(&self.gl, egl, egl_display, old);
            }
            if surface.dmabuf_handle().is_none() {
                self.destroy_cached_dmabufs_for_surface(egl, egl_display, surface.surface_id);
            }

            match create_surface_resource(
                &self.gl,
                egl,
                egl_display,
                self.egl_image_target_texture_2d,
                surface,
                &mut self.texture_upload_rgba,
            ) {
                Ok(resource) => {
                    if surface.cpu_pixels().is_some() {
                        self.frame_stats.shm_upload_bytes = self
                            .frame_stats
                            .shm_upload_bytes
                            .saturating_add(surface_upload_byte_len(surface));
                    } else if surface.dmabuf_handle().is_some() {
                        self.frame_stats.dmabuf_imports =
                            self.frame_stats.dmabuf_imports.saturating_add(1);
                    }
                    self.failed_surface_generations.remove(&surface.surface_id);
                    self.surface_resources.insert(surface.surface_id, resource);
                }
                Err(error) => {
                    if surface.dmabuf_handle().is_some() {
                        self.frame_stats.dmabuf_import_failures =
                            self.frame_stats.dmabuf_import_failures.saturating_add(1);
                    }
                    let should_log = self
                        .failed_surface_generations
                        .get(&surface.surface_id)
                        .is_none_or(|generation| *generation != surface.generation);
                    if should_log {
                        eprintln!(
                            "oblivion-one compositor: failed to import surface {} on EGL/GLES: {error}",
                            surface.surface_id
                        );
                        self.failed_surface_generations
                            .insert(surface.surface_id, surface.generation);
                    }
                }
            }
        }

        self.frame_stats.dmabuf_cache_entries = self.dmabuf_resource_cache.len();
        self.frame_stats.dmabuf_cache_peak_entries = self.dmabuf_cache_peak_entries;
        Ok(())
    }

    fn switch_dmabuf_surface_resource(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        surface: &RenderableSurface,
    ) -> RendererResult<()> {
        let Some(handle) = surface.dmabuf_handle() else {
            return Ok(());
        };
        let key = DmabufImageKey::from_handle(surface.buffer_id(), handle);

        if let Some(mut cached) = self.dmabuf_resource_cache.remove(&key) {
            if native_egl_debug_enabled() {
                eprintln!(
                    "oblivion-one compositor: dmabuf cache=hit surface={} key={key:?} texture={:?} egl_image={:?}",
                    surface.surface_id,
                    cached.image.texture,
                    cached.image.egl_image.map(|image| image.as_ptr()),
                );
            }
            cached.image.generation = surface.generation;
            self.frame_stats.dmabuf_reuses = self.frame_stats.dmabuf_reuses.saturating_add(1);
            if let Some(old) = self.surface_resources.insert(
                surface.surface_id,
                EglSurfaceResource {
                    image: cached.image,
                    dmabuf_key: Some(key),
                    buffer_lifetime: Some(surface.buffer_identity().downgrade()),
                },
            ) {
                self.cache_or_destroy_dmabuf_resource(egl, egl_display, surface.surface_id, old);
            }
            return Ok(());
        }

        if native_egl_debug_enabled() {
            eprintln!(
                "oblivion-one compositor: dmabuf cache=miss surface={} key={key:?} layout={:?}",
                surface.surface_id,
                surface.dmabuf_handle(),
            );
        }

        let Some(old) = self.surface_resources.remove(&surface.surface_id) else {
            let resource = create_surface_resource(
                &self.gl,
                egl,
                egl_display,
                self.egl_image_target_texture_2d,
                surface,
                &mut self.texture_upload_rgba,
            )?;
            self.frame_stats.dmabuf_imports = self.frame_stats.dmabuf_imports.saturating_add(1);
            self.surface_resources.insert(surface.surface_id, resource);
            return Ok(());
        };
        self.cache_or_destroy_dmabuf_resource(egl, egl_display, surface.surface_id, old);

        let resource = create_surface_resource(
            &self.gl,
            egl,
            egl_display,
            self.egl_image_target_texture_2d,
            surface,
            &mut self.texture_upload_rgba,
        )?;
        self.frame_stats.dmabuf_imports = self.frame_stats.dmabuf_imports.saturating_add(1);
        self.surface_resources.insert(surface.surface_id, resource);
        Ok(())
    }

    fn cache_or_destroy_dmabuf_resource(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        surface_id: u32,
        resource: EglSurfaceResource,
    ) {
        let Some(key) = resource.dmabuf_key else {
            destroy_surface_resource(&self.gl, egl, egl_display, resource);
            return;
        };
        let Some(buffer_lifetime) = resource.buffer_lifetime else {
            destroy_image_resource(&self.gl, egl, egl_display, resource.image);
            return;
        };
        if !buffer_lifetime.is_alive() {
            if native_egl_debug_enabled() {
                eprintln!(
                    "oblivion-one compositor: dmabuf cache=evict reason=dead-before-cache key={key:?}"
                );
            }
            destroy_image_resource(&self.gl, egl, egl_display, resource.image);
            self.frame_stats.dmabuf_cache_evictions =
                self.frame_stats.dmabuf_cache_evictions.saturating_add(1);
            return;
        }

        self.prune_cached_dmabufs_for_surface(egl, egl_display, surface_id);
        if let Some(replaced) = self.dmabuf_resource_cache.insert(
            key,
            CachedDmabufResource {
                image: resource.image,
                buffer_lifetime,
                surface_id,
            },
        ) {
            destroy_image_resource(&self.gl, egl, egl_display, replaced.image);
        }
        self.dmabuf_cache_peak_entries = self
            .dmabuf_cache_peak_entries
            .max(self.dmabuf_resource_cache.len());
    }

    fn prune_cached_dmabufs_for_surface(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        surface_id: u32,
    ) {
        let cached = self
            .dmabuf_resource_cache
            .values()
            .filter(|cached| cached.surface_id == surface_id)
            .count();
        if cached < MAX_CACHED_DMABUF_RESOURCES_PER_SURFACE {
            return;
        }
        let Some(key) = self
            .dmabuf_resource_cache
            .iter()
            .find_map(|(key, cached)| (cached.surface_id == surface_id).then_some(key.clone()))
        else {
            return;
        };
        if let Some(resource) = self.dmabuf_resource_cache.remove(&key) {
            if native_egl_debug_enabled() {
                eprintln!(
                    "oblivion-one compositor: dmabuf cache=evict reason=surface-bound key={key:?}"
                );
            }
            destroy_image_resource(&self.gl, egl, egl_display, resource.image);
            self.frame_stats.dmabuf_cache_evictions =
                self.frame_stats.dmabuf_cache_evictions.saturating_add(1);
        }
    }

    fn destroy_cached_dmabufs_for_surface(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        surface_id: u32,
    ) {
        let keys = self
            .dmabuf_resource_cache
            .iter()
            .filter_map(|(key, cached)| (cached.surface_id == surface_id).then_some(key.clone()))
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(resource) = self.dmabuf_resource_cache.remove(&key) {
                if native_egl_debug_enabled() {
                    eprintln!(
                        "oblivion-one compositor: dmabuf cache=evict reason=surface-destroyed key={key:?}"
                    );
                }
                destroy_image_resource(&self.gl, egl, egl_display, resource.image);
                self.frame_stats.dmabuf_cache_evictions =
                    self.frame_stats.dmabuf_cache_evictions.saturating_add(1);
            }
        }
    }

    fn evict_dead_cached_dmabufs(&mut self, egl: &EglInstance, egl_display: egl::Display) {
        let dead = dead_cached_dmabuf_keys(&self.dmabuf_resource_cache);
        for key in dead {
            if let Some(cached) = self.dmabuf_resource_cache.remove(&key) {
                if native_egl_debug_enabled() {
                    eprintln!(
                        "oblivion-one compositor: dmabuf cache=evict reason=buffer-dead key={key:?}"
                    );
                }
                destroy_image_resource(&self.gl, egl, egl_display, cached.image);
                self.frame_stats.dmabuf_cache_evictions =
                    self.frame_stats.dmabuf_cache_evictions.saturating_add(1);
            }
        }
    }

    fn scene_cache_is_current(
        &self,
        width: u32,
        height: u32,
        content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
    ) -> bool {
        self.scene_cache_key.is_some_and(|key| {
            key.is_current(
                width,
                height,
                content_generation,
                output_scale_key,
                surface_signatures,
            )
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "hot EGL command rebuild path passes borrowed frame state directly to avoid transient config allocation"
    )]
    fn rebuild_scene_commands(
        &mut self,
        width: u32,
        height: u32,
        surfaces: &[RenderableSurface],
        content_generation: u64,
        output_scale: f64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
    ) {
        self.vertices.clear();
        self.commands.clear();
        self.vertices.reserve((1 + surfaces.len()) * 6);
        self.commands.reserve(1 + surfaces.len());

        push_draw_command(
            &mut self.vertices,
            &mut self.commands,
            EglDrawLayer::Wallpaper,
            EglRect::new(0.0, 0.0, width as f32, height as f32),
            width,
            height,
        );

        let origins = compositor::surface_origins(surfaces);
        let render_assignments =
            compositor::surface_render_space_assignments(surfaces, output_scale);
        for ((surface, (origin_x, origin_y)), render_assignment) in
            surfaces.iter().zip(origins).zip(render_assignments)
        {
            for rect in compositor::server_frame_rects_for_surface(surface) {
                push_draw_command(
                    &mut self.vertices,
                    &mut self.commands,
                    EglDrawLayer::Solid(rect.color),
                    EglRect::new(
                        compositor::scale_logical_coordinate(
                            origin_x.saturating_add(rect.x),
                            output_scale,
                        ) as f32,
                        compositor::scale_logical_coordinate(
                            origin_y.saturating_add(rect.y),
                            output_scale,
                        ) as f32,
                        compositor::scale_logical_extent(rect.width, output_scale) as f32,
                        compositor::scale_logical_extent(rect.height, output_scale) as f32,
                    ),
                    width,
                    height,
                );
            }
            let render_plan = compositor::surface_render_plan_with_clip(
                surface,
                render_assignment.target,
                render_assignment.visual_clip,
            );
            let uv = EglUvRect::new(
                render_plan.content_uv.left,
                render_plan.content_uv.top,
                render_plan.content_uv.right,
                render_plan.content_uv.bottom,
            );
            let sampling = surface_sampling_for_plan(
                surface.buffer_size().width,
                surface.buffer_size().height,
                render_plan.content_target.x(),
                render_plan.content_target.y(),
                render_plan.content_target.width(),
                render_plan.content_target.height(),
                uv,
            );
            push_draw_command_with_uv(
                &mut self.vertices,
                &mut self.commands,
                EglDrawLayer::Surface(surface.surface_id),
                EglRect::new(
                    render_plan.content_target.x() as f32,
                    render_plan.content_target.y() as f32,
                    render_plan.content_target.width() as f32,
                    render_plan.content_target.height() as f32,
                ),
                uv,
                sampling,
                width,
                height,
            );
        }

        self.scene_cache_key = Some(EglSceneCacheKey::new(
            width,
            height,
            content_generation,
            output_scale_key,
            surface_signatures,
        ));
    }

    fn rebuild_overlay_commands(
        &mut self,
        width: u32,
        height: u32,
        visual_state: DesktopVisualState,
        overlay_surfaces: &[RenderableSurface],
        client_cursor: Option<compositor::ClientCursorRenderState<'_>>,
        output_scale: f64,
    ) {
        self.cursor_vertices.clear();
        self.cursor_commands.clear();

        let origins = compositor::surface_origins(overlay_surfaces);
        let render_assignments =
            compositor::surface_render_space_assignments(overlay_surfaces, output_scale);
        for ((surface, (origin_x, origin_y)), render_assignment) in
            overlay_surfaces.iter().zip(origins).zip(render_assignments)
        {
            push_egl_surface_commands(
                &mut self.cursor_vertices,
                &mut self.cursor_commands,
                width,
                height,
                surface,
                origin_x,
                origin_y,
                render_assignment,
                output_scale,
            );
        }

        if let Some((cursor_x, cursor_y)) = visual_state.cursor
            && let Some(cursor) = self.cursor_resource.as_ref()
        {
            push_draw_command(
                &mut self.cursor_vertices,
                &mut self.cursor_commands,
                EglDrawLayer::Cursor,
                EglRect::new(
                    cursor_x as f32,
                    cursor_y as f32,
                    cursor.size.0 as f32,
                    cursor.size.1 as f32,
                ),
                width,
                height,
            );
        }

        if let Some(cursor) = client_cursor
            && self
                .surface_resources
                .contains_key(&cursor.surface.surface_id)
        {
            let visual_target = compositor::SurfaceTargetRect::new(
                compositor::scale_logical_coordinate(
                    cursor.logical_x.saturating_add(cursor.surface.x),
                    output_scale,
                ),
                compositor::scale_logical_coordinate(
                    cursor.logical_y.saturating_add(cursor.surface.y),
                    output_scale,
                ),
                compositor::scale_logical_extent(cursor.surface.width, output_scale),
                compositor::scale_logical_extent(cursor.surface.height, output_scale),
            );
            let render_plan = compositor::surface_render_plan(cursor.surface, visual_target);
            let uv = EglUvRect::new(
                render_plan.content_uv.left,
                render_plan.content_uv.top,
                render_plan.content_uv.right,
                render_plan.content_uv.bottom,
            );
            push_draw_command_with_uv(
                &mut self.cursor_vertices,
                &mut self.cursor_commands,
                EglDrawLayer::Surface(cursor.surface.surface_id),
                EglRect::new(
                    render_plan.content_target.x() as f32,
                    render_plan.content_target.y() as f32,
                    render_plan.content_target.width() as f32,
                    render_plan.content_target.height() as f32,
                ),
                uv,
                surface_sampling_for_plan(
                    cursor.surface.buffer_size().width,
                    cursor.surface.buffer_size().height,
                    render_plan.content_target.x(),
                    render_plan.content_target.y(),
                    render_plan.content_target.width(),
                    render_plan.content_target.height(),
                    uv,
                ),
                width,
                height,
            );
        }
    }

    fn draw_textured_layers(&mut self, plan: &RepaintPlan) -> RendererResult<()> {
        let command_count = self
            .commands
            .len()
            .saturating_add(self.cursor_commands.len());
        unsafe {
            self.gl.clear_color(0.0, 0.0, 0.0, 1.0);
            self.gl.use_program(Some(self.program));
            self.gl.active_texture(glow::TEXTURE0);
            self.gl.bind_vertex_array(Some(self.vertex_array));
        }

        let execution = plan
            .render_execution(self.current_size.0, self.current_size.1)
            .ok_or_else(|| io::Error::other("repaint execution conversion failed"))?;
        match execution {
            RenderExecution::Full => {
                unsafe {
                    self.gl.disable(glow::SCISSOR_TEST);
                    self.gl.clear(glow::COLOR_BUFFER_BIT);
                }
                self.draw_command_batch(true)?;
                self.draw_command_batch(false)?;
                self.frame_stats.draw_command_replays = command_count;
            }
            RenderExecution::Scissored {
                scissors,
                disable_scissor_after,
            } => {
                unsafe {
                    self.gl.enable(glow::SCISSOR_TEST);
                }
                let mut draw_result = Ok(());
                for [x, y, width, height] in &scissors {
                    unsafe {
                        self.gl.scissor(*x, *y, *width, *height);
                        self.gl.clear(glow::COLOR_BUFFER_BIT);
                    }
                    draw_result = self
                        .draw_command_batch(true)
                        .and_then(|()| self.draw_command_batch(false));
                    if draw_result.is_err() {
                        break;
                    }
                }
                if disable_scissor_after {
                    unsafe {
                        self.gl.disable(glow::SCISSOR_TEST);
                    }
                }
                draw_result?;
                self.frame_stats.scissor_passes = scissors.len();
                self.frame_stats.draw_command_replays =
                    command_count.saturating_mul(scissors.len());
            }
        }

        unsafe {
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl.bind_vertex_array(None);
            self.gl.bind_texture(glow::TEXTURE_2D, None);
        }
        Ok(())
    }

    fn draw_command_batch(&mut self, scene: bool) -> RendererResult<()> {
        let (vertices, commands) = if scene {
            (&self.vertices, &self.commands)
        } else {
            (&self.cursor_vertices, &self.cursor_commands)
        };
        if vertices.is_empty() || commands.is_empty() {
            return Ok(());
        }

        ensure_vertex_buffer_capacity(
            &self.gl,
            self.vertex_buffer,
            &mut self.vertex_buffer_capacity,
            vertices.len() * std::mem::size_of::<EglTexturedVertex>(),
        );
        unsafe {
            self.gl
                .bind_buffer(glow::ARRAY_BUFFER, Some(self.vertex_buffer));
            self.gl.buffer_sub_data_u8_slice(
                glow::ARRAY_BUFFER,
                0,
                bytemuck::cast_slice(vertices.as_slice()),
            );
        }

        let mut current_sampling = None;
        for command in commands {
            let Some(texture) = self.texture_for_layer(command.layer) else {
                continue;
            };
            unsafe {
                self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                if current_sampling != Some(command.sampling) {
                    let filter = match command.sampling {
                        SurfaceSampling::ExactNearest => glow::NEAREST,
                        SurfaceSampling::ScaledLinear => glow::LINEAR,
                    } as i32;
                    self.gl
                        .tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter);
                    self.gl
                        .tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter);
                    current_sampling = Some(command.sampling);
                }
                self.gl.draw_arrays(
                    glow::TRIANGLES,
                    command.vertex_start as i32,
                    command.vertex_count as i32,
                );
            }
        }
        Ok(())
    }

    fn texture_for_layer(&self, layer: EglDrawLayer) -> Option<GlTexture> {
        match layer {
            EglDrawLayer::Wallpaper => self
                .wallpaper_resource
                .as_ref()
                .map(|resource| resource.texture),
            EglDrawLayer::Solid(color) => self
                .frame_resources
                .get(&color)
                .map(|resource| resource.texture),
            EglDrawLayer::Surface(surface_id) => self
                .surface_resources
                .get(&surface_id)
                .map(|resource| resource.image.texture),
            EglDrawLayer::Cursor => self
                .cursor_resource
                .as_ref()
                .map(|resource| resource.texture),
        }
    }

    pub(crate) fn destroy(&mut self, egl: &EglInstance, egl_display: egl::Display) {
        if let Some(resource) = self.wallpaper_resource.take() {
            destroy_image_resource(&self.gl, egl, egl_display, resource);
        }
        if let Some(resource) = self.cursor_resource.take() {
            destroy_image_resource(&self.gl, egl, egl_display, resource);
        }
        for (_, resource) in self.frame_resources.drain() {
            destroy_image_resource(&self.gl, egl, egl_display, resource);
        }
        for (_, resource) in self.surface_resources.drain() {
            destroy_surface_resource(&self.gl, egl, egl_display, resource);
        }
        for (_, resource) in self.dmabuf_resource_cache.drain() {
            destroy_image_resource(&self.gl, egl, egl_display, resource.image);
        }

        unsafe {
            self.gl.delete_buffer(self.vertex_buffer);
            self.gl.delete_vertex_array(self.vertex_array);
            self.gl.delete_program(self.program);
        }
    }
}

struct EglImageResource {
    texture: GlTexture,
    size: (u32, u32),
    generation: u64,
    egl_image: Option<egl::Image>,
}

struct EglSurfaceResource {
    image: EglImageResource,
    dmabuf_key: Option<DmabufImageKey>,
    buffer_lifetime: Option<WeakBufferIdentity>,
}

struct CachedDmabufResource<R> {
    image: R,
    buffer_lifetime: WeakBufferIdentity,
    surface_id: u32,
}

fn dead_cached_dmabuf_keys<R>(
    cache: &HashMap<DmabufImageKey, CachedDmabufResource<R>>,
) -> Vec<DmabufImageKey> {
    cache
        .iter()
        .filter_map(|(key, cached)| (!cached.buffer_lifetime.is_alive()).then_some(key.clone()))
        .collect()
}

impl EglSurfaceResource {
    fn update_for(&self, surface: &RenderableSurface) -> EglSurfaceResourceUpdate {
        let buffer_size = surface.buffer_size();
        if self.image.size != (buffer_size.width, buffer_size.height) {
            return EglSurfaceResourceUpdate::Recreate;
        }
        if self.image.generation == surface.generation {
            return EglSurfaceResourceUpdate::Reuse;
        }
        if surface.cpu_pixels().is_some() && self.image.egl_image.is_none() {
            return EglSurfaceResourceUpdate::UploadDamage;
        }
        if surface
            .dmabuf_handle()
            .map(|handle| DmabufImageKey::from_handle(surface.buffer_id(), handle))
            .is_some_and(|key| self.dmabuf_key.as_ref() == Some(&key))
        {
            return EglSurfaceResourceUpdate::ReuseDmabuf;
        }
        if surface.dmabuf_handle().is_some() {
            return EglSurfaceResourceUpdate::Recreate;
        }
        EglSurfaceResourceUpdate::UnsupportedBuffer
    }

    fn write_shm_damage(
        &mut self,
        gl: &glow::Context,
        surface: &RenderableSurface,
        upload_rgba: &mut Vec<u8>,
    ) -> usize {
        let upload_bytes =
            write_surface_pixels_to_resource(gl, &self.image, surface, false, upload_rgba);
        self.image.generation = surface.generation;
        upload_bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EglSurfaceResourceUpdate {
    Reuse,
    ReuseDmabuf,
    UploadDamage,
    Recreate,
    UnsupportedBuffer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EglSceneCacheKey {
    width: u32,
    height: u32,
    content_generation: u64,
    output_scale_key: u32,
    surface_signature_hash: u64,
}

impl EglSceneCacheKey {
    fn new(
        width: u32,
        height: u32,
        content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
    ) -> Self {
        Self {
            width,
            height,
            content_generation,
            output_scale_key,
            surface_signature_hash: egl_scene_surface_signature_hash(surface_signatures),
        }
    }

    fn is_current(
        self,
        width: u32,
        height: u32,
        content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
    ) -> bool {
        self.width == width
            && self.height == height
            && self.content_generation == content_generation
            && self.output_scale_key == output_scale_key
            && self.surface_signature_hash == egl_scene_surface_signature_hash(surface_signatures)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EglSceneSurfaceSignature {
    surface_id: u32,
    commit_sequence: u64,
    buffer_id: u64,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    render_x: i32,
    render_y: i32,
    clip_x: i32,
    clip_y: i32,
    clip_width: u32,
    clip_height: u32,
    generation: u64,
}

fn split_external_overlay_surfaces(
    surfaces: &[RenderableSurface],
    external_overlay_surface_ids: &[u32],
) -> (Vec<RenderableSurface>, Vec<RenderableSurface>) {
    if external_overlay_surface_ids.is_empty() {
        return (Vec::new(), Vec::new());
    }
    surfaces
        .iter()
        .cloned()
        .partition(|surface| !external_overlay_surface_ids.contains(&surface.surface_id))
}

#[expect(
    clippy::too_many_arguments,
    reason = "EGL command emission keeps draw target, surface, and placement data explicit"
)]
fn push_egl_surface_commands(
    vertices: &mut Vec<EglTexturedVertex>,
    commands: &mut Vec<EglDrawCommand>,
    width: u32,
    height: u32,
    surface: &RenderableSurface,
    origin_x: i32,
    origin_y: i32,
    render_assignment: compositor::SurfaceRenderSpaceAssignment,
    output_scale: f64,
) {
    for rect in compositor::server_frame_rects_for_surface(surface) {
        push_draw_command(
            vertices,
            commands,
            EglDrawLayer::Solid(rect.color),
            EglRect::new(
                compositor::scale_logical_coordinate(origin_x.saturating_add(rect.x), output_scale)
                    as f32,
                compositor::scale_logical_coordinate(origin_y.saturating_add(rect.y), output_scale)
                    as f32,
                compositor::scale_logical_extent(rect.width, output_scale) as f32,
                compositor::scale_logical_extent(rect.height, output_scale) as f32,
            ),
            width,
            height,
        );
    }
    let render_plan = compositor::surface_render_plan_with_clip(
        surface,
        render_assignment.target,
        render_assignment.visual_clip,
    );
    let uv = EglUvRect::new(
        render_plan.content_uv.left,
        render_plan.content_uv.top,
        render_plan.content_uv.right,
        render_plan.content_uv.bottom,
    );
    let sampling = surface_sampling_for_plan(
        surface.buffer_size().width,
        surface.buffer_size().height,
        render_plan.content_target.x(),
        render_plan.content_target.y(),
        render_plan.content_target.width(),
        render_plan.content_target.height(),
        uv,
    );
    push_draw_command_with_uv(
        vertices,
        commands,
        EglDrawLayer::Surface(surface.surface_id),
        EglRect::new(
            render_plan.content_target.x() as f32,
            render_plan.content_target.y() as f32,
            render_plan.content_target.width() as f32,
            render_plan.content_target.height() as f32,
        ),
        uv,
        sampling,
        width,
        height,
    );
}

fn egl_scene_surface_signatures(surfaces: &[RenderableSurface]) -> Vec<EglSceneSurfaceSignature> {
    surfaces
        .iter()
        .map(|surface| {
            let render_placement = surface.render_placement.unwrap_or(surface.placement);
            let clip = surface.visual_clip;
            EglSceneSurfaceSignature {
                surface_id: surface.surface_id,
                commit_sequence: surface.commit_sequence.get(),
                buffer_id: surface.buffer_id().get(),
                x: surface.x,
                y: surface.y,
                width: surface.width,
                height: surface.height,
                render_x: render_placement.local_x,
                render_y: render_placement.local_y,
                clip_x: clip.map_or(0, |clip| clip.x()),
                clip_y: clip.map_or(0, |clip| clip.y()),
                clip_width: clip.map_or(0, |clip| clip.width()),
                clip_height: clip.map_or(0, |clip| clip.height()),
                generation: surface.generation,
            }
        })
        .collect()
}

fn egl_scene_surface_signature_hash(signatures: &[EglSceneSurfaceSignature]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for signature in signatures {
        hash = fnv1a_u64(hash, u64::from(signature.surface_id));
        hash = fnv1a_u64(hash, signature.commit_sequence);
        hash = fnv1a_u64(hash, signature.buffer_id);
        hash = fnv1a_u64(hash, signature.x as u32 as u64);
        hash = fnv1a_u64(hash, signature.y as u32 as u64);
        hash = fnv1a_u64(hash, u64::from(signature.width));
        hash = fnv1a_u64(hash, u64::from(signature.height));
        hash = fnv1a_u64(hash, signature.render_x as u32 as u64);
        hash = fnv1a_u64(hash, signature.render_y as u32 as u64);
        hash = fnv1a_u64(hash, signature.clip_x as u32 as u64);
        hash = fnv1a_u64(hash, signature.clip_y as u32 as u64);
        hash = fnv1a_u64(hash, u64::from(signature.clip_width));
        hash = fnv1a_u64(hash, u64::from(signature.clip_height));
        hash = fnv1a_u64(hash, signature.generation);
    }
    hash
}

const fn fnv1a_u64(hash: u64, value: u64) -> u64 {
    (hash ^ value).wrapping_mul(0x0000_0100_0000_01b3)
}

fn surface_upload_byte_len(surface: &RenderableSurface) -> usize {
    let size = surface.buffer_size();
    (size.width as usize)
        .saturating_mul(size.height as usize)
        .saturating_mul(4)
}

fn create_surface_resource(
    gl: &glow::Context,
    egl: &EglInstance,
    egl_display: egl::Display,
    egl_image_target_texture_2d: Option<GlEglImageTargetTexture2DOes>,
    surface: &RenderableSurface,
    upload_rgba: &mut Vec<u8>,
) -> RendererResult<EglSurfaceResource> {
    let image = if surface.cpu_pixels().is_some() {
        let buffer_size = surface.buffer_size();
        let mut resource = create_uploaded_resource(gl, buffer_size.width, buffer_size.height)?;
        write_surface_pixels_to_resource(gl, &resource, surface, true, upload_rgba);
        resource.generation = surface.generation;
        resource
    } else if let Some(handle) = surface.dmabuf_handle() {
        create_dmabuf_resource(
            gl,
            egl,
            egl_display,
            egl_image_target_texture_2d,
            handle,
            surface.generation,
        )?
    } else {
        return Err(io::Error::other("surface has no importable buffer").into());
    };

    if native_egl_debug_enabled() && surface.dmabuf_handle().is_some() {
        eprintln!(
            "oblivion-one compositor: dmabuf cache=create surface={} buffer_id={} texture={:?} egl_image={:?}",
            surface.surface_id,
            surface.buffer_id().get(),
            image.texture,
            image.egl_image.map(|egl_image| egl_image.as_ptr()),
        );
    }

    Ok(EglSurfaceResource {
        image,
        dmabuf_key: surface
            .dmabuf_handle()
            .map(|handle| DmabufImageKey::from_handle(surface.buffer_id(), handle)),
        buffer_lifetime: surface
            .dmabuf_handle()
            .map(|_| surface.buffer_identity().downgrade()),
    })
}

fn create_uploaded_resource(
    gl: &glow::Context,
    width: u32,
    height: u32,
) -> RendererResult<EglImageResource> {
    let texture = unsafe { gl.create_texture().map_err(io::Error::other)? };
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        configure_texture(gl);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA as i32,
            width as i32,
            height as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
    }
    Ok(EglImageResource {
        texture,
        size: (width, height),
        generation: 0,
        egl_image: None,
    })
}

fn create_dmabuf_resource(
    gl: &glow::Context,
    egl: &EglInstance,
    egl_display: egl::Display,
    egl_image_target_texture_2d: Option<GlEglImageTargetTexture2DOes>,
    handle: &oblivion_one::render_backend::buffer::DmabufBufferHandle,
    generation: u64,
) -> RendererResult<EglImageResource> {
    let Some(egl_image_target_texture_2d) = egl_image_target_texture_2d else {
        return Err(io::Error::other("GL_OES_EGL_image is unavailable").into());
    };
    let attributes = EglGlesDmabufImportAttributes::from_handle(handle).map_err(|error| {
        io::Error::other(format!("invalid dmabuf import attributes: {error:?}"))
    })?;
    let no_context = unsafe { egl::Context::from_ptr(egl::NO_CONTEXT) };
    let null_client_buffer = unsafe { egl::ClientBuffer::from_ptr(ptr::null_mut()) };
    let image = egl.create_image(
        egl_display,
        no_context,
        EGL_LINUX_DMA_BUF_EXT,
        null_client_buffer,
        attributes.as_slice(),
    )?;
    let texture = unsafe { gl.create_texture().map_err(io::Error::other)? };
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        configure_texture(gl);
        egl_image_target_texture_2d(glow::TEXTURE_2D, image.as_ptr());
        let error = gl.get_error();
        if error != glow::NO_ERROR {
            gl.delete_texture(texture);
            let _ = egl.destroy_image(egl_display, image);
            return Err(io::Error::other(format!(
                "glEGLImageTargetTexture2DOES failed with GL error 0x{error:x}"
            ))
            .into());
        }
    }

    let size = handle.size();
    Ok(EglImageResource {
        texture,
        size: (size.width, size.height),
        generation,
        egl_image: Some(image),
    })
}

fn write_surface_pixels_to_resource(
    gl: &glow::Context,
    resource: &EglImageResource,
    surface: &RenderableSurface,
    force_full_upload: bool,
    upload_rgba: &mut Vec<u8>,
) -> usize {
    if force_full_upload
        || surface.damage.is_full()
        || surface
            .damage
            .covers_surface(surface.buffer_size().width, surface.buffer_size().height)
    {
        let Some(pixels) = surface.cpu_pixels() else {
            return 0;
        };
        let buffer_size = surface.buffer_size();
        return write_argb_pixels_to_resource(
            gl,
            resource,
            SurfaceDamageRect::full(buffer_size.width, buffer_size.height),
            pixels,
            upload_rgba,
        );
    }

    let buffer_size = surface.buffer_size();
    let mut uploaded_bytes = 0usize;
    for rect in surface
        .damage
        .clipped_rects(buffer_size.width, buffer_size.height)
    {
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        if !pack_surface_rect_rgba(surface, rect, upload_rgba) {
            continue;
        }
        uploaded_bytes = uploaded_bytes.saturating_add(write_rgba_bytes_to_resource(
            gl,
            resource,
            rect,
            upload_rgba,
        ));
    }
    uploaded_bytes
}

fn write_argb_pixels_to_resource(
    gl: &glow::Context,
    resource: &EglImageResource,
    rect: SurfaceDamageRect,
    pixels: &[u32],
    upload_rgba: &mut Vec<u8>,
) -> usize {
    pack_argb_pixels_rgba(pixels, upload_rgba);
    write_rgba_bytes_to_resource(gl, resource, rect, upload_rgba)
}

fn write_rgba_bytes_to_resource(
    gl: &glow::Context,
    resource: &EglImageResource,
    rect: SurfaceDamageRect,
    rgba: &[u8],
) -> usize {
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(resource.texture));
        gl.tex_sub_image_2d(
            glow::TEXTURE_2D,
            0,
            rect.x as i32,
            rect.y as i32,
            rect.width as i32,
            rect.height as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(rgba)),
        );
    }
    (rect.width as usize)
        .saturating_mul(rect.height as usize)
        .saturating_mul(4)
}

fn pack_argb_pixels_rgba(pixels: &[u32], output: &mut Vec<u8>) {
    output.resize(pixels.len().saturating_mul(4), 0);
    for (index, &pixel) in pixels.iter().enumerate() {
        let base = index * 4;
        output[base] = ((pixel >> 16) & 0xff) as u8;
        output[base + 1] = ((pixel >> 8) & 0xff) as u8;
        output[base + 2] = (pixel & 0xff) as u8;
        output[base + 3] = ((pixel >> 24) & 0xff) as u8;
    }
}

fn pack_surface_rect_rgba(
    surface: &RenderableSurface,
    rect: SurfaceDamageRect,
    output: &mut Vec<u8>,
) -> bool {
    let Some(surface_pixels) = surface.cpu_pixels() else {
        return false;
    };
    let surface_width = surface.buffer_size().width as usize;
    let rect_x = rect.x as usize;
    let rect_y = rect.y as usize;
    let rect_width = rect.width as usize;
    let rect_height = rect.height as usize;

    output.resize(rect_width.saturating_mul(rect_height).saturating_mul(4), 0);
    let mut output_index = 0;
    for row_index in 0..rect_height {
        let Some(start) = (rect_y + row_index)
            .checked_mul(surface_width)
            .and_then(|row_start| row_start.checked_add(rect_x))
        else {
            output.clear();
            return false;
        };
        let Some(end) = start.checked_add(rect_width) else {
            output.clear();
            return false;
        };
        let Some(row) = surface_pixels.get(start..end) else {
            output.clear();
            return false;
        };
        for &pixel in row {
            output[output_index] = ((pixel >> 16) & 0xff) as u8;
            output[output_index + 1] = ((pixel >> 8) & 0xff) as u8;
            output[output_index + 2] = (pixel & 0xff) as u8;
            output[output_index + 3] = ((pixel >> 24) & 0xff) as u8;
            output_index += 4;
        }
    }
    true
}

fn configure_texture(gl: &glow::Context) {
    unsafe {
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
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
    }
}

fn destroy_surface_resource(
    gl: &glow::Context,
    egl: &EglInstance,
    egl_display: egl::Display,
    resource: EglSurfaceResource,
) {
    destroy_image_resource(gl, egl, egl_display, resource.image);
}

fn destroy_image_resource(
    gl: &glow::Context,
    egl: &EglInstance,
    egl_display: egl::Display,
    resource: EglImageResource,
) {
    unsafe {
        gl.delete_texture(resource.texture);
    }
    if let Some(image) = resource.egl_image {
        let _ = egl.destroy_image(egl_display, image);
    }
}

fn ensure_vertex_buffer_capacity(
    gl: &glow::Context,
    vertex_buffer: GlBuffer,
    current_capacity: &mut usize,
    required_size: usize,
) {
    if *current_capacity >= required_size && *current_capacity > 0 {
        return;
    }
    let capacity = required_size
        .max(MIN_VERTEX_BUFFER_BYTES)
        .next_power_of_two();
    unsafe {
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(vertex_buffer));
        gl.buffer_data_size(glow::ARRAY_BUFFER, capacity as i32, glow::DYNAMIC_DRAW);
    }
    *current_capacity = capacity;
}

pub(crate) fn choose_native_egl_config(
    egl: &EglInstance,
    display: egl::Display,
    native_visual_id: u32,
) -> RendererResult<egl::Config> {
    let mut configs = Vec::with_capacity(egl.get_config_count(display)?);
    egl.get_configs(display, &mut configs)?;
    let candidates = configs
        .iter()
        .copied()
        .map(|config| native_egl_config_candidate(egl, display, config))
        .collect::<Result<Vec<_>, _>>()?;
    if native_egl_debug_enabled() {
        for candidate in &candidates {
            eprintln!("{}", native_egl_config_candidate_diagnostic(candidate));
        }
    }
    let selected = select_native_egl_config_candidate(&candidates, native_visual_id)?;
    configs
        .get(selected)
        .copied()
        .ok_or_else(|| io::Error::other("selected EGL config index out of range").into())
}

pub(crate) fn choose_surfaceless_egl_config(
    egl: &EglInstance,
    display: egl::Display,
    native_visual_id: u32,
) -> RendererResult<egl::Config> {
    let mut configs = Vec::with_capacity(egl.get_config_count(display)?);
    egl.get_configs(display, &mut configs)?;
    let candidates = configs
        .iter()
        .copied()
        .map(|config| native_egl_config_candidate(egl, display, config))
        .collect::<Result<Vec<_>, _>>()?;
    let selected = candidates
        .iter()
        .position(|candidate| {
            native_egl_config_candidate_matches_common(candidate, native_visual_id)
        })
        .ok_or_else(|| {
            io::Error::other(format!(
                "EGL has no GLES3-capable surfaceless config for native visual {}",
                native_visual_label(native_visual_id)
            ))
        })?;
    configs.get(selected).copied().ok_or_else(|| {
        io::Error::other("selected surfaceless EGL config index out of range").into()
    })
}

#[cfg(test)]
pub(crate) fn select_native_egl_visual_format(
    formats: &[u32],
    candidates: &[NativeEglConfigCandidate],
) -> RendererResult<u32> {
    formats
        .iter()
        .copied()
        .find(|format| select_native_egl_config_candidate(candidates, *format).is_ok())
        .ok_or_else(|| {
            let requested = formats
                .iter()
                .map(|format| native_visual_label(*format))
                .collect::<Vec<_>>()
                .join(", ");
            io::Error::other(format!(
                "EGL has no GLES3-capable GBM window config for requested native visuals: {requested}"
            ))
            .into()
        })
}

pub(crate) fn select_native_egl_config_candidate(
    candidates: &[NativeEglConfigCandidate],
    native_visual_id: u32,
) -> RendererResult<usize> {
    candidates
        .iter()
        .position(|candidate| native_egl_config_candidate_matches(candidate, native_visual_id))
        .ok_or_else(|| {
            io::Error::other(format!(
                "EGL has no GLES3-capable GBM window config for native visual {}",
                native_visual_label(native_visual_id)
            ))
            .into()
        })
}

fn native_egl_config_candidate_matches(
    candidate: &NativeEglConfigCandidate,
    native_visual_id: u32,
) -> bool {
    native_egl_config_candidate_matches_common(candidate, native_visual_id)
        && (candidate.surface_type & egl::WINDOW_BIT) != 0
}

fn native_egl_config_candidate_matches_common(
    candidate: &NativeEglConfigCandidate,
    native_visual_id: u32,
) -> bool {
    candidate.native_visual_id == native_visual_id
        && (candidate.renderable_type & egl::OPENGL_ES3_BIT) != 0
        && candidate.red_size >= 8
        && candidate.green_size >= 8
        && candidate.blue_size >= 8
}

fn native_egl_config_candidate(
    egl: &EglInstance,
    display: egl::Display,
    config: egl::Config,
) -> RendererResult<NativeEglConfigCandidate> {
    Ok(NativeEglConfigCandidate {
        config_id: egl.get_config_attrib(display, config, egl::CONFIG_ID)?,
        native_visual_id: egl.get_config_attrib(display, config, egl::NATIVE_VISUAL_ID)? as u32,
        surface_type: egl.get_config_attrib(display, config, egl::SURFACE_TYPE)?,
        renderable_type: egl.get_config_attrib(display, config, egl::RENDERABLE_TYPE)?,
        red_size: egl.get_config_attrib(display, config, egl::RED_SIZE)?,
        green_size: egl.get_config_attrib(display, config, egl::GREEN_SIZE)?,
        blue_size: egl.get_config_attrib(display, config, egl::BLUE_SIZE)?,
        alpha_size: egl.get_config_attrib(display, config, egl::ALPHA_SIZE)?,
    })
}

fn native_egl_debug_enabled() -> bool {
    std::env::var_os("OBLIVION_ONE_DEBUG_EGL").is_some()
}

fn native_egl_config_candidate_diagnostic(candidate: &NativeEglConfigCandidate) -> String {
    format!(
        "native EGL config config_id={} visual={} window={} gles3={} rgba={}/{}/{}/{} surface_type=0x{:x} renderable_type=0x{:x}",
        candidate.config_id,
        native_visual_label(candidate.native_visual_id),
        (candidate.surface_type & egl::WINDOW_BIT) != 0,
        (candidate.renderable_type & egl::OPENGL_ES3_BIT) != 0,
        candidate.red_size,
        candidate.green_size,
        candidate.blue_size,
        candidate.alpha_size,
        candidate.surface_type,
        candidate.renderable_type,
    )
}

pub(crate) fn native_visual_label(native_visual_id: u32) -> String {
    format!(
        "{}/0x{native_visual_id:08x}",
        native_visual_fourcc(native_visual_id)
    )
}

fn native_visual_fourcc(native_visual_id: u32) -> String {
    let bytes = native_visual_id.to_le_bytes();
    if bytes
        .iter()
        .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
    {
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        "????".to_string()
    }
}

pub(crate) fn create_gles_context(
    egl: &EglInstance,
    display: egl::Display,
    config: egl::Config,
) -> RendererResult<egl::Context> {
    egl.create_context(display, config, None, gles_context_attributes())
        .map_err(|error| io::Error::other(format_gles3_context_error(&error)).into())
}

fn gles_context_attributes() -> &'static [egl::Int] {
    &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE]
}

fn format_gles3_context_error(error: &dyn Error) -> String {
    format!("failed to create required GLES3 context: {error}")
}

pub(crate) fn load_egl_image_target_texture_2d(
    egl: &EglInstance,
) -> Option<GlEglImageTargetTexture2DOes> {
    let symbol = egl.get_proc_address("glEGLImageTargetTexture2DOES")?;
    Some(unsafe {
        std::mem::transmute::<extern "system" fn(), GlEglImageTargetTexture2DOes>(symbol)
    })
}

pub(crate) fn load_swap_buffers_with_damage(
    egl: &EglInstance,
    display: egl::Display,
) -> Option<EglSwapBuffersWithDamage> {
    let extensions = egl
        .query_string(Some(display), egl::EXTENSIONS)
        .ok()?
        .to_string_lossy();
    let mut extensions = extensions.split_ascii_whitespace();
    let has_khr = extensions
        .clone()
        .any(|extension| extension == "EGL_KHR_swap_buffers_with_damage");
    let has_ext = extensions.any(|extension| extension == "EGL_EXT_swap_buffers_with_damage");
    let symbol_name = if has_khr {
        "eglSwapBuffersWithDamageKHR"
    } else if has_ext {
        "eglSwapBuffersWithDamageEXT"
    } else {
        return None;
    };
    let symbol = egl.get_proc_address(symbol_name)?;
    Some(unsafe { std::mem::transmute::<extern "system" fn(), EglSwapBuffersWithDamage>(symbol) })
}

pub(crate) fn detect_partial_repaint_capabilities(
    egl: &EglInstance,
    display: egl::Display,
    swap_buffers_with_damage: bool,
) -> EglPartialRepaintCapabilities {
    let extensions = egl
        .query_string(Some(display), egl::EXTENSIONS)
        .map(|extensions| extensions.to_string_lossy().into_owned())
        .unwrap_or_default();
    EglPartialRepaintCapabilities {
        buffer_age: extensions
            .split_ascii_whitespace()
            .any(|extension| extension == "EGL_EXT_buffer_age")
            && !buffer_age_disabled(),
        swap_buffers_with_damage,
    }
}

fn query_egl_buffer_age(
    egl: &EglInstance,
    display: egl::Display,
    surface: egl::Surface,
    supported: bool,
) -> BufferAge {
    if !supported {
        return BufferAge::Unsupported;
    }
    match egl.query_surface(display, surface, EGL_BUFFER_AGE_EXT) {
        Ok(age) => BufferAge::Value(age),
        Err(error) => {
            if native_egl_debug_enabled() {
                eprintln!("EGL buffer-age query failed: {error}");
            }
            BufferAge::QueryFailed
        }
    }
}

fn buffer_age_disabled() -> bool {
    std::env::var_os("OBLIVION_ONE_DISABLE_BUFFER_AGE").is_some_and(|value| value == "1")
}

pub(crate) fn egl_swap_buffers_with_damage(
    egl: &EglInstance,
    display: egl::Display,
    surface: egl::Surface,
    swap_buffers_with_damage: Option<EglSwapBuffersWithDamage>,
    damage: &EglOutputDamage,
    output_size: (u32, u32),
) -> RendererResult<()> {
    let Some(swap_buffers_with_damage) = swap_buffers_with_damage else {
        egl.swap_buffers(display, surface)?;
        return Ok(());
    };
    let Some(rects) = damage.to_egl_rects(output_size.0, output_size.1) else {
        egl.swap_buffers(display, surface)?;
        return Ok(());
    };

    let ok = unsafe {
        swap_buffers_with_damage(
            display.as_ptr(),
            surface.as_ptr(),
            rects.as_ptr(),
            rects.rect_count() as egl::Int,
        )
    };
    if ok == egl::TRUE {
        Ok(())
    } else {
        Err(egl
            .get_error()
            .map(|error| io::Error::other(format!("eglSwapBuffersWithDamage failed: {error}")))
            .unwrap_or_else(|| io::Error::other("eglSwapBuffersWithDamage failed"))
            .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oblivion_one::render_backend::buffer::{
        BufferIdAllocator, BufferSize, DmabufBufferHandle, DmabufImageKey, DmabufPlane,
        DmabufPlaneDescriptor, DrmFormat, DrmModifier,
    };

    const XR24: u32 = u32::from_le_bytes(*b"XR24");
    const AR24: u32 = u32::from_le_bytes(*b"AR24");

    #[derive(Clone)]
    struct DropProbe(std::rc::Rc<std::cell::Cell<usize>>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.set(self.0.get().saturating_add(1));
        }
    }

    fn native_candidate(config_id: egl::Int, native_visual_id: u32) -> NativeEglConfigCandidate {
        NativeEglConfigCandidate {
            config_id,
            native_visual_id,
            surface_type: egl::WINDOW_BIT,
            renderable_type: egl::OPENGL_ES3_BIT,
            red_size: 8,
            green_size: 8,
            blue_size: 8,
            alpha_size: 0,
        }
    }

    fn config_attribute_value(attributes: &[egl::Int], key: egl::Int) -> Option<egl::Int> {
        attributes
            .chunks_exact(2)
            .take_while(|pair| pair[0] != egl::NONE)
            .find(|pair| pair[0] == key)
            .map(|pair| pair[1])
    }

    #[test]
    fn gles_context_attributes_request_client_version_3_only() {
        let client_version =
            config_attribute_value(gles_context_attributes(), egl::CONTEXT_CLIENT_VERSION).unwrap();

        assert_eq!(client_version, 3);
        assert!(!gles_context_attributes().contains(&2));
    }

    #[test]
    fn gles_context_creation_error_mentions_required_gles3() {
        let error = io::Error::other("driver rejected context");

        let message = format_gles3_context_error(&error);

        assert!(message.contains("required GLES3 context"));
        assert!(message.contains("driver rejected context"));
    }

    #[test]
    fn native_egl_config_selection_rejects_gles2_only_candidates() {
        let mut candidate = native_candidate(1, XR24);
        candidate.renderable_type = egl::OPENGL_ES2_BIT;
        let candidates = [candidate];

        let error = select_native_egl_config_candidate(&candidates, XR24).unwrap_err();

        assert!(error.to_string().contains("GLES3"));
    }

    #[test]
    fn surfaceless_config_does_not_require_window_bit() {
        let mut candidate = native_candidate(1, XR24);
        candidate.surface_type = 0;

        assert!(native_egl_config_candidate_matches_common(&candidate, XR24));
        assert!(!native_egl_config_candidate_matches(&candidate, XR24));
    }

    #[test]
    fn native_egl_config_selection_prefers_requested_xrgb8888() {
        let candidates = [
            native_candidate(1, AR24),
            native_candidate(2, XR24),
            native_candidate(3, XR24),
        ];

        let selected = select_native_egl_config_candidate(&candidates, XR24).unwrap();

        assert_eq!(selected, 1);
    }

    #[test]
    fn native_egl_config_selection_ignores_wrong_visual() {
        let candidates = [native_candidate(1, AR24)];

        assert!(select_native_egl_config_candidate(&candidates, XR24).is_err());
    }

    #[test]
    fn native_egl_config_selection_accepts_zero_alpha_for_xrgb8888() {
        let candidates = [native_candidate(7, XR24)];

        let selected = select_native_egl_config_candidate(&candidates, XR24).unwrap();

        assert_eq!(selected, 0);
    }

    #[test]
    fn native_egl_format_selection_falls_back_to_argb8888_when_xrgb8888_absent() {
        let available_formats = [XR24, AR24];
        let candidates = [native_candidate(9, AR24)];

        let selected = select_native_egl_visual_format(&available_formats, &candidates).unwrap();

        assert_eq!(selected, AR24);
    }

    #[test]
    fn native_egl_config_selection_rejects_missing_window_bit() {
        let mut candidate = native_candidate(1, XR24);
        candidate.surface_type = 0;

        assert!(select_native_egl_config_candidate(&[candidate], XR24).is_err());
    }

    #[test]
    fn native_egl_config_selection_rejects_missing_gles_renderable_bit() {
        let mut candidate = native_candidate(1, XR24);
        candidate.renderable_type = 0;

        assert!(select_native_egl_config_candidate(&[candidate], XR24).is_err());
    }

    #[test]
    fn native_egl_config_selection_diagnostic_names_requested_fourcc_and_hex() {
        let error = select_native_egl_config_candidate(&[], XR24).unwrap_err();
        let diagnostic = error.to_string();

        assert!(diagnostic.contains("XR24"));
        assert!(diagnostic.contains("0x34325258"));
    }

    #[test]
    fn output_damage_tracker_separates_scene_rebuild_from_authoritative_damage() {
        let mut tracker = EglOutputDamageTracker::default();
        tracker.damage_for_frame(
            1280,
            800,
            true,
            None,
            DesktopVisualState::wallpaper_only(),
            None,
        );
        tracker.commit_presented(EglOutputDamageTracker::candidate_state(
            1280,
            800,
            DesktopVisualState::wallpaper_only(),
            None,
        ));
        let precise = OutputDamage::rects(1280, 800, [OutputRect::new(10, 20, 30, 40)]);

        assert_eq!(
            tracker.damage_for_frame(
                1280,
                800,
                true,
                Some(precise.clone()),
                DesktopVisualState::wallpaper_only(),
                None,
            ),
            precise
        );
    }

    #[test]
    fn output_damage_tracker_limits_cursor_motion_to_old_and_new_bounds() {
        let mut tracker = EglOutputDamageTracker::default();
        tracker.damage_for_frame(
            1280,
            800,
            true,
            None,
            DesktopVisualState::with_cursor(10, 10),
            None,
        );
        tracker.commit_presented(EglOutputDamageTracker::candidate_state(
            1280,
            800,
            DesktopVisualState::with_cursor(10, 10),
            None,
        ));
        let damage = tracker.damage_for_frame(
            1280,
            800,
            false,
            Some(OutputDamage::Empty),
            DesktopVisualState::with_cursor(20, 22),
            None,
        );

        assert_eq!(damage.rect_count(), 2);
        let (cursor_width, cursor_height) = compositor::cursor_texture_size();
        assert_eq!(
            damage.pixels(1280, 800),
            Some(u64::from(cursor_width) * u64::from(cursor_height) * 2)
        );
    }

    #[test]
    fn output_damage_tracker_repeats_candidate_damage_until_presented() {
        let mut tracker = EglOutputDamageTracker::default();
        tracker.damage_for_frame(
            1280,
            800,
            true,
            None,
            DesktopVisualState::with_cursor(10, 10),
            None,
        );
        tracker.commit_presented(EglOutputDamageTracker::candidate_state(
            1280,
            800,
            DesktopVisualState::with_cursor(10, 10),
            None,
        ));

        let first = tracker.damage_for_frame(
            1280,
            800,
            false,
            Some(OutputDamage::Empty),
            DesktopVisualState::with_cursor(20, 22),
            None,
        );
        let retry = tracker.damage_for_frame(
            1280,
            800,
            false,
            Some(OutputDamage::Empty),
            DesktopVisualState::with_cursor(20, 22),
            None,
        );

        assert_eq!(retry, first);
        assert_ne!(retry, OutputDamage::Empty);
    }

    #[test]
    fn argb_pixels_pack_to_rgba_without_changing_channel_order() {
        let mut packed = Vec::new();

        pack_argb_pixels_rgba(&[0x1122_3344, 0xaa55_6677], &mut packed);

        assert_eq!(packed, vec![0x22, 0x33, 0x44, 0x11, 0x55, 0x66, 0x77, 0xaa]);
    }

    #[test]
    fn scene_cache_key_invalidates_when_surface_geometry_changes() {
        let initial_signature = EglSceneSurfaceSignature {
            surface_id: 7,
            commit_sequence: 1,
            buffer_id: 11,
            x: 10,
            y: 20,
            width: 800,
            height: 600,
            render_x: 0,
            render_y: 0,
            clip_x: 0,
            clip_y: 0,
            clip_width: 0,
            clip_height: 0,
            generation: 1,
        };
        let resized_signature = EglSceneSurfaceSignature {
            width: 420,
            height: 320,
            ..initial_signature
        };
        let key = EglSceneCacheKey::new(1280, 800, 9, 120, &[initial_signature]);

        assert!(key.is_current(1280, 800, 9, 120, &[initial_signature]));
        assert!(!key.is_current(1280, 800, 9, 120, &[resized_signature]));
    }

    #[test]
    fn scene_cache_key_invalidates_when_visual_assignment_changes() {
        let initial_signature = EglSceneSurfaceSignature {
            surface_id: 7,
            commit_sequence: 1,
            buffer_id: 11,
            x: 10,
            y: 20,
            width: 800,
            height: 600,
            render_x: 0,
            render_y: 0,
            clip_x: 0,
            clip_y: 0,
            clip_width: 0,
            clip_height: 0,
            generation: 1,
        };
        let cropped_signature = EglSceneSurfaceSignature {
            render_x: 5,
            render_y: 7,
            clip_x: 0,
            clip_y: 0,
            clip_width: 0,
            clip_height: 0,
            ..initial_signature
        };
        let key = EglSceneCacheKey::new(1280, 800, 9, 120, &[initial_signature]);

        assert!(!key.is_current(1280, 800, 9, 120, &[cropped_signature]));
    }

    #[test]
    fn scene_cache_key_invalidates_when_visible_buffer_identity_changes() {
        let initial = EglSceneSurfaceSignature {
            surface_id: 7,
            commit_sequence: 1,
            buffer_id: 11,
            x: 10,
            y: 20,
            width: 800,
            height: 600,
            render_x: 0,
            render_y: 0,
            clip_x: 0,
            clip_y: 0,
            clip_width: 0,
            clip_height: 0,
            generation: 1,
        };
        let replacement = EglSceneSurfaceSignature {
            buffer_id: 12,
            ..initial
        };
        let key = EglSceneCacheKey::new(1280, 800, 9, 120, &[initial]);

        assert!(!key.is_current(1280, 800, 9, 120, &[replacement]));
    }

    #[test]
    fn dmabuf_resource_key_matches_same_handle_for_surface() {
        let mut ids = BufferIdAllocator::default();
        let identity = ids.allocate().expect("test buffer identity");
        let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);

        assert_eq!(
            DmabufImageKey::from_handle(identity.id(), &handle),
            DmabufImageKey::from_handle(identity.id(), &handle)
        );
    }

    #[test]
    fn dmabuf_resource_key_separates_buffer_ids_when_raw_fd_is_identical() {
        let mut ids = BufferIdAllocator::default();
        let first = ids.allocate().expect("first test buffer identity");
        let second = ids.allocate().expect("second test buffer identity");
        let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);

        assert_ne!(
            DmabufImageKey::from_handle(first.id(), &handle),
            DmabufImageKey::from_handle(second.id(), &handle)
        );
    }

    #[test]
    fn dmabuf_resource_key_separates_plane_layout_for_same_buffer_id() {
        let mut ids = BufferIdAllocator::default();
        let identity = ids.allocate().expect("test buffer identity");
        let first = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let second = test_dmabuf_handle(256, 144, 2048, DrmModifier::LINEAR);

        assert_ne!(
            DmabufImageKey::from_handle(identity.id(), &first),
            DmabufImageKey::from_handle(identity.id(), &second)
        );
    }

    #[test]
    fn dead_buffer_cache_entry_is_evicted_exactly_once() {
        let mut ids = BufferIdAllocator::default();
        let identity = ids.allocate().expect("test buffer identity");
        let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let key = DmabufImageKey::from_handle(identity.id(), &handle);
        let drops = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut cache = HashMap::from([(
            key.clone(),
            CachedDmabufResource {
                image: DropProbe(std::rc::Rc::clone(&drops)),
                buffer_lifetime: identity.downgrade(),
                surface_id: 7,
            },
        )]);

        drop(identity);
        for dead in dead_cached_dmabuf_keys(&cache) {
            drop(cache.remove(&dead));
        }
        assert!(dead_cached_dmabuf_keys(&cache).is_empty());
        assert_eq!(drops.get(), 1);

        drop(cache.remove(&key));
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn renderer_cache_recreation_drops_all_previous_generation_entries() {
        let mut ids = BufferIdAllocator::default();
        let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let drops = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut cache = HashMap::new();
        for surface_id in 1..=3 {
            let identity = ids.allocate().expect("test buffer identity");
            cache.insert(
                DmabufImageKey::from_handle(identity.id(), &handle),
                CachedDmabufResource {
                    image: DropProbe(std::rc::Rc::clone(&drops)),
                    buffer_lifetime: identity.downgrade(),
                    surface_id,
                },
            );
        }

        drop(cache);

        assert_eq!(drops.get(), 3);
    }

    fn test_dmabuf_handle(
        width: u32,
        height: u32,
        stride: u32,
        modifier: DrmModifier,
    ) -> DmabufBufferHandle {
        let fd = std::fs::File::open("/dev/null")
            .expect("/dev/null exists for dmabuf identity tests")
            .into();
        DmabufBufferHandle::new(
            BufferSize::new(width, height).expect("test dmabuf size is non-zero"),
            DrmFormat::Xrgb8888,
            vec![DmabufPlane::new(
                fd,
                DmabufPlaneDescriptor {
                    plane_index: 0,
                    offset: 0,
                    stride,
                    modifier,
                },
            )],
        )
        .expect("test dmabuf metadata is valid")
    }
}
