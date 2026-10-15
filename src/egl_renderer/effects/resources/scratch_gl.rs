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
}
