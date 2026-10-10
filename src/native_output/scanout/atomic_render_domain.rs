use std::{ffi::c_void, io, os::fd::OwnedFd, ptr};

use gbm::AsRaw as _;
use glow::HasContext;
use khronos_egl as egl;
use oblivion_one::{
    cursor_theme::CompositorCursorImage,
    effects::{
        EffectManifest, EffectRegistryGeneration, RegistryReloadError, TrustedEffectRegistry,
    },
    native::kms::DrmFormatModifierPair,
    render_backend::egl_gles::{EglGlesDmabufFormat, EglGlesDmabufImportAttributes},
    window_lifecycle_animation::{LifecycleRenderEvidence, LifecycleRenderFallbacks},
};

use crate::{
    egl_renderer::native_fence::{NativeFenceFunctions, NativeRenderFence},
    egl_renderer::{
        CreatedGlesContext, EglContextPriorityPolicy, EglFrameOutcome, EglInstance,
        EglOutputRenderTarget, EglSceneFrameCommit, FrameSkipReason, GlesSceneFrameStats,
        GlesSceneRenderer, OutputDamage, OutputFramebufferOrigin, choose_surfaceless_egl_config,
        create_compositor_gles_context, detect_partial_repaint_capabilities,
        load_egl_image_target_texture_2d,
    },
    egl_renderer::{EglImageGuard, GlEglImageTargetTexture2DOes},
    native_output::runtime::FrozenNativeRenderFrame,
};

use super::{
    AtomicRenderPhaseDurations, EXPLICIT_OUTPUT_SLOT_CAPACITY, OutputSlotId,
    output_slot::{AtomicRenderSlotDescriptor, RenderTargetRequest, validate_split_slot_pairing},
};

const EGL_PLATFORM_GBM_KHR: egl::Enum = 0x31d7;
const EGL_LINUX_DMA_BUF_EXT: egl::Enum = 0x3270;

/// Owns the native display object for the entire EGLDisplay lifetime.
struct EglNativeDisplay {
    egl: EglInstance,
    display: egl::Display,
    render_device: Option<gbm::Device<OwnedFd>>,
    initialized: bool,
}

impl EglNativeDisplay {
    fn terminate_and_drop_device(&mut self) {
        let initialized = self.initialized;
        self.initialized = false;
        let egl = &self.egl;
        let display = self.display;
        let render_device = &mut self.render_device;
        terminate_before_release_native_device(
            initialized,
            || {
                let _ = egl.terminate(display);
            },
            || drop(render_device.take()),
        );
    }
}

impl Drop for EglNativeDisplay {
    fn drop(&mut self) {
        self.terminate_and_drop_device();
    }
}

/// EGL capability discovery precedes format selection; it retains the render
/// GBM device so the display backing cannot be dropped between these phases.
pub(super) struct AtomicRenderDomainBootstrap {
    native_display: EglNativeDisplay,
    renderable_dmabuf_formats: Vec<EglGlesDmabufFormat>,
}

/// Worker-safe setup data for constructing an EGL domain on its owning thread.
/// Initialized EGL/GL handles are deliberately absent: `egl::Context` contains
/// a raw native context pointer and is not naturally `Send`.
#[allow(dead_code, reason = "Phase 2B.2 worker bootstrap boundary")]
pub(crate) struct AtomicRenderDomainBootstrapPayload {
    pub(crate) render_device_fd: OwnedFd,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format_modifier: DrmFormatModifierPair,
    pub(crate) render_slot_descriptors: Vec<AtomicRenderSlotDescriptor>,
}

#[allow(dead_code, reason = "Phase 2B.2 worker bootstrap qualification")]
fn assert_atomic_render_domain_bootstrap_payload_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<AtomicRenderDomainBootstrapPayload>();
}

