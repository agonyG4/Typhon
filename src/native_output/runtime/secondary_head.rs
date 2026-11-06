//! Phase 4b: drive secondary output heads planned at bootstrap.
//!
//! Each planned secondary connector gets its own explicit Atomic EGL/GBM
//! scanout, an initial blocking modeset of its own connector/CRTC/plane, a
//! KMS commit worker, and an `OutputHead` parked beside the primary. The
//! compositor learns the head through `add_logical_output`, so clients see a
//! second `wl_output` and windows can be placed on it.
//!
//! Secondary heads draw no cursor yet (the pointer stays on the primary until
//! the union clamp lands), and a head that fails to come up is withdrawn
//! without affecting the primary. Secondary heads are opt-in with
//! `TYPHON_SECONDARY_HEADS=1` until input, focus, and workspace switching
//! work across outputs: with a second output attached the pointer cannot
//! reach it, yet workspaces and keyboard focus already follow it.

use super::kms_worker_startup::start_kms_commit_worker;
use super::*;
use oblivion_one::native::kms::{AtomicKmsError, AtomicKmsErrorKind};
use std::os::fd::AsFd;

pub(super) fn secondary_heads_enabled() -> bool {
    std::env::var_os("TYPHON_SECONDARY_HEADS").is_some_and(|value| value == "1")
}

impl NativeRuntime {
    pub(super) fn bring_up_secondary_heads(
        &mut self,
        plans: Vec<crate::native_output::output::NativeSecondaryHeadPlan>,
    ) {
        for plan in plans {
            let name = plan.selection.capabilities.connector_name.clone();
            match self.bring_up_secondary_head(plan) {
                Ok(head) => {
                    println!(
                        "native output head active: {} connector {} crtc {} {} at output {:?}",
                        name,
                        head.target.connector_id,
                        head.target.crtc_id,
                        head.mode_label,
                        head.output_id,
                    );
                    self.parked_heads.push(head);
                }
                Err(error) => {
                    eprintln!("native output head {name} unavailable: {error}");
                }
            }
        }
        self.publish_secondary_pointer_regions();
    }

    /// Lets the pointer cross into every parked head's logical rectangle.
    pub(super) fn publish_secondary_pointer_regions(&mut self) {
        let regions = self
            .parked_heads
            .iter()
            .filter_map(|head| self.server.logical_output_rect(head.output_id))
            .filter_map(|(x, y, width, height)| {
                Some((
                    x,
                    y,
                    u32::try_from(width).ok()?,
                    u32::try_from(height).ok()?,
                ))
            })
            .collect::<Vec<_>>();
        println!("native output pointer layout: secondary regions {regions:?}");
        let _ = self.input_state.set_secondary_pointer_regions(regions);
    }

    fn bring_up_secondary_head(
        &mut self,
        plan: crate::native_output::output::NativeSecondaryHeadPlan,
    ) -> NativeResult<super::output_head::OutputHead> {
        let crate::native_output::output::NativeSecondaryHeadPlan {
            selection,
            position,
        } = plan;
        let mut target = selection.target;
        let output_capabilities = selection.capabilities;
        let output_refresh_rate = super::super::output::output_refresh_rate_for_mode(&target.mode);
        let output_id = self
            .server
            .add_logical_output(
                output_capabilities.connector_name.clone(),
                position,
                (target.width, target.height),
                1.0,
                output_refresh_rate.rounded_hz(),
            )
            .ok_or_else(|| io::Error::other("logical output identity allocation exhausted"))?;
        self.server.enter_output_presentation(output_id);
        let result = self.initialize_secondary_head(
            output_id,
            &mut target,
            output_capabilities,
            output_refresh_rate,
        );
        self.server.enter_output_presentation(self.head.output_id);
        if result.is_err() {
            self.server.withdraw_logical_output(output_id);
        }
        result
    }

