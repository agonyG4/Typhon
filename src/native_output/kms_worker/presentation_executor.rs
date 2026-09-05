use super::super::payload::{KmsCursorUpdate, KmsPrimaryUpdate};
use super::{
    AtomicKmsWorkerExecutor, KmsCommitExecutor, KmsWorkerSubmission, KmsWorkerSubmitFailure,
};
use crate::native_output::kms_worker::KmsCommitJob;
use crate::native_output::presentation::cursor_trace::{
    CursorKmsAssignment, CursorKmsSubmitContext, trace_cursor_kms_submit,
};
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
        if touch_cursor {
            let cursor_epoch = match job.kind {
                crate::native_output::runtime::AtomicCommitKind::PlaneDelta {
                    cursor_epoch,
                    ..
                } => Some(cursor_epoch),
                crate::native_output::runtime::AtomicCommitKind::CompositedPrimary { .. }
                | crate::native_output::runtime::AtomicCommitKind::DirectPrimary { .. } => None,
            };
            trace_cursor_kms_submit(
                self.submitter.pipeline(),
                match &job.cursor {
                    KmsCursorUpdate::Set(state) => CursorKmsAssignment::Set(state),
                    KmsCursorUpdate::Disable => CursorKmsAssignment::Disable,
                    KmsCursorUpdate::Unchanged => CursorKmsAssignment::Unchanged,
                },
                CursorKmsSubmitContext {
                    output_generation: job.output_generation,
                    transaction_id: Some(job.transaction_id),
                    token: job.token,
                    crtc_id: job.crtc_id,
                    cursor_epoch,
                    cursor_revision: job.owners.cursor().map(|owner| owner.revision),
                    submission_kind: if matches!(&job.primary, KmsPrimaryUpdate::Unchanged) {
                        "cursor_only"
                    } else {
                        "primary_plus_cursor"
                    },
                    transport: "worker",
                    delivery: job.cursor_delivery,
                },
            );
        }
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