impl AtomicRenderDomainBootstrap {
    pub(super) fn new(render_device: gbm::Device<OwnedFd>) -> io::Result<Self> {
        let egl = unsafe { EglInstance::load_required() }.map_err(native_egl_io_error)?;
        let display = unsafe {
            egl.get_platform_display(
                EGL_PLATFORM_GBM_KHR,
                render_device.as_raw_mut() as egl::NativeDisplayType,
                &[egl::ATTRIB_NONE],
            )
        }
        .map_err(native_egl_io_error)?;
        let mut native_display = EglNativeDisplay {
            egl,
            display,
            render_device: Some(render_device),
            initialized: false,
        };
        native_display
            .egl
            .initialize(display)
            .map_err(native_egl_io_error)?;
        native_display.initialized = true;
        native_display
            .egl
            .bind_api(egl::OPENGL_ES_API)
            .map_err(native_egl_io_error)?;

        let extensions = native_display
            .egl
            .query_string(Some(display), egl::EXTENSIONS)
            .map_err(native_egl_io_error)?
            .to_string_lossy();
        for required in [
            "EGL_KHR_surfaceless_context",
            "EGL_KHR_image_base",
            "EGL_EXT_image_dma_buf_import",
            "EGL_EXT_image_dma_buf_import_modifiers",
        ] {
            if !extensions
                .split_ascii_whitespace()
                .any(|entry| entry == required)
            {
                return Err(io::Error::other(format!(
                    "explicit Atomic EGL/GBM requires {required}"
                )));
            }
        }

        let renderable_dmabuf_formats =
            crate::egl_renderer::dmabuf::query_egl_renderable_dmabuf_formats(
                &native_display.egl,
                display,
            );
        Ok(Self {
            native_display,
            renderable_dmabuf_formats,
        })
    }

    pub(super) fn renderable_dmabuf_formats(&self) -> &[EglGlesDmabufFormat] {
        &self.renderable_dmabuf_formats
    }

    pub(super) fn initialize_renderer(
        self,
        width: u32,
        height: u32,
        format: DrmFormatModifierPair,
    ) -> io::Result<(AtomicRenderDomain, AtomicRenderDomainInfo)> {
        let display = self.native_display.display;
        let mut created_context = None;
        let setup = (|| {
            let egl = &self.native_display.egl;
            let config = choose_surfaceless_egl_config(egl, display, format.fourcc)
                .map_err(native_egl_io_error)?;
            let created_gles_context: CreatedGlesContext = create_compositor_gles_context(
                egl,
                display,
                config,
                EglContextPriorityPolicy::from_environment(),
            )
            .map_err(native_egl_io_error)?;
            let context = created_gles_context.context;
            let priority = created_gles_context.priority;
            created_context = Some(context);
            egl.make_current(display, None, None, Some(context))
                .map_err(native_egl_io_error)?;

            let image_target = load_egl_image_target_texture_2d(egl).ok_or_else(|| {
                io::Error::other("explicit Atomic EGL/GBM requires GL_OES_EGL_image")
            })?;
            let native_fence_functions =
                NativeFenceFunctions::load(egl, display).map_err(|error| {
                    io::Error::other(format!(
                        "native output fence initialization failed: {error}"
                    ))
                })?;
            let gl = unsafe {
                glow::Context::from_loader_function(|name| {
                    egl.get_proc_address(name)
                        .map(|symbol| symbol as *const c_void)
                        .unwrap_or(ptr::null())
                })
            };
            let scene = GlesSceneRenderer::new_current(
                egl,
                width,
                height,
                Some(image_target),
                detect_partial_repaint_capabilities(egl, display, true, true, false),
                oblivion_one::cursor_theme::shared_compositor_cursor_image(),
            )
            .map_err(native_egl_io_error)?;

            let egl_vendor = egl
                .query_string(Some(display), egl::VENDOR)
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "unknown".to_string());
            let dmabuf_feedback =
                crate::egl_renderer::dmabuf::query_egl_dmabuf_feedback(egl, display);
            let (dmabuf_main_device_path, dmabuf_main_device) =
                crate::egl_renderer::dmabuf::query_egl_main_device(egl, display)
                    .map_or((None, None), |(path, device)| (Some(path), Some(device)));
            let lifecycle_animation_available = scene.lifecycle_animation_available();
            let squash_animation_available = scene.squash_animation_available();

            Ok((
                context,
                priority,
                gl,
                scene,
                native_fence_functions,
                AtomicRenderDomainInfo {
                    dmabuf_feedback,
                    egl_vendor,
                    dmabuf_main_device,
                    dmabuf_main_device_path,
                    lifecycle_animation_available,
                    squash_animation_available,
                },
            ))
        })();

