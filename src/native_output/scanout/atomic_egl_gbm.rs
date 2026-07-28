use std::{
    fs, io, iter,
    os::fd::{AsFd, AsRawFd},
    os::unix::fs::MetadataExt,
    ptr,
    time::Instant,
};

use gbm::AsRaw as _;
use glow::HasContext;
use khronos_egl as egl;
use oblivion_one::compositor::{
    CompositorFrameBatchId, DirectScanoutFeedbackCapabilities, DirectScanoutFormatCapability,
    FrameBatchDiscardReason, OwnCompositorServer, SurfaceDamagePresentation,
};
use oblivion_one::native::kms::{AtomicDiscovery, DrmFormatModifierPair};
use oblivion_one::native::presentation_deadline::{MonotonicTimestampNs, PresentationTarget};
use oblivion_one::render_backend::{
    buffer::{DrmFormat, DrmModifier},
    egl_gles::EglGlesDmabufFormat,
};

use crate::egl_renderer::dmabuf::{query_egl_main_device, query_egl_renderable_dmabuf_formats};
use crate::egl_renderer::native_fence::{NativeFenceFunctions, NativeRenderFence};
use crate::egl_renderer::{
    EglFrameOutcome, EglInstance, EglOutputRenderTarget, EglSceneFrameCommit, FrameSkipReason,
    GlesSceneRenderer, OutputFramebufferOrigin, choose_surfaceless_egl_config, create_gles_context,
    detect_partial_repaint_capabilities, load_egl_image_target_texture_2d,
};
use crate::native_output::runtime::{
    DirectTerminalCallbackDisposition, direct_terminal_callback_owner_leaks,
    settle_dropped_output_transaction, settle_failed_output_transaction,
    settle_no_visual_change_output_transaction, settle_superseded_output_transaction,
};

use super::atomic_direct::{direct_candidate_key, direct_scanout_debug};
use super::*;

#[cfg(test)]
mod confirmed_pageflip_tests;
mod direct;
mod worker;

