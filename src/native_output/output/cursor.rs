use super::*;
use oblivion_one::cursor_theme::CompositorCursorImage;
use std::sync::Arc;

pub(crate) const NATIVE_HARDWARE_CURSOR_SIZE: u32 = 64;
const INITIAL_CURSOR_EPOCH: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeCursorImageKey {
    pub(crate) surface_id: u32,
    pub(crate) buffer_id: u64,
    pub(crate) commit_sequence: u64,
    pub(crate) hotspot_x: i32,
    pub(crate) hotspot_y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) buffer_scale: u32,
    pub(crate) buffer_transform: u32,
}

impl NativeCursorImageKey {
    pub(crate) fn for_surface(surface: &RenderableSurface, hotspot_x: i32, hotspot_y: i32) -> Self {
        Self {
            surface_id: surface.surface_id,
            buffer_id: surface.buffer_id().get(),
            commit_sequence: surface.commit_sequence.0,
            hotspot_x,
            hotspot_y,
            width: surface.width,
            height: surface.height,
            buffer_scale: surface.buffer_scale,
            buffer_transform: cursor_transform_key(surface.buffer_transform),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeCursorSourceKey {
    Theme,
    Client(NativeCursorImageKey),
}

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

    fn upload_image(&mut self, image: &CompositorCursorImage) -> io::Result<()> {
        let bytes = native_cursor_argb_bytes(
            &image.pixels_argb8888,
            image.width,
            image.height,
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
    theme_cache: Option<AtomicCursorBuffer>,
    client_cache: Option<(NativeCursorImageKey, AtomicCursorBuffer)>,
}

impl AtomicCursorResources {
    fn take_cached(&mut self, source_key: NativeCursorSourceKey) -> Option<AtomicCursorBuffer> {
        match source_key {
            NativeCursorSourceKey::Theme => self.theme_cache.take(),
            NativeCursorSourceKey::Client(key) => self
                .client_cache
                .take()
                .and_then(|(cached_key, buffer)| (cached_key == key).then_some(buffer)),
        }
    }

    fn cache_current(&mut self, source_key: NativeCursorSourceKey, buffer: AtomicCursorBuffer) {
        match source_key {
            NativeCursorSourceKey::Theme => {
                if let Some(previous) = self.theme_cache.replace(buffer) {
                    self.retired.push(previous);
                }
            }
            NativeCursorSourceKey::Client(key) => {
                if let Some((previous_key, previous)) = self.client_cache.replace((key, buffer))
                    && previous_key != key
                {
                    self.retired.push(previous);
                }
            }
        }
    }

    fn retire_cached_mismatch(&mut self, source_key: NativeCursorSourceKey) {
        if let NativeCursorSourceKey::Client(key) = source_key
            && self
                .client_cache
                .as_ref()
                .is_some_and(|(cached_key, _)| *cached_key != key)
            && let Some((_, buffer)) = self.client_cache.take()
        {
            self.retired.push(buffer);
        }
    }

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
        if let Some(theme) = self.theme_cache.as_mut() {
            theme.disarm_drm_cleanup();
        }
        if let Some((_, client)) = self.client_cache.as_mut() {
            client.disarm_drm_cleanup();
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
    pub(crate) image_uploads: u64,
    pub(crate) client_image_uploads: u64,
    pub(crate) image_cache_hits: u64,
    pub(crate) position_submissions: u64,
    pub(crate) primary_submissions: u64,
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
    pub(crate) image: Arc<CompositorCursorImage>,
    theme_image: Arc<CompositorCursorImage>,
    source_key: NativeCursorSourceKey,
    desired: AtomicCursorVisualState,
    submitted: AtomicCursorVisualState,
    current: AtomicCursorVisualState,
    resources: AtomicCursorResources,
    pub(crate) plane: AtomicCursorPlaneProperties,
    pub(crate) generation: u64,
    /// Output-local identity for the desired KMS cursor state. This is
    /// intentionally independent of compositor scene/cursor generations.
    desired_epoch: u64,
    submitted_epoch: u64,
    hardware_path_active: bool,
    pub(crate) dirty: AtomicCursorDirty,
    pub(crate) counters: AtomicCursorCounters,
    failure_latched: bool,
    client_image_failure: Option<NativeCursorImageKey>,
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
        image: Arc<CompositorCursorImage>,
    ) -> io::Result<Self> {
        if plane.format_modifier.modifier != 0 {
            return Err(io::Error::other(
                "Atomic cursor CPU fallback requires a linear cursor format",
            ));
        }
        validate_atomic_cursor_image(&image, width, height)?;
        let mut buffer = AtomicCursorBuffer::create(file, width, height)?;
        if let Err(error) = buffer.upload_image(&image) {
            drop(buffer);
            return Err(error);
        }
        let state = atomic_cursor_state_for_image(&image, Some(buffer.framebuffer.get()));
        Ok(Self {
            image: image.clone(),
            theme_image: image,
            source_key: NativeCursorSourceKey::Theme,
            desired: state.clone(),
            submitted: state.clone(),
            current: state,
            resources: AtomicCursorResources {
                current: Some(buffer),
                retired: Vec::new(),
                theme_cache: None,
                client_cache: None,
            },
            plane,
            generation,
            desired_epoch: INITIAL_CURSOR_EPOCH,
            submitted_epoch: INITIAL_CURSOR_EPOCH,
            hardware_path_active: false,
            dirty: AtomicCursorDirty::default(),
            counters: AtomicCursorCounters::default(),
            failure_latched: false,
            client_image_failure: None,
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

    pub(crate) const fn desired_epoch(&self) -> u64 {
        self.desired_epoch
    }

    fn advance_desired_epoch(&mut self) {
        self.desired_epoch = next_cursor_epoch(self.desired_epoch, self.submitted_epoch);
    }

    pub(crate) fn set_hardware_path_active(&mut self, active: bool) {
        if self.hardware_path_active != active {
            self.hardware_path_active = active;
            self.advance_desired_epoch();
        }
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
        self.submitted_epoch = self.desired_epoch;
        self.current = state;
        self.dirty = AtomicCursorDirty::default();
    }

    pub(crate) fn set_position(&mut self, x: i32, y: i32) {
        if self.desired.x != x || self.desired.y != y {
            self.desired.x = x;
            self.desired.y = y;
            self.advance_desired_epoch();
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
            self.advance_desired_epoch();
            self.dirty.visibility = true;
            self.counters.updates_requested = self.counters.updates_requested.saturating_add(1);
            if self.pending_token.is_some() {
                self.counters.updates_coalesced = self.counters.updates_coalesced.saturating_add(1);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn needs_submission(&self) -> bool {
        self.needs_submission_for(Some(&self.desired))
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
        if self.dirty.position {
            self.counters.position_submissions =
                self.counters.position_submissions.saturating_add(1);
        }
        self.submitted = state.clone();
        self.submitted_epoch = self.desired_epoch;
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
        self.counters.primary_submissions = self.counters.primary_submissions.saturating_add(1);
        self.submitted = state;
        self.submitted_epoch = self.desired_epoch;
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

    pub(crate) fn replace_image(
        &mut self,
        file: &fs::File,
        image: Arc<CompositorCursorImage>,
        source_key: NativeCursorImageKey,
    ) -> io::Result<()> {
        if self.source_key == NativeCursorSourceKey::Client(source_key) {
            return Ok(());
        }
        self.replace_image_with_source(file, image, NativeCursorSourceKey::Client(source_key))
    }

    pub(crate) fn restore_theme_image(&mut self, file: &fs::File) -> io::Result<()> {
        if self.source_key == NativeCursorSourceKey::Theme {
            return Ok(());
        }
        self.replace_image_with_source(file, self.theme_image.clone(), NativeCursorSourceKey::Theme)
    }

    pub(crate) const fn using_theme_image(&self) -> bool {
        matches!(self.source_key, NativeCursorSourceKey::Theme)
    }

    pub(crate) fn client_image_matches(&self, key: NativeCursorImageKey) -> bool {
        self.source_key == NativeCursorSourceKey::Client(key)
    }

    pub(crate) fn client_image_failure_matches(&self, key: NativeCursorImageKey) -> bool {
        self.client_image_failure == Some(key)
    }

    pub(crate) fn note_client_image_failure(&mut self, key: NativeCursorImageKey) {
        self.client_image_failure = Some(key);
    }

    fn replace_image_with_source(
        &mut self,
        file: &fs::File,
        image: Arc<CompositorCursorImage>,
        source_key: NativeCursorSourceKey,
    ) -> io::Result<()> {
        self.resources.retire_cached_mismatch(source_key);
        let mut replacement = self.resources.take_cached(source_key);
        let cache_hit = replacement.is_some();
        if replacement.is_none() {
            replacement = Some(AtomicCursorBuffer::create(
                file,
                self.resources
                    .current
                    .as_ref()
                    .map_or(self.image.width, |buffer| buffer.width),
                self.resources
                    .current
                    .as_ref()
                    .map_or(self.image.height, |buffer| buffer.height),
            )?);
            if let Err(error) = replacement
                .as_mut()
                .expect("new cursor buffer is present")
                .upload_image(&image)
            {
                drop(replacement);
                return Err(error);
            }
        }
        if let Some(previous) = self.resources.current.take() {
            self.resources.cache_current(self.source_key, previous);
        }
        self.resources.current = replacement;
        let framebuffer_id = self
            .resources
            .current
            .as_ref()
            .map(|buffer| buffer.framebuffer.get());
        self.image = image;
        if cache_hit {
            self.counters.image_cache_hits = self.counters.image_cache_hits.saturating_add(1);
        } else {
            self.counters.image_uploads = self.counters.image_uploads.saturating_add(1);
            if matches!(source_key, NativeCursorSourceKey::Client(_)) {
                self.counters.client_image_uploads =
                    self.counters.client_image_uploads.saturating_add(1);
            }
        }
        if matches!(source_key, NativeCursorSourceKey::Client(_)) {
            // A new image is a meaningful retry point after a cursor-plane
            // TEST_ONLY failure.  Pointer motion alone never clears this
            // latch, so a rejected plane cannot create a retry storm.
            self.failure_latched = false;
        }
        self.source_key = source_key;
        self.client_image_failure = None;
        self.desired.framebuffer_id = framebuffer_id;
        self.desired.hotspot_x = self.image.hotspot_x;
        self.desired.hotspot_y = self.image.hotspot_y;
        self.desired.width = self.image.width;
        self.desired.height = self.image.height;
        self.desired.image_generation = self.desired.image_generation.saturating_add(1);
        self.advance_desired_epoch();
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
        self.set_visible(false);
    }

    pub(crate) fn prepare_for_recovery(
        &mut self,
        file: &fs::File,
        plane: AtomicCursorPlaneProperties,
        width: u32,
        height: u32,
        generation: u64,
    ) -> io::Result<AtomicCursorVisualState> {
        validate_atomic_cursor_image(&self.image, width, height)?;
        let mut replacement = AtomicCursorBuffer::create(file, width, height)?;
        if let Err(error) = replacement.upload_image(&self.image) {
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
        restored.hotspot_x = self.image.hotspot_x;
        restored.hotspot_y = self.image.hotspot_y;
        restored.width = self.image.width;
        restored.height = self.image.height;
        restored.framebuffer_id = framebuffer_id;
        if !restored.kms_equivalent(&self.desired) {
            self.advance_desired_epoch();
        }
        self.desired = restored.clone();
        self.submitted = AtomicCursorVisualState::hidden(self.image.width, self.image.height);
        self.submitted.framebuffer_id = framebuffer_id;
        self.submitted_epoch = 0;
        self.current = self.submitted.clone();
        self.pending_token = None;
        self.pending_is_primary = false;
        self.dirty = AtomicCursorDirty::default();
        self.failure_latched = false;
        self.client_image_failure = None;
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
        self.client_image_failure = None;
    }

    pub(crate) fn disarm_drm_cleanup(&mut self) {
        self.drm_cleanup_armed = false;
        self.resources.disarm_drm_cleanup();
    }
}

pub(crate) fn client_cursor_image(
    surface: &RenderableSurface,
    hotspot_x: i32,
    hotspot_y: i32,
) -> Option<Arc<CompositorCursorImage>> {
    // A viewport can crop or scale a cursor buffer. Until that transformation
    // is represented in the native image conversion, use software composition
    // rather than uploading an image with the wrong dimensions or hotspot.
    if surface.viewport_source.is_some() || surface.viewport_destination.is_some() {
        return None;
    }
    let pixels = surface.cpu_pixels()?;
    let source_size = surface.buffer_size();
    if source_size.width == 0
        || source_size.height == 0
        || surface.width == 0
        || surface.height == 0
    {
        return None;
    }
    let (pixels, (source_width, source_height)) = transform_cursor_pixels(
        pixels,
        source_size.width,
        source_size.height,
        surface.buffer_transform,
    )?;
    let target_width = usize::try_from(surface.width).ok()?;
    let target_height = usize::try_from(surface.height).ok()?;
    let mut normalized = vec![0; target_width.checked_mul(target_height)?];
    for y in 0..target_height {
        let source_y = y.saturating_mul(source_height) / target_height;
        for x in 0..target_width {
            let source_x = x.saturating_mul(source_width) / target_width;
            normalized[y * target_width + x] = pixels[source_y * source_width + source_x];
        }
    }
    let hotspot = normalize_cursor_hotspot(
        hotspot_x,
        hotspot_y,
        source_size.width,
        source_size.height,
        source_width as u32,
        source_height as u32,
        surface.width,
        surface.height,
        surface.buffer_transform,
    )?;
    CompositorCursorImage::from_argb8888(
        normalized,
        surface.width,
        surface.height,
        hotspot.0,
        hotspot.1,
    )
    .ok()
    .map(Arc::new)
}

fn transform_cursor_pixels(
    pixels: &[u32],
    width: u32,
    height: u32,
    transform: wayland_server::protocol::wl_output::Transform,
) -> Option<(Vec<u32>, (usize, usize))> {
    let width = usize::try_from(width).ok()?;
    let height = usize::try_from(height).ok()?;
    let count = width.checked_mul(height)?;
    if pixels.len() < count {
        return None;
    }
    let rotated = matches!(
        transform,
        wayland_server::protocol::wl_output::Transform::_90
            | wayland_server::protocol::wl_output::Transform::_270
            | wayland_server::protocol::wl_output::Transform::Flipped90
            | wayland_server::protocol::wl_output::Transform::Flipped270
    );
    let output_width = if rotated { height } else { width };
    let output_height = if rotated { width } else { height };
    let mut output = vec![0; output_width.checked_mul(output_height)?];
    for y in 0..output_height {
        for x in 0..output_width {
            let (source_x, source_y) = cursor_source_coordinate(x, y, width, height, transform);
            output[y * output_width + x] = pixels[source_y * width + source_x];
        }
    }
    Some((output, (output_width, output_height)))
}

fn cursor_source_coordinate(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    transform: wayland_server::protocol::wl_output::Transform,
) -> (usize, usize) {
    use wayland_server::protocol::wl_output::Transform;
    match transform {
        Transform::Normal => (x, y),
        Transform::_90 => (y, height - 1 - x),
        Transform::_180 => (width - 1 - x, height - 1 - y),
        Transform::_270 => (height - 1 - y, x),
        Transform::Flipped => (width - 1 - x, y),
        Transform::Flipped90 => (y, x),
        Transform::Flipped180 => (x, height - 1 - y),
        Transform::Flipped270 => (height - 1 - y, width - 1 - x),
        _ => (x.min(width - 1), y.min(height - 1)),
    }
}

#[allow(clippy::too_many_arguments)]
fn normalize_cursor_hotspot(
    hotspot_x: i32,
    hotspot_y: i32,
    source_width: u32,
    source_height: u32,
    transformed_width: u32,
    transformed_height: u32,
    target_width: u32,
    target_height: u32,
    transform: wayland_server::protocol::wl_output::Transform,
) -> Option<(i32, i32)> {
    if hotspot_x < 0
        || hotspot_y < 0
        || source_width == 0
        || source_height == 0
        || hotspot_x >= i32::try_from(source_width).ok()?
        || hotspot_y >= i32::try_from(source_height).ok()?
        || transformed_width == 0
        || transformed_height == 0
    {
        return None;
    }
    let (x, y) = match transform {
        wayland_server::protocol::wl_output::Transform::Normal => (hotspot_x, hotspot_y),
        wayland_server::protocol::wl_output::Transform::_90 => (
            i32::try_from(source_height)
                .ok()?
                .saturating_sub(1 + hotspot_y),
            hotspot_x,
        ),
        wayland_server::protocol::wl_output::Transform::_180 => (
            i32::try_from(source_width)
                .ok()?
                .saturating_sub(1 + hotspot_x),
            i32::try_from(source_height)
                .ok()?
                .saturating_sub(1 + hotspot_y),
        ),
        wayland_server::protocol::wl_output::Transform::_270 => (
            hotspot_y,
            i32::try_from(source_width)
                .ok()?
                .saturating_sub(1 + hotspot_x),
        ),
        wayland_server::protocol::wl_output::Transform::Flipped => (
            i32::try_from(source_width)
                .ok()?
                .saturating_sub(1 + hotspot_x),
            hotspot_y,
        ),
        wayland_server::protocol::wl_output::Transform::Flipped90 => (hotspot_y, hotspot_x),
        wayland_server::protocol::wl_output::Transform::Flipped180 => (
            hotspot_x,
            i32::try_from(source_height)
                .ok()?
                .saturating_sub(1 + hotspot_y),
        ),
        wayland_server::protocol::wl_output::Transform::Flipped270 => (
            i32::try_from(source_width)
                .ok()?
                .saturating_sub(1 + hotspot_y),
            i32::try_from(source_height)
                .ok()?
                .saturating_sub(1 + hotspot_x),
        ),
        _ => return None,
    };
    let x = i64::from(x)
        .saturating_mul(i64::from(target_width))
        .checked_div(i64::from(transformed_width))?;
    let y = i64::from(y)
        .saturating_mul(i64::from(target_height))
        .checked_div(i64::from(transformed_height))?;
    Some((
        i32::try_from(x)
            .ok()?
            .clamp(0, i32::try_from(target_width).ok()?.saturating_sub(1)),
        i32::try_from(y)
            .ok()?
            .clamp(0, i32::try_from(target_height).ok()?.saturating_sub(1)),
    ))
}

fn cursor_transform_key(transform: wayland_server::protocol::wl_output::Transform) -> u32 {
    use wayland_server::protocol::wl_output::Transform;
    match transform {
        Transform::Normal => 0,
        Transform::_90 => 1,
        Transform::_180 => 2,
        Transform::_270 => 3,
        Transform::Flipped => 4,
        Transform::Flipped90 => 5,
        Transform::Flipped180 => 6,
        Transform::Flipped270 => 7,
        _ => u32::MAX,
    }
}

pub(crate) fn cursor_image_fits_buffer(
    image: &CompositorCursorImage,
    width: u32,
    height: u32,
) -> bool {
    image.width <= width && image.height <= height
}

pub(crate) fn validate_atomic_cursor_image(
    image: &CompositorCursorImage,
    width: u32,
    height: u32,
) -> io::Result<()> {
    if cursor_image_fits_buffer(image, width, height) {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "Atomic cursor theme image {}x{} exceeds usable cursor buffer {}x{}",
        image.width, image.height, width, height
    )))
}

fn atomic_cursor_state_for_image(
    image: &CompositorCursorImage,
    framebuffer_id: Option<u32>,
) -> AtomicCursorVisualState {
    AtomicCursorVisualState {
        visible: true,
        x: 0,
        y: 0,
        hotspot_x: image.hotspot_x,
        hotspot_y: image.hotspot_y,
        width: image.width,
        height: image.height,
        framebuffer_id,
        image_generation: 1,
    }
}

fn next_cursor_epoch(current: u64, submitted: u64) -> u64 {
    let mut next = current.wrapping_add(1);
    if next == 0 {
        next = INITIAL_CURSOR_EPOCH;
    }
    if next == submitted {
        next = next.wrapping_add(1);
        if next == 0 {
            next = INITIAL_CURSOR_EPOCH;
        }
    }
    next
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use crate::native_output::runtime::{
        NativeCursorOutputArbitration, update_cursor_output_arbitration,
    };
    use oblivion_one::native::kms::{
        AtomicPlaneProperties, DrmFormatModifierPair, PlanePropertyId, PropertyId,
    };
    use oblivion_one::native::scheduler::NativeFrameScheduler;

    fn property(id: u32) -> PlanePropertyId {
        PlanePropertyId(PropertyId::new(id).expect("test property id is nonzero"))
    }

    fn test_cursor() -> NativeAtomicCursor {
        let state = AtomicCursorVisualState::hidden(64, 64);
        NativeAtomicCursor {
            image: Arc::new(CompositorCursorImage::builtin_fallback()),
            theme_image: Arc::new(CompositorCursorImage::builtin_fallback()),
            source_key: NativeCursorSourceKey::Theme,
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
            desired_epoch: INITIAL_CURSOR_EPOCH,
            submitted_epoch: INITIAL_CURSOR_EPOCH,
            hardware_path_active: false,
            dirty: AtomicCursorDirty::default(),
            counters: AtomicCursorCounters::default(),
            failure_latched: false,
            client_image_failure: None,
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
    fn redundant_position_does_not_advance_cursor_epoch() {
        let mut cursor = test_cursor();
        let initial_epoch = cursor.desired_epoch();

        cursor.set_position(0, 0);

        assert_eq!(cursor.desired_epoch(), initial_epoch);
    }

    #[test]
    fn new_position_advances_cursor_epoch_once() {
        let mut cursor = test_cursor();
        let initial_epoch = cursor.desired_epoch();

        cursor.set_position(100, 200);
        assert_ne!(cursor.desired_epoch(), initial_epoch);
        let moved_epoch = cursor.desired_epoch();

        cursor.set_position(100, 200);

        assert_eq!(cursor.desired_epoch(), moved_epoch);
    }

    #[test]
    fn visibility_change_advances_cursor_epoch_once() {
        let mut cursor = test_cursor();
        let initial_epoch = cursor.desired_epoch();

        cursor.set_visible(true);
        assert_ne!(cursor.desired_epoch(), initial_epoch);
        let visible_epoch = cursor.desired_epoch();

        cursor.set_visible(true);

        assert_eq!(cursor.desired_epoch(), visible_epoch);
    }

    #[test]
    fn hardware_path_transition_advances_cursor_epoch_once() {
        let mut cursor = test_cursor();
        let initial_epoch = cursor.desired_epoch();

        cursor.set_hardware_path_active(true);
        assert_ne!(cursor.desired_epoch(), initial_epoch);
        let active_epoch = cursor.desired_epoch();

        cursor.set_hardware_path_active(true);
        assert_eq!(cursor.desired_epoch(), active_epoch);

        cursor.set_hardware_path_active(false);
        assert_ne!(cursor.desired_epoch(), active_epoch);
    }

    #[test]
    fn cursor_epoch_wrap_skips_zero_and_submitted_epoch() {
        let mut cursor = test_cursor();
        cursor.desired_epoch = u64::MAX;
        cursor.submitted_epoch = 1;

        cursor.set_position(100, 200);

        assert_eq!(cursor.desired_epoch(), 2);
    }

    #[test]
    fn idle_theme_cursor_motion_opens_cursor_only_deadline_without_scene_damage() {
        let mut cursor = test_cursor();
        cursor.desired.visible = true;
        cursor.current.visible = true;
        cursor.set_position(100, 200);
        assert!(cursor.needs_submission());

        let mut arbitration = NativeCursorOutputArbitration::default();
        let scheduler = NativeFrameScheduler::new(165, 0);
        let (cursor_changed, deadline_due, cursor_work_pending) = update_cursor_output_arbitration(
            &mut arbitration,
            cursor.desired_epoch(),
            INITIAL_CURSOR_EPOCH,
            1_000,
            &scheduler,
            false,
            true,
        );

        assert!(cursor_changed);
        assert!(!deadline_due);
        assert!(!cursor_work_pending);
        let deadline = arbitration.deadline_ns().expect("cursor deadline is armed");
        let (_, deadline_due, cursor_work_pending) = update_cursor_output_arbitration(
            &mut arbitration,
            cursor.desired_epoch(),
            INITIAL_CURSOR_EPOCH,
            deadline,
            &scheduler,
            false,
            true,
        );
        assert!(deadline_due);
        assert!(cursor_work_pending);
    }

    #[test]
    fn atomic_cursor_state_uses_theme_hotspot() {
        let image =
            CompositorCursorImage::from_argb8888(vec![0xffff_ffff; 2 * 3], 2, 3, 1, 2).unwrap();
        let state = atomic_cursor_state_for_image(&image, Some(7));

        assert_eq!(state.hotspot_x, 1);
        assert_eq!(state.hotspot_y, 2);
        assert_eq!((state.width, state.height), (2, 3));
        assert_eq!(state.framebuffer_id, Some(7));
    }

    #[test]
    fn oversized_theme_cursor_falls_back_to_software_in_auto() {
        let image =
            CompositorCursorImage::from_argb8888(vec![0xffff_ffff; 65 * 64], 65, 64, 0, 0).unwrap();
        assert!(!cursor_image_fits_buffer(
            &image,
            NATIVE_HARDWARE_CURSOR_SIZE,
            64
        ));
    }

    #[test]
    fn oversized_theme_cursor_fails_in_hardware_mode() {
        let image =
            CompositorCursorImage::from_argb8888(vec![0xffff_ffff; 65 * 64], 65, 64, 0, 0).unwrap();
        assert!(validate_atomic_cursor_image(&image, NATIVE_HARDWARE_CURSOR_SIZE, 64).is_err());
    }

    #[test]
    fn client_cursor_transform_preserves_pixel_orientation_and_hotspot() {
        use wayland_server::protocol::wl_output::Transform;

        let (pixels, (width, height)) =
            transform_cursor_pixels(&[0, 1, 2, 3, 4, 5], 2, 3, Transform::_90).unwrap();
        assert_eq!((width, height), (3, 2));
        assert_eq!(pixels, vec![4, 2, 0, 5, 3, 1]);
        assert_eq!(
            normalize_cursor_hotspot(1, 2, 2, 3, 3, 2, 3, 2, Transform::_90),
            Some((0, 1))
        );
    }

    #[test]
    fn client_cursor_hotspot_outside_source_is_rejected() {
        use wayland_server::protocol::wl_output::Transform;

        assert_eq!(
            normalize_cursor_hotspot(2, 0, 2, 2, 2, 2, 2, 2, Transform::Normal),
            None
        );
    }

    #[test]
    fn client_cursor_image_key_changes_for_commit_and_hotspot() {
        let first = NativeCursorImageKey {
            surface_id: 7,
            buffer_id: 11,
            commit_sequence: 3,
            hotspot_x: 1,
            hotspot_y: 2,
            width: 32,
            height: 32,
            buffer_scale: 1,
            buffer_transform: 0,
        };
        let mut next = first;
        next.commit_sequence += 1;
        assert_ne!(first, next);
        next = first;
        next.hotspot_x += 1;
        assert_ne!(first, next);
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
