use super::*;

pub(crate) const NATIVE_HARDWARE_CURSOR_SIZE: u32 = 64;

#[derive(Debug)]
struct AtomicCursorBuffer {
    fd: RawFd,
    handle: u32,
    framebuffer: FramebufferId,
    width: u32,
    height: u32,
    pitch: u32,
    size: usize,
    mapping: *mut c_void,
    drm_cleanup_armed: bool,
}

impl AtomicCursorBuffer {
    fn create(file: &fs::File, width: u32, height: u32) -> io::Result<Self> {
        let dumb = drm_ffi::mode::dumbbuffer::create(file.as_fd(), width, height, 32, 0)?;
        let descriptor = ExplicitFramebufferDescriptor::new(
            width,
            height,
            DRM_FORMAT_ARGB8888,
            &[ExplicitFramebufferPlane {
                handle: dumb.handle,
                pitch: dumb.pitch,
                offset: 0,
                modifier: 0,
            }],
        )?;
        let framebuffer = match add_explicit_framebuffer(file.as_fd(), &descriptor) {
            Ok(framebuffer) => framebuffer,
            Err(error) => {
                let _ = drm_ffi::mode::dumbbuffer::destroy(file.as_fd(), dumb.handle);
                return Err(error);
            }
        };
        let map = match drm_ffi::mode::dumbbuffer::map(file.as_fd(), dumb.handle, 0, 0) {
            Ok(map) => map,
            Err(error) => {
                let _ = drm_ffi::mode::rm_fb(file.as_fd(), framebuffer.get());
                let _ = drm_ffi::mode::dumbbuffer::destroy(file.as_fd(), dumb.handle);
                return Err(error);
            }
        };
        let size = match usize::try_from(dumb.size) {
            Ok(size) => size,
            Err(_) => {
                let _ = drm_ffi::mode::rm_fb(file.as_fd(), framebuffer.get());
                let _ = drm_ffi::mode::dumbbuffer::destroy(file.as_fd(), dumb.handle);
                return Err(io::Error::other("Atomic cursor dumb buffer size overflow"));
            }
        };
        let mapping = unsafe {
            libc::mmap(
                ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                map.offset as libc::off_t,
            )
        };
        if mapping == libc::MAP_FAILED {
            let error = io::Error::last_os_error();
            let _ = drm_ffi::mode::rm_fb(file.as_fd(), framebuffer.get());
            let _ = drm_ffi::mode::dumbbuffer::destroy(file.as_fd(), dumb.handle);
            return Err(error);
        }
        Ok(Self {
            fd: file.as_raw_fd(),
            handle: dumb.handle,
            framebuffer,
            width,
            height,
            pitch: dumb.pitch,
            size,
            mapping,
            drm_cleanup_armed: true,
        })
    }

    fn upload_default_cursor(&mut self) -> io::Result<()> {
        let (source_width, source_height) = cursor_texture_size();
        let bytes = native_cursor_argb_bytes(
            &cursor_texture_pixels(),
            source_width,
            source_height,
            self.width,
            self.height,
            self.pitch,
        )?;
        let destination =
            unsafe { slice::from_raw_parts_mut(self.mapping.cast::<u8>(), self.size) };
        destination.copy_from_slice(&bytes);
        Ok(())
    }

    fn disarm_drm_cleanup(&mut self) {
        self.drm_cleanup_armed = false;
    }
}

impl Drop for AtomicCursorBuffer {
    fn drop(&mut self) {
        if self.drm_cleanup_armed {
            let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
            let _ = drm_ffi::mode::rm_fb(fd, self.framebuffer.get());
        }
        let _ = unsafe { libc::munmap(self.mapping, self.size) };
        if self.drm_cleanup_armed {
            let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
            let _ = drm_ffi::mode::dumbbuffer::destroy(fd, self.handle);
        }
    }
}

#[derive(Debug, Default)]
struct AtomicCursorResources {
    current: Option<AtomicCursorBuffer>,
    retired: Vec<AtomicCursorBuffer>,
}

impl AtomicCursorResources {
    fn retire_safe(&mut self, keep: &[Option<u32>]) {
        self.retired.retain(|buffer| {
            keep.iter()
                .flatten()
                .any(|framebuffer| *framebuffer == buffer.framebuffer.get())
        });
    }

