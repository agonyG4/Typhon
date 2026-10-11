//! Reactor-side completion delivery for the Atomic render worker.
//!
//! The worker event source only transports completed renderer-owned values.
//! Output transactions and protocol ownership remain in the normal reactor
//! settlement path.

use super::*;

impl NativeRuntime {
    pub(super) fn expect_render_worker_completion(
        &mut self,
        identity: crate::native_output::scanout::AtomicRenderWorkerIdentity,
    ) -> NativeResult<()> {
        if self.pending_render_worker_identity.is_some()
            || self.pending_render_worker_completion.is_some()
        {
            return Err(io::Error::other(
                "render worker admission attempted while prior completion ownership is active",
            )
            .into());
        }
        self.pending_render_worker_identity = Some(identity);
        Ok(())
    }

    pub(super) fn process_render_worker_events(&mut self) -> NativeResult<()> {
        loop {
            let event = self.scanout.try_take_render_worker_event()?;
            let Some(event) = event else {
                return Ok(());
            };
            match event {
                crate::native_output::scanout::AtomicRenderWorkerEvent::Rendered(completion) => {
                    if self.pending_render_worker_completion.is_some() {
                        return Err(io::Error::other(
                            "render worker produced a second completion before reactor settlement",
                        )
                        .into());
                    }
                    let expected = self.pending_render_worker_identity.take().ok_or_else(|| {
                        io::Error::other(
                            "render worker completion arrived without an executing continuation",
                        )
                    })?;
                    if !crate::native_output::scanout::completion_matches_job(
                        expected,
                        completion.identity(),
                    ) {
                        return Err(io::Error::other(
                            "render worker completion identity does not match the executing continuation",
                        )
                        .into());
                    }
                    let completion_observed_at_ns = monotonic_now_ns()?;
                    let dispatch_to_start_ns = completion
                        .worker_started_at_ns
                        .map(|started| started.saturating_sub(completion.dispatch_at_ns));
                    self.perf.log("native.render_worker.completion", || {
                        vec![
                            NativePerfField::u64("job_id", completion.job_id.get()),
                            NativePerfField::u64("frame_id", completion.frame_id),
                            NativePerfField::u64("transaction_id", completion.transaction_id.get()),
                            NativePerfField::u64("dispatch_at_ns", completion.dispatch_at_ns),
                            NativePerfField::str(
                                "worker_started_at_ns",
                                completion
                                    .worker_started_at_ns
                                    .map_or_else(|| "unavailable".to_owned(), |v| v.to_string()),
                            ),
                            NativePerfField::str(
                                "worker_finished_at_ns",
                                completion
                                    .worker_finished_at_ns
                                    .map_or_else(|| "unavailable".to_owned(), |v| v.to_string()),
                            ),
                            NativePerfField::u64(
                                "reactor_completion_observed_at_ns",
                                completion_observed_at_ns,
                            ),
                            NativePerfField::str(
                                "dispatch_to_start_ns",
                                dispatch_to_start_ns
                                    .map_or_else(|| "unavailable".to_owned(), |v| v.to_string()),
                            ),
                            NativePerfField::u64("worker_wall_ns", completion.worker_wall_ns),
                            NativePerfField::str(
                                "worker_thread_cpu_ns",
                                completion
                                    .worker_thread_cpu_ns
                                    .map_or_else(|| "unavailable".to_owned(), |v| v.to_string()),
                            ),
                            NativePerfField::u64(
                                "egl_context_activation_ns",
                                completion.phase_durations.egl_context_activation_ns,
                            ),
                            NativePerfField::u64(
                                "scene_drawing_ns",
                                completion.phase_durations.scene_drawing_ns,
                            ),
                            NativePerfField::u64(
                                "native_render_fence_export_ns",
                                completion.phase_durations.native_render_fence_export_ns,
                            ),
                            NativePerfField::u64(
                                "render_demand_coalesced_count",
                                self.render_demand_coalesced_count,
                            ),
                        ]
                    });
                    self.pending_render_worker_completion = Some(completion);
                    self.queued_redraw_requested = true;
                    self.event_loop.request_continuation(
                        oblivion_one::native::event_loop::NativeContinuationReason::FrameScheduler,
                    )?;
                }
                crate::native_output::scanout::AtomicRenderWorkerEvent::Fatal {
                    job_id,
                    gpu_execution_uncertain,
                    reason,
                } => {
                    return Err(io::Error::other(format!(
                        "Atomic render worker failed: reason={reason} job_id={:?} gpu_execution_uncertain={gpu_execution_uncertain}; synchronous fallback is forbidden after worker ownership",
                        job_id.map(|id| id.get()),
                    ))
                    .into());
                }
            }
        }
    }
}