        match setup {
            Ok((context, priority, gl, scene, native_fence_functions, info)) => {
                priority.log_startup("atomic-egl-gbm");
                Ok((
                    AtomicRenderDomain {
                        native_display: Some(self.native_display),
                        egl_context: Some(context),
                        gl: Some(gl),
                        scene: Some(scene),
                        native_fence_functions,
                        pool: None,
                        width,
                        height,
                        torn_down: false,
                    },
                    info,
                ))
            }
            Err(error) => {
                let egl = &self.native_display.egl;
                let _ = egl.make_current(display, None, None, None);
                if let Some(context) = created_context {
                    let _ = egl.destroy_context(display, context);
                }
                Err(error)
            }
        }
    }
}

pub(super) struct AtomicRenderDomainInfo {
    pub(super) dmabuf_feedback: super::EglGlesDmabufFeedback,
    pub(super) egl_vendor: String,
    pub(super) dmabuf_main_device: Option<u64>,
    pub(super) dmabuf_main_device_path: Option<String>,
    pub(super) lifecycle_animation_available: bool,
    pub(super) squash_animation_available: bool,
}

/// Exclusive owner of EGL/GL execution state and imported output render slots.
pub(super) struct AtomicRenderDomain {
    native_display: Option<EglNativeDisplay>,
    egl_context: Option<egl::Context>,
    gl: Option<glow::Context>,
    scene: Option<GlesSceneRenderer>,
    native_fence_functions: NativeFenceFunctions,
    pool: Option<AtomicRenderPool>,
    width: u32,
    height: u32,
    torn_down: bool,
}

impl AtomicRenderDomain {
    pub(super) fn install_pool(
        &mut self,
        descriptors: Vec<AtomicRenderSlotDescriptor>,
        physical_pool_generation: u64,
        physical_slot_identities: &[(OutputSlotId, u64)],
    ) -> io::Result<()> {
        if self.pool.is_some() {
            return Err(io::Error::other("Atomic render pool is already installed"));
        }
        let pool_generation = physical_pool_generation;
        let render_identities = descriptors
            .iter()
            .map(|descriptor| (descriptor.id, descriptor.pool_generation))
            .collect::<Vec<_>>();
        validate_split_slot_pairing(
            physical_slot_identities,
            &render_identities,
            pool_generation,
        )?;
        let native_display = self.native_display.as_ref().ok_or_else(|| {
            io::Error::other("Atomic EGL display is unavailable during pool import")
        })?;
        let gl = self.gl.as_ref().ok_or_else(|| {
            io::Error::other("Atomic GLES context is unavailable during pool import")
        })?;
        let image_target = load_egl_image_target_texture_2d(&native_display.egl)
            .ok_or_else(|| io::Error::other("explicit Atomic EGL/GBM requires GL_OES_EGL_image"))?;
        let pool = AtomicRenderPool::import_descriptors(
            descriptors,
            pool_generation,
            &native_display.egl,
            native_display.display,
            gl,
            image_target,
        )?;
        if let Err(error) = validate_split_slot_pairing(
            physical_slot_identities,
            &pool.identities(),
            pool_generation,
        ) {
            pool.teardown(gl, &native_display.egl, native_display.display);
            return Err(error);
        }
        self.pool = Some(pool);
        Ok(())
    }

    pub(super) fn rebind_pool_generation(&mut self, generation: u64) {
        if let Some(pool) = self.pool.as_mut() {
            pool.rebind_pool_generation(generation);
        }
    }

    pub(super) fn pool_identities(&self) -> Vec<(OutputSlotId, u64)> {
        self.pool
            .as_ref()
            .map_or_else(Vec::new, AtomicRenderPool::identities)
    }

    pub(super) fn finish_render_work(&self) {
        if let Some(gl) = self.gl.as_ref() {
            unsafe { gl.finish() };
        }
    }

    pub(super) fn lifecycle_animation_available(&self) -> bool {
        self.scene
            .as_ref()
            .is_some_and(GlesSceneRenderer::lifecycle_animation_available)
    }

    pub(super) fn squash_animation_available(&self) -> bool {
        self.scene
            .as_ref()
            .is_some_and(GlesSceneRenderer::squash_animation_available)
    }

    pub(super) fn reload_trusted_effect_registry(
        &mut self,
        registry: &TrustedEffectRegistry,
        manifest: EffectManifest,
    ) -> Result<std::sync::Arc<EffectRegistryGeneration>, RegistryReloadError> {
        self.scene
            .as_mut()
            .expect("live Atomic render domain owns its scene renderer")
            .reload_trusted_effect_registry(registry, manifest)
    }