#[path = "atomic_egl_gbm_transactions.rs"]
mod atomic_egl_gbm_transactions;

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
    direct: DirectScanoutControl,
    width: u32,
    height: u32,
    dmabuf_feedback: EglGlesDmabufFeedback,
    dmabuf_main_device: Option<u64>,
    dmabuf_main_device_path: Option<String>,
    dmabuf_scanout_capabilities: DirectScanoutFeedbackCapabilities,
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
    pub(crate) render_fence_timing_unavailable: u64,
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

    pub(crate) fn suspend_for_session(&mut self) -> io::Result<()> {
        self.direct_scanout_suspend()?;
        if let Some(token) = self.swapchain()?.worker_queued_token() {
            self.swapchain_mut()?.suspend_abandon_worker_queued(token)?;
        }
        self.swapchain_mut()?.suspend_abandon_ready()?;
        Ok(())
    }

    pub(crate) fn complete_session_recovery(
        &mut self,
        recovery: AtomicExplicitRecovery,
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
        let abandoned_ready = self.swapchain_mut()?.take_suspended_ready_frame();
        self.swapchain_mut()?.recover_suspended_slot(true)?;
        if let Some(frame) = abandoned_ready {
            self.scene.discard_rendered(frame.scene_commit);
            drop(frame.surface_damage);
        }
        if let Some(frame) = self.swapchain_mut()?.retire_pending_after_recovery() {
            self.scene.discard_rendered(frame.scene_commit);
            drop(frame.surface_damage);
        }
        match self.direct.complete_suspended() {
            DirectReleaseOutcome::Released {
                presented,
                suspended,
            } => {
                drop(presented);
                drop(suspended);
                Ok(())
            }
            DirectReleaseOutcome::Deferred { reason } => Err(io::Error::other(format!(
                "direct ownership release remained deferred after recovery: {reason:?}"
            ))),
            DirectReleaseOutcome::Violation { reason } => Err(io::Error::other(format!(
                "direct ownership release violated after recovery: {reason:?}"
            ))),
        }
    }

    pub(crate) fn release_direct_for_target_destroyed(&mut self) -> io::Result<()> {
        match self
            .direct
            .request_direct_release(DirectReleaseProof::TargetDestroyed, false)
        {
            DirectReleaseOutcome::Released {
                presented,
                suspended,
            } => {
                drop(presented);
                drop(suspended);
                Ok(())
            }
            DirectReleaseOutcome::Deferred { reason } => Err(io::Error::other(format!(
                "direct target-destroyed release remained deferred: {reason:?}"
            ))),
            DirectReleaseOutcome::Violation { reason } => Err(io::Error::other(format!(
                "direct target-destroyed release violated ownership: {reason:?}"
            ))),
        }
    }

    pub(crate) fn retain_direct_for_unproven_teardown(&mut self) {
        let _ = self
            .direct
            .request_direct_release(DirectReleaseProof::Unproven, false);
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
        self.direct
            .framebuffer_cache
            .clear_for_generation(generation);
        self.direct.drm_generation = generation;
        self.direct.inhibit_until_composited_present = true;
        self.direct.identity_viewport_metadata_logged = false;
        self.direct.last_debug_candidate = None;
        self.dmabuf_scanout_capabilities.output_generation = generation;
        self.scene.invalidate_presented_damage_history();
    }

    pub(crate) fn invalidate_direct_validation_cache(&mut self) {
        self.direct.invalidate_direct_validation_cache();
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
                oblivion_one::cursor_theme::shared_compositor_cursor_image(),
            )
            .map_err(native_egl_io_error)?;
            let renderer_dmabuf_feedback = query_egl_dmabuf_feedback(&egl, egl_display);
            let mut scanout_capabilities = Vec::new();
            for format in &discovery.plane_scanout_formats {
                if format.fourcc != DrmFormat::XRGB8888_FOURCC
                    || format.modifier == DrmModifier::INVALID.0
                {
                    continue;
                }
                let modifier = DrmModifier(format.modifier);
                if !renderer_dmabuf_feedback.supports(DrmFormat::Xrgb8888, modifier)
                    || !probe.supports(*format)
                {
                    continue;
                }
                scanout_capabilities.push(DirectScanoutFormatCapability {
                    format: format.fourcc,
                    modifier: format.modifier,
                });
            }
            let scanout_capabilities = DirectScanoutFeedbackCapabilities::new(
                kms.metadata()?.rdev(),
                pool_generation,
                discovery.pipeline.plane.get(),
                scanout_capabilities,
            );
            let dmabuf_feedback = EglGlesDmabufFeedback::with_scanout_tranche(
                scanout_capabilities.formats.iter().map(|format| {
                    EglGlesDmabufFormat::new(
                        DrmFormat::from_fourcc(format.format),
                        DrmModifier(format.modifier),
                    )
                }),
                renderer_dmabuf_feedback.formats().iter().copied(),
            );
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
                scanout_capabilities,
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
                scanout_capabilities,
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
                direct: DirectScanoutControl::new(kms.as_fd(), pool_generation),
                width,
                height,
                dmabuf_feedback,
                dmabuf_main_device,
                dmabuf_main_device_path,
                dmabuf_scanout_capabilities: scanout_capabilities,
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

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_to_slot(
        &mut self,
        slot: OutputSlotId,
        renderer: &mut NativeFrameRenderer,
        server: &OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
        damage: &NativeOutputDamage,
        gpu_sampling_started: &mut bool,
    ) -> io::Result<AtomicSlotRenderOutcome> {
        self.egl
            .make_current(self.egl_display, None, None, Some(self.egl_context))
            .map_err(native_egl_io_error)?;
        let (framebuffer, buffer_age) = {
            let slot = self.slot(slot)?;
            let (presentation_serial, presentation_pending) =
                self.swapchain.as_ref().map_or((0, false), |swapchain| {
                    (
                        swapchain.presentation_serial(),
                        swapchain.pending_slot().is_some(),
                    )
                });
            (
                slot.gl_framebuffer,
                slot.buffer_age(presentation_serial, presentation_pending),
            )
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
        *gpu_sampling_started = true;
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
                    framebuffer_origin: OutputFramebufferOrigin::TopLeftScanout,
                },
                request,
            )
            .map_err(native_egl_io_error)?;
        match outcome {
            EglFrameOutcome::Rendered { commit, stats } => {
                let fence = self.create_render_fence()?;
                Ok(AtomicSlotRenderOutcome::Rendered(Box::new(
                    AtomicRenderedFrameParts {
                        slot,
                        scene_commit: commit,
                        render_fence: fence,
                        stats,
                        render_us: elapsed_micros(started),
                    },
                )))
            }
            EglFrameOutcome::Skipped { reason, .. } => {
                // The renderer did not sample scene content.  Keep the slot
                // on the pre-GPU cancellation path and do not classify this
                // normal no-damage result as a post-draw failure.
                *gpu_sampling_started = false;
                Ok(AtomicSlotRenderOutcome::Skipped {
                    slot,
                    reason,
                    render_us: elapsed_micros(started),
                })
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_frame(
        &mut self,
        renderer: &mut NativeFrameRenderer,
        server: &mut OwnCompositorServer,
        output_transactions: &mut OutputTransactionLedger,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
        damage: &NativeOutputDamage,
        render_generation: u64,
        output_generation: u64,
        target: PresentationTarget,
        pacing_mode: NativeOutputPacingMode,
        cursor: Option<CursorPlaneAssignment>,
    ) -> io::Result<AtomicFrameRenderOutcome> {
        let (slot, frame_id, pool_generation) = {
            let swapchain = self.swapchain_mut()?;
            let slot = swapchain.acquire_render_slot_for(pacing_mode)?;
            (slot, swapchain.next_frame_id(), swapchain.pool_generation())
        };
        let framebuffer_id = match self.framebuffer(slot) {
            Ok(framebuffer) => framebuffer.get(),
            Err(error) => {
                self.swapchain_mut()?.cancel_render_before_gpu(slot)?;
                return Err(error);
            }
        };
        let protocol_batch_id = server.take_frame_batch_for_render(frame_id);
        let surface_damage = server.capture_surface_damage_presentation();
        let transaction_id = match output_transactions.allocate_id() {
            Ok(transaction_id) => transaction_id,
            Err(error) => {
                server.restore_frame_batch_after_render_failure(protocol_batch_id);
                self.swapchain_mut()?.cancel_render_before_gpu(slot)?;
                return Err(io::Error::other(error));
            }
        };
        let transaction = match OutputTransaction::composited(
            transaction_id,
            output_generation,
            MonotonicTimestampNs::new(monotonic_now_ns()?),
            target,
            pacing_mode,
            frame_id,
            render_generation,
            pool_generation,
            slot,
            framebuffer_id,
            cursor,
            protocol_batch_id,
        ) {
            Ok(transaction) => transaction,
            Err(error) => {
                server.restore_frame_batch_after_render_failure(protocol_batch_id);
                self.swapchain_mut()?.cancel_render_before_gpu(slot)?;
                return Err(io::Error::other(error));
            }
        };
        if let Err(error) = output_transactions.insert(transaction) {
            server.restore_frame_batch_after_render_failure(protocol_batch_id);
            self.swapchain_mut()?.cancel_render_before_gpu(slot)?;
            return Err(io::Error::other(error));
        }
        // This is the estimator's production render boundary. Everything before it may
        // include protocol bookkeeping or diagnostics; everything after it is explicit
        // scene encoding, fence export, and GPU work owned by this output frame.
        let composite_started_at = MonotonicTimestampNs::new(monotonic_now_ns()?);
        let mut gpu_sampling_started = false;
        let parts = match self.render_to_slot(
            slot,
            renderer,
            server,
            input_state,
            cursor_mode,
            damage,
            &mut gpu_sampling_started,
        ) {
            Ok(AtomicSlotRenderOutcome::Rendered(parts)) => *parts,
            Ok(AtomicSlotRenderOutcome::Skipped {
                slot,
                reason,
                render_us,
            }) => {
                settle_dropped_output_transaction(
                    output_transactions,
                    transaction_id,
                    OutputTransactionDropReason::NoVisualChange,
                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                    |obligations| {
                        let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                            io::Error::other("skipped render transaction has no frame batch")
                        })?;
                        server.restore_frame_batch_after_render_failure(batch_id);
                        self.swapchain_mut()?.cancel_render_before_gpu(slot)?;
                        Ok(())
                    },
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                return Ok(AtomicFrameRenderOutcome::Skipped { reason, render_us });
            }
            Err(error) => {
                let failure_stage = if gpu_sampling_started {
                    OutputTransactionFailureStage::RenderExecution
                } else {
                    OutputTransactionFailureStage::RenderPreparation
                };
                settle_failed_output_transaction(
                    output_transactions,
                    transaction_id,
                    failure_stage,
                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                    |obligations| {
                        let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                            io::Error::other("render failure transaction has no frame batch")
                        })?;
                        if gpu_sampling_started {
                            server.discard_frame_batch(
                                batch_id,
                                FrameBatchDiscardReason::FatalOutputFailure,
                            );
                            let _ = self.swapchain_mut()?.quarantine_rendering(
                                None,
                                OutputQuarantineReason::PostDrawRenderFailure,
                            );
                        } else {
                            server.restore_frame_batch_after_render_failure(batch_id);
                            self.swapchain_mut()?.cancel_render_before_gpu(slot)?;
                        }
                        Ok(())
                    },
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                return Err(error);
            }
        };
        let rendered_at = MonotonicTimestampNs::new(monotonic_now_ns()?);
        let frame = RenderedOutputFrame {
            id: frame_id,
            transaction_id,
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
        match self.swapchain_mut()?.finish_render_owned(frame) {
            Ok(frame_id) => {
                output_transactions
                    .mark_ready(transaction_id, rendered_at)
                    .map_err(io::Error::other)?;
                server.complete_rendered_frame_callbacks(protocol_batch_id);
                Ok(AtomicFrameRenderOutcome::Rendered {
                    frame_id,
                    transaction_id,
                })
            }
            Err(error) => {
                settle_failed_output_transaction(
                    output_transactions,
                    transaction_id,
                    OutputTransactionFailureStage::RenderExecution,
                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                    |obligations| {
                        let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                            io::Error::other("ready ownership failure has no frame batch")
                        })?;
                        server.discard_frame_batch(
                            batch_id,
                            FrameBatchDiscardReason::FatalOutputFailure,
                        );
                        Ok(())
                    },
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                Err(error)
            }
        }
    }

    pub(crate) fn complete_pageflip(
        &mut self,
        token: PageFlipToken,
    ) -> io::Result<CompositedPageflipCompletion> {
        let generation = self.swapchain()?.pool_generation();
        let completed = self.swapchain_mut()?.complete_pageflip(token, generation)?;
        let timing_result = monotonic_now_ns().and_then(|observed_at| {
            completed
                .frame
                .render_fence
                .sample_timing_nonblocking(observed_at)
        });
        let RenderedOutputFrame {
            id,
            transaction_id,
            target,
            scene_commit,
            surface_damage,
            protocol_batch_id,
            composite_started_at,
            rendered_at,
            ..
        } = completed.frame;
        let (fence_signal, timing_error) = complete_confirmed_pageflip_with_timing(
            timing_result.map(|sample| {
                sample.map(|(timestamp, quality)| (MonotonicTimestampNs::new(timestamp), quality))
            }),
            || {
                self.scene.commit_presented(scene_commit);
                if let Some(pool) = self.pool.as_mut() {
                    pool.slots[usize::from(completed.new_current.get())].last_presented_serial =
                        Some(completed.presentation_serial);
                }
            },
        );
        if let Some(error) = timing_error {
            self.counters.render_fence_timing_unavailable += 1;
            eprintln!(
                "native render-fence timing unavailable after confirmed pageflip for frame {id}: {error}"
            );
        }
        Ok(CompositedPageflipCompletion {
            presented: PresentedOutputFrame {
                frame_id: id,
                transaction_id,
                target,
                composite_started_at,
                rendered_at,
                submit_started_at: completed.submit_started_at,
                submit_returned_at: completed.submit_returned_at,
                fence_signal,
            },
            protocol_batch_id,
            surface_damage,
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

    fn discard_failed_frame_resources(&mut self, frame: RenderedOutputFrame) {
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
        self.direct.inhibit_until_composited_present = false;
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

    pub(crate) fn dmabuf_scanout_capabilities(&self) -> Option<DirectScanoutFeedbackCapabilities> {
        Some(self.dmabuf_scanout_capabilities.clone())
    }

    pub(crate) fn disarm_drm_cleanup(&mut self) {
        self.drm_cleanup_armed = false;
        self.direct.disarm_drm_cleanup();
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

fn complete_confirmed_pageflip_with_timing<T>(
    timing: io::Result<Option<T>>,
    complete_ownership: impl FnOnce(),
) -> (Option<T>, Option<io::Error>) {
    complete_ownership();
    match timing {
        Ok(timing) => (timing, None),
        Err(error) => (None, Some(error)),
    }
}

pub(crate) enum AtomicSlotRenderOutcome {
    Rendered(Box<AtomicRenderedFrameParts>),
    Skipped {
        slot: OutputSlotId,
        reason: FrameSkipReason,
        render_us: u64,
    },
}

pub(crate) enum AtomicFrameRenderOutcome {
    Rendered {
        frame_id: u64,
        transaction_id: OutputTransactionId,
    },
    Skipped {
        reason: FrameSkipReason,
        render_us: u64,
    },
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

#[derive(Debug)]
pub(crate) struct CompositedPageflipCompletion {
    pub(crate) presented: PresentedOutputFrame,
    pub(crate) protocol_batch_id: CompositorFrameBatchId,
    pub(crate) surface_damage: SurfaceDamagePresentation,
}

#[derive(Debug)]
pub(crate) struct PresentedOutputFrame {
    pub(crate) frame_id: u64,
    pub(crate) transaction_id: OutputTransactionId,
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
        // Prove that GLES no longer samples any imported client or scanout
        // image before releasing frame-owned protocol buffers during runtime
        // shutdown.
        unsafe { self.gl.finish() };
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
