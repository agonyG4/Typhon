use std::{
    fs, io, iter,
    os::fd::{AsFd, AsRawFd},
    ptr,
};

use gbm::AsRaw as _;
use glow::HasContext;
use khronos_egl as egl;
use oblivion_one::compositor::FrameBatchDiscardReason;
use oblivion_one::native::kms::AtomicFlipRequest;
use oblivion_one::native::kms::{AtomicDiscovery, DrmFormatModifierPair};
use oblivion_one::native::sync_file::SyncFileDeadlineHint;

use crate::egl_renderer::dmabuf::query_egl_renderable_dmabuf_formats;
use crate::egl_renderer::native_fence::{NativeFenceFunctions, NativeRenderFence};
use crate::egl_renderer::{
    EglFrameOutcome, EglInstance, EglOutputRenderTarget, EglSceneFrameCommit, GlesSceneRenderer,
    choose_surfaceless_egl_config, create_gles_context, detect_partial_repaint_capabilities,
    load_egl_image_target_texture_2d,
};

use super::*;

pub(crate) struct AtomicEglGbmScanout {
    _device: gbm::Device<std::os::fd::OwnedFd>,
    egl: EglInstance,
    egl_display: egl::Display,
    egl_context: egl::Context,
    gl: glow::Context,
    scene: GlesSceneRenderer,
    native_fence_functions: NativeFenceFunctions,
    pool: Option<AtomicOutputPool>,
    swapchain: Option<AtomicOutputSwapchain>,
    width: u32,
    height: u32,
    dmabuf_feedback: EglGlesDmabufFeedback,
    dmabuf_main_device: Option<u64>,
    dmabuf_main_device_path: Option<String>,
    pub(crate) format_modifier: DrmFormatModifierPair,
    drm_cleanup_armed: bool,
    deadline_hints_enabled: bool,
    counters: ExplicitOutputCounters,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ExplicitOutputCounters {
    pub(crate) sync_file_deadline_hints_applied: u64,
    pub(crate) sync_file_deadline_hints_unsupported: u64,
    pub(crate) sync_file_deadline_hints_failed: u64,
    pub(crate) atomic_in_fence_submissions: u64,
    pub(crate) atomic_out_fences_received: u64,
    pub(crate) atomic_out_fence_missing: u64,
}

struct DeviceAllocationProbe<'a> {
    device: &'a gbm::Device<std::os::fd::OwnedFd>,
    width: u32,
    height: u32,
}

impl GbmAllocationProbe for DeviceAllocationProbe<'_> {
    fn supports(&mut self, candidate: DrmFormatModifierPair) -> bool {
        let Ok(format) = gbm::Format::try_from(candidate.fourcc) else {
            return false;
        };
        self.device
            .create_buffer_object_with_modifiers2::<()>(
                self.width,
                self.height,
                format,
                iter::once(gbm::Modifier::from(candidate.modifier)),
                gbm::BufferObjectFlags::SCANOUT | gbm::BufferObjectFlags::RENDERING,
            )
            .is_ok()
    }
}

impl AtomicEglGbmScanout {
    pub(crate) fn prepare_session_recovery(&self) -> io::Result<AtomicExplicitRecovery> {
        let swapchain = self.swapchain()?;
        let current = swapchain.current();
        Ok(AtomicExplicitRecovery {
            framebuffer: self.framebuffer(current)?,
            current,
            pool_generation: swapchain.pool_generation(),
        })
    }

    pub(crate) fn suspend_for_session(
        &mut self,
        server: &mut OwnCompositorServer,
    ) -> io::Result<()> {
        if let Some(frame) = self.swapchain_mut()?.suspend_abandon_ready()? {
            server.discard_frame_batch(
                frame.protocol_batch_id,
                FrameBatchDiscardReason::SuspendAbandonment,
            );
            self.scene.discard_rendered(frame.scene_commit);
            drop(frame.surface_damage);
        }
        Ok(())
    }