    pub(super) fn publish_material_effect_generation(
        &mut self,
        generation: &EffectRegistryGeneration,
    ) {
        self.scene
            .as_mut()
            .expect("live Atomic render domain owns its scene renderer")
            .publish_material_effect_generation(generation);
    }

    pub(super) fn publish_effect_registry_generation(
        &mut self,
        generation: EffectRegistryGeneration,
    ) -> Result<(), RegistryReloadError> {
        self.scene
            .as_mut()
            .expect("live Atomic render domain owns its scene renderer")
            .publish_effect_registry_generation(generation)
    }

    pub(super) fn set_cursor_image(&mut self, image: std::sync::Arc<CompositorCursorImage>) {
        self.scene
            .as_mut()
            .expect("live Atomic render domain owns its scene renderer")
            .set_cursor_image(image);
    }

    pub(super) fn commit_presented(
        &mut self,
        frame: EglSceneFrameCommit,
        presented_transition_damage: OutputDamage,
    ) {
        self.scene
            .as_mut()
            .expect("live Atomic render domain owns its scene renderer")
            .commit_presented(frame, presented_transition_damage);
    }

    pub(super) fn discard_rendered(&mut self, frame: EglSceneFrameCommit) {
        self.scene
            .as_mut()
            .expect("live Atomic render domain owns its scene renderer")
            .discard_rendered(frame);
    }

    pub(super) fn invalidate_presented_damage_history(&mut self) {
        self.scene
            .as_mut()
            .expect("live Atomic render domain owns its scene renderer")
            .invalidate_presented_damage_history();
    }

    pub(super) fn frame_swap_failed(&mut self) {
        self.scene
            .as_mut()
            .expect("live Atomic render domain owns its scene renderer")
            .frame_swap_failed();
    }

    pub(super) fn create_render_fence(&self) -> io::Result<NativeRenderFence> {
        let native_display = self
            .native_display
            .as_ref()
            .ok_or_else(|| io::Error::other("Atomic EGL display is unavailable"))?;
        let gl = self
            .gl
            .as_ref()
            .ok_or_else(|| io::Error::other("Atomic GLES context is unavailable"))?;
        NativeRenderFence::create(
            &native_display.egl,
            native_display.display,
            gl,
            self.native_fence_functions,
        )
        .map_err(|error| io::Error::other(format!("native render fence export failed: {error}")))
    }

