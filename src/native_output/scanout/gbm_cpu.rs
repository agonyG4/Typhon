use super::*;

pub(crate) struct NativeGbmScanout {
    pub(crate) _device: gbm::Device<OwnedFd>,
    pub(crate) fd: RawFd,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) buffers: Vec<NativeGbmScanoutBuffer>,
    pub(crate) current_index: usize,
    pub(crate) ready_index: Option<usize>,
    pub(crate) pending_index: Option<usize>,
    pub(crate) page_flip: AtomicCommitState,
    pub(crate) backend_generation: u64,
    pub(crate) drm_cleanup_armed: bool,
    pub(crate) staging: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeIndexedScanoutRecovery {
    pub(crate) index: usize,
    pub(crate) framebuffer_id: u32,
}

pub(crate) fn prepare_indexed_session_recovery(
    framebuffer_ids: &[u32],
    current_index: usize,
    ready_index: Option<usize>,
) -> io::Result<NativeIndexedScanoutRecovery> {
    let index = ready_index.unwrap_or(current_index);
    let framebuffer_id = *framebuffer_ids
        .get(index)
        .ok_or_else(|| io::Error::other("indexed recovery buffer index is out of range"))?;
    if framebuffer_id == 0 {
        return Err(io::Error::other("indexed recovery framebuffer ID is zero"));
    }
    Ok(NativeIndexedScanoutRecovery {
        index,
        framebuffer_id,
    })
}

pub(crate) fn complete_indexed_session_recovery(
    framebuffer_ids: &[u32],
    current_index: &mut usize,
    ready_index: &mut Option<usize>,
    pending_index: &mut Option<usize>,
    recovery: NativeIndexedScanoutRecovery,
) -> io::Result<()> {
    let framebuffer_id = *framebuffer_ids
        .get(recovery.index)
        .ok_or_else(|| io::Error::other("indexed recovery buffer index is out of range"))?;
    if framebuffer_id != recovery.framebuffer_id {
        return Err(io::Error::other(
            "indexed recovery buffer changed before completion",
        ));
    }
    match *ready_index {
        Some(index) if index == recovery.index => {
            *ready_index = None;
            *current_index = recovery.index;
        }
        None if *current_index == recovery.index => {}
        _ => {
            return Err(io::Error::other(
                "indexed recovery slot changed before completion",
            ));
        }
    }
    *pending_index = None;
    Ok(())
}

pub(crate) struct NativeGbmScanoutBuffer {
    pub(crate) bo: gbm::BufferObject<()>,
    pub(crate) fb_id: u32,
    pub(crate) pitch: u32,
}

impl NativeGbmScanout {
    pub(crate) fn create(
        kms: &fs::File,
        width: u32,
        height: u32,
        backend_generation: u64,
    ) -> io::Result<Self> {
        let gbm_fd = duplicate_fd_cloexec(kms.as_raw_fd()).map_err(io::Error::from_raw_os_error)?;
        let device = gbm::Device::new(gbm_fd)?;
        let usage = gbm::BufferObjectFlags::SCANOUT
            | gbm::BufferObjectFlags::WRITE
            | gbm::BufferObjectFlags::LINEAR;
        if !device.is_format_supported(gbm::Format::Xrgb8888, usage) {
            return Err(io::Error::other(
                "GBM device does not support writable XRGB8888 scanout buffers",
            ));
        }

        let mut buffers = Vec::new();
        for _ in 0..3 {
            let bo = device.create_buffer_object(width, height, gbm::Format::Xrgb8888, usage)?;
            let fb_id = add_gbm_framebuffer(kms.as_fd(), &bo)?;
            let pitch = bo.stride();
            buffers.push(NativeGbmScanoutBuffer { bo, fb_id, pitch });
        }
        println!(
            "native scanout: GBM write/pageflip buffers ready: {}x{}, {} buffer(s), backend {}",
            width,
            height,
            buffers.len(),
            device.backend_name()
        );
        Ok(Self {
            _device: device,
            fd: kms.as_raw_fd(),
            width,
            height,
            buffers,
            current_index: 0,
            ready_index: None,
            pending_index: None,
            page_flip: AtomicCommitState::default(),
            backend_generation,
            drm_cleanup_armed: true,
            staging: Vec::new(),
        })
    }

