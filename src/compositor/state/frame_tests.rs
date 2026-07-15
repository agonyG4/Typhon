use super::super::*;

#[cfg(test)]
mod frame_consumption_tests {
    use super::*;

    #[test]
    fn empty_submitted_frame_batch_is_still_owned_until_completion() {
        let mut state = CompositorState::default();
        state.capture_frame_callbacks_for_render();
        state.mark_prepared_frame_submitted();

        assert!(state.has_submitted_frame_batch());
        state.complete_pending_presentation_feedbacks(
            FramePresentation::software_now(state.presentation_clock).unwrap(),
        );
        assert!(!state.has_submitted_frame_batch());
        assert!(state.frame_batches.is_empty());
    }

    #[test]
    fn prepare_publication_does_not_create_a_submitted_frame_batch() {
        let mut state = CompositorState::default();
        state.commit_ready_explicit_sync_buffers();
        assert!(!state.has_submitted_frame_batch());
    }

    #[test]
    fn empty_frame_batch_is_explicit_and_registry_is_bounded_to_two() {
        let mut state = CompositorState::default();
        let first = state.take_frame_batch_for_render(10);
        let second = state.take_frame_batch_for_render(11);
        assert_eq!(state.frame_batches.len(), 2);
        assert!(state.frame_batches[&first].callbacks.is_empty());
        assert!(
            state.frame_batches[&second]
                .presentation_feedbacks
                .is_empty()
        );

        let overflow = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.take_frame_batch_for_render(12)
        }));
        assert!(overflow.is_err());
        assert_eq!(state.frame_batches.len(), 2);
    }

    #[test]
    fn unrelated_completion_cannot_consume_ready_frame_batch() {
        let mut state = CompositorState::default();
        let submitted = state.take_frame_batch_for_render(20);
        let ready = state.take_frame_batch_for_render(21);
        let presentation = FramePresentation::software_now(state.presentation_clock).unwrap();

        state.complete_presented_frame_batch(20, submitted, presentation);

        assert!(!state.frame_batches.contains_key(&submitted));
        assert!(state.frame_batches.contains_key(&ready));
        state.restore_frame_batch_after_render_failure(ready);
        assert!(state.frame_batches.is_empty());
    }

    #[test]
    fn mismatched_frame_and_batch_identity_completes_nothing() {
        let mut state = CompositorState::default();
        let batch = state.take_frame_batch_for_render(30);
        let presentation = FramePresentation::software_now(state.presentation_clock).unwrap();

        let mismatch = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.complete_presented_frame_batch(31, batch, presentation)
        }));

        assert!(mismatch.is_err());
        assert!(state.frame_batches.contains_key(&batch));
    }

    fn test_dmabuf_release(point: u64) -> SurfaceBufferRelease {
        SurfaceBufferRelease::ExplicitSync(ExplicitSyncPoint::for_tests(99, point))
    }

    fn test_dmabuf_points(releases: &[SurfaceBufferRelease]) -> Vec<u64> {
        releases
            .iter()
            .map(|release| match release {
                SurfaceBufferRelease::ExplicitSync(point) => point.point,
                SurfaceBufferRelease::WlBuffer(_) => panic!("test release is not explicit sync"),
            })
            .collect()
    }

    #[test]
    fn frame_batch_captures_only_releases_pending_at_capture_and_restores_order() {
        let mut state = CompositorState::default();
        state.queue_dmabuf_buffer_release(test_dmabuf_release(1));
        state.queue_dmabuf_buffer_release(test_dmabuf_release(2));
        let batch = state.take_frame_batch_for_render(40);
        state.queue_dmabuf_buffer_release(test_dmabuf_release(3));

        assert_eq!(
            test_dmabuf_points(&state.frame_batches[&batch].dmabuf_releases_to_complete_on_present),
            vec![1, 2]
        );
        assert_eq!(
            test_dmabuf_points(&state.pending_dmabuf_buffer_releases),
            vec![3]
        );

        state.restore_frame_batch_after_render_failure(batch);
        assert_eq!(
            test_dmabuf_points(&state.pending_dmabuf_buffer_releases),
            vec![1, 2, 3]
        );
        assert_eq!(state.buffer_release_metrics.buffer_releases_restored, 2);
    }

    #[test]
    fn pending_and_ready_batches_keep_release_sets_disjoint() {
        let mut state = CompositorState::default();
        state.queue_dmabuf_buffer_release(test_dmabuf_release(10));
        let pending = state.take_frame_batch_for_render(50);
        state.queue_dmabuf_buffer_release(test_dmabuf_release(11));
        let ready = state.take_frame_batch_for_render(51);

        assert_eq!(
            test_dmabuf_points(
                &state.frame_batches[&pending].dmabuf_releases_to_complete_on_present
            ),
            vec![10]
        );
        assert_eq!(
            test_dmabuf_points(&state.frame_batches[&ready].dmabuf_releases_to_complete_on_present),
            vec![11]
        );

        let presentation = FramePresentation::software_now(state.presentation_clock).unwrap();
        state.complete_presented_frame_batch(50, pending, presentation);
        assert!(state.frame_batches.contains_key(&ready));
        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
        assert_eq!(
            test_dmabuf_points(&state.frame_batches[&ready].dmabuf_releases_to_complete_on_present),
            vec![11]
        );
        state.restore_frame_batch_after_render_failure(ready);
    }

    #[test]
    fn frame_batch_completes_all_owned_dmabuf_releases_on_its_matching_presentation() {
        let mut state = CompositorState::default();
        state.queue_dmabuf_buffer_release(test_dmabuf_release(20));
        state.queue_dmabuf_buffer_release(test_dmabuf_release(21));
        let batch = state.take_frame_batch_for_render(60);
        let presentation = FramePresentation::software_now(state.presentation_clock).unwrap();
        state.complete_presented_frame_batch(60, batch, presentation);

        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 2);
        assert_eq!(state.buffer_release_metrics.buffer_releases_deferred, 0);
    }

    #[test]
    fn direct_frame_batch_completion_releases_once_and_rejects_duplicate_completion() {
        let mut state = CompositorState::default();
        state.queue_dmabuf_buffer_release(test_dmabuf_release(25));
        let batch = state.take_frame_batch_for_render(65);
        let presentation =
            FramePresentation::synchronized(state.presentation_clock, 1, 0, 1).unwrap();

        state.complete_direct_presented_frame_batch(65, batch, 7, presentation);
        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
        assert!(state.frame_batches.is_empty());

        let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.complete_direct_presented_frame_batch(65, batch, 7, presentation);
        }));
        assert!(duplicate.is_err());
        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
    }

    #[test]
    fn failed_frame_retains_release_until_safe_teardown_without_duplication() {
        let mut state = CompositorState::default();
        state.queue_dmabuf_buffer_release(test_dmabuf_release(30));
        let batch = state.take_frame_batch_for_render(70);
        state.discard_frame_batch(batch, FrameBatchDiscardReason::FatalOutputFailure);

        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 0);
        assert_eq!(state.retired_frame_batches.len(), 1);
        state.complete_frame_batch_after_safe_abandonment(
            batch,
            FrameBatchDiscardReason::OutputDestroyed,
        );
        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
        assert!(state.retired_frame_batches.is_empty());
        state.release_client_buffers_for_shutdown();
        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
    }

    #[test]
    fn three_buffer_terminal_sequence_releases_both_spares_without_a_drain_frame() {
        let mut state = CompositorState::default();
        // A is sampled. B replaces A, then C replaces B before output capture.
        state.queue_dmabuf_buffer_release(test_dmabuf_release(100));
        state.queue_dmabuf_buffer_release(test_dmabuf_release(101));
        let frame = state.take_frame_batch_for_render(1745);
        state.complete_presented_frame_batch(
            1745,
            frame,
            FramePresentation::software_now(state.presentation_clock).unwrap(),
        );

        assert_eq!(state.buffer_release_metrics.buffer_releases_captured, 2);
        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 2);
        assert!(state.pending_dmabuf_buffer_releases.is_empty());
        // Either released spare can be used immediately; no synthetic presentation intervenes.
        state.queue_dmabuf_buffer_release(test_dmabuf_release(102));
        assert_eq!(state.pending_dmabuf_buffer_releases.len(), 1);
    }

    #[test]
    fn release_queued_after_capture_is_not_completed_by_the_earlier_frame() {
        let mut state = CompositorState::default();
        state.queue_dmabuf_buffer_release(test_dmabuf_release(200));
        let frame_n = state.take_frame_batch_for_render(80);
        state.queue_dmabuf_buffer_release(test_dmabuf_release(201));

        state.complete_presented_frame_batch(
            80,
            frame_n,
            FramePresentation::software_now(state.presentation_clock).unwrap(),
        );

        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
        assert_eq!(
            test_dmabuf_points(&state.pending_dmabuf_buffer_releases),
            vec![201]
        );
    }

    #[test]
    fn reused_buffer_with_distinct_explicit_sync_points_is_not_a_duplicate() {
        let mut state = CompositorState::default();
        let first = test_dmabuf_release(300);
        let second = match &first {
            SurfaceBufferRelease::ExplicitSync(point) => {
                SurfaceBufferRelease::ExplicitSync(ExplicitSyncPoint {
                    timeline: point.timeline.clone(),
                    point: 301,
                })
            }
            SurfaceBufferRelease::WlBuffer(_) => unreachable!(),
        };
        state.queue_dmabuf_buffer_release(first);
        state.queue_dmabuf_buffer_release(second);

        assert_eq!(state.pending_dmabuf_buffer_releases.len(), 2);
        assert_eq!(
            state
                .buffer_release_metrics
                .buffer_release_duplicate_attempts,
            0
        );
    }

    #[test]
    fn exact_explicit_sync_point_queued_twice_is_a_true_duplicate() {
        let mut state = CompositorState::default();
        let release = test_dmabuf_release(400);
        state.queue_dmabuf_buffer_release(release.clone());
        state.queue_dmabuf_buffer_release(release);

        assert_eq!(state.pending_dmabuf_buffer_releases.len(), 1);
        assert_eq!(
            state
                .buffer_release_metrics
                .buffer_release_duplicate_attempts,
            1
        );
    }

    #[test]
    fn adversarial_three_buffer_client_completes_one_thousand_presentations() {
        let mut state = CompositorState::default();
        let mut reusable = [false, true, true];
        let mut current = 0usize;
        let mut next_release_point = 1_000u64;

        for frame_id in 1..=1_000 {
            let mut released_this_frame = Vec::with_capacity(2);
            for _ in 0..2 {
                let Some(next) = reusable.iter().position(|available| *available) else {
                    panic!("three-buffer client starved before presentation {frame_id}");
                };
                reusable[next] = false;
                released_this_frame.push(current);
                current = next;
                state.queue_dmabuf_buffer_release(test_dmabuf_release(next_release_point));
                next_release_point += 1;
            }

            let completed_before = state.buffer_release_metrics.buffer_releases_completed;
            let batch = state.take_frame_batch_for_render(frame_id);
            state.complete_presented_frame_batch(
                frame_id,
                batch,
                FramePresentation::software_now(state.presentation_clock).unwrap(),
            );
            assert_eq!(
                state.buffer_release_metrics.buffer_releases_completed - completed_before,
                released_this_frame.len() as u64
            );
            for released in released_this_frame {
                reusable[released] = true;
            }
            reusable[current] = false;
        }

        assert_eq!(state.buffer_release_metrics.buffer_releases_captured, 2_000);
        assert_eq!(
            state.buffer_release_metrics.buffer_releases_completed,
            2_000
        );
        assert_eq!(
            state
                .buffer_release_metrics
                .buffer_release_duplicate_attempts,
            0
        );
        assert!(state.frame_batches.is_empty());
        assert!(state.pending_dmabuf_buffer_releases.is_empty());
    }
}