    pub(super) fn render_to_slot(
        &mut self,
        target: RenderTargetRequest,
        frame_id: u64,
        frozen_frame: &mut FrozenNativeRenderFrame,
        gpu_sampling_started: &mut bool,
        mut phase_durations: Option<&mut AtomicRenderPhaseDurations>,
    ) -> io::Result<AtomicSlotRenderOutcome> {
        let context_activation_started =
            phase_durations.as_ref().map(|_| std::time::Instant::now());
        let native_display = self
            .native_display
            .as_ref()
            .ok_or_else(|| io::Error::other("Atomic EGL display is unavailable"))?;
        let context = self
            .egl_context
            .ok_or_else(|| io::Error::other("Atomic EGL context is unavailable"))?;
        native_display
            .egl
            .make_current(native_display.display, None, None, Some(context))
            .map_err(native_egl_io_error)?;
        if let (Some(durations), Some(started)) =
            (phase_durations.as_deref_mut(), context_activation_started)
        {
            durations.egl_context_activation_ns = elapsed_nanos(started);
        }

        let target_preparation_started =
            phase_durations.as_ref().map(|_| std::time::Instant::now());
        let (framebuffer, sampleable_texture) = self
            .pool
            .as_ref()
            .ok_or_else(|| io::Error::other("explicit render pool is unavailable"))?
            .render_slot_for_request(target)?;
        let request = frozen_frame
            .draw_request(Some(frame_id))
            .ok_or_else(|| io::Error::other("frozen render frame request already consumed"))?;
        if let (Some(durations), Some(started)) =
            (phase_durations.as_deref_mut(), target_preparation_started)
        {
            durations.render_target_preparation_ns = durations
                .render_target_preparation_ns
                .saturating_add(elapsed_nanos(started));
        }

        let started = std::time::Instant::now();
        *gpu_sampling_started = true;
        let scene_drawing_started = phase_durations.as_ref().map(|_| std::time::Instant::now());
        let outcome = self
            .scene
            .as_mut()
            .ok_or_else(|| io::Error::other("Atomic scene renderer is unavailable"))?
            .draw_scene_to_target(
                &native_display.egl,
                native_display.display,
                EglOutputRenderTarget {
                    framebuffer,
                    sampleable_texture: Some(sampleable_texture),
                    width: self.width,
                    height: self.height,
                    buffer_age: target.buffer_age,
                    framebuffer_origin: OutputFramebufferOrigin::TopLeftScanout,
                },
                request,
            );
        if let (Some(durations), Some(scene_started)) =
            (phase_durations.as_deref_mut(), scene_drawing_started)
        {
            durations.scene_drawing_ns = elapsed_nanos(scene_started);
        }
        let outcome = outcome.map_err(native_egl_io_error)?;
        match outcome {
            EglFrameOutcome::Rendered {
                commit,
                stats,
                lifecycle_evidence,
            } => {
                self.scene
                    .as_mut()
                    .expect("live Atomic render domain owns its scene renderer")
                    .trace_render_fence_export_begin();
                let fence_started = phase_durations.as_ref().map(|_| std::time::Instant::now());
                let fence_result = self.create_render_fence();
                if let (Some(durations), Some(started)) =
                    (phase_durations.as_deref_mut(), fence_started)
                {
                    durations.native_render_fence_export_ns = elapsed_nanos(started);
                }
                self.scene
                    .as_mut()
                    .expect("live Atomic render domain owns its scene renderer")
                    .trace_render_fence_export_end();
                let fence = fence_result?;
                Ok(AtomicSlotRenderOutcome::Rendered(Box::new(
                    AtomicRenderedFrameParts {
                        slot: target.slot,
                        scene_commit: commit,
                        render_fence: fence,
                        stats,
                        lifecycle_evidence,
                        render_us: elapsed_micros(started),
                    },
                )))
            }
            EglFrameOutcome::Skipped { reason, .. } => {
                *gpu_sampling_started = false;
                Ok(AtomicSlotRenderOutcome::Skipped {
                    slot: target.slot,
                    reason,
                    render_us: elapsed_micros(started),
                })
            }
            EglFrameOutcome::LifecycleFallback { fallbacks, .. } => {
                Ok(AtomicSlotRenderOutcome::LifecycleFallback {
                    fallbacks,
                    render_us: elapsed_micros(started),
                })
            }
        }
    }

    pub(super) fn capture_scene(
        &mut self,
        frozen_frame: &mut FrozenNativeRenderFrame,
    ) -> io::Result<Vec<u8>> {
        let native_display = self
            .native_display
            .as_ref()
            .ok_or_else(|| io::Error::other("Atomic EGL display is unavailable"))?;
        let context = self
            .egl_context
            .ok_or_else(|| io::Error::other("Atomic EGL context is unavailable"))?;
        native_display
            .egl
            .make_current(native_display.display, None, None, Some(context))
            .map_err(native_egl_io_error)?;
        let request = frozen_frame
            .draw_request(None)
            .ok_or_else(|| io::Error::other("frozen capture frame request already consumed"))?;
        self.scene
            .as_mut()
            .ok_or_else(|| io::Error::other("Atomic scene renderer is unavailable"))?
            .capture_scene(
                &native_display.egl,
                native_display.display,
                request,
                OutputFramebufferOrigin::TopLeftScanout,
            )
            .map_err(native_egl_io_error)
    }

    pub(super) fn teardown(&mut self) {
        if self.torn_down {
            return;
        }
        if let (Some(native_display), Some(context)) =
            (self.native_display.as_ref(), self.egl_context)
        {
            let _ =
                native_display
                    .egl
                    .make_current(native_display.display, None, None, Some(context));
            self.finish_render_work();
            if let (Some(pool), Some(gl)) = (self.pool.take(), self.gl.as_ref()) {
                pool.teardown(gl, &native_display.egl, native_display.display);
            }
            drop(self.scene.take());
            let _ = native_display
                .egl
                .make_current(native_display.display, None, None, None);
            let _ = native_display
                .egl
                .destroy_context(native_display.display, context);
            self.egl_context = None;
        }
        drop(self.gl.take());
        if let Some(mut native_display) = self.native_display.take() {
            native_display.terminate_and_drop_device();
        }
        self.torn_down = true;
    }
}

impl Drop for AtomicRenderDomain {
    fn drop(&mut self) {
        self.teardown();
    }
}

struct AtomicRenderPool {
    slots: [AtomicRenderSlot; EXPLICIT_OUTPUT_SLOT_CAPACITY],
    pool_generation: u64,
}

