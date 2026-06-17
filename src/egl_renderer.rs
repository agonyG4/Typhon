use std::{collections::HashMap, error::Error, ffi::c_void, io, os::fd::AsRawFd, ptr, sync::Arc};

use glow::HasContext;
use khronos_egl as egl;
use oblivion_one::{
    compositor::{
        self, DesktopVisualState, RenderableSurface, ShellOverlayImage, SurfaceDamageRect,
        cursor_texture_pixels, cursor_texture_size,
    },
    render_backend::{
        RenderBackendProfile,
        buffer::DmabufBufferHandle,
        egl_gles::{EGL_LINUX_DMA_BUF_EXT, EglGlesDmabufFeedback, EglGlesDmabufImportAttributes},
    },
};
use wayland_egl::WlEglSurface;
use wayland_sys::client::wl_proxy;
use winit::{
    raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle},
    window::Window,
};

mod damage;
pub(crate) mod dmabuf;
mod geometry;
mod program;

#[cfg(test)]
use damage::cursor_damage_rect;
use damage::{EglOutputDamage, EglOutputDamageTracker, ShellOverlayDamageState};
use dmabuf::{query_egl_dmabuf_feedback, query_egl_main_device};
use geometry::{
    EglDrawCommand, EglDrawLayer, EglRect, EglTexturedVertex, EglUvRect, MIN_VERTEX_BUFFER_BYTES,
    VERTEX_STRIDE, push_draw_command, push_draw_command_with_uv,
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
}

pub struct EglSceneDrawRequest<'a> {
    pub width: u32,
    pub height: u32,
    pub surfaces: &'a [RenderableSurface],
    pub content_generation: u64,
    pub visual_state: DesktopVisualState,
    pub output_scale: f64,
    pub shell_overlay: Option<&'a ShellOverlayImage>,
}

pub struct EglGlesFrameRenderer {
    egl: EglInstance,
    egl_display: egl::Display,
    egl_context: egl::Context,
    egl_surface: egl::Surface,
    wl_egl_surface: WlEglSurface,
    scene: GlesSceneRenderer,
    swap_buffers_with_damage: Option<EglSwapBuffersWithDamage>,
    dmabuf_feedback: EglGlesDmabufFeedback,
    dmabuf_main_device: Option<u64>,
    dmabuf_main_device_path: Option<String>,
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
    wallpaper_resource: Option<EglImageResource>,
    cursor_resource: Option<EglImageResource>,
    shell_overlay_resource: Option<EglImageResource>,
    surface_resources: HashMap<u32, EglSurfaceResource>,
    dmabuf_resource_cache: HashMap<DmabufResourceKey, EglImageResource>,
    active_surface_ids: Vec<u32>,
    failed_surface_generations: HashMap<u32, u64>,
    frame_resources: HashMap<compositor::ServerFrameColor, EglImageResource>,
    egl_image_target_texture_2d: Option<GlEglImageTargetTexture2DOes>,
    damage_tracker: EglOutputDamageTracker,
    frame_stats: GlesSceneFrameStats,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GlesRendererInfo {
    pub vendor: String,
    pub renderer: String,
    pub version: String,
}

impl EglGlesFrameRenderer {
    pub fn new(window: Arc<Window>) -> RendererResult<Self> {
        let initial_size = window.inner_size();
        let width = initial_size.width.max(1);
        let height = initial_size.height.max(1);
        let (wl_display, wl_surface) = wayland_handles_for_window(window.as_ref())?;

        let egl = unsafe { EglInstance::load_required()? };
        let egl_display = unsafe { egl.get_display(wl_display) }
            .ok_or_else(|| io::Error::other("EGL could not open the Wayland display"))?;
        egl.initialize(egl_display)?;
        egl.bind_api(egl::OPENGL_ES_API)?;

        let egl_config = choose_egl_config(&egl, egl_display)?;
        let egl_context = create_gles_context(&egl, egl_display, egl_config)?;
        let wl_egl_surface =
            unsafe { WlEglSurface::new_from_raw(wl_surface, width as i32, height as i32)? };
        let egl_surface = unsafe {
            egl.create_window_surface(
                egl_display,
                egl_config,
                wl_egl_surface.ptr() as egl::NativeWindowType,
                None,
            )?
        };
        egl.make_current(
            egl_display,
            Some(egl_surface),
            Some(egl_surface),
            Some(egl_context),
        )?;
        if let Err(error) = egl.swap_interval(egl_display, 1) {
            eprintln!("oblivion-one compositor: EGL swap interval unavailable: {error}");
        }

        let egl_image_target_texture_2d =
            load_egl_image_target_texture_2d(&egl).or_else(|| {
                eprintln!(
                    "oblivion-one compositor: GL_OES_EGL_image entry point unavailable; dmabuf surfaces will be skipped"
                );
                None
        });
        let scene =
            GlesSceneRenderer::new_current(&egl, width, height, egl_image_target_texture_2d)?;
        let swap_buffers_with_damage = load_swap_buffers_with_damage(&egl, egl_display);
        let dmabuf_feedback = query_egl_dmabuf_feedback(&egl, egl_display);
        let (dmabuf_main_device_path, dmabuf_main_device) =
            match query_egl_main_device(&egl, egl_display) {
                Some((path, main_device)) => (Some(path), Some(main_device)),
                None => (None, None),
            };

        let vendor = unsafe { scene.gl.get_parameter_string(glow::VENDOR) };
        let renderer = unsafe { scene.gl.get_parameter_string(glow::RENDERER) };
        let version = unsafe { scene.gl.get_parameter_string(glow::VERSION) };
        eprintln!(
            "oblivion-one compositor: EGL/GLES renderer active: {vendor} {renderer} ({version}, profile {})",
            RenderBackendProfile::egl_gles().kind.as_str()
        );

        Ok(Self {
            egl,
            egl_display,
            egl_context,
            egl_surface,
            wl_egl_surface,
            scene,
            swap_buffers_with_damage,
            dmabuf_feedback,
            dmabuf_main_device,
            dmabuf_main_device_path,
        })
    }

