use super::*;
use oblivion_one::compositor::{
    RenderableSurfaceDamage, ResolvedEffectScene, SurfaceCommitCounter, SurfaceCommitSequence,
    SurfaceOpaqueRegion, SurfacePlacement, SurfaceRenderBackend, SurfaceResourceSyncState,
};
use oblivion_one::core::SceneNodeId;
use oblivion_one::effects::{
    CompiledFrameGraph, CompiledRenderPass, CustomFragmentSpec, DualKawaseBlurSpec,
    EffectAlphaMode, EffectColorConversion, EffectFailurePolicy, EffectFootprint,
    EffectFrameDemand, EffectInstanceExecutionDemand, EffectNode, EffectNodeId,
    EffectParameterBlock, EffectPassExecutionDemand, EffectProgram, EffectProgramId, EffectRect,
    EffectRegion, EffectSource, EffectWorkingSpace, GraphTextureId, GraphTexturePhysicalRect,
    GraphTexturePlan, GraphTextureSource, RenderPassKind, ShaderModuleId, validate_effect_program,
};
use oblivion_one::presentation_animation::{
    AnimationTime, PresentationEngine, PresentationGroupOpacity, PresentationOpacity,
    PresentationRect, PresentationRetainedVisualIdentity, PresentationRetainedVisualKind,
};
use oblivion_one::render_backend::buffer::{
    BufferIdAllocator, BufferIdentity, BufferSize, CommittedSurfaceBuffer, DmabufBufferHandle,
    DmabufImageKey, DmabufPlane, DmabufPlaneDescriptor, DrmFormat, DrmModifier,
};
use oblivion_one::window_lifecycle_animation::{
    LifecycleDirection, LifecycleEffectKind, LifecycleFrameSample, LifecycleSceneSample,
    LifecycleVisualGroup, LifecycleVisualSource, LifecycleVisualSourceKind, LifecycleWindowSample,
};

fn test_lifecycle_identity(
    window_id: oblivion_one::compositor::WindowId,
    started_at: u64,
) -> PresentationRetainedVisualIdentity {
    PresentationEngine::enabled()
        .begin_retained_visual(
            SceneNodeId::from_raw(window_id.get()).expect("test scene node"),
            PresentationRetainedVisualKind::WindowLifecycle,
            AnimationTime::from_nanos(started_at),
        )
        .expect("test retained identity")
}

const XR24: u32 = u32::from_le_bytes(*b"XR24");
const AR24: u32 = u32::from_le_bytes(*b"AR24");

fn egl_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .expect("EGL test lock is not poisoned")
}

struct GlesEffectTestHarness {
    egl: EglInstance,
    display: egl::Display,
    context: egl::Context,
    surface: egl::Surface,
    gl: glow::Context,
    renderer: GlesSceneRenderer,
    test_output_texture: Option<glow::Texture>,
    test_output_framebuffer: Option<glow::Framebuffer>,
    _egl_test_lock: std::sync::MutexGuard<'static, ()>,
}

