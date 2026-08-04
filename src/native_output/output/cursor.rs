use super::cursor_buffer::*;
use super::*;
use oblivion_one::cursor_theme::CompositorCursorImage;
use std::sync::Arc;

pub(crate) const NATIVE_HARDWARE_CURSOR_SIZE: u32 = 64;
const INITIAL_CURSOR_EPOCH: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorOutputIdentity {
    pub(crate) crtc_id: u32,
    pub(crate) mode_width: u32,
    pub(crate) mode_height: u32,
    pub(crate) output_transform: u32,
    pub(crate) output_scale_milli: u32,
}

impl CursorOutputIdentity {
    pub(crate) const fn new(crtc_id: u32, mode_width: u32, mode_height: u32) -> Self {
        Self {
            crtc_id,
            mode_width,
            mode_height,
            output_transform: 0,
            output_scale_milli: 1_000,
        }
    }

    #[allow(dead_code)]
    pub(crate) const fn with_transform_scale(
        crtc_id: u32,
        mode_width: u32,
        mode_height: u32,
        output_transform: u32,
        output_scale_milli: u32,
    ) -> Self {
        Self {
            crtc_id,
            mode_width,
            mode_height,
            output_transform,
            output_scale_milli,
        }
    }
}

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
    revisions: CursorRevisionTracker,
    hardware_path_active: bool,
    pub(crate) dirty: AtomicCursorDirty,
    pub(crate) counters: AtomicCursorCounters,
    plane_lifecycle: CursorPlaneLifecycle,
    capability_cache: crate::native_output::presentation::plane_policy::PlaneCapabilityCache,
    scheduled_test_policy: crate::native_output::presentation::plane_policy::KmsCursorTestPolicy,
    crtc_id: u32,
    mode_width: u32,
    mode_height: u32,
    output_transform: u32,
    output_scale_milli: u32,
    client_image_failure: Option<NativeCursorImageKey>,
    pending_token: Option<PageFlipToken>,
    pending_is_primary: bool,
    worker_queued: Option<WorkerQueuedCursorSubmission>,
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
        output: CursorOutputIdentity,
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
            revisions: CursorRevisionTracker::new(),
            hardware_path_active: false,
            dirty: AtomicCursorDirty::default(),
            counters: AtomicCursorCounters::default(),
            plane_lifecycle: CursorPlaneLifecycle::new(generation),
            capability_cache: Default::default(),
            scheduled_test_policy:
                crate::native_output::presentation::plane_policy::KmsCursorTestPolicy::Required,
            crtc_id: output.crtc_id,
            mode_width: output.mode_width,
            mode_height: output.mode_height,
            output_transform: output.output_transform,
            output_scale_milli: output.output_scale_milli,
            client_image_failure: None,
            pending_token: None,
            pending_is_primary: false,
            worker_queued: None,
            suspended_desired: None,
            drm_cleanup_armed: true,
        })
    }

    pub(crate) fn desired(&self) -> &AtomicCursorVisualState {
        &self.desired
    }

    pub(crate) fn pin_framebuffer_for(
        &self,
        state: &AtomicCursorVisualState,
    ) -> io::Result<CursorFramebufferPin> {
        let framebuffer = state
            .framebuffer_id
            .and_then(FramebufferId::new)
            .ok_or_else(|| io::Error::other("cursor assignment has no framebuffer"))?;
        self.resources.pin_framebuffer(framebuffer).ok_or_else(|| {
            io::Error::other(format!(
                "cursor framebuffer {} is not owned by this output",
                framebuffer.get()
            ))
        })
    }

    pub(crate) fn current(&self) -> &AtomicCursorVisualState {
        &self.current
    }

    pub(crate) fn presented_plane_state(
        &self,
    ) -> crate::native_output::presentation::plane::PresentedCursorState {
        use crate::native_output::presentation::plane::CursorCoupling;

        let coupling = if self.current.visible {
            CursorCoupling::IndependentPlane
        } else {
            CursorCoupling::Hidden
        };
        let presented = self.presented_plane_state_with(self.revisions.presented(), coupling);
        debug_assert!(presented.kms_equivalent_to(&self.current));
        presented
    }

    pub(crate) fn presented_plane_state_with(
        &self,
        revision: crate::native_output::presentation::plane::CursorRevision,
        coupling: crate::native_output::presentation::plane::CursorCoupling,
    ) -> crate::native_output::presentation::plane::PresentedCursorState {
        crate::native_output::presentation::plane::PresentedCursorState::from_atomic(
            revision,
            coupling,
            &self.current,
        )
    }

    pub(crate) const fn desired_epoch(&self) -> u64 {
        self.desired_epoch
    }

    pub(crate) const fn desired_revision(
        &self,
    ) -> crate::native_output::presentation::plane::CursorRevision {
        self.revisions.desired()
    }

    #[cfg(test)]
    pub(crate) const fn submitted_epoch(&self) -> u64 {
        self.submitted_epoch
    }

    #[cfg(test)]
    pub(crate) const fn worker_queued_epoch(&self) -> Option<u64> {
        match self.worker_queued.as_ref() {
            Some(queued) => Some(queued.cursor_epoch),
            None => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn queue_worker_submission(
        &mut self,
        transaction_id: crate::native_output::OutputTransactionId,
        token: PageFlipToken,
        cursor_epoch: u64,
        visual_state: AtomicCursorVisualState,
    ) -> io::Result<()> {
        self.queue_worker_submission_with_capability_key(
            transaction_id,
            token,
            cursor_epoch,
            visual_state,
            None,
        )
    }

    pub(crate) fn queue_worker_submission_with_capability_key(
        &mut self,
        transaction_id: crate::native_output::OutputTransactionId,
        token: PageFlipToken,
        cursor_epoch: u64,
        visual_state: AtomicCursorVisualState,
        capability_key: Option<CursorCapabilityKey>,
    ) -> io::Result<()> {
        if self.worker_queued.is_some() {
            return Err(io::Error::other(
                "Atomic cursor worker submission already queued",
            ));
        }
        if cursor_epoch != self.desired_epoch {
            return Err(io::Error::other(
                "Atomic cursor worker submission has a stale desired epoch",
            ));
        }
        self.queue_owned_worker_submission_with_capability_key(
            transaction_id,
            token,
            cursor_epoch,
            self.desired_revision(),
            visual_state,
            capability_key,
        )
    }

    #[cfg(test)]
    pub(crate) fn queue_owned_worker_submission(
        &mut self,
        transaction_id: crate::native_output::OutputTransactionId,
        token: PageFlipToken,
        cursor_epoch: u64,
        revision: crate::native_output::presentation::plane::CursorRevision,
        visual_state: AtomicCursorVisualState,
    ) -> io::Result<()> {
        self.queue_owned_worker_submission_with_capability_key(
            transaction_id,
            token,
            cursor_epoch,
            revision,
            visual_state,
            None,
        )
    }

    pub(crate) fn queue_owned_worker_submission_with_capability_key(
        &mut self,
        transaction_id: crate::native_output::OutputTransactionId,
        token: PageFlipToken,
        cursor_epoch: u64,
        revision: crate::native_output::presentation::plane::CursorRevision,
        visual_state: AtomicCursorVisualState,
        capability_key: Option<CursorCapabilityKey>,
    ) -> io::Result<()> {
        if self.worker_queued.is_some() {
            return Err(io::Error::other(
                "Atomic cursor worker submission already queued",
            ));
        }
        self.worker_queued = Some(WorkerQueuedCursorSubmission {
            transaction_id,
            token,
            cursor_epoch,
            revision,
            visual_state,
            capability_key,
        });
        Ok(())
    }

    pub(crate) fn take_worker_submission(
        &mut self,
        transaction_id: crate::native_output::OutputTransactionId,
        token: PageFlipToken,
        cursor_epoch: u64,
    ) -> io::Result<WorkerQueuedCursorSubmission> {
        let Some(queued) = self.worker_queued.as_ref() else {
            return Err(io::Error::other(
                "Atomic cursor worker submission is not queued",
            ));
        };
        if queued.transaction_id != transaction_id
            || queued.token != token
            || queued.cursor_epoch != cursor_epoch
        {
            return Err(io::Error::other(
                "Atomic cursor worker submission identity mismatch",
            ));
        }
        self.worker_queued
            .take()
            .ok_or_else(|| io::Error::other("Atomic cursor worker submission disappeared"))
    }

    pub(crate) fn cancel_worker_submission(
        &mut self,
        transaction_id: crate::native_output::OutputTransactionId,
        token: PageFlipToken,
        cursor_epoch: u64,
    ) -> io::Result<()> {
        let _ = self.take_worker_submission(transaction_id, token, cursor_epoch)?;
        Ok(())
    }

    fn advance_desired_epoch(&mut self) {
        self.desired_epoch = next_cursor_epoch(self.desired_epoch, self.submitted_epoch);
    }

    fn advance_image_revision(&mut self) {
        self.advance_desired_epoch();
        self.revisions.advance_image();
    }

    fn advance_motion_revision(&mut self) {
        self.advance_desired_epoch();
        self.revisions.advance_motion();
    }

    fn advance_visibility_revision(&mut self) {
        self.advance_desired_epoch();
        self.revisions.advance_visibility();
    }

    pub(crate) fn revision_for_legacy_epoch(
        &self,
        cursor_epoch: u64,
    ) -> crate::native_output::presentation::plane::CursorRevision {
        if cursor_epoch == self.desired_epoch {
            self.revisions.desired()
        } else {
            let epoch = std::num::NonZeroU64::new(cursor_epoch)
                .expect("queued cursor epoch must be nonzero");
            crate::native_output::presentation::plane::CursorRevision::from_legacy_epoch(epoch)
        }
    }

    pub(crate) fn set_hardware_path_active(&mut self, active: bool) {
        if self.hardware_path_active != active {
            self.hardware_path_active = active;
            self.advance_visibility_revision();
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
        self.revisions.mark_initial_presented();
        self.dirty = AtomicCursorDirty::default();
        self.plane_lifecycle.confirm_initial_clear(self.generation);
    }

    pub(crate) fn set_position(&mut self, x: i32, y: i32) {
        if self.desired.x != x || self.desired.y != y {
            self.desired.x = x;
            self.desired.y = y;
            self.advance_motion_revision();
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
        let visible = visible && !self.capability_quarantined();
        if self.desired.visible != visible {
            self.desired.visible = visible;
            self.advance_visibility_revision();
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

    pub(crate) fn begin_submission_at_revision_with_capability_key(
        &mut self,
        token: PageFlipToken,
        state: AtomicCursorVisualState,
        cursor_epoch: u64,
        revision: crate::native_output::presentation::plane::CursorRevision,
        capability_key: Option<CursorCapabilityKey>,
    ) -> AtomicCursorVisualState {
        if self.dirty.position {
            self.counters.position_submissions =
                self.counters.position_submissions.saturating_add(1);
        }
        self.submitted = state.clone();
        self.submitted_epoch = cursor_epoch;
        self.revisions.mark_submitted(revision);
        self.pending_token = Some(token);
        self.pending_is_primary = false;
        self.dirty = AtomicCursorDirty::default();
        self.counters.updates_submitted = self.counters.updates_submitted.saturating_add(1);
        if state.visible
            && let Some(key) = capability_key
        {
            self.mark_capability_proven(key);
        }
        state
    }

    pub(crate) fn complete_submission(
        &mut self,
        token: PageFlipToken,
        generation: u64,
    ) -> io::Result<()> {
        self.prepare_submission_completion(token, generation)?;
        self.commit_submission_completion(token, generation);
        Ok(())
    }

    pub(crate) fn prepare_submission_completion(
        &self,
        token: PageFlipToken,
        generation: u64,
    ) -> io::Result<()> {
        if generation != self.generation {
            return Err(io::Error::other("stale Atomic cursor DRM generation"));
        }
        if self.pending_token != Some(token) {
            return Err(io::Error::other("stale Atomic cursor pageflip token"));
        }
        Ok(())
    }

    pub(crate) fn commit_submission_completion(&mut self, token: PageFlipToken, generation: u64) {
        debug_assert_eq!(generation, self.generation);
        debug_assert_eq!(self.pending_token, Some(token));
        self.pending_token = None;
        self.pending_is_primary = false;
        self.current = self.submitted.clone();
        self.revisions.mark_presented();
        self.counters.updates_completed = self.counters.updates_completed.saturating_add(1);
        let keep = [
            self.desired.framebuffer_id,
            self.submitted.framebuffer_id,
            self.current.framebuffer_id,
        ];
        self.resources.retire_safe(&keep);
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
        self.begin_primary_submission_at_epoch(token, state, self.desired_epoch);
    }

    pub(crate) fn begin_primary_submission_at_epoch(
        &mut self,
        token: PageFlipToken,
        state: AtomicCursorVisualState,
        cursor_epoch: u64,
    ) {
        let revision = self.revision_for_legacy_epoch(cursor_epoch);
        self.begin_primary_submission_at_revision(token, state, cursor_epoch, revision);
    }

    pub(crate) fn begin_primary_submission_at_revision(
        &mut self,
        token: PageFlipToken,
        state: AtomicCursorVisualState,
        cursor_epoch: u64,
        revision: crate::native_output::presentation::plane::CursorRevision,
    ) {
        self.begin_primary_submission_at_revision_with_capability_key(
            token,
            state,
            cursor_epoch,
            revision,
            None,
        );
    }

    pub(crate) fn begin_primary_submission_at_revision_with_capability_key(
        &mut self,
        token: PageFlipToken,
        state: AtomicCursorVisualState,
        cursor_epoch: u64,
        revision: crate::native_output::presentation::plane::CursorRevision,
        capability_key: Option<CursorCapabilityKey>,
    ) {
        self.counters.primary_submissions = self.counters.primary_submissions.saturating_add(1);
        self.submitted = state.clone();
        self.submitted_epoch = cursor_epoch;
        self.revisions.mark_submitted(revision);
        self.pending_token = Some(token);
        self.pending_is_primary = true;
        self.dirty = AtomicCursorDirty::default();
        self.counters.updates_submitted = self.counters.updates_submitted.saturating_add(1);
        if state.visible
            && let Some(key) = capability_key
        {
            self.mark_capability_proven(key);
        }
    }

    #[cfg(test)]
    pub(crate) fn mark_capability_quarantined(&mut self) {
        if let Some(key) = self.capability_key_for(&self.desired.clone()) {
            self.quarantine_capability(key, CursorQuarantineReason::PermanentSubmitRejection);
        }
    }

    pub(crate) fn note_test_failure_for(&mut self, key: Option<CursorCapabilityKey>) {
        self.counters.test_failures = self.counters.test_failures.saturating_add(1);
        if let Some(key) = key {
            self.quarantine_capability(key, CursorQuarantineReason::TestOnlyRejected);
        }
    }

    pub(crate) fn note_submit_failure_for(&mut self, key: Option<CursorCapabilityKey>) {
        self.counters.submit_failures = self.counters.submit_failures.saturating_add(1);
        if let Some(key) = key {
            self.quarantine_capability(key, CursorQuarantineReason::PermanentSubmitRejection);
        }
    }

    pub(crate) fn note_software_fallback(&mut self) {
        self.counters.software_fallbacks = self.counters.software_fallbacks.saturating_add(1);
    }

    pub(crate) fn note_composed_software_fallback(&mut self) {
        self.counters.composed_cursor_fallbacks =
            self.counters.composed_cursor_fallbacks.saturating_add(1);
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

    pub(crate) fn theme_image_matches(&self, image: &Arc<CompositorCursorImage>) -> bool {
        Arc::ptr_eq(&self.theme_image, image)
    }

    pub(crate) fn replace_theme_image(
        &mut self,
        file: &fs::File,
        image: Arc<CompositorCursorImage>,
        generation: u64,
    ) -> io::Result<()> {
        if !Arc::ptr_eq(&self.theme_image, &image) {
            self.resources.retire_theme_cache();
        }
        self.replace_image_with_source(file, image.clone(), NativeCursorSourceKey::Theme)?;
        self.theme_image = image;
        self.desired.image_generation = generation;
        Ok(())
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
        self.source_key = source_key;
        self.client_image_failure = None;
        self.desired.framebuffer_id = framebuffer_id;
        self.desired.hotspot_x = self.image.hotspot_x;
        self.desired.hotspot_y = self.image.hotspot_y;
        self.desired.width = self.image.width;
        self.desired.height = self.image.height;
        self.desired.image_generation = self.desired.image_generation.saturating_add(1);
        self.advance_image_revision();
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

    pub(crate) fn abandon_pageflip_for_recovery(&mut self) {
        self.pending_token = None;
        self.pending_is_primary = false;
        self.worker_queued = None;
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
        self.plane_lifecycle.rearm_generation(generation);
        self.capability_cache.invalidate_generation(generation);
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
            self.advance_image_revision();
        }
        self.desired = restored.clone();
        self.submitted = AtomicCursorVisualState::hidden(self.image.width, self.image.height);
        self.submitted.framebuffer_id = framebuffer_id;
        self.submitted_epoch = 0;
        self.current = self.submitted.clone();
        self.pending_token = None;
        self.pending_is_primary = false;
        self.dirty = AtomicCursorDirty::default();
        self.client_image_failure = None;
        Ok(restored)
    }

    pub(crate) fn capability_key_for(
        &self,
        state: &AtomicCursorVisualState,
    ) -> Option<crate::native_output::presentation::plane_policy::CursorCapabilityKey> {
        use crate::native_output::presentation::plane_policy::{
            CursorCapabilityKey, CursorGeometryClass, CursorGeometryInput,
            normalize_cursor_geometry,
        };

        let geometry = normalize_cursor_geometry(CursorGeometryInput {
            pointer_x: state.x,
            pointer_y: state.y,
            hotspot_x: state.hotspot_x,
            hotspot_y: state.hotspot_y,
            cursor_width: state.width,
            cursor_height: state.height,
            output_width: self.mode_width,
            output_height: self.mode_height,
        })?;
        Some(CursorCapabilityKey {
            output_generation: self.generation,
            crtc_id: self.crtc_id,
            plane_id: self.plane.plane_id,
            mode_width: self.mode_width,
            mode_height: self.mode_height,
            output_transform: self.output_transform,
            output_scale_milli: self.output_scale_milli,
            format: self.plane.format_modifier.fourcc,
            modifier: self.plane.format_modifier.modifier,
            cursor_width: state.width,
            cursor_height: state.height,
            hotspot_property_available: false,
            geometry_class: geometry.class,
            source_x: geometry.source.x,
            source_y: geometry.source.y,
            source_width: geometry.source.width,
            source_height: geometry.source.height,
            // For fully-visible motion, the destination origin is a
            // position-only property and must not invalidate the capability
            // proof. Clipped destinations retain their exact boundary so an
            // edge/corner crop cannot reuse a proof for another crop.
            destination_x: if geometry.class == CursorGeometryClass::FullyVisible {
                0
            } else {
                geometry.destination.x
            },
            destination_y: if geometry.class == CursorGeometryClass::FullyVisible {
                0
            } else {
                geometry.destination.y
            },
            destination_width: geometry.destination.width,
            destination_height: geometry.destination.height,
        })
    }

    #[cfg(test)]
    pub(crate) fn capability_status(&self, key: CursorCapabilityKey) -> CursorCapabilityStatus {
        self.capability_cache.status(key)
    }

    pub(crate) fn capability_cache(&self) -> &PlaneCapabilityCache {
        &self.capability_cache
    }

    pub(crate) fn set_scheduled_test_policy(
        &mut self,
        policy: crate::native_output::presentation::plane_policy::KmsCursorTestPolicy,
    ) {
        self.scheduled_test_policy = policy;
    }

    pub(crate) const fn scheduled_test_policy(
        &self,
    ) -> crate::native_output::presentation::plane_policy::KmsCursorTestPolicy {
        self.scheduled_test_policy
    }

    pub(crate) fn mark_capability_proven(&mut self, key: CursorCapabilityKey) {
        self.capability_cache.mark_proven(key);
    }

    pub(crate) fn quarantine_capability(
        &mut self,
        key: CursorCapabilityKey,
        reason: CursorQuarantineReason,
    ) {
        self.capability_cache.quarantine(key, reason);
    }

    pub(crate) fn capability_quarantined(&self) -> bool {
        self.capability_key_for(&self.desired).is_some_and(|key| {
            matches!(
                self.capability_cache.status(key),
                CursorCapabilityStatus::Quarantined { .. }
            )
        })
    }

    #[cfg(test)]
    pub(crate) fn current_capability_proven(&self) -> bool {
        self.capability_key_for(&self.desired)
            .is_some_and(|key| self.capability_cache.status(key) == CursorCapabilityStatus::Proven)
    }

    pub(crate) fn rearm_generation(&mut self, generation: u64) {
        self.generation = generation;
        self.plane_lifecycle.rearm_generation(generation);
        self.capability_cache.invalidate_generation(generation);
        self.pending_token = None;
        self.pending_is_primary = false;
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
#[path = "cursor_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) fn test_cursor_for_worker() -> NativeAtomicCursor {
    tests::test_cursor_for_worker()
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