    pub(crate) fn paint_server_frame(
        &mut self,
        renderer: &mut NativeFrameRenderer,
        server: &OwnCompositorServer,
        input_state: &NativeInputState,
        cursor_mode: NativeCursorRenderMode,
        damage: &NativeOutputDamage,
    ) -> io::Result<NativePaintStats> {
        let total_start = Instant::now();
        let index = self.next_render_index()?;
        let render_start = Instant::now();
        let rendered =
            renderer.render_server_frame(self.width, self.height, server, input_state, cursor_mode);
        let render_us = elapsed_micros(render_start);
        let buffer = &mut self.buffers[index];
        let byte_len = buffer
            .pitch
            .checked_mul(self.height)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| io::Error::other("GBM scanout buffer size overflow"))?;
        let copy_start = Instant::now();
        self.staging.resize(byte_len, 0);
        let copy_bytes = copy_argb_frame_to_xrgb_mapping_damage(
            rendered.pixels,
            self.width,
            self.height,
            buffer.pitch,
            &mut self.staging,
            damage.frame_copy_damage_for_scene(rendered.scene_rebuild_kind),
        )?;
        let copy_us = elapsed_micros(copy_start);
        let write_start = Instant::now();
        buffer.bo.write(&self.staging)?;
        let write_us = elapsed_micros(write_start);
        self.ready_index = Some(index);
        Ok(NativePaintStats {
            backend: NativeScanoutKind::GbmCpuWritePageFlip,
            scanout_format: None,
            width: self.width,
            height: self.height,
            bytes: byte_len,
            copy_bytes,
            write_bytes: byte_len,
            gpu_draw_us: 0,
            egl_swap_us: 0,
            shm_upload_bytes: 0,
            dmabuf_imports: 0,
            dmabuf_reuses: 0,
            dmabuf_import_failures: 0,
            dmabuf_cache_entries: 0,
            dmabuf_cache_peak_entries: 0,
            dmabuf_cache_evictions: 0,
            scene_rebuild: rendered.scene_rebuild_kind,
            frame_copy: rendered.frame_copy_kind,
            total_us: elapsed_micros(total_start),
            render_us,
            copy_us,
            write_us,
            gles_repaint: None,
            swap_with_damage_used: false,
        })
    }

    pub(crate) fn fb_id(&self) -> u32 {
        self.ready_index
            .map(|index| self.buffers[index].fb_id)
            .unwrap_or_else(|| self.buffers[self.current_index].fb_id)
    }

    pub(crate) fn prepare_session_recovery(&self) -> io::Result<NativeIndexedScanoutRecovery> {
        let framebuffer_ids = self
            .buffers
            .iter()
            .map(|buffer| buffer.fb_id)
            .collect::<Vec<_>>();
        prepare_indexed_session_recovery(&framebuffer_ids, self.current_index, self.ready_index)
    }

    pub(crate) fn finish_initial_scanout(&mut self) {
        if let Some(index) = self.ready_index.take() {
            self.current_index = index;
        }
    }

    pub(crate) fn present(&mut self, kms: &KmsBackendSelection) -> io::Result<Option<u64>> {
        if self.page_flip.is_pending() {
            return Ok(None);
        }
        let Some(index) = self.ready_index.take() else {
            return Ok(None);
        };
        if index == self.current_index {
            return Ok(None);
        }
        let framebuffer = FramebufferId::new(self.buffers[index].fb_id)
            .ok_or_else(|| io::Error::other("ready CPU GBM framebuffer ID is zero"))?;
        let token = PageFlipToken::new(allocate_native_page_flip_token())
            .expect("native pageflip allocator never returns zero");
        self.page_flip
            .begin(token, framebuffer, self.backend_generation, Instant::now())
            .map_err(io::Error::other)?;
        match kms.submit_flip(framebuffer, token) {
            Ok(()) => {
                self.pending_index = Some(index);
                Ok(Some(token.get()))
            }
            Err(error) => {
                self.page_flip.submission_failed(token);
                self.ready_index = Some(index);
                Err(io::Error::other(error))
            }
        }
    }

    pub(crate) fn drain_page_flip_events(&mut self, fd: RawFd) -> io::Result<NativePageFlipDrain> {
        let mut drain = NativePageFlipDrain::default();
        for event in drain_drm_page_flip_events(fd)? {
            let expected = self.page_flip.pending_token().map(PageFlipToken::get);
            let Some(token) = PageFlipToken::new(event.user_data) else {
                drain.stale_events = drain.stale_events.saturating_add(1);
                drain.last_stale_token = Some(event.user_data);
                continue;
            };
            match self.page_flip.complete(token, self.backend_generation) {
                AtomicCompletion::Completed { .. } => {
                    if drain.completion.is_none() {
                        if let Some(index) = self.pending_index.take() {
                            self.current_index = index;
                        }
                        drain.completion = Some(event);
                    } else {
                        drain.stale_events = drain.stale_events.saturating_add(1);
                    }
                }
                AtomicCompletion::Mismatched => {
                    drain.mismatched_events = drain.mismatched_events.saturating_add(1);
                    drain.last_mismatch = expected.map(|expected| (expected, event.user_data));
                }
                AtomicCompletion::Stale | AtomicCompletion::StaleGeneration => {
                    drain.stale_events = drain.stale_events.saturating_add(1);
                    drain.last_stale_token = Some(event.user_data);
                }
            }
        }
        Ok(drain)
    }

    pub(crate) fn page_flip_pending(&self) -> bool {
        self.page_flip.is_pending()
    }

    pub(crate) fn suspend_page_flip(&mut self) {
        self.page_flip.abandon();
        // pending_index remains quarantined and is excluded from rendering until
        // a synchronous recovery modeset retires the old scanout generation.
    }

    pub(crate) fn complete_session_recovery(
        &mut self,
        recovery: NativeIndexedScanoutRecovery,
    ) -> io::Result<()> {
        let framebuffer_ids = self
            .buffers
            .iter()
            .map(|buffer| buffer.fb_id)
            .collect::<Vec<_>>();
        complete_indexed_session_recovery(
            &framebuffer_ids,
            &mut self.current_index,
            &mut self.ready_index,
            &mut self.pending_index,
            recovery,
        )
    }

    pub(crate) fn rebind_session_generation(&mut self, generation: u64) {
        self.backend_generation = generation;
    }

    pub(crate) fn disarm_drm_cleanup(&mut self) {
        self.drm_cleanup_armed = false;
    }

    pub(crate) fn ready_frame_queued(&self) -> bool {
        self.ready_index.is_some()
    }

    pub(crate) fn render_target_available(&self) -> bool {
        self.next_render_index().is_ok()
    }

    pub(crate) fn next_render_index(&self) -> io::Result<usize> {
        if let Some(index) = self.ready_index {
            return Ok(index);
        }
        next_free_scanout_index(
            self.buffers.len(),
            self.current_index,
            self.ready_index,
            self.pending_index,
        )
        .ok_or_else(|| io::Error::other("no free GBM scanout buffer is available"))
    }
}