    pub fn dmabuf_feedback(&self) -> &EglGlesDmabufFeedback {
        &self.dmabuf_feedback
    }

    pub const fn dmabuf_main_device(&self) -> Option<u64> {
        self.dmabuf_main_device
    }

    pub fn dmabuf_main_device_path(&self) -> Option<&str> {
        self.dmabuf_main_device_path.as_deref()
    }

    pub fn draw_scene(&mut self, request: EglSceneDrawRequest<'_>) -> RendererResult<()> {
        let width = request.width.max(1);
        let height = request.height.max(1);
        if self.scene.current_size() != (width, height) {
            self.wl_egl_surface
                .resize(width as i32, height as i32, 0, 0);
        }
        let damage = self
            .scene
            .draw_scene(&self.egl, self.egl_display, request)?;
        self.swap_buffers(damage)
    }

    fn swap_buffers(&self, damage: EglOutputDamage) -> RendererResult<()> {
        egl_swap_buffers_with_damage(
            &self.egl,
            self.egl_display,
            self.egl_surface,
            self.swap_buffers_with_damage,
            damage,
        )
    }
}

impl GlesSceneRenderer {
    pub(crate) fn new_current(
        egl: &EglInstance,
        width: u32,
        height: u32,
        egl_image_target_texture_2d: Option<GlEglImageTargetTexture2DOes>,
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
            wallpaper_resource: None,
            cursor_resource: None,
            shell_overlay_resource: None,
            surface_resources: HashMap::new(),
            dmabuf_resource_cache: HashMap::new(),
            active_surface_ids: Vec::new(),
            failed_surface_generations: HashMap::new(),
            frame_resources: HashMap::new(),
            egl_image_target_texture_2d,
            damage_tracker: EglOutputDamageTracker::default(),
            frame_stats: GlesSceneFrameStats::default(),
        })
    }

    pub(crate) const fn current_size(&self) -> (u32, u32) {
        self.current_size
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
        request: EglSceneDrawRequest<'_>,
    ) -> RendererResult<EglOutputDamage> {
        let EglSceneDrawRequest {
            width,
            height,
            surfaces,
            content_generation,
            visual_state,
            output_scale,
            shell_overlay,
        } = request;
        let width = width.max(1);
        let height = height.max(1);
        let output_scale_key = compositor::output_scale_key(output_scale);
        let scaled_visual_state =
            compositor::scale_desktop_visual_state(visual_state, output_scale);
        self.frame_stats = GlesSceneFrameStats::default();
        self.ensure_output_size(egl, egl_display, width, height)?;
        self.ensure_wallpaper_resource(egl, egl_display, width, height)?;
        self.ensure_frame_resources()?;
        if scaled_visual_state.cursor.is_some() {
            self.ensure_cursor_resource(egl, egl_display)?;
        }
        self.ensure_shell_overlay_resource(egl, egl_display, shell_overlay)?;
        self.sync_surface_resources(egl, egl_display, surfaces)?;

        let surface_signatures = egl_scene_surface_signatures(surfaces);
        let scene_changed = !self.scene_cache_is_current(
            width,
            height,
            content_generation,
            output_scale_key,
            &surface_signatures,
        );
        let shell_overlay_damage = shell_overlay.and_then(shell_overlay_damage_state);
        let output_damage = self.damage_tracker.damage_for_frame(
            width,
            height,
            scene_changed,
            scaled_visual_state,
            shell_overlay_damage,
        );

        if scene_changed {
            self.frame_stats.scene_rebuilt = true;
            self.rebuild_scene_commands(
                width,
                height,
                surfaces,
                content_generation,
                output_scale,
                output_scale_key,
                &surface_signatures,
            );
        }
        self.rebuild_overlay_commands(width, height, scaled_visual_state, shell_overlay);
        self.draw_textured_layers()?;
        Ok(output_damage)
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
        if let Some(resource) = self.wallpaper_resource.take() {
            destroy_image_resource(&self.gl, egl, egl_display, resource);
        }
        if let Some(resource) = self.shell_overlay_resource.take() {
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

        let mut pixels =
            vec![compositor::NESTED_OUTPUT_BACKGROUND; width as usize * height as usize];
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

    fn ensure_shell_overlay_resource(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        shell_overlay: Option<&ShellOverlayImage>,
    ) -> RendererResult<()> {
        let Some(shell_overlay) = shell_overlay else {
            if let Some(old) = self.shell_overlay_resource.take() {
                destroy_image_resource(&self.gl, egl, egl_display, old);
            }
            return Ok(());
        };
        if shell_overlay.width == 0 || shell_overlay.height == 0 {
            return Ok(());
        }
        if shell_overlay.content_bounds().is_none() {
            if let Some(old) = self.shell_overlay_resource.take() {
                destroy_image_resource(&self.gl, egl, egl_display, old);
            }
            return Ok(());
        }
        let update = self
            .shell_overlay_resource
            .as_ref()
            .is_none_or(|resource| resource.size != (shell_overlay.width, shell_overlay.height));
        if update {
            if let Some(old) = self.shell_overlay_resource.take() {
                destroy_image_resource(&self.gl, egl, egl_display, old);
            }
            self.shell_overlay_resource = Some(create_uploaded_resource(
                &self.gl,
                shell_overlay.width,
                shell_overlay.height,
            )?);
        }

        let Some(resource) = self.shell_overlay_resource.as_mut() else {
            return Ok(());
        };
        if resource.generation != shell_overlay.generation {
            write_argb_pixels_to_resource(
                &self.gl,
                resource,
                SurfaceDamageRect::full(shell_overlay.width, shell_overlay.height),
                &shell_overlay.pixels,
                &mut self.texture_upload_rgba,
            );
            resource.generation = shell_overlay.generation;
        }
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
    ) -> RendererResult<()> {
        self.active_surface_ids.clear();
        self.active_surface_ids
            .extend(surfaces.iter().map(|surface| surface.surface_id));
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

        for surface in surfaces {
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

        Ok(())
    }

    fn switch_dmabuf_surface_resource(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        surface: &RenderableSurface,
    ) -> RendererResult<()> {
        let Some(key) = DmabufResourceKey::from_surface(surface) else {
            return Ok(());
        };

        if let Some(mut image) = self.dmabuf_resource_cache.remove(&key) {
            image.generation = surface.generation;
            self.frame_stats.dmabuf_reuses = self.frame_stats.dmabuf_reuses.saturating_add(1);
            if let Some(old) = self.surface_resources.insert(
                surface.surface_id,
                EglSurfaceResource {
                    image,
                    dmabuf_key: Some(key),
                },
            ) {
                self.cache_or_destroy_dmabuf_resource(egl, egl_display, surface.surface_id, old);
            }
            return Ok(());
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

        self.prune_cached_dmabufs_for_surface(egl, egl_display, surface_id);
        self.dmabuf_resource_cache.insert(key, resource.image);
    }

    fn prune_cached_dmabufs_for_surface(
        &mut self,
        egl: &EglInstance,
        egl_display: egl::Display,
        surface_id: u32,
    ) {
        let cached = self
            .dmabuf_resource_cache
            .keys()
            .filter(|key| key.surface_id == surface_id)
            .count();
        if cached < MAX_CACHED_DMABUF_RESOURCES_PER_SURFACE {
            return;
        }
        let Some(key) = self
            .dmabuf_resource_cache
            .keys()
            .find(|key| key.surface_id == surface_id)
            .cloned()
        else {
            return;
        };
        if let Some(resource) = self.dmabuf_resource_cache.remove(&key) {
            destroy_image_resource(&self.gl, egl, egl_display, resource);
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
            .keys()
            .filter(|key| key.surface_id == surface_id)
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(resource) = self.dmabuf_resource_cache.remove(&key) {
                destroy_image_resource(&self.gl, egl, egl_display, resource);
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
        for (surface, (origin_x, origin_y)) in surfaces.iter().zip(origins) {
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
            let visual_target = compositor::SurfaceTargetRect::new(
                compositor::scale_logical_coordinate(origin_x, output_scale),
                compositor::scale_logical_coordinate(origin_y, output_scale),
                compositor::scale_logical_extent(surface.width, output_scale),
                compositor::scale_logical_extent(surface.height, output_scale),
            );
            let render_plan = compositor::surface_render_plan(surface, visual_target);
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
                EglUvRect::new(
                    render_plan.content_uv.left,
                    render_plan.content_uv.top,
                    render_plan.content_uv.right,
                    render_plan.content_uv.bottom,
                ),
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
        shell_overlay: Option<&ShellOverlayImage>,
    ) {
        self.cursor_vertices.clear();
        self.cursor_commands.clear();
        if let Some(shell_overlay) = shell_overlay
            && self.shell_overlay_resource.is_some()
        {
            for region in shell_overlay.regions() {
                let bounds = region.output;
                let texture = region.texture;
                let rect = EglRect::new(
                    bounds.x as f32,
                    bounds.y as f32,
                    bounds.width as f32,
                    bounds.height as f32,
                );
                let uv = EglUvRect::from_pixel_bounds(
                    texture.x,
                    texture.y,
                    texture.width,
                    texture.height,
                    shell_overlay.width,
                    shell_overlay.height,
                );
                push_draw_command_with_uv(
                    &mut self.cursor_vertices,
                    &mut self.cursor_commands,
                    EglDrawLayer::ShellOverlay,
                    rect,
                    uv,
                    width,
                    height,
                );
            }
        }

        let Some((cursor_x, cursor_y)) = visual_state.cursor else {
            return;
        };
        let Some(cursor) = self.cursor_resource.as_ref() else {
            return;
        };

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

    fn draw_textured_layers(&mut self) -> RendererResult<()> {
        unsafe {
            self.gl.clear_color(0.0, 0.0, 0.0, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
            self.gl.use_program(Some(self.program));
            self.gl.active_texture(glow::TEXTURE0);
            self.gl.bind_vertex_array(Some(self.vertex_array));
        }

        self.draw_command_batch(true)?;
        self.draw_command_batch(false)?;

        unsafe {
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

        for command in commands {
            let Some(texture) = self.texture_for_layer(command.layer) else {
                continue;
            };
            unsafe {
                self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
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
            EglDrawLayer::ShellOverlay => self
                .shell_overlay_resource
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
        if let Some(resource) = self.shell_overlay_resource.take() {
            destroy_image_resource(&self.gl, egl, egl_display, resource);
        }
        for (_, resource) in self.frame_resources.drain() {
            destroy_image_resource(&self.gl, egl, egl_display, resource);
        }
        for (_, resource) in self.surface_resources.drain() {
            destroy_surface_resource(&self.gl, egl, egl_display, resource);
        }
        for (_, resource) in self.dmabuf_resource_cache.drain() {
            destroy_image_resource(&self.gl, egl, egl_display, resource);
        }

        unsafe {
            self.gl.delete_buffer(self.vertex_buffer);
            self.gl.delete_vertex_array(self.vertex_array);
            self.gl.delete_program(self.program);
        }
    }
}

impl Drop for EglGlesFrameRenderer {
    fn drop(&mut self) {
        self.scene.destroy(&self.egl, self.egl_display);
        let _ = self.egl.make_current(self.egl_display, None, None, None);
        let _ = self.egl.destroy_surface(self.egl_display, self.egl_surface);
        let _ = self.egl.destroy_context(self.egl_display, self.egl_context);
        let _ = self.egl.terminate(self.egl_display);
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
    dmabuf_key: Option<DmabufResourceKey>,
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
            .and_then(|_| DmabufResourceKey::from_surface(surface))
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DmabufResourceKey {
    surface_id: u32,
    width: u32,
    height: u32,
    format: u32,
    planes: Vec<DmabufPlaneKey>,
}

impl DmabufResourceKey {
    fn from_surface(surface: &RenderableSurface) -> Option<Self> {
        Self::from_handle(surface.surface_id, surface.dmabuf_handle()?)
    }

    fn from_handle(surface_id: u32, handle: &DmabufBufferHandle) -> Option<Self> {
        let size = handle.size();
        let planes = handle
            .planes()
            .iter()
            .map(|plane| {
                let descriptor = plane.descriptor();
                Some(DmabufPlaneKey {
                    fd: plane.fd().as_raw_fd(),
                    plane_index: descriptor.plane_index,
                    offset: descriptor.offset,
                    stride: descriptor.stride,
                    modifier: descriptor.modifier.0,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            surface_id,
            width: size.width,
            height: size.height,
            format: handle.format().as_fourcc(),
            planes,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DmabufPlaneKey {
    fd: i32,
    plane_index: u32,
    offset: u32,
    stride: u32,
    modifier: u64,
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
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    preview_committed_width: u32,
    preview_committed_height: u32,
    preview_anchor_bits: u32,
    generation: u64,
}

fn egl_scene_surface_signatures(surfaces: &[RenderableSurface]) -> Vec<EglSceneSurfaceSignature> {
    surfaces
        .iter()
        .map(|surface| {
            let preview = surface.resize_preview;
            EglSceneSurfaceSignature {
                surface_id: surface.surface_id,
                x: surface.x,
                y: surface.y,
                width: surface.width,
                height: surface.height,
                preview_committed_width: preview
                    .map(|preview| preview.committed_width)
                    .unwrap_or(0),
                preview_committed_height: preview
                    .map(|preview| preview.committed_height)
                    .unwrap_or(0),
                preview_anchor_bits: preview
                    .map(|preview| {
                        u32::from(preview.anchor_right) | (u32::from(preview.anchor_bottom) << 1)
                    })
                    .unwrap_or(0),
                generation: surface.generation,
            }
        })
        .collect()
}

fn egl_scene_surface_signature_hash(signatures: &[EglSceneSurfaceSignature]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for signature in signatures {
        hash = fnv1a_u64(hash, u64::from(signature.surface_id));
        hash = fnv1a_u64(hash, signature.x as u32 as u64);
        hash = fnv1a_u64(hash, signature.y as u32 as u64);
        hash = fnv1a_u64(hash, u64::from(signature.width));
        hash = fnv1a_u64(hash, u64::from(signature.height));
        hash = fnv1a_u64(hash, u64::from(signature.preview_committed_width));
        hash = fnv1a_u64(hash, u64::from(signature.preview_committed_height));
        hash = fnv1a_u64(hash, u64::from(signature.preview_anchor_bits));
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

    Ok(EglSurfaceResource {
        image,
        dmabuf_key: DmabufResourceKey::from_surface(surface),
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

fn shell_overlay_damage_state(
    shell_overlay: &ShellOverlayImage,
) -> Option<ShellOverlayDamageState> {
    if shell_overlay.regions().is_empty() {
        return None;
    }

    Some(ShellOverlayDamageState::new(
        shell_overlay.generation,
        shell_overlay.regions().iter().map(|region| {
            let bounds = region.output;
            SurfaceDamageRect {
                x: bounds.x,
                y: bounds.y,
                width: bounds.width,
                height: bounds.height,
            }
        }),
    ))
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

fn wayland_handles_for_window(
    window: &Window,
) -> RendererResult<(egl::NativeDisplayType, *mut wl_proxy)> {
    let display_handle = window.display_handle()?.as_raw();
    let window_handle = window.window_handle()?.as_raw();
    let RawDisplayHandle::Wayland(display) = display_handle else {
        return Err(io::Error::other("EGL/GLES output requires a Wayland display").into());
    };
    let RawWindowHandle::Wayland(window) = window_handle else {
        return Err(io::Error::other("EGL/GLES output requires a Wayland window").into());
    };
    Ok((
        display.display.as_ptr(),
        window.surface.as_ptr().cast::<wl_proxy>(),
    ))
}

pub(crate) fn choose_egl_config(
    egl: &EglInstance,
    display: egl::Display,
) -> RendererResult<egl::Config> {
    let attributes = [
        egl::SURFACE_TYPE,
        egl::WINDOW_BIT,
        egl::RENDERABLE_TYPE,
        egl::OPENGL_ES2_BIT | egl::OPENGL_ES3_BIT,
        egl::RED_SIZE,
        8,
        egl::GREEN_SIZE,
        8,
        egl::BLUE_SIZE,
        8,
        egl::ALPHA_SIZE,
        8,
        egl::NONE,
    ];
    egl.choose_first_config(display, &attributes)?
        .ok_or_else(|| io::Error::other("EGL has no GLES window config").into())
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
                "EGL has no GLES GBM window config for requested native visuals: {requested}"
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
                "EGL has no GLES GBM window config for native visual {}",
                native_visual_label(native_visual_id)
            ))
            .into()
        })
}

fn native_egl_config_candidate_matches(
    candidate: &NativeEglConfigCandidate,
    native_visual_id: u32,
) -> bool {
    candidate.native_visual_id == native_visual_id
        && (candidate.surface_type & egl::WINDOW_BIT) != 0
        && (candidate.renderable_type & (egl::OPENGL_ES2_BIT | egl::OPENGL_ES3_BIT)) != 0
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
        "native EGL config config_id={} visual={} window={} gles2={} gles3={} rgba={}/{}/{}/{} surface_type=0x{:x} renderable_type=0x{:x}",
        candidate.config_id,
        native_visual_label(candidate.native_visual_id),
        (candidate.surface_type & egl::WINDOW_BIT) != 0,
        (candidate.renderable_type & egl::OPENGL_ES2_BIT) != 0,
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
    let gles3 = [egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE];
    match egl.create_context(display, config, None, &gles3) {
        Ok(context) => Ok(context),
        Err(gles3_error) => {
            let gles2 = [egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE];
            egl.create_context(display, config, None, &gles2)
                .map_err(|gles2_error| {
                    io::Error::other(format!(
                        "failed to create GLES context: GLES3={gles3_error}; GLES2={gles2_error}"
                    ))
                    .into()
                })
        }
    }
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
    let symbol_name = if extensions.contains("EGL_KHR_swap_buffers_with_damage") {
        "eglSwapBuffersWithDamageKHR"
    } else if extensions.contains("EGL_EXT_swap_buffers_with_damage") {
        "eglSwapBuffersWithDamageEXT"
    } else {
        return None;
    };
    let symbol = egl.get_proc_address(symbol_name)?;
    Some(unsafe { std::mem::transmute::<extern "system" fn(), EglSwapBuffersWithDamage>(symbol) })
}

pub(crate) fn egl_swap_buffers_with_damage(
    egl: &EglInstance,
    display: egl::Display,
    surface: egl::Surface,
    swap_buffers_with_damage: Option<EglSwapBuffersWithDamage>,
    damage: EglOutputDamage,
) -> RendererResult<()> {
    let Some(swap_buffers_with_damage) = swap_buffers_with_damage else {
        egl.swap_buffers(display, surface)?;
        return Ok(());
    };
    let Some(rects) = damage.to_egl_rects() else {
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
        BufferSize, DmabufPlane, DmabufPlaneDescriptor, DrmFormat, DrmModifier,
    };

    const XR24: u32 = u32::from_le_bytes(*b"XR24");
    const AR24: u32 = u32::from_le_bytes(*b"AR24");

    fn native_candidate(config_id: egl::Int, native_visual_id: u32) -> NativeEglConfigCandidate {
        NativeEglConfigCandidate {
            config_id,
            native_visual_id,
            surface_type: egl::WINDOW_BIT,
            renderable_type: egl::OPENGL_ES2_BIT,
            red_size: 8,
            green_size: 8,
            blue_size: 8,
            alpha_size: 0,
        }
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
    fn output_damage_tracker_uses_full_damage_for_first_or_scene_frame() {
        let mut tracker = EglOutputDamageTracker::default();

        assert_eq!(
            tracker.damage_for_frame(1280, 800, true, DesktopVisualState::wallpaper_only(), None),
            EglOutputDamage::full(1280, 800)
        );
        assert_eq!(
            tracker.damage_for_frame(1280, 800, true, DesktopVisualState::wallpaper_only(), None),
            EglOutputDamage::full(1280, 800)
        );
    }

    #[test]
    fn output_damage_tracker_limits_damage_to_old_and_new_cursor_rects() {
        let mut tracker = EglOutputDamageTracker::default();
        tracker.damage_for_frame(
            1280,
            800,
            true,
            DesktopVisualState::with_cursor(10, 10),
            None,
        );

        let damage = tracker.damage_for_frame(
            1280,
            800,
            false,
            DesktopVisualState::with_cursor(20, 22),
            None,
        );

        assert_eq!(
            damage,
            EglOutputDamage::two_rects(
                cursor_damage_rect(10, 10, 1280, 800).unwrap(),
                cursor_damage_rect(20, 22, 1280, 800).unwrap()
            )
        );
    }

    #[test]
    fn output_damage_tracker_limits_shell_overlay_change_to_overlay_rects() {
        let mut tracker = EglOutputDamageTracker::default();
        let old_overlay = SurfaceDamageRect {
            x: 16,
            y: 10,
            width: 420,
            height: 32,
        };
        let new_overlay = SurfaceDamageRect {
            x: 16,
            y: 10,
            width: 520,
            height: 32,
        };
        tracker.damage_for_frame(
            1280,
            800,
            true,
            DesktopVisualState::wallpaper_only(),
            Some(ShellOverlayDamageState::new(1, [old_overlay])),
        );

        let damage = tracker.damage_for_frame(
            1280,
            800,
            false,
            DesktopVisualState::wallpaper_only(),
            Some(ShellOverlayDamageState::new(2, [new_overlay])),
        );

        assert_eq!(
            damage.to_egl_rects().unwrap().as_slice(),
            &[16, 10, 420, 32, 16, 10, 520, 32]
        );
    }

    #[test]
    fn output_damage_converts_to_egl_rect_list() {
        let damage = EglOutputDamage::two_rects(
            SurfaceDamageRect {
                x: 4,
                y: 8,
                width: 16,
                height: 24,
            },
            SurfaceDamageRect {
                x: 40,
                y: 48,
                width: 8,
                height: 12,
            },
        );

        assert_eq!(
            damage.to_egl_rects().unwrap().as_slice(),
            &[4, 8, 16, 24, 40, 48, 8, 12]
        );
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
            x: 10,
            y: 20,
            width: 800,
            height: 600,
            preview_committed_width: 0,
            preview_committed_height: 0,
            preview_anchor_bits: 0,
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
    fn scene_cache_key_invalidates_when_resize_preview_crop_changes() {
        let initial_signature = EglSceneSurfaceSignature {
            surface_id: 7,
            x: 10,
            y: 20,
            width: 800,
            height: 600,
            preview_committed_width: 800,
            preview_committed_height: 600,
            preview_anchor_bits: 0,
            generation: 1,
        };
        let cropped_signature = EglSceneSurfaceSignature {
            preview_committed_width: 640,
            preview_committed_height: 480,
            preview_anchor_bits: 3,
            ..initial_signature
        };
        let key = EglSceneCacheKey::new(1280, 800, 9, 120, &[initial_signature]);

        assert!(!key.is_current(1280, 800, 9, 120, &[cropped_signature]));
    }

    #[test]
    fn dmabuf_resource_key_matches_same_handle_for_surface() {
        let handle = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);

        assert_eq!(
            DmabufResourceKey::from_handle(7, &handle),
            DmabufResourceKey::from_handle(7, &handle)
        );
    }

    #[test]
    fn dmabuf_resource_key_separates_swapchain_buffers() {
        let first = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);
        let second = test_dmabuf_handle(256, 144, 1024, DrmModifier::LINEAR);

        assert_ne!(
            DmabufResourceKey::from_handle(7, &first),
            DmabufResourceKey::from_handle(7, &second)
        );
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
