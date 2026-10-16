use std::time::Instant;

use glow::HasContext;

use super::super::metrics::EffectResourceMetrics;

impl super::EffectGlResourceCache {
    pub(crate) fn begin_scratch_activity_frame(&mut self) {
        self.pool.begin_scratch_activity_frame();
    }

    pub(crate) fn metrics(&self) -> EffectResourceMetrics {
        let metrics = self.pool.metrics();
        if let Some(publisher) = &self.scratch_residency_publisher {
            publisher.publish(
                metrics.generation,
                metrics.active_scratch_count,
                metrics.active_scratch_bytes,
                metrics.reusable_scratch_count,
                metrics.reusable_scratch_bytes,
                metrics.scratch_residency,
            );
        }
        metrics
    }

    pub(crate) fn effect_scratch_residency_snapshot(
        &self,
    ) -> crate::gpu_residency::EffectScratchResidencyLiveSnapshot {
        self.metrics().scratch_residency_live_snapshot()
    }

    pub(crate) fn set_scratch_residency_publisher(
        &mut self,
        publisher: Option<
            std::sync::Arc<crate::gpu_residency::AtomicEffectScratchResidencySnapshot>,
        >,
    ) {
        self.scratch_residency_publisher = publisher;
        let _ = self.metrics();
    }

    pub(crate) fn next_scratch_maintenance_deadline(&self) -> Option<Instant> {
        self.pool.next_scratch_maintenance_deadline()
    }

    pub(crate) fn maintain_scratch(&mut self, gl: &glow::Context, now: Instant) -> bool {
        let Some(kind) = self.pool.scratch_residency.due_kind(now) else {
            return false;
        };
        let batch = self.pool.reclaim_scratch_batch(kind, now);
        if !batch.retired.is_empty() {
            unsafe {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                if let Some(framebuffer) = self.scratch_fbo.take() {
                    self.delete_effect_framebuffer(gl, framebuffer);
                }
                if let Some(framebuffer) = self.lifecycle_composition_fbo.take() {
                    self.delete_effect_framebuffer(gl, framebuffer);
                }
            }
            for retired in &batch.retired {
                self.delete_effect_texture(gl, retired.id);
            }
        }
        let _ = self.metrics();
        batch.more_eligible
    }
}

#[cfg(test)]
mod tests {
    use glow::HasContext;

    use super::super::{
        EffectGlResourceCache, EffectTextureFilter, EffectTextureFormat, EffectTextureKey,
        EffectWorkingSpace,
    };

    #[test]
    fn warm_and_cold_reclaim_delete_gl_textures_and_recreate_effect_fbos() {
        let mut harness = crate::egl_renderer::tests::GlesEffectTestHarness::new(16, 16);
        let mut cache = EffectGlResourceCache::new();
        let make_key = |size| {
            EffectTextureKey::new(
                size,
                size,
                EffectTextureFormat::Rgba8,
                EffectTextureFilter::Nearest,
                EffectWorkingSpace::OutputEncodedSrgb,
            )
        };
        let stale = cache
            .acquire(&harness.gl, make_key(8))
            .expect("stale scratch texture realizes");
        let stale_gl = cache.gl_textures[&stale.id];
        cache
            .bind_render_target(&harness.gl, &stale)
            .expect("scratch FBO attaches the texture");
        cache
            .bind_lifecycle_composition_target(&harness.gl, &stale)
            .expect("lifecycle FBO attaches the texture");
        cache.unbind_render_target(&harness.gl);
        cache.release(stale.clone()).expect("stale scratch returns");
        let warm_deadline = cache
            .next_scratch_maintenance_deadline()
            .expect("scratch use arms maintenance");

        for size in [9, 10] {
            cache.begin_scratch_activity_frame();
            let recent = cache
                .acquire(&harness.gl, make_key(size))
                .expect("recent scratch texture realizes");
            cache.release(recent).expect("recent scratch returns");
        }

        assert!(!cache.maintain_scratch(
            &harness.gl,
            warm_deadline + std::time::Duration::from_millis(1),
        ));
        assert!(!cache.gl_textures.contains_key(&stale.id));
        assert!(!unsafe { harness.gl.is_texture(stale_gl) });
        assert!(cache.scratch_fbo.is_none());
        assert!(cache.lifecycle_composition_fbo.is_none());

        let recent = cache
            .acquire(&harness.gl, make_key(10))
            .expect("warm-protected scratch texture reuses");
        cache
            .bind_render_target(&harness.gl, &recent)
            .expect("scratch FBO lazily recreates");
        cache
            .bind_lifecycle_composition_target(&harness.gl, &recent)
            .expect("lifecycle-composition FBO lazily recreates");
        cache.unbind_render_target(&harness.gl);
        cache
            .bind_render_target(&harness.gl, &recent)
            .expect("scratch target reattaches after lifecycle FBO creation");
        unsafe {
            harness.gl.disable(glow::SCISSOR_TEST);
            harness.gl.disable(glow::BLEND);
            harness.gl.color_mask(true, true, true, true);
            harness.gl.clear_color(0.25, 0.5, 0.75, 1.0);
            harness.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        cache.unbind_render_target(&harness.gl);
        cache
            .bind_read_target(&harness.gl, &recent)
            .expect("read FBO lazily attaches current texture");
        let mut pixel = [0_u8; 4];
        unsafe {
            harness.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            harness.gl.read_pixels(
                0,
                0,
                1,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixel)),
            );
        }
        cache.unbind_render_target(&harness.gl);
        assert!((i16::from(pixel[0]) - 64).abs() <= 1);
        assert!((i16::from(pixel[1]) - 128).abs() <= 1);
        assert!((i16::from(pixel[2]) - 191).abs() <= 1);
        assert_eq!(pixel[3], 255);
        cache.release(recent).expect("warm scratch returns");