impl AtomicRenderPool {
    fn import_descriptors(
        descriptors: Vec<AtomicRenderSlotDescriptor>,
        pool_generation: u64,
        egl: &EglInstance,
        display: egl::Display,
        gl: &glow::Context,
        image_target: GlEglImageTargetTexture2DOes,
    ) -> io::Result<Self> {
        let identities = descriptors
            .iter()
            .map(|descriptor| (descriptor.id, descriptor.pool_generation))
            .collect::<Vec<_>>();
        validate_render_slot_identities(&identities, pool_generation)?;
        let mut slots = Vec::with_capacity(EXPLICIT_OUTPUT_SLOT_CAPACITY);
        for descriptor in descriptors {
            match AtomicRenderSlot::import(descriptor, egl, display, gl, image_target) {
                Ok(slot) => slots.push(slot),
                Err(error) => {
                    Self::teardown_partial(&slots, gl, egl, display);
                    return Err(error);
                }
            }
        }
        let slots: [AtomicRenderSlot; EXPLICIT_OUTPUT_SLOT_CAPACITY] = match slots.try_into() {
            Ok(slots) => slots,
            Err(slots) => {
                Self::teardown_partial(&slots, gl, egl, display);
                return Err(io::Error::other(
                    "explicit render pool did not construct 3 slots",
                ));
            }
        };
        Ok(Self {
            slots,
            pool_generation,
        })
    }

    fn render_slot_for_request(
        &self,
        request: RenderTargetRequest,
    ) -> io::Result<(glow::Framebuffer, glow::Texture)> {
        if request.pool_generation != self.pool_generation {
            return Err(io::Error::other(
                "render target request belongs to a different pool generation",
            ));
        }
        let slot = self
            .slots
            .iter()
            .find(|slot| slot.id == request.slot)
            .filter(|slot| slot.pool_generation == request.pool_generation)
            .ok_or_else(|| {
                io::Error::other("explicit render slot does not match target request")
            })?;
        Ok((slot.gl_framebuffer, slot.texture))
    }

    fn identities(&self) -> Vec<(OutputSlotId, u64)> {
        self.slots
            .iter()
            .map(|slot| (slot.id, slot.pool_generation))
            .collect()
    }

    fn rebind_pool_generation(&mut self, generation: u64) {
        self.pool_generation = generation;
        for slot in &mut self.slots {
            slot.pool_generation = generation;
        }
    }

    fn teardown(self, gl: &glow::Context, egl: &EglInstance, display: egl::Display) {
        Self::teardown_partial(&self.slots, gl, egl, display);
        drop(self);
    }

    fn teardown_partial(
        slots: &[AtomicRenderSlot],
        gl: &glow::Context,
        egl: &EglInstance,
        display: egl::Display,
    ) {
        teardown_render_slot_resources(
            slots,
            |slot| unsafe {
                gl.delete_framebuffer(slot.gl_framebuffer);
                gl.delete_texture(slot.texture);
            },
            |slot| {
                let _ = egl.destroy_image(display, slot.egl_image);
            },
        );
    }
}

fn teardown_render_slot_resources<T>(
    slots: &[T],
    mut delete_gl_resources: impl FnMut(&T),
    mut destroy_egl_images: impl FnMut(&T),
) {
    for slot in slots {
        delete_gl_resources(slot);
    }
    for slot in slots {
        destroy_egl_images(slot);
    }
}

struct AtomicRenderSlot {
    id: OutputSlotId,
    pool_generation: u64,
    egl_image: egl::Image,
    texture: glow::Texture,
    gl_framebuffer: glow::Framebuffer,
}

