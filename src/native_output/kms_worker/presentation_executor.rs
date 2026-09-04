use super::super::payload::{KmsCursorUpdate, KmsPrimaryUpdate};
use super::{
    AtomicKmsWorkerExecutor, KmsCommitExecutor, KmsWorkerSubmission, KmsWorkerSubmitFailure,
};
use crate::native_output::kms_worker::KmsCommitJob;
use oblivion_one::native::kms::{AtomicKmsError, AtomicKmsErrorKind};
use std::os::fd::{AsRawFd, BorrowedFd};

impl KmsCommitExecutor for AtomicKmsWorkerExecutor {
    fn test_only(&self, job: &KmsCommitJob) -> Result<(), KmsWorkerSubmitFailure> {
        let presentation_mode = job.presentation_mode();
        let content_type = job.content_type();
        if presentation_mode.is_async() && matches!(&job.primary, KmsPrimaryUpdate::Unchanged) {
            return Err(KmsWorkerSubmitFailure {
                error: AtomicKmsError::new(
                    AtomicKmsErrorKind::Unsupported,
                    "Async TEST_ONLY cannot be a cursor-only update",
                ),
            });
        }
        let touch_cursor = !matches!(&job.cursor, KmsCursorUpdate::Unchanged);
        let cursor = match &job.cursor {
            KmsCursorUpdate::Set(state) => Some(state),
            KmsCursorUpdate::Disable | KmsCursorUpdate::Unchanged => None,
        };
        let test = match &job.primary {
            KmsPrimaryUpdate::Framebuffer { framebuffer, .. } if touch_cursor => {
                self.submitter.test_primary_with_presentation(
                    *framebuffer,
                    job.token,
                    cursor,
                    presentation_mode,
                    content_type,
                )
            }
            KmsPrimaryUpdate::Framebuffer { framebuffer, .. } => self
                .submitter
                .test_primary_without_cursor_with_presentation(
                    *framebuffer,
                    job.token,
                    presentation_mode,
                    content_type,
                ),
            KmsPrimaryUpdate::Unchanged => self.submitter.test_cursor(cursor),
        };
        test.map(|_| ())
            .map_err(|error| KmsWorkerSubmitFailure { error })
    }

    fn submit(&self, job: &KmsCommitJob) -> Result<KmsWorkerSubmission, KmsWorkerSubmitFailure> {
        let presentation_mode = job.presentation_mode();
        let content_type = job.content_type();
        if presentation_mode.is_async() && matches!(&job.primary, KmsPrimaryUpdate::Unchanged) {
            return Err(KmsWorkerSubmitFailure {
                error: AtomicKmsError::new(
                    AtomicKmsErrorKind::Unsupported,
                    "Async pageflip cannot be a cursor-only update",
                ),
            });
        }
        let touch_cursor = !matches!(&job.cursor, KmsCursorUpdate::Unchanged);
        let cursor = match &job.cursor {
            KmsCursorUpdate::Set(state) => Some(state),
            KmsCursorUpdate::Disable | KmsCursorUpdate::Unchanged => None,
        };
        crate::pointer_debug::cursor_presentation_log_lazy(|| {
            let cursor_fields = match &job.cursor {
                KmsCursorUpdate::Set(state) => format!(
                    "enabled=true fb_id={:?} crtc_id_property={:?} src=(0,0,{}, {}) crtc=({},{},{},{}) hotspot=({},{}) image_generation={}",
                    state.framebuffer_id,
                    self.submitter
                        .pipeline()
                        .cursor_plane
                        .as_ref()
                        .map(|plane| plane.crtc_id),
                    u64::from(state.width) << 16,
                    u64::from(state.height) << 16,
                    state.x.saturating_sub(state.hotspot_x),
                    state.y.saturating_sub(state.hotspot_y),
                    state.width,
                    state.height,
                    state.hotspot_x,
                    state.hotspot_y,
                    state.image_generation
                ),
                KmsCursorUpdate::Disable => String::from(
                    "enabled=false fb_id=0 crtc_id_property=0 src=(0,0,0,0) crtc=(disabled)",
                ),
                KmsCursorUpdate::Unchanged => String::from("enabled=unchanged"),
            };
            format!(
                "event=cursor_kms_submit output_generation={} transaction_id={} token={} crtc_id={} plane_id={:?} cursor_revision={:?} cursor_epoch=not_available submission_kind={:?} delivery={:?} {}",
                job.output_generation,
                job.transaction_id.get(),
                job.token.get(),
                job.crtc_id,
                self.submitter
                    .pipeline()
                    .cursor_plane
                    .as_ref()
                    .map(|plane| plane.plane_id),
                job.owners.cursor().map(|owner| owner.revision),
                job.kind,
                job.cursor_delivery,
                cursor_fields
            )
        });
        let input_fence = match &job.primary {
            KmsPrimaryUpdate::Framebuffer { in_fence, .. } if !presentation_mode.is_async() => {
                in_fence.as_ref().map(|fence| {
                    // SAFETY: the job-owned fence remains alive through the ioctl.
                    unsafe { BorrowedFd::borrow_raw(fence.as_raw_fd()) }
                })
            }
            _ => None,
        };
        let submission = match &job.primary {
            KmsPrimaryUpdate::Framebuffer {
                framebuffer,
                request_out_fence,
                ..
            } if touch_cursor => self.submitter.submit_primary_with_presentation(
                *framebuffer,
                job.token,
                cursor,
                input_fence,
                *request_out_fence,
                false,
                presentation_mode,
                content_type,
            ),
            KmsPrimaryUpdate::Framebuffer {
                framebuffer,
                request_out_fence,
                ..
            } => self
                .submitter
                .submit_primary_without_cursor_with_presentation(
                    *framebuffer,
                    job.token,
                    input_fence,
                    *request_out_fence,
                    false,
                    presentation_mode,
                    content_type,
                ),
            KmsPrimaryUpdate::Unchanged => self.submitter.submit_cursor(cursor, job.token, false),
        };
        submission
            .map(|submission| KmsWorkerSubmission {
                out_fence: submission.out_fence,
            })
            .map_err(|error| KmsWorkerSubmitFailure { error })
    }
}