    pub(crate) fn complete_session_recovery(
        &mut self,
        recovery: AtomicExplicitRecovery,
        server: &mut OwnCompositorServer,
    ) -> io::Result<()> {
        let swapchain = self.swapchain()?;
        if swapchain.pool_generation() != recovery.pool_generation
            || swapchain.current() != recovery.current
            || self.framebuffer(recovery.current)? != recovery.framebuffer
        {
            return Err(io::Error::other(
                "explicit output recovery token no longer matches the active pool",
            ));
        }
        let fence_signaled = swapchain.suspended_ready_fence_signaled()?;
        if !fence_signaled {
            return Err(io::Error::other(
                "suspended-ready output fence is not signaled after recovery modeset",
            ));
        }
        self.swapchain_mut()?.recover_suspended_slot(true)?;
        if let Some(frame) = self.swapchain_mut()?.retire_pending_after_recovery() {
            server.discard_frame_batch(
                frame.protocol_batch_id,
                FrameBatchDiscardReason::SuspendAbandonment,
            );
            self.scene.discard_rendered(frame.scene_commit);
            drop(frame.surface_damage);
        }
        Ok(())
    }

    pub(crate) fn rebind_session_generation(&mut self, generation: u64) {
        let Some(pool) = self.pool.as_mut() else {
            return;
        };
        if let Some(swapchain) = self.swapchain.as_mut() {
            swapchain
                .rebind_pool_generation(generation)
                .expect("recovery retires all non-current explicit output ownership");
        }
        pool.pool_generation = generation;
        for slot in &mut pool.slots {
            slot.pool_generation = generation;
        }
        self.scene.invalidate_presented_damage_history();
    }