impl AtomicRenderSlot {
    fn import(
        descriptor: AtomicRenderSlotDescriptor,
        egl: &EglInstance,
        display: egl::Display,
        gl: &glow::Context,
        image_target: GlEglImageTargetTexture2DOes,
    ) -> io::Result<Self> {
        let AtomicRenderSlotDescriptor {
            id,
            pool_generation,
            buffer,
        } = descriptor;
        let attributes = EglGlesDmabufImportAttributes::from_handle(&buffer)
            .map_err(|error| io::Error::other(format!("invalid EGL output import: {error:?}")))?;
        let no_context = unsafe { egl::Context::from_ptr(egl::NO_CONTEXT) };
        let null_buffer = unsafe { egl::ClientBuffer::from_ptr(ptr::null_mut()) };
        let image = egl
            .create_image(
                display,
                no_context,
                EGL_LINUX_DMA_BUF_EXT,
                null_buffer,
                attributes.as_slice(),
            )
            .map_err(|error| {
                io::Error::other(format!("EGL output image import failed: {error}"))
            })?;
        let image_guard = EglImageGuard::new(image, |image| {
            let _ = egl.destroy_image(display, image);
        });

        let texture = unsafe { gl.create_texture().map_err(io::Error::other)? };
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
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
            image_target(glow::TEXTURE_2D, image_guard.image().as_ptr());
            if gl.get_error() != glow::NO_ERROR {
                gl.delete_texture(texture);
                return Err(io::Error::other(
                    "glEGLImageTargetTexture2DOES failed for explicit output BO",
                ));
            }
        }
        let framebuffer = match unsafe { gl.create_framebuffer() } {
            Ok(framebuffer) => framebuffer,
            Err(error) => {
                unsafe { gl.delete_texture(texture) };
                return Err(io::Error::other(error));
            }
        };
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                gl.delete_framebuffer(framebuffer);
                gl.delete_texture(texture);
                return Err(io::Error::other("explicit output FBO is incomplete"));
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
        }
        Ok(Self {
            id,
            pool_generation,
            egl_image: image_guard.disarm(),
            texture,
            gl_framebuffer: framebuffer,
        })
    }
}

fn validate_render_slot_identities(
    identities: &[(OutputSlotId, u64)],
    pool_generation: u64,
) -> io::Result<()> {
    if identities.len() != EXPLICIT_OUTPUT_SLOT_CAPACITY {
        return Err(io::Error::other(
            "explicit render pool has the wrong slot count",
        ));
    }
    let mut ids = std::collections::HashSet::with_capacity(identities.len());
    for (id, generation) in identities {
        if *generation != pool_generation || !ids.insert(id.get()) {
            return Err(io::Error::other(
                "explicit render pool has invalid slot identity",
            ));
        }
    }
    Ok(())
}

pub(crate) enum AtomicSlotRenderOutcome {
    Rendered(Box<AtomicRenderedFrameParts>),
    Skipped {
        slot: OutputSlotId,
        reason: FrameSkipReason,
        render_us: u64,
    },
    LifecycleFallback {
        fallbacks: LifecycleRenderFallbacks,
        render_us: u64,
    },
}

pub(crate) struct AtomicRenderedFrameParts {
    pub(crate) slot: OutputSlotId,
    pub(crate) scene_commit: EglSceneFrameCommit,
    pub(crate) render_fence: NativeRenderFence,
    pub(crate) stats: GlesSceneFrameStats,
    pub(crate) lifecycle_evidence: LifecycleRenderEvidence,
    pub(crate) render_us: u64,
}

fn native_egl_io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn elapsed_nanos(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn elapsed_micros(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn terminate_before_release_native_device(
    display_initialized: bool,
    terminate_display: impl FnOnce(),
    release_render_device: impl FnOnce(),
) {
    if display_initialized {
        terminate_display();
    }
    release_render_device();
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::{
        AtomicRenderDomainBootstrapPayload, teardown_render_slot_resources,
        terminate_before_release_native_device,
    };

    #[test]
    fn render_domain_worker_bootstrap_payload_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<AtomicRenderDomainBootstrapPayload>();
    }

    #[test]
    fn native_gbm_device_is_released_after_egl_display_termination() {
        let events = RefCell::new(Vec::new());
        terminate_before_release_native_device(
            true,
            || events.borrow_mut().push("terminate EGLDisplay"),
            || events.borrow_mut().push("drop render GBM device"),
        );
        assert_eq!(
            events.into_inner(),
            ["terminate EGLDisplay", "drop render GBM device"]
        );
    }

    #[test]
    fn imported_slot_gl_objects_are_deleted_before_egl_images() {
        let events = RefCell::new(Vec::new());
        teardown_render_slot_resources(
            &[0, 1],
            |slot| {
                events.borrow_mut().push(format!("framebuffer-{slot}"));
                events.borrow_mut().push(format!("texture-{slot}"));
            },
            |slot| events.borrow_mut().push(format!("image-{slot}")),
        );
        assert_eq!(
            events.into_inner(),
            [
                "framebuffer-0",
                "texture-0",
                "framebuffer-1",
                "texture-1",
                "image-0",
                "image-1",
            ]
        );
    }
}
