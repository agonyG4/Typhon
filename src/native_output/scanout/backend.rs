use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeScanoutKind {
    NativeEglGbm,
    GbmCpuWritePageFlip,
    DumbFramebuffer,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeScanoutBufferSnapshot {
    pub(crate) backend: NativeScanoutKind,
    pub(crate) capacity: Option<usize>,
    pub(crate) current: Option<usize>,
    pub(crate) pending: Option<usize>,
    pub(crate) ready: Option<usize>,
    pub(crate) free_count: Option<usize>,
    pub(crate) gbm_surface_has_free_buffers: Option<bool>,
}

impl NativeScanoutKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::NativeEglGbm => "native EGL/GLES GBM pageflip",
            Self::GbmCpuWritePageFlip => "GBM CPU-write pageflip",
            Self::DumbFramebuffer => "KMS dumb framebuffer",
            Self::Unavailable => "unavailable",
        }
    }

    pub(crate) const fn metric_name(self) -> &'static str {
        match self {
            Self::NativeEglGbm => "native-egl-gbm",
            Self::GbmCpuWritePageFlip => "gbm-cpu-write-pageflip",
            Self::DumbFramebuffer => "dumb-framebuffer",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativePaintStats {
    pub(crate) backend: NativeScanoutKind,
    pub(crate) scanout_format: Option<u32>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) bytes: usize,
    pub(crate) copy_bytes: usize,
    pub(crate) write_bytes: usize,
    pub(crate) gpu_draw_us: u64,
    pub(crate) egl_swap_us: u64,
    pub(crate) shm_upload_bytes: usize,
    pub(crate) dmabuf_imports: usize,
    pub(crate) dmabuf_reuses: usize,
    pub(crate) dmabuf_import_failures: usize,
    pub(crate) dmabuf_cache_entries: usize,
    pub(crate) dmabuf_cache_peak_entries: usize,
    pub(crate) dmabuf_cache_evictions: usize,
    pub(crate) scene_rebuild: DesktopSceneRebuildKind,
    pub(crate) frame_copy: DesktopFrameCopyKind,
    pub(crate) total_us: u64,
    pub(crate) render_us: u64,
    pub(crate) copy_us: u64,
    pub(crate) write_us: u64,
    pub(crate) gles_repaint: Option<GlesSceneFrameStats>,
    pub(crate) swap_with_damage_used: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativePaintOutcome {
    Skipped(NativePaintStats),
    Rendered(NativePaintStats),
}

impl NativePaintOutcome {
    pub(crate) const fn stats(self) -> NativePaintStats {
        match self {
            Self::Skipped(stats) | Self::Rendered(stats) => stats,
        }
    }

    pub(crate) fn require_rendered(self, context: &str) -> io::Result<NativePaintStats> {
        match self {
            Self::Rendered(stats) => Ok(stats),
            Self::Skipped(_) => Err(io::Error::other(format!(
                "{context} unexpectedly produced no rendered frame"
            ))),
        }
    }
}

impl NativePaintStats {
    pub(crate) fn fields(self) -> Vec<NativePerfField> {
        let mut fields = vec![
            NativePerfField::str("backend", self.backend.metric_name()),
            NativePerfField::str("scanout", self.backend.as_str()),
            NativePerfField::u64("width", u64::from(self.width)),
            NativePerfField::u64("height", u64::from(self.height)),
            NativePerfField::usize("bytes", self.bytes),
            NativePerfField::usize("copy_bytes", self.copy_bytes),
            NativePerfField::usize("full_frame_cpu_copy_bytes", self.copy_bytes),
            NativePerfField::usize("write_bytes", self.write_bytes),
            NativePerfField::u64("gpu_draw_us", self.gpu_draw_us),
            NativePerfField::u64("egl_swap_us", self.egl_swap_us),
            NativePerfField::usize("shm_upload_bytes", self.shm_upload_bytes),
            NativePerfField::usize("dmabuf_imports", self.dmabuf_imports),
            NativePerfField::usize("dmabuf_reuses", self.dmabuf_reuses),
            NativePerfField::usize("dmabuf_import_failures", self.dmabuf_import_failures),
            NativePerfField::usize("dmabuf_cache_entries", self.dmabuf_cache_entries),
            NativePerfField::usize("dmabuf_cache_peak_entries", self.dmabuf_cache_peak_entries),
            NativePerfField::usize("dmabuf_cache_evictions", self.dmabuf_cache_evictions),
            NativePerfField::str("scene_rebuild", self.scene_rebuild.as_str()),
            NativePerfField::str("frame_copy", self.frame_copy.as_str()),
            NativePerfField::u64("paint_us", self.total_us),
            NativePerfField::u64("render_us", self.render_us),
            NativePerfField::u64("copy_us", self.copy_us),
            NativePerfField::u64("write_us", self.write_us),
        ];
        if let Some(scanout_format) = self.scanout_format {
            fields.push(NativePerfField::str(
                "scanout_format",
                native_visual_label(scanout_format),
            ));
        }
        if let Some(repaint) = self.gles_repaint {
            let output_pixels = u64::from(self.width).saturating_mul(u64::from(self.height));
            let rendered = repaint.repaint_mode != crate::egl_renderer::RepaintMode::Skip;
            fields.extend([
                NativePerfField::str("frame_decision", repaint.repaint_mode.as_str()),
                NativePerfField::str(
                    "logical_damage",
                    if repaint.current_damage_pixels == 0 {
                        "empty"
                    } else if repaint.current_damage_pixels >= output_pixels {
                        "full"
                    } else {
                        "rects"
                    },
                ),
                NativePerfField::str("repaint_mode", repaint.repaint_mode.as_str()),
                NativePerfField::usize("current_damage_rects", repaint.current_damage_rects),
                NativePerfField::u64("current_damage_pixels", repaint.current_damage_pixels),
                NativePerfField::usize("repair_damage_rects", repaint.repair_damage_rects),
                NativePerfField::u64("repair_damage_pixels", repaint.repair_damage_pixels),
                NativePerfField::usize("scissor_passes", repaint.scissor_passes),
                NativePerfField::usize("draw_command_replays", repaint.draw_command_replays),
                NativePerfField::usize("damage_history_depth", repaint.history_depth),
                NativePerfField::u64(
                    "output_pixels_avoided",
                    output_pixels.saturating_sub(repaint.repair_damage_pixels),
                ),
                NativePerfField::bool("swap_with_damage", self.swap_with_damage_used),
                NativePerfField::bool("partial_repaint_enabled", repaint.partial_repaint_enabled),
                NativePerfField::bool(
                    "contradictory_empty_damage",
                    repaint.contradictory_empty_damage,
                ),
                NativePerfField::bool("scene_snapshot_committed", rendered),
                NativePerfField::bool("egl_swap_attempted", rendered),
                NativePerfField::bool("egl_swap_succeeded", rendered),
                NativePerfField::bool("gbm_front_buffer_locked", rendered),
                NativePerfField::bool("ready_frame_created", rendered),
            ]);
            if let Some(age) = repaint.buffer_age {
                fields.push(NativePerfField::u64("egl_buffer_age", u64::from(age)));
            }
            if let Some(reason) = repaint.fallback_reason {
                fields.push(NativePerfField::str("full_repaint_reason", reason.as_str()));
            }
        }
        fields
    }
}