impl GlesEffectTestHarness {
    fn new(width: u32, height: u32) -> Self {
        let egl_test_lock = egl_test_lock();
        const EGL_PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31dd;
        let egl = unsafe { EglInstance::load_required() }
            .expect("EGL loader is required for the effect coordinate test");
        let display = unsafe {
            egl.get_platform_display(
                EGL_PLATFORM_SURFACELESS_MESA,
                std::ptr::null_mut(),
                &[egl::ATTRIB_NONE],
            )
            .or_else(|_| {
                egl.get_display(egl::DEFAULT_DISPLAY)
                    .ok_or(egl::Error::BadDisplay)
            })
        }
        .expect("EGL display is available");
        egl.initialize(display).expect("EGL initializes");
        egl.bind_api(egl::OPENGL_ES_API)
            .expect("EGL binds the GLES API");
        let config_attributes = [
            egl::SURFACE_TYPE,
            egl::PBUFFER_BIT,
            egl::RENDERABLE_TYPE,
            egl::OPENGL_ES3_BIT,
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
        let count = egl
            .matching_config_count(display, &config_attributes)
            .expect("EGL returns GLES3 pbuffer configs");
        assert!(count > 0, "EGL exposes a GLES3 pbuffer config");
        let mut configs = Vec::with_capacity(count);
        egl.choose_config(display, &config_attributes, &mut configs)
            .expect("EGL chooses a GLES3 pbuffer config");
        let config = configs[0];
        let context = create_gles_context(&egl, display, config).expect("GLES3 context creates");
        let surface = egl
            .create_pbuffer_surface(
                display,
                config,
                &[
                    egl::WIDTH,
                    width as egl::Int,
                    egl::HEIGHT,
                    height as egl::Int,
                    egl::NONE,
                ],
            )
            .expect("GLES3 pbuffer surface creates");
        egl.make_current(display, Some(surface), Some(surface), Some(context))
            .expect("EGL makes the GLES3 context current");
        let cursor_image = Arc::new(
            CompositorCursorImage::from_argb8888(vec![0xffff_ffff], 1, 1, 0, 0)
                .expect("test cursor image is valid"),
        );
        let renderer = GlesSceneRenderer::new_current(
            &egl,
            width,
            height,
            None,
            EglPartialRepaintCapabilities {
                buffer_age: false,
                partial_render_repair: false,
                swap_buffers_with_damage: false,
            },
            cursor_image,
        )
        .expect("test GLES renderer creates");
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name)
                    .map(|symbol| symbol as *const c_void)
                    .unwrap_or(ptr::null())
            })
        };
        Self {
            _egl_test_lock: egl_test_lock,
            egl,
            display,
            context,
            surface,
            gl,
            renderer,
            test_output_texture: None,
            test_output_framebuffer: None,
        }
    }

    fn install_texture_backed_output(&mut self) {
        assert!(self.test_output_texture.is_none());
        let width = self.renderer.scene_state.current_size.0;
        let height = self.renderer.scene_state.current_size.1;
        let texture = unsafe { self.gl.create_texture().expect("output texture creates") };
        let framebuffer = unsafe {
            self.gl
                .create_framebuffer()
                .expect("output framebuffer creates")
        };
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            self.gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::NEAREST as i32,
            );
            self.gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::NEAREST as i32,
            );
            self.gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            self.gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                width as i32,
                height as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            assert_eq!(
                self.gl.check_framebuffer_status(glow::FRAMEBUFFER),
                glow::FRAMEBUFFER_COMPLETE,
                "test output framebuffer is complete"
            );
        }
        self.renderer.scene_state.active_output_framebuffer = Some(framebuffer);
        self.renderer.scene_state.active_output_texture = Some(texture);
        self.test_output_texture = Some(texture);
        self.test_output_framebuffer = Some(framebuffer);
        self.renderer.establish_ordinary_scene_state();
    }
}

impl Drop for GlesEffectTestHarness {
    fn drop(&mut self) {
        self.renderer.destroy(&self.egl, self.display);
        unsafe {
            if let Some(framebuffer) = self.test_output_framebuffer.take() {
                self.gl.delete_framebuffer(framebuffer);
            }
            if let Some(texture) = self.test_output_texture.take() {
                self.gl.delete_texture(texture);
            }
        }
        let _ = self.egl.make_current(self.display, None, None, None);
        let _ = self.egl.destroy_surface(self.display, self.surface);
        let _ = self.egl.destroy_context(self.display, self.context);
        let _ = self.egl.terminate(self.display);
    }
}

mod support;
use support::*;

mod checkpoint_causal_gles;
mod checkpoint_replay;
mod dependency_free_replay_cache;
mod dmabuf;
mod effect_capture;
mod effect_passes;
mod effect_replay;
mod egl_config;
mod egl_resources;
mod lamp;
mod lifecycle_integration;
mod scene_rendering;
mod shm;
mod squash;
use egl_resources::{DropProbe, fake_egl_image};

mod effect_session;
mod lamp_geometry_tests;
mod presentation_clip;