    pub(crate) fn create_unattached_pool(
        kms: &fs::File,
        discovery: &AtomicDiscovery,
        width: u32,
        height: u32,
        pool_generation: u64,
    ) -> io::Result<Self> {
        let gbm_fd = duplicate_fd_cloexec(kms.as_raw_fd()).map_err(io::Error::from_raw_os_error)?;
        let device = gbm::Device::new(gbm_fd)?;
        let egl = unsafe { EglInstance::load_required() }.map_err(native_egl_io_error)?;
        const EGL_PLATFORM_GBM_KHR: egl::Enum = 0x31d7;
        let egl_display = unsafe {
            egl.get_platform_display(
                EGL_PLATFORM_GBM_KHR,
                device.as_raw_mut() as egl::NativeDisplayType,
                &[egl::ATTRIB_NONE],
            )
        }
        .map_err(native_egl_io_error)?;
        egl.initialize(egl_display).map_err(native_egl_io_error)?;
        let mut created_context = None;
        let result = (|| {
            egl.bind_api(egl::OPENGL_ES_API)
                .map_err(native_egl_io_error)?;
            let extensions = egl
                .query_string(Some(egl_display), egl::EXTENSIONS)
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

            let egl_formats = query_egl_renderable_dmabuf_formats(&egl, egl_display);
            let mut probe = DeviceAllocationProbe {
                device: &device,
                width,
                height,
            };
            let format_modifier = select_output_format_modifier(
                &discovery.plane_scanout_formats,
                &egl_formats,
                &mut probe,
            )?;
            let config = choose_surfaceless_egl_config(&egl, egl_display, format_modifier.fourcc)
                .map_err(native_egl_io_error)?;
            let egl_context =
                create_gles_context(&egl, egl_display, config).map_err(native_egl_io_error)?;
            created_context = Some(egl_context);
            if let Err(error) = egl.make_current(egl_display, None, None, Some(egl_context)) {
                return Err(native_egl_io_error(error));
            }
            let image_target = load_egl_image_target_texture_2d(&egl).ok_or_else(|| {
                io::Error::other("explicit Atomic EGL/GBM requires GL_OES_EGL_image")
            })?;
            let native_fence_functions =
                NativeFenceFunctions::load(&egl, egl_display).map_err(|error| {
                    io::Error::other(format!(
                        "native output fence initialization failed: {error}"
                    ))
                })?;
            let gl = unsafe {
                glow::Context::from_loader_function(|name| {
                    egl.get_proc_address(name)
                        .map(|symbol| symbol as *const _)
                        .unwrap_or(ptr::null())
                })
            };
            let scene = GlesSceneRenderer::new_current(
                &egl,
                width,
                height,
                Some(image_target),
                detect_partial_repaint_capabilities(&egl, egl_display, false),
            )
            .map_err(native_egl_io_error)?;
            let dmabuf_feedback = query_egl_dmabuf_feedback(&egl, egl_display);
            let (dmabuf_main_device_path, dmabuf_main_device) =
                query_egl_main_device(&egl, egl_display)
                    .map_or((None, None), |(path, device)| (Some(path), Some(device)));
            let format = gbm::Format::try_from(format_modifier.fourcc)
                .map_err(|_| io::Error::other("selected output FourCC is unsupported by GBM"))?;
            let usage = gbm::BufferObjectFlags::SCANOUT | gbm::BufferObjectFlags::RENDERING;
            let drm = kms.as_fd();
            let mut slots = Vec::with_capacity(EXPLICIT_OUTPUT_SLOT_CAPACITY);
            for raw_id in 0..EXPLICIT_OUTPUT_SLOT_CAPACITY {
                let slot = (|| {
                    let bo = device.create_buffer_object_with_modifiers2::<()>(
                        width,
                        height,
                        format,
                        iter::once(gbm::Modifier::from(format_modifier.modifier)),
                        usage,
                    )?;
                    let descriptor = explicit_framebuffer_descriptor(&bo)?;
                    let framebuffer = add_explicit_framebuffer(drm, &descriptor)?;
                    let id = OutputSlotId::new(u8::try_from(raw_id).unwrap()).unwrap();
                    AtomicOutputSlot::import(
                        id,
                        pool_generation,
                        bo,
                        framebuffer,
                        &egl,
                        egl_display,
                        &gl,
                        image_target,
                    )
                    .inspect_err(|_| {
                        let _ = drm_ffi::mode::rm_fb(drm, framebuffer.get());
                    })
                })();
                match slot {
                    Ok(slot) => slots.push(slot),
                    Err(error) => {
                        teardown_atomic_slots(&slots, &gl, &egl, egl_display, drm);
                        return Err(error);
                    }
                }
            }
            if let Err(error) = AtomicOutputPool::validate_slots(&slots, pool_generation) {
                teardown_atomic_slots(&slots, &gl, &egl, egl_display, drm);
                return Err(error);
            }
            let slots: [AtomicOutputSlot; EXPLICIT_OUTPUT_SLOT_CAPACITY] = match slots.try_into() {
                Ok(slots) => slots,
                Err(slots) => {
                    teardown_atomic_slots(&slots, &gl, &egl, egl_display, drm);
                    return Err(io::Error::other(
                        "explicit output pool did not construct 3 slots",
                    ));
                }
            };
            let pool = AtomicOutputPool::from_validated_slots(slots, pool_generation);
            Ok((
                egl_context,
                gl,
                native_fence_functions,
                scene,
                pool,
                format_modifier,
                dmabuf_feedback,
                dmabuf_main_device,
                dmabuf_main_device_path,
            ))
        })();

        match result {
            Ok((
                egl_context,
                gl,
                native_fence_functions,
                scene,
                pool,
                format_modifier,
                dmabuf_feedback,
                dmabuf_main_device,
                dmabuf_main_device_path,
            )) => Ok(Self {
                _device: device,
                egl,
                egl_display,
                egl_context,
                gl,
                scene,
                native_fence_functions,
                pool: Some(pool),
                swapchain: None,
                width,
                height,
                dmabuf_feedback,
                dmabuf_main_device,
                dmabuf_main_device_path,
                format_modifier,
                drm_cleanup_armed: true,
                deadline_hints_enabled: true,
                counters: ExplicitOutputCounters::default(),
            }),
            Err(error) => {
                let _ = egl.make_current(egl_display, None, None, None);
                if let Some(context) = created_context {
                    let _ = egl.destroy_context(egl_display, context);
                }
                let _ = egl.terminate(egl_display);
                Err(error)
            }
        }
    }