pub(crate) fn next_free_scanout_index(
    buffer_count: usize,
    current: usize,
    ready: Option<usize>,
    pending: Option<usize>,
) -> Option<usize> {
    (0..buffer_count)
        .find(|index| Some(*index) != pending && Some(*index) != ready && *index != current)
}

impl Drop for NativeGbmScanout {
    fn drop(&mut self) {
        if !self.drm_cleanup_armed {
            return;
        }
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        for buffer in &self.buffers {
            let _ = drm_ffi::mode::rm_fb(fd, buffer.fb_id);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct NativeGbmFramebufferMetadata {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: u32,
    pub(crate) handles: [u32; 4],
    pub(crate) pitches: [u32; 4],
    pub(crate) offsets: [u32; 4],
    pub(crate) modifiers: [u64; 4],
    pub(crate) flags: u32,
}

impl NativeGbmFramebufferMetadata {
    pub(crate) fn from_bo(bo: &gbm::BufferObject<()>) -> Self {
        let mut handles = [0; 4];
        let mut pitches = [0; 4];
        let mut offsets = [0; 4];
        let mut modifiers = [0; 4];
        let plane_count = bo.plane_count().clamp(1, 4);
        let modifier = u64::from(bo.modifier());
        for plane in 0..plane_count {
            let index = plane as usize;
            handles[index] = unsafe { bo.handle_for_plane(plane as i32).u32_ };
            if handles[index] == 0 {
                handles[index] = unsafe { bo.handle().u32_ };
            }
            pitches[index] = bo.stride_for_plane(plane as i32);
            if pitches[index] == 0 {
                pitches[index] = bo.stride();
            }
            offsets[index] = bo.offset(plane as i32);
            modifiers[index] = modifier;
        }
        let flags = if bo.modifier() == gbm::Modifier::Invalid {
            0
        } else {
            drm_sys::DRM_MODE_FB_MODIFIERS
        };
        Self {
            width: bo.width(),
            height: bo.height(),
            format: bo.format() as u32,
            handles,
            pitches,
            offsets,
            modifiers,
            flags,
        }
    }
}

pub(crate) fn add_gbm_framebuffer(
    fd: BorrowedFd<'_>,
    bo: &gbm::BufferObject<()>,
) -> io::Result<u32> {
    let metadata = NativeGbmFramebufferMetadata::from_bo(bo);
    drm_ffi::mode::add_fb2(
        fd,
        metadata.width,
        metadata.height,
        metadata.format,
        &metadata.handles,
        &metadata.pitches,
        &metadata.offsets,
        &metadata.modifiers,
        metadata.flags,
    )
    .map(|framebuffer| framebuffer.fb_id)
}

pub(crate) fn set_fd_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