    fn disarm_drm_cleanup(&mut self) {
        if let Some(current) = self.current.as_mut() {
            current.disarm_drm_cleanup();
        }
        for retired in &mut self.retired {
            retired.disarm_drm_cleanup();
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AtomicCursorDirty {
    pub(crate) position: bool,
    pub(crate) visibility: bool,
    pub(crate) image: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AtomicCursorCounters {
    pub(crate) updates_requested: u64,
    pub(crate) updates_submitted: u64,
    pub(crate) updates_completed: u64,
    pub(crate) updates_coalesced: u64,
    pub(crate) hidden_updates_suppressed: u64,
    pub(crate) test_failures: u64,
    pub(crate) submit_failures: u64,
    pub(crate) software_fallbacks: u64,
}

#[derive(Debug)]
pub(crate) struct NativeAtomicCursor {
    desired: AtomicCursorVisualState,
    submitted: AtomicCursorVisualState,
    current: AtomicCursorVisualState,
    resources: AtomicCursorResources,
    pub(crate) plane: AtomicCursorPlaneProperties,
    pub(crate) generation: u64,
    pub(crate) dirty: AtomicCursorDirty,
    pub(crate) counters: AtomicCursorCounters,
    failure_latched: bool,
    pending_token: Option<PageFlipToken>,
    pending_is_primary: bool,
    suspended_desired: Option<AtomicCursorVisualState>,
    drm_cleanup_armed: bool,
}

impl NativeAtomicCursor {
    pub(crate) fn create(
        file: &fs::File,
        plane: AtomicCursorPlaneProperties,
        width: u32,
        height: u32,
        generation: u64,
    ) -> io::Result<Self> {
        if plane.format_modifier.modifier != 0 {
            return Err(io::Error::other(
                "Atomic cursor CPU fallback requires a linear cursor format",
            ));
        }
        let mut buffer = AtomicCursorBuffer::create(file, width, height)?;
        if let Err(error) = buffer.upload_default_cursor() {
            drop(buffer);
            return Err(error);
        }
        let state = AtomicCursorVisualState {
            visible: true,
            x: 0,
            y: 0,
            hotspot_x: 0,
            hotspot_y: 0,
            width,
            height,
            framebuffer_id: Some(buffer.framebuffer.get()),
            image_generation: 1,
        };
        Ok(Self {
            desired: state.clone(),
            submitted: state.clone(),
            current: state,
            resources: AtomicCursorResources {
                current: Some(buffer),
                retired: Vec::new(),
            },
            plane,
            generation,
            dirty: AtomicCursorDirty::default(),
            counters: AtomicCursorCounters::default(),
            failure_latched: false,
            pending_token: None,
            pending_is_primary: false,
            suspended_desired: None,
            drm_cleanup_armed: true,
        })
    }

    pub(crate) fn desired(&self) -> &AtomicCursorVisualState {
        &self.desired
    }

    pub(crate) fn current(&self) -> &AtomicCursorVisualState {
        &self.current
    }

    /// The initial modeset has already made `state` the kernel-owned state.
    /// Promote it without manufacturing a redundant cursor-only pageflip.
    pub(crate) fn mark_initial_submitted(&mut self, state: Option<&AtomicCursorVisualState>) {
        let state = state.cloned().unwrap_or_else(|| AtomicCursorVisualState {
            visible: false,
            framebuffer_id: None,
            ..self.desired.clone()
        });
        self.submitted = state.clone();
        self.current = state;
        self.dirty = AtomicCursorDirty::default();
    }

    pub(crate) fn set_position(&mut self, x: i32, y: i32) {
        if self.desired.x != x || self.desired.y != y {
            self.desired.x = x;
            self.desired.y = y;
            self.dirty.position = true;
            self.counters.updates_requested = self.counters.updates_requested.saturating_add(1);
            if !self.desired.visible && !self.current.visible {
                self.counters.hidden_updates_suppressed =
                    self.counters.hidden_updates_suppressed.saturating_add(1);
            } else if self.pending_token.is_some() {
                self.counters.updates_coalesced = self.counters.updates_coalesced.saturating_add(1);
            }
        }
    }

    pub(crate) fn set_visible(&mut self, visible: bool) {
        let visible = visible && !self.failure_latched;
        if self.desired.visible != visible {
            self.desired.visible = visible;
            self.dirty.visibility = true;
            self.counters.updates_requested = self.counters.updates_requested.saturating_add(1);
            if self.pending_token.is_some() {
                self.counters.updates_coalesced = self.counters.updates_coalesced.saturating_add(1);
            }
        }
    }

    pub(crate) fn needs_submission(&self) -> bool {
        !self.desired.kms_equivalent(&self.current) && self.pending_token.is_none()
    }

    pub(crate) fn needs_submission_for(&self, desired: Option<&AtomicCursorVisualState>) -> bool {
        let hidden = AtomicCursorVisualState {
            visible: false,
            framebuffer_id: None,
            ..self.desired.clone()
        };
        let desired = desired.unwrap_or(&hidden);
        !desired.kms_equivalent(&self.current) && self.pending_token.is_none()
    }

    pub(crate) fn begin_submission(
        &mut self,
        token: PageFlipToken,
        state: AtomicCursorVisualState,
    ) -> AtomicCursorVisualState {
        self.submitted = state.clone();
        self.pending_token = Some(token);
        self.pending_is_primary = false;
        self.dirty = AtomicCursorDirty::default();
        self.counters.updates_submitted = self.counters.updates_submitted.saturating_add(1);
        state
    }

    pub(crate) fn complete_submission(
        &mut self,
        token: PageFlipToken,
        generation: u64,
    ) -> io::Result<()> {
        if generation != self.generation {
            return Err(io::Error::other("stale Atomic cursor DRM generation"));
        }
        if self.pending_token != Some(token) {
            return Err(io::Error::other("stale Atomic cursor pageflip token"));
        }
        self.pending_token = None;
        self.pending_is_primary = false;
        self.current = self.submitted.clone();
        self.counters.updates_completed = self.counters.updates_completed.saturating_add(1);
        let keep = [
            self.desired.framebuffer_id,
            self.submitted.framebuffer_id,
            self.current.framebuffer_id,
        ];
        self.resources.retire_safe(&keep);
        Ok(())
    }

    pub(crate) fn pending_token(&self) -> Option<PageFlipToken> {
        self.pending_token
    }

    pub(crate) fn pending_is_primary(&self) -> bool {
        self.pending_is_primary
    }

    pub(crate) fn begin_primary_submission(
        &mut self,
        token: PageFlipToken,
        state: AtomicCursorVisualState,
    ) {
        self.submitted = state;
        self.pending_token = Some(token);
        self.pending_is_primary = true;
        self.dirty = AtomicCursorDirty::default();
        self.counters.updates_submitted = self.counters.updates_submitted.saturating_add(1);
    }

    pub(crate) fn mark_failure_latched(&mut self) {
        self.failure_latched = true;
    }

    pub(crate) fn note_test_failure(&mut self) {
        self.counters.test_failures = self.counters.test_failures.saturating_add(1);
        self.mark_failure_latched();
    }

    pub(crate) fn note_submit_failure(&mut self) {
        self.counters.submit_failures = self.counters.submit_failures.saturating_add(1);
        self.mark_failure_latched();
    }

    pub(crate) fn note_software_fallback(&mut self) {
        self.counters.software_fallbacks = self.counters.software_fallbacks.saturating_add(1);
    }

    #[allow(dead_code)]
    pub(crate) fn replace_default_image(&mut self, file: &fs::File) -> io::Result<()> {
        let mut replacement =
            AtomicCursorBuffer::create(file, self.desired.width, self.desired.height)?;
        if let Err(error) = replacement.upload_default_cursor() {
            drop(replacement);
            return Err(error);
        }
        if let Some(previous) = self.resources.current.replace(replacement) {
            self.resources.retired.push(previous);
        }
        let framebuffer_id = self
            .resources
            .current
            .as_ref()
            .map(|buffer| buffer.framebuffer.get());
        self.desired.framebuffer_id = framebuffer_id;
        self.desired.image_generation = self.desired.image_generation.saturating_add(1);
        self.dirty.image = true;
        if !self.desired.visible && !self.current.visible {
            self.counters.hidden_updates_suppressed =
                self.counters.hidden_updates_suppressed.saturating_add(1);
        }
        Ok(())
    }

    pub(crate) fn suspend_for_session(&mut self) {
        self.suspended_desired = Some(self.desired.clone());
        self.pending_token = None;
        self.pending_is_primary = false;
        self.desired.visible = false;
        self.dirty.visibility = true;
    }

    pub(crate) fn prepare_for_recovery(
        &mut self,
        file: &fs::File,
        plane: AtomicCursorPlaneProperties,
        width: u32,
        height: u32,
        generation: u64,
    ) -> io::Result<AtomicCursorVisualState> {
        let mut replacement = AtomicCursorBuffer::create(file, width, height)?;
        if let Err(error) = replacement.upload_default_cursor() {
            drop(replacement);
            return Err(error);
        }
        if let Some(previous) = self.resources.current.replace(replacement) {
            self.resources.retired.push(previous);
        }
        self.plane = plane;
        self.generation = generation;
        let framebuffer_id = self
            .resources
            .current
            .as_ref()
            .map(|buffer| buffer.framebuffer.get());
        let mut restored = self
            .suspended_desired
            .take()
            .unwrap_or_else(|| self.desired.clone());
        restored.width = width;
        restored.height = height;
        restored.framebuffer_id = framebuffer_id;
        self.desired = restored.clone();
        self.submitted = AtomicCursorVisualState::hidden(width, height);
        self.submitted.framebuffer_id = framebuffer_id;
        self.current = self.submitted.clone();
        self.pending_token = None;
        self.pending_is_primary = false;
        self.dirty = AtomicCursorDirty::default();
        self.failure_latched = false;
        Ok(restored)
    }

    pub(crate) const fn failure_latched(&self) -> bool {
        self.failure_latched
    }

    pub(crate) fn rearm_generation(&mut self, generation: u64) {
        self.generation = generation;
        self.pending_token = None;
        self.pending_is_primary = false;
        self.failure_latched = false;
    }

    pub(crate) fn disarm_drm_cleanup(&mut self) {
        self.drm_cleanup_armed = false;
        self.resources.disarm_drm_cleanup();
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use oblivion_one::native::kms::{
        AtomicPlaneProperties, DrmFormatModifierPair, PlanePropertyId, PropertyId,
    };

    fn property(id: u32) -> PlanePropertyId {
        PlanePropertyId(PropertyId::new(id).expect("test property id is nonzero"))
    }

    fn test_cursor() -> NativeAtomicCursor {
        let state = AtomicCursorVisualState::hidden(64, 64);
        NativeAtomicCursor {
            desired: state.clone(),
            submitted: state.clone(),
            current: state,
            resources: AtomicCursorResources::default(),
            plane: AtomicCursorPlaneProperties {
                plane_id: 4,
                crtc_id: 2,
                fb_id: 1,
                crtc_x: 2,
                crtc_y: 3,
                crtc_w: 4,
                crtc_h: 5,
                src_x: 6,
                src_y: 7,
                src_w: 8,
                src_h: 9,
                in_formats: None,
                rotation: None,
                property_ids: AtomicPlaneProperties {
                    fb_id: property(10),
                    crtc_id: property(11),
                    src_x: property(12),
                    src_y: property(13),
                    src_w: property(14),
                    src_h: property(15),
                    crtc_x: property(16),
                    crtc_y: property(17),
                    crtc_w: property(18),
                    crtc_h: property(19),
                    plane_type: property(20),
                    in_fence_fd: None,
                    in_formats: None,
                    damage_clips: None,
                    rotation: None,
                    alpha: None,
                    pixel_blend_mode: None,
                    color_encoding: None,
                    color_range: None,
                },
                format_modifier: DrmFormatModifierPair {
                    fourcc: DRM_FORMAT_ARGB8888,
                    modifier: 0,
                },
                alpha_maximum: None,
                pixel_blend_mode_premultiplied: None,
            },
            generation: 1,
            dirty: AtomicCursorDirty::default(),
            counters: AtomicCursorCounters::default(),
            failure_latched: false,
            pending_token: None,
            pending_is_primary: false,
            suspended_desired: None,
            drm_cleanup_armed: false,
        }
    }

    #[test]
    fn hidden_cursor_position_changes_do_not_need_submission() {
        let mut cursor = test_cursor();
        cursor.set_position(100, 200);

        assert!(!cursor.needs_submission());
    }

    #[test]
    fn hidden_cursor_image_changes_do_not_need_submission() {
        let mut cursor = test_cursor();
        cursor.desired.framebuffer_id = Some(99);
        cursor.desired.image_generation = 2;
        cursor.dirty.image = true;

        assert!(!cursor.needs_submission());
    }

    #[test]
    fn hidden_to_visible_submits_latest_position() {
        let mut cursor = test_cursor();
        cursor.set_position(100, 200);
        cursor.set_visible(true);

        assert!(cursor.needs_submission());
        assert_eq!(cursor.desired().x, 100);
        assert_eq!(cursor.desired().y, 200);
    }

    #[test]
    fn visible_to_hidden_submits_plane_disable() {
        let mut cursor = test_cursor();
        cursor.desired.visible = true;
        cursor.current.visible = true;
        cursor.set_visible(false);

        assert!(cursor.needs_submission());
    }

    #[test]
    fn visible_cursor_position_change_needs_submission() {
        let mut cursor = test_cursor();
        cursor.desired.visible = true;
        cursor.current.visible = true;
        cursor.set_position(100, 200);

        assert!(cursor.needs_submission());
    }

    #[test]
    fn failure_latch_keeps_plane_disabled_after_input_visibility_sync() {
        let mut cursor = test_cursor();
        cursor.mark_failure_latched();
        cursor.set_visible(true);

        assert!(!cursor.desired().visible);
        assert!(!cursor.needs_submission());
    }

    #[test]
    fn initial_software_modeset_records_a_disabled_cursor_plane() {
        let mut cursor = test_cursor();
        cursor.desired.visible = true;

        cursor.mark_initial_submitted(None);

        assert!(!cursor.current().visible);
        assert_eq!(cursor.current().framebuffer_id, None);
        assert!(!cursor.needs_submission_for(None));
    }
}

pub(crate) fn native_cursor_argb_bytes(
    pixels: &[u32],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
    pitch: u32,
) -> io::Result<Vec<u8>> {
    if source_width > target_width || source_height > target_height {
        return Err(io::Error::other(
            "native cursor texture exceeds target buffer",
        ));
    }
    let source_width = usize::try_from(source_width)
        .map_err(|_| io::Error::other("native cursor source width overflow"))?;
    let source_height = usize::try_from(source_height)
        .map_err(|_| io::Error::other("native cursor source height overflow"))?;
    let target_width = usize::try_from(target_width)
        .map_err(|_| io::Error::other("native cursor target width overflow"))?;
    let target_height = usize::try_from(target_height)
        .map_err(|_| io::Error::other("native cursor target height overflow"))?;
    let pitch =
        usize::try_from(pitch).map_err(|_| io::Error::other("invalid native cursor pitch"))?;
    let row_bytes = source_width
        .checked_mul(mem::size_of::<u32>())
        .ok_or_else(|| io::Error::other("native cursor source row overflow"))?;
    let min_pitch = target_width
        .checked_mul(mem::size_of::<u32>())
        .ok_or_else(|| io::Error::other("native cursor target row overflow"))?;
    if pitch < min_pitch {
        return Err(io::Error::other("native cursor pitch is too small"));
    }
    let pixel_count = source_width
        .checked_mul(source_height)
        .ok_or_else(|| io::Error::other("native cursor source overflow"))?;
    if pixels.len() < pixel_count {
        return Err(io::Error::other("native cursor source is too small"));
    }
    let byte_len = pitch
        .checked_mul(target_height)
        .ok_or_else(|| io::Error::other("native cursor target overflow"))?;
    let source_bytes_len = pixel_count
        .checked_mul(mem::size_of::<u32>())
        .ok_or_else(|| io::Error::other("native cursor source byte overflow"))?;
    let source_bytes =
        unsafe { slice::from_raw_parts(pixels.as_ptr().cast::<u8>(), source_bytes_len) };
    let mut bytes = vec![0; byte_len];
    for y in 0..source_height {
        let source_start = y
            .checked_mul(row_bytes)
            .ok_or_else(|| io::Error::other("native cursor source offset overflow"))?;
        let target_start = y
            .checked_mul(pitch)
            .ok_or_else(|| io::Error::other("native cursor target offset overflow"))?;
        bytes[target_start..target_start + row_bytes]
            .copy_from_slice(&source_bytes[source_start..source_start + row_bytes]);
    }
    Ok(bytes)
}