    pub(crate) fn create_render_fence(&self) -> io::Result<NativeRenderFence> {
        NativeRenderFence::create(
            &self.egl,
            self.egl_display,
            &self.gl,
            self.native_fence_functions,
        )
        .map_err(|error| io::Error::other(format!("native render fence export failed: {error}")))
    }

    pub(crate) fn initial_slot(&self) -> OutputSlotId {
        OutputSlotId::new(0).expect("slot zero is valid")
    }

    pub(crate) fn framebuffer(&self, slot: OutputSlotId) -> io::Result<FramebufferId> {
        Ok(self.slot(slot)?.framebuffer)
    }

    pub(crate) fn plane_count(&self) -> io::Result<u32> {
        self.pool
            .as_ref()
            .map(|pool| pool.slots[0].bo.plane_count())
            .ok_or_else(|| io::Error::other("explicit output pool is unavailable"))
    }

    pub(crate) fn render_to_slot(
        &mut self,
        slot: OutputSlotId,
        renderer: &mut NativeFrameRenderer,
        server: &OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
        damage: &NativeOutputDamage,
    ) -> io::Result<AtomicRenderedFrameParts> {
        self.egl
            .make_current(self.egl_display, None, None, Some(self.egl_context))
            .map_err(native_egl_io_error)?;
        let (framebuffer, buffer_age) = {
            let slot = self.slot(slot)?;
            let serial = self
                .swapchain
                .as_ref()
                .map_or(0, AtomicOutputSwapchain::presentation_serial);
            (slot.gl_framebuffer, slot.buffer_age(serial))
        };
        let request = renderer.egl_scene_draw_request(
            self.width,
            self.height,
            server,
            input_state,
            cursor_mode,
            Some(damage.as_renderer_damage(self.width, self.height)),
        );
        let started = Instant::now();
        let outcome = self
            .scene
            .draw_scene_to_target(
                &self.egl,
                self.egl_display,
                EglOutputRenderTarget {
                    framebuffer,
                    width: self.width,
                    height: self.height,
                    buffer_age,
                },
                request,
            )
            .map_err(native_egl_io_error)?;
        let EglFrameOutcome::Rendered { commit, stats } = outcome else {
            return Err(io::Error::other(
                "explicit Atomic output render unexpectedly produced no frame",
            ));
        };
        let fence = self.create_render_fence()?;
        Ok(AtomicRenderedFrameParts {
            slot,
            scene_commit: commit,
            render_fence: fence,
            stats,
            render_us: elapsed_micros(started),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_frame(
        &mut self,
        renderer: &mut NativeFrameRenderer,
        server: &mut OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
        damage: &NativeOutputDamage,
        render_generation: u64,
        target: PresentationTarget,
        composite_started_at: MonotonicTimestampNs,
    ) -> io::Result<u64> {
        let (slot, frame_id, pool_generation) = {
            let swapchain = self.swapchain_mut()?;
            let slot = swapchain.acquire_render_slot()?;
            (slot, swapchain.next_frame_id(), swapchain.pool_generation())
        };
        let protocol_batch_id = server.take_frame_batch_for_render(frame_id);
        let surface_damage = server.capture_surface_damage_presentation();
        let parts =
            match self.render_to_slot(slot, renderer, server, input_state, cursor_mode, damage) {
                Ok(parts) => parts,
                Err(error) => {
                    server.discard_frame_batch(
                        protocol_batch_id,
                        FrameBatchDiscardReason::FatalOutputFailure,
                    );
                    let _ = self
                        .swapchain_mut()?
                        .quarantine_rendering(None, OutputQuarantineReason::PostDrawRenderFailure);
                    return Err(error);
                }
            };
        let rendered_at = MonotonicTimestampNs::new(monotonic_now_ns()?);
        let frame = RenderedOutputFrame {
            id: frame_id,
            slot,
            render_generation,
            pool_generation,
            target,
            render_fence: parts.render_fence,
            scene_commit: parts.scene_commit,
            surface_damage,
            protocol_batch_id,
            composite_started_at,
            fence_exported_at: rendered_at,
            rendered_at,
            cpu_prepass_duration_ns: 0,
            cpu_encode_duration_ns: parts.render_us.saturating_mul(1_000),
        };
        self.swapchain_mut()?.finish_render_owned(frame)
    }

    pub(crate) fn submit_ready_frame(
        &mut self,
        kms: &KmsBackendSelection,
        server: &mut OwnCompositorServer,
    ) -> io::Result<u64> {
        let mut frame = self.swapchain_mut()?.take_ready_for_submission()?;
        let framebuffer = self.framebuffer(frame.slot)?;
        let token = PageFlipToken::new(allocate_native_page_flip_token())
            .expect("allocated native pageflip token is nonzero");
        if self.deadline_hints_enabled {
            match frame
                .render_fence
                .apply_deadline_hint(frame.target.presentation_time.get(), monotonic_now_ns()?)
            {
                Ok(Some(SyncFileDeadlineHint::Applied)) => {
                    self.counters.sync_file_deadline_hints_applied += 1;
                }
                Ok(None) => {}
                Ok(Some(SyncFileDeadlineHint::Unsupported)) => {
                    self.counters.sync_file_deadline_hints_unsupported += 1;
                    self.deadline_hints_enabled = false;
                }
                Err(error)
                    if matches!(error.raw_os_error(), Some(libc::EBADF) | Some(libc::EFAULT)) =>
                {
                    let frame = self.swapchain_mut()?.submission_failed(frame)?;
                    self.discard_failed_frame(server, frame);
                    return Err(io::Error::other(format!(
                        "invalid native fence deadline-hint contract: {error}"
                    )));
                }
                Err(error) => {
                    self.counters.sync_file_deadline_hints_failed += 1;
                    eprintln!("native sync-file deadline hints disabled: {error}");
                    self.deadline_hints_enabled = false;
                }
            }
        }
        let in_fence = match frame.render_fence.take_submission_fd() {
            Ok(fence) => fence,
            Err(error) => {
                let frame = self.swapchain_mut()?.submission_failed(frame)?;
                self.discard_failed_frame(server, frame);
                return Err(error);
            }
        };
        let submit_started_at = MonotonicTimestampNs::new(monotonic_now_ns()?);
        let submission = kms.submit_atomic_flip(AtomicFlipRequest {
            framebuffer,
            token,
            in_fence,
        });
        let submit_returned_at = MonotonicTimestampNs::new(monotonic_now_ns()?);
        match submission {
            Ok(submission) => {
                self.counters.atomic_in_fence_submissions += 1;
                if submission.out_fence.is_some() {
                    self.counters.atomic_out_fences_received += 1;
                } else {
                    self.counters.atomic_out_fence_missing += 1;
                }
                self.swapchain_mut()?.submission_succeeded(
                    frame,
                    token,
                    submission.out_fence,
                    submit_started_at,
                    submit_returned_at,
                )?;
                Ok(token.get())
            }
            Err(error) => {
                let frame = self.swapchain_mut()?.submission_failed(frame)?;
                self.discard_failed_frame(server, frame);
                Err(io::Error::other(format!(
                    "explicit Atomic output submission failed: {error}"
                )))
            }
        }
    }

    pub(crate) fn complete_pageflip(
        &mut self,
        token: PageFlipToken,
        presentation: FramePresentation,
        server: &mut OwnCompositorServer,
    ) -> io::Result<PresentedOutputFrame> {
        let generation = self.swapchain()?.pool_generation();
        let completed = self.swapchain_mut()?.complete_pageflip(token, generation)?;
        let fence_signal = completed
            .frame
            .render_fence
            .sample_timing_nonblocking(monotonic_now_ns()?)?
            .map(|(timestamp, quality)| (MonotonicTimestampNs::new(timestamp), quality));
        let RenderedOutputFrame {
            id,
            target,
            scene_commit,
            surface_damage,
            protocol_batch_id,
            composite_started_at,
            rendered_at,
            ..
        } = completed.frame;
        self.scene.commit_presented(scene_commit);
        server.commit_surface_damage_presented(surface_damage);
        server.complete_presented_frame_batch(id, protocol_batch_id, presentation);
        if let Some(pool) = self.pool.as_mut() {
            pool.slots[usize::from(completed.new_current.get())].last_presented_serial =
                Some(completed.presentation_serial);
        }
        Ok(PresentedOutputFrame {
            frame_id: id,
            target,
            composite_started_at,
            rendered_at,
            submit_started_at: completed.submit_started_at,
            submit_returned_at: completed.submit_returned_at,
            fence_signal,
        })
    }

    pub(crate) fn pending_timing_fd(&self) -> Option<RawFd> {
        self.swapchain.as_ref()?.pending_timing_fd()
    }

    pub(crate) const fn counters(&self) -> ExplicitOutputCounters {
        self.counters
    }

    pub(crate) fn sample_pending_timing(
        &mut self,
        observed_at: MonotonicTimestampNs,
    ) -> io::Result<Option<PendingFenceTiming>> {
        let Some(frame) = self.swapchain_mut()?.pending_frame_mut() else {
            return Ok(None);
        };
        let sample = frame
            .render_fence
            .sample_timing_nonblocking(observed_at.get())?;
        let Some((signaled_at, quality)) = sample else {
            return Ok(None);
        };
        let timing = PendingFenceTiming {
            frame_id: frame.id,
            target: frame.target,
            composite_started_at: frame.composite_started_at,
            signaled_at: MonotonicTimestampNs::new(signaled_at),
            quality,
        };
        drop(frame.render_fence.take_timing_fd());
        Ok(Some(timing))
    }

    fn discard_failed_frame(
        &mut self,
        server: &mut OwnCompositorServer,
        frame: RenderedOutputFrame,
    ) {
        server.discard_frame_batch(
            frame.protocol_batch_id,
            FrameBatchDiscardReason::FatalOutputFailure,
        );
        self.scene.discard_rendered(frame.scene_commit);
        drop(frame.surface_damage);
    }

    pub(crate) fn promote_initial_presented(
        &mut self,
        slot: OutputSlotId,
        scene_commit: EglSceneFrameCommit,
    ) -> io::Result<()> {
        let pool = self
            .pool
            .as_mut()
            .ok_or_else(|| io::Error::other("explicit output pool is unavailable"))?;
        let slots = OutputSlotSet::new([
            OutputSlotId::new(0).unwrap(),
            OutputSlotId::new(1).unwrap(),
            OutputSlotId::new(2).unwrap(),
        ])?;
        self.swapchain = Some(AtomicOutputSwapchain::from_presented_slots(
            slots,
            slot,
            pool.pool_generation,
        )?);
        pool.slots[usize::from(slot.get())].last_presented_serial = Some(0);
        self.scene.commit_presented(scene_commit);
        Ok(())
    }

    pub(crate) fn swapchain(&self) -> io::Result<&AtomicOutputSwapchain> {
        self.swapchain
            .as_ref()
            .ok_or_else(|| io::Error::other("explicit output swapchain is not presented"))
    }

    pub(crate) fn swapchain_mut(&mut self) -> io::Result<&mut AtomicOutputSwapchain> {
        self.swapchain
            .as_mut()
            .ok_or_else(|| io::Error::other("explicit output swapchain is not presented"))
    }

    pub(crate) fn dmabuf_feedback(&self) -> EglGlesDmabufFeedback {
        self.dmabuf_feedback.clone()
    }

    pub(crate) const fn dmabuf_main_device(&self) -> Option<u64> {
        self.dmabuf_main_device
    }

    pub(crate) fn dmabuf_main_device_path(&self) -> Option<String> {
        self.dmabuf_main_device_path.clone()
    }

    pub(crate) fn disarm_drm_cleanup(&mut self) {
        self.drm_cleanup_armed = false;
    }

    fn slot(&self, slot: OutputSlotId) -> io::Result<&AtomicOutputSlot> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| io::Error::other("explicit output pool is unavailable"))?;
        pool.slots
            .get(usize::from(slot.get()))
            .filter(|candidate| candidate.id == slot)
            .ok_or_else(|| io::Error::other("explicit output slot is unavailable"))
    }
}

pub(crate) struct AtomicRenderedFrameParts {
    pub(crate) slot: OutputSlotId,
    pub(crate) scene_commit: EglSceneFrameCommit,
    pub(crate) render_fence: NativeRenderFence,
    pub(crate) stats: GlesSceneFrameStats,
    pub(crate) render_us: u64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingFenceTiming {
    pub(crate) frame_id: u64,
    pub(crate) target: PresentationTarget,
    pub(crate) composite_started_at: MonotonicTimestampNs,
    pub(crate) signaled_at: MonotonicTimestampNs,
    pub(crate) quality: FenceTimestampQuality,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PresentedOutputFrame {
    pub(crate) frame_id: u64,
    pub(crate) target: PresentationTarget,
    pub(crate) composite_started_at: MonotonicTimestampNs,
    pub(crate) rendered_at: MonotonicTimestampNs,
    pub(crate) submit_started_at: MonotonicTimestampNs,
    pub(crate) submit_returned_at: MonotonicTimestampNs,
    pub(crate) fence_signal: Option<(MonotonicTimestampNs, FenceTimestampQuality)>,
}

impl AtomicRenderedFrameParts {
    pub(crate) fn paint_stats(&self, format: u32, width: u32, height: u32) -> NativePaintStats {
        NativePaintStats {
            backend: NativeScanoutKind::AtomicEglGbmExplicit,
            scanout_format: Some(format),
            width,
            height,
            bytes: 0,
            copy_bytes: 0,
            write_bytes: 0,
            gpu_draw_us: self.render_us,
            egl_swap_us: 0,
            shm_upload_bytes: self.stats.shm_upload_bytes,
            dmabuf_imports: self.stats.dmabuf_imports,
            dmabuf_reuses: self.stats.dmabuf_reuses,
            dmabuf_import_failures: self.stats.dmabuf_import_failures,
            dmabuf_cache_entries: self.stats.dmabuf_cache_entries,
            dmabuf_cache_peak_entries: self.stats.dmabuf_cache_peak_entries,
            dmabuf_cache_evictions: self.stats.dmabuf_cache_evictions,
            scene_rebuild: if self.stats.scene_rebuilt {
                DesktopSceneRebuildKind::Full
            } else {
                DesktopSceneRebuildKind::None
            },
            frame_copy: DesktopFrameCopyKind::None,
            total_us: self.render_us,
            render_us: self.render_us,
            copy_us: 0,
            write_us: 0,
            gles_repaint: Some(self.stats),
            swap_with_damage_used: false,
        }
    }
}

impl Drop for AtomicEglGbmScanout {
    fn drop(&mut self) {
        let _ = self
            .egl
            .make_current(self.egl_display, None, None, Some(self.egl_context));
        if let Some(pool) = self.pool.take() {
            let drm = self._device.as_fd();
            if self.drm_cleanup_armed {
                pool.teardown(&self.gl, &self.egl, self.egl_display, drm);
            } else {
                // GL/EGL resources must still be deleted; revoked DRM skips rm_fb.
                unsafe {
                    for slot in &pool.slots {
                        self.gl.delete_framebuffer(slot.gl_framebuffer);
                        self.gl.delete_texture(slot.texture);
                    }
                }
                for slot in &pool.slots {
                    let _ = self.egl.destroy_image(self.egl_display, slot.egl_image);
                }
                drop(pool);
            }
        }
        let _ = self.egl.make_current(self.egl_display, None, None, None);
        let _ = self.egl.destroy_context(self.egl_display, self.egl_context);
        let _ = self.egl.terminate(self.egl_display);
    }
}