        let cached_gl_ids = cache.gl_textures.keys().copied().collect::<Vec<_>>();
        let cached_gl_textures = cached_gl_ids
            .iter()
            .map(|id| cache.gl_textures[id])
            .collect::<Vec<_>>();
        let cold_deadline = cache
            .next_scratch_maintenance_deadline()
            .expect("active scratch still has a cold-idle deadline");
        assert!(!cache.maintain_scratch(
            &harness.gl,
            cold_deadline + std::time::Duration::from_millis(1),
        ));
        assert_eq!(cache.pool.metrics().reusable_scratch_count, 0);
        for id in cached_gl_ids {
            assert!(!cache.gl_textures.contains_key(&id));
        }
        for texture in cached_gl_textures {
            assert!(!unsafe { harness.gl.is_texture(texture) });
        }
        let cold_snapshot = cache.effect_scratch_residency_snapshot();
        assert_eq!(cold_snapshot.reusable_count, 0);
        assert_eq!(cold_snapshot.reusable_bytes, 0);
        assert_eq!(cold_snapshot.policy.pool_current_estimated_bytes, 0);
        assert_eq!(cache.next_scratch_maintenance_deadline(), None);

        let cold_restart = cache
            .acquire(&harness.gl, make_key(10))
            .expect("cold restart texture allocates");
        let cold_gl = cache.gl_textures[&cold_restart.id];
        cache
            .bind_render_target(&harness.gl, &cold_restart)
            .expect("post-cold FBO lazily recreates");
        unsafe {
            harness.gl.disable(glow::SCISSOR_TEST);
            harness.gl.disable(glow::BLEND);
            harness.gl.color_mask(true, true, true, true);
            harness.gl.clear_color(0.25, 0.5, 0.75, 1.0);
            harness.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        cache.unbind_render_target(&harness.gl);
        cache
            .bind_read_target(&harness.gl, &cold_restart)
            .expect("post-cold read FBO reattaches current texture");
        let mut cold_pixel = [0_u8; 4];
        unsafe {
            harness.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            harness.gl.read_pixels(
                0,
                0,
                1,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut cold_pixel)),
            );
        }
        cache.unbind_render_target(&harness.gl);
        assert!((i16::from(cold_pixel[0]) - 64).abs() <= 1);
        assert!((i16::from(cold_pixel[1]) - 128).abs() <= 1);
        assert!((i16::from(cold_pixel[2]) - 191).abs() <= 1);
        assert_eq!(cold_pixel[3], 255);
        assert_eq!(cold_pixel, pixel);
        assert!(unsafe { harness.gl.is_texture(cold_gl) });
        cache
            .release(cold_restart.clone())
            .expect("cold scratch returns");
        let immediately_reused = cache
            .acquire(&harness.gl, make_key(10))
            .expect("second cold activation reuses");
        assert_eq!(immediately_reused.id, cold_restart.id);
        cache
            .release(immediately_reused)
            .expect("reused cold scratch returns");
        cache.destroy(&harness.gl);
    }

    #[test]
    fn partial_cold_maintenance_rearms_once_after_a_bounded_batch() {
        let harness = crate::egl_renderer::tests::GlesEffectTestHarness::new(16, 16);
        let mut cache = EffectGlResourceCache::new();
        let make_key = |size| {
            EffectTextureKey::new(
                size,
                size,
                EffectTextureFormat::Rgba8,
                EffectTextureFilter::Nearest,
                EffectWorkingSpace::OutputEncodedSrgb,
            )
        };
        for size in 4..22 {
            let texture = cache
                .acquire(&harness.gl, make_key(size))
                .expect("scratch texture realizes");
            cache.release(texture).expect("scratch texture returns");
        }

        let warm_deadline = cache
            .next_scratch_maintenance_deadline()
            .expect("scratch activity arms maintenance");
        assert!(!cache.maintain_scratch(
            &harness.gl,
            warm_deadline + std::time::Duration::from_millis(1),
        ));
        let cold_deadline = cache
            .next_scratch_maintenance_deadline()
            .expect("cold-idle deadline remains after warm convergence");
        let first_batch_at = cold_deadline + std::time::Duration::from_millis(1);

        assert!(cache.maintain_scratch(&harness.gl, first_batch_at));
        assert_eq!(cache.pool.metrics().reusable_scratch_count, 2);
        let continuation_deadline = cache
            .next_scratch_maintenance_deadline()
            .expect("partial cold trim rearms one continuation");
        assert_eq!(
            continuation_deadline,
            first_batch_at + std::time::Duration::from_millis(5)
        );

        assert!(!cache.maintain_scratch(&harness.gl, continuation_deadline));
        assert_eq!(cache.pool.metrics().reusable_scratch_count, 0);
        assert_eq!(cache.next_scratch_maintenance_deadline(), None);
        cache.destroy(&harness.gl);
    }

    #[test]
    fn cold_maintenance_keeps_active_gl_scratch_until_it_is_returned() {
        let harness = crate::egl_renderer::tests::GlesEffectTestHarness::new(16, 16);
        let mut cache = EffectGlResourceCache::new();
        let key = EffectTextureKey::new(
            8,
            8,
            EffectTextureFormat::Rgba8,
            EffectTextureFilter::Nearest,
            EffectWorkingSpace::OutputEncodedSrgb,
        );
        let active = cache
            .acquire(&harness.gl, key)
            .expect("active scratch realizes");
        let texture = cache.gl_textures[&active.id];
        let warm_deadline = cache
            .next_scratch_maintenance_deadline()
            .expect("active scratch schedules warm maintenance");
        assert!(!cache.maintain_scratch(
            &harness.gl,
            warm_deadline + std::time::Duration::from_millis(1),
        ));
        let cold_deadline = cache
            .next_scratch_maintenance_deadline()
            .expect("active scratch still schedules cold convergence");
        let cold_now = cold_deadline + std::time::Duration::from_millis(1);

        assert!(!cache.maintain_scratch(&harness.gl, cold_now));
        assert!(cache.pool.is_checked_out(active.id));
        assert!(unsafe { harness.gl.is_texture(texture) });
        assert_eq!(cache.next_scratch_maintenance_deadline(), None);

        cache.release(active).expect("active scratch returns");
        let returned_deadline = cache
            .next_scratch_maintenance_deadline()
            .expect("returning active scratch re-arms cold cleanup");
        assert!(returned_deadline <= cold_now);
        assert!(!cache.maintain_scratch(&harness.gl, cold_now));
        assert!(!unsafe { harness.gl.is_texture(texture) });
        assert_eq!(cache.pool.metrics().reusable_scratch_count, 0);
        assert_eq!(cache.next_scratch_maintenance_deadline(), None);
        cache.destroy(&harness.gl);
    }

    #[test]
    fn synchronous_scene_renderer_warm_and_cold_maintenance_preserves_then_converges() {
        let mut harness = crate::egl_renderer::tests::GlesEffectTestHarness::new(16, 16);
        let make_key = |size| {
            EffectTextureKey::new(
                size,
                size,
                EffectTextureFormat::Rgba8,
                EffectTextureFilter::Nearest,
                EffectWorkingSpace::OutputEncodedSrgb,
            )
        };
        let stale = harness
            .renderer
            .effect_runtime
            .effect_resources
            .acquire(&harness.gl, make_key(8))
            .expect("stale renderer scratch allocates");
        let stale_id = stale.id;
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .release(stale)
            .expect("stale renderer scratch returns");
        let warm_deadline = harness
            .renderer
            .next_scratch_maintenance_deadline()
            .expect("synchronous renderer exposes its scratch deadline");

        harness
            .renderer
            .effect_runtime
            .effect_resources
            .begin_scratch_activity_frame();
        let previous = harness
            .renderer
            .effect_runtime
            .effect_resources
            .acquire(&harness.gl, make_key(9))
            .expect("previous epoch scratch allocates");
        let previous_id = previous.id;
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .release(previous)
            .expect("previous epoch scratch returns");
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .begin_scratch_activity_frame();
        let latest = harness
            .renderer
            .effect_runtime
            .effect_resources
            .acquire(&harness.gl, make_key(10))
            .expect("latest epoch scratch allocates");
        let latest_id = latest.id;
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .release(latest)
            .expect("latest epoch scratch returns");

        assert!(
            !harness
                .renderer
                .maintain_effect_scratch(warm_deadline + std::time::Duration::from_millis(1))
        );
        let warm_snapshot = harness.renderer.effect_scratch_residency_snapshot();
        assert_eq!(warm_snapshot.active_count, 0);
        assert_eq!(warm_snapshot.reusable_count, 2);
        assert_eq!(warm_snapshot.policy.warm_reclaimed_texture_count, 1);
        assert!(
            !harness
                .renderer
                .effect_runtime
                .effect_resources
                .gl_textures
                .contains_key(&stale_id)
        );
        assert!(
            harness
                .renderer
                .effect_runtime
                .effect_resources
                .gl_textures
                .contains_key(&previous_id)
        );
        assert!(
            harness
                .renderer
                .effect_runtime
                .effect_resources
                .gl_textures
                .contains_key(&latest_id)
        );

        let active = harness
            .renderer
            .effect_runtime
            .effect_resources
            .acquire(&harness.gl, make_key(10))
            .expect("protected latest scratch reuses");
        assert_eq!(active.id, latest_id);
        let cold_deadline = harness
            .renderer
            .next_scratch_maintenance_deadline()
            .expect("renderer keeps its cold deadline");
        assert!(
            !harness
                .renderer
                .maintain_effect_scratch(cold_deadline + std::time::Duration::from_millis(1))
        );
        let active_snapshot = harness.renderer.effect_scratch_residency_snapshot();
        assert_eq!(active_snapshot.active_count, 1);
        assert_eq!(active_snapshot.reusable_count, 0);
        assert!(
            harness
                .renderer
                .effect_runtime
                .effect_resources
                .gl_textures
                .contains_key(&latest_id)
        );
        assert_eq!(harness.renderer.next_scratch_maintenance_deadline(), None);

        harness
            .renderer
            .effect_runtime
            .effect_resources
            .release(active)
            .expect("active scratch returns");
        let returned_deadline = harness
            .renderer
            .next_scratch_maintenance_deadline()
            .expect("renderer re-arms cleanup after the active lease returns");
        assert!(!harness.renderer.maintain_effect_scratch(returned_deadline));
        let cold_snapshot = harness.renderer.effect_scratch_residency_snapshot();
        assert_eq!(cold_snapshot.active_count, 0);
        assert_eq!(cold_snapshot.reusable_count, 0);
        assert_eq!(cold_snapshot.reusable_bytes, 0);
        assert_eq!(cold_snapshot.policy.pool_current_estimated_bytes, 0);
        assert_eq!(cold_snapshot.policy.cold_reclaim_events, 2);
        assert_eq!(cold_snapshot.policy.cold_reclaimed_texture_count, 2);
        assert_eq!(harness.renderer.next_scratch_maintenance_deadline(), None);
    }
}