    fn initialize_secondary_head(
        &mut self,
        output_id: OutputId,
        target: &mut NativeAppliedOutputConfiguration,
        output_capabilities: NativeOutputCapabilities,
        output_refresh_rate: oblivion_one::compositor::OutputRefreshRate,
    ) -> NativeResult<super::output_head::OutputHead> {
        let connector_id = ConnectorId::new(target.connector_id)
            .ok_or_else(|| io::Error::other("secondary connector ID is zero"))?;
        let crtc_id = CrtcId::new(target.crtc_id)
            .ok_or_else(|| io::Error::other("secondary CRTC ID is zero"))?;
        let discovery = KmsBackendSelection::discover_atomic_pipeline(
            self.kms.file().as_fd(),
            connector_id,
            crtc_id,
            u32::from_le_bytes(*b"XR24"),
        )
        .or_else(|error| {
            if error.kind != AtomicKmsErrorKind::NoCompatiblePrimaryPlane {
                return Err(error);
            }
            KmsBackendSelection::discover_atomic_pipeline(
                self.kms.file().as_fd(),
                connector_id,
                crtc_id,
                u32::from_le_bytes(*b"AR24"),
            )
        })?;
        if !discovery.optional.in_fence_fd {
            return Err(io::Error::other(
                "secondary head requires primary-plane IN_FENCE_FD for explicit Atomic EGL/GBM",
            )
            .into());
        }
        let output_epoch = OutputEpochId::initial();
        let output_generation = output_epoch.get();
        let effect_generation = self.server.trusted_effect_registry().current();
        let explicit = AtomicEglGbmScanout::create_unattached_pool(
            self.kms.file(),
            &discovery,
            target.width,
            target.height,
            output_epoch,
            self.drm_session_generation,
            1,
            output_id,
            OutputColorDepth::default(),
            std::sync::Arc::clone(&effect_generation),
            self.cursor_image.clone(),
        )?;
        let mut scanout = NativeScanoutBackend::from_atomic_explicit(explicit);
        target.color_depth = scanout.output_color_depth();
        // A render worker received the current registry generation when the
        // pool was created; only the synchronous renderer is published here.
        if !scanout.render_worker_active() {
            scanout.publish_material_effect_generation(&effect_generation);
            scanout
                .publish_effect_registry_generation((*effect_generation).clone())
                .map_err(io::Error::other)?;
        }
        let mode_label = format!(
            "{}x{}@{}",
            target.width,
            target.height,
            output_refresh_rate.rounded_hz()
        );

        // Initial frame and blocking modeset of this head's own pipeline.
        // Secondary heads have no cursor plane: they draw the cursor while
        // the pointer is on them (the input state gives them a head-local
        // view while they present).
        let cursor_render_mode = NativeCursorRenderMode::Software;
        let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut scanout else {
            unreachable!("secondary heads use explicit Atomic EGL/GBM scanout");
        };
        let slot = explicit.initial_slot();
        let framebuffer = explicit.framebuffer(slot)?;
        KmsBackendSelection::test_atomic_modeset_from_discovery_with_cursor(
            self.kms.file().as_fd(),
            &discovery,
            target.mode,
            target.width,
            target.height,
            framebuffer,
            None,
        )?;
        let initial_damage = NativeOutputDamage::full_output(target.width, target.height);
        let initial_scene = ResolvedNativeFrameScene::from_server(&self.server).into_owned();
        let head_view = self
            .server
            .logical_output_rect(output_id)
            .and_then(|(x, y, width, height)| {
                Some((
                    x,
                    y,
                    u32::try_from(width).ok()?,
                    u32::try_from(height).ok()?,
                ))
            })
            .map(|rect| self.input_state.enter_head_view(rect));
        let frozen_initial = FrozenNativeRenderFrame::freeze(
            target.width,
            target.height,
            initial_scene,
            &self.server,
            &self.input_state,
            cursor_render_mode,
            Some(initial_damage.as_renderer_damage(target.width, target.height)),
        );
        if let Some(saved) = head_view {
            self.input_state.leave_head_view(saved);
        }
        let initial_render_target = explicit.render_target_request(slot)?;
        let (initial_outcome, frozen_initial) =
            explicit.render_initial_frame(initial_render_target, 0, frozen_initial)?;
        let mut parts = match initial_outcome {
            AtomicSlotRenderOutcome::Rendered(parts) => *parts,
            AtomicSlotRenderOutcome::Skipped { slot, reason, .. } => {
                explicit.swapchain_mut()?.cancel_render_before_gpu(slot)?;
                return Err(io::Error::other(format!(
                    "secondary initial modeset frame skipped: {reason:?}"
                ))
                .into());
            }
            AtomicSlotRenderOutcome::LifecycleFallback { fallbacks, .. } => {
                return Err(io::Error::other(format!(
                    "secondary initial modeset lifecycle fallback: {fallbacks:?}"
                ))
                .into());
            }
        };
        let mut initial_presented_scene = NativeFrameSceneSnapshot::from_resolved_frame_scene(
            output_id,
            0,
            &frozen_initial.scene,
            NativeCursorDamageBounds::default(),
        );
        initial_presented_scene.lifecycle =
            oblivion_one::window_lifecycle_animation::LifecycleFrameSnapshot::qualified_from_sample(
                &frozen_initial.scene.lifecycle,
                &parts.lifecycle_evidence,
            );
        drop(frozen_initial);
        let initial_framebuffer = explicit.framebuffer(parts.slot)?;
        let submission_fence = parts.render_fence.take_submission_fd()?;
        let kms_backend =
            KmsBackendSelection::initialize_atomic_from_discovery_with_fence_and_cursor(
                self.kms.file().as_fd(),
                self.head.kms_backend.requested,
                discovery,
                target.mode,
                target.width,
                target.height,
                initial_framebuffer,
                submission_fence,
                None,
            )
            .map_err(|error: AtomicKmsError| io::Error::other(error.to_string()))?;
        explicit.promote_initial_presented(parts.slot, parts.scene_commit)?;
        scanout.finish_initial_scanout();
        println!(
            "native output head modeset: {} {} crtc {}",
            output_capabilities.connector_name, mode_label, target.crtc_id
        );

        let kms_commit_worker_policy = self.head.kms_commit_worker_policy;
        let (kms_commit_worker, kms_commit_worker_transport, kms_commit_worker_startup) =
            start_kms_commit_worker(kms_commit_worker_policy, &kms_backend)?;
        let kms_commit_worker_reactor_token = kms_commit_worker
            .as_ref()
            .map(|worker| {
                self.event_loop
                    .register(worker.event_fd(), NativeEventSource::KmsCommitWorker)
            })
            .transpose()?;
        let render_worker_reactor_token = scanout
            .render_worker_event_fd()
            .map(|fd| {
                self.event_loop
                    .register(fd, NativeEventSource::RenderWorker)
            })
            .transpose()?;

        let refresh_interval_ns = output_refresh_rate.interval_ns();
        let refresh_interval = Duration::from_nanos(refresh_interval_ns);
        let scheduler_anchor_ns = monotonic_now_ns()?;
        let mode_timing = KmsModeTiming::from_mode(&target.mode, refresh_interval_ns);
        let triple_buffer_policy = self.head.triple_buffer_policy;
        let vrr_range = super::vrr_cadence::VrrRange::discover(
            output_refresh_rate.refresh_millihz(),
            output_capabilities.edid_vrr_range,
            super::vrr_cadence::VrrRange::override_min_hz_from_environment(),
        );
        Ok(super::output_head::OutputHead {
            output_id,
            target: target.clone(),
            kms_backend,
            output_capabilities,
            output_mode_mutation_policy: self.head.output_mode_mutation_policy,
            output_configuration_generation: OutputConfigurationGeneration::initial(),
            output_configuration_transactions: OutputConfigurationTransactions::new(),
            pending_output_configuration: None,
            mode_label,
            output_refresh_rate,
            output_epoch,
            scanout: mem::ManuallyDrop::new(scanout),
            cursor_output_arbitration: NativeCursorOutputArbitration::default(),
            cursor_render_mode,
            presented_planes: initial_presented(None),
            atomic_cursor: None,
            legacy_cursor: None,
            output_render_fence_token: None,
            kms_commit_worker,
            kms_commit_worker_reactor_token,
            render_worker_reactor_token,
            render_worker_admission_stopped: false,
            pending_render_worker_identity: None,
            pending_render_worker_context: None,
            pending_render_worker_outer: None,
            pending_render_worker_completion: None,
            pending_render_worker_capture: None,
            pending_dmabuf_release_fence: None,
            output_reconfiguration_rearm_retry_deadline_ns: None,
            kms_commit_worker_policy,
            kms_commit_worker_transport,
            kms_commit_worker_startup,
            worker_quarantine: KmsWorkerQuarantine::default(),
            emergency_quarantined_worker_jobs: Vec::new(),
            submitted_worker_ownership: Vec::new(),
            emergency_quarantined_submitted_ownership: Vec::new(),
            kms_teardown_safety: KmsTeardownSafety::Unproven,
            kms_teardown_safety_established: false,
            scanout_destroyed: false,
            deferred_worker_pageflip: None,
            deferred_worker_completion: None,
            worker_timeout_pending: None,
            cycle_worker_handoff: None,
            forced_shutdown_inflight: None,
            forced_shutdown_pacing_settled: None,
            frame_scheduler: NativeFrameScheduler::new_with_refresh_interval_ns(
                refresh_interval_ns,
                scheduler_anchor_ns,
            ),
            atomic_commit_arbiter: AtomicCommitArbiter::new(),
            output_transactions: OutputTransactionLedger::new_for_output(output_id),
            confirmed_kms_presentation: ConfirmedKmsPresentationState::default(),
            vrr_cadence: super::vrr_cadence::VrrCadenceRecorder::new(vrr_range),
            presentation_timing: KmsPresentationTimingModel::new(mode_timing, output_generation),
            cursor_timing:
                crate::native_output::presentation::cursor_timing::CursorTimingDomain::new(
                    mode_timing,
                    output_generation,
                ),
            presentation_deadline: PresentationDeadlinePlanner::new(refresh_interval),
            scheduled_presentation_target: None,
            render_latch_target: None,
            render_journal: AdaptiveRenderJournal::default(),
            adaptive_buffering: AdaptiveBufferingController::new(triple_buffer_policy),
            triple_buffer_policy,
            pending_proven_deadline_miss: None,
            last_rendered_scene_generation: self.server.scene_render_generation(),
            last_direct_candidate_key: None,
            direct_fallback_tracker: None,
            last_refresh_sequence: 0,
            last_submitted_cursor_epoch: 0,
            last_primary_presented_at_ns: None,
            scene_history: NativeSceneHistory::new(initial_presented_scene),
            last_client_cursor_damage: None,
            last_software_cursor_damage: None,
            last_client_cursor_path: None,
            render_demand_coalesced_count: 0,
            frame_index: 0,
            mismatched_pageflip_events: 0,
            stale_pageflip_events: 0,
            presentation_cadence: PresentationCadenceMetrics::default(),
            frame_pacing: NativeFramePacing::from_env(),
            last_acquire_ready_at_ns: None,
            scaled_primary_probe: ScaledPrimaryProbeCache::default(),
        })
    }
}
