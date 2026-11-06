//! Per-output frame-batch ownership.
//!
//! Each native output head presents on its own vblank. The compositor keeps
//! one prepared and one submitted frame batch per output: the runtime enters
//! an output before running that head's presentation phase, which swaps the
//! output's batch slots into the legacy single-output slots every frame-batch
//! API already uses. Frame callbacks are captured only for surfaces whose
//! primary output (largest visible intersection, ties to the earliest
//! `OutputId`) is the presenting output; surfaces on no output follow the
//! native output. With a single logical output nothing is filtered.

use super::*;

type OutputFrameSlots = (
    Option<CompositorFrameBatchId>,
    Option<CompositorFrameBatchId>,
);

impl CompositorState {
    /// Presentation key of an output: `None` is the native output.
    fn output_presentation_key(&self, output_id: OutputId) -> Option<OutputId> {
        (Some(output_id) != self.native_output_id).then_some(output_id)
    }

    pub(in crate::compositor) fn enter_output_presentation(&mut self, output_id: OutputId) {
        let key = self.output_presentation_key(output_id);
        if key == self.presenting_output {
            return;
        }
        let current: OutputFrameSlots = (
            self.legacy_prepared_frame_batch.take(),
            self.legacy_submitted_frame_batch.take(),
        );
        if current != (None, None) {
            self.parked_output_frame_slots
                .insert(self.presenting_output, current);
        }
        let (prepared, submitted) = self
            .parked_output_frame_slots
            .remove(&key)
            .unwrap_or_default();
        self.legacy_prepared_frame_batch = prepared;
        self.legacy_submitted_frame_batch = submitted;
        self.presenting_output = key;
    }

    /// Whether the presenting output's batch takes buffer releases. Releases
    /// stay with the native output so a second head cannot release a buffer
    /// the native head has not presented past.
    pub(in crate::compositor) fn presenting_output_owns_dmabuf_releases(&self) -> bool {
        self.presenting_output.is_none()
    }

    /// Live frame batches may be one prepared plus one submitted per output.
    pub(in crate::compositor) fn frame_batch_capacity(&self) -> usize {
        2 * self.logical_output_ids.len().max(1)
    }

    fn frame_callbacks_are_output_scoped(&self) -> bool {
        self.logical_output_ids.len() > 1
    }

    /// Presentation key of the output that owns `surface_id`'s frame work.
    fn surface_frame_output(&mut self, surface_id: u32) -> Option<OutputId> {
        let outputs = self
            .surface_output_memberships
            .get(&surface_id)
            .map(|membership| {
                let mut outputs = membership
                    .physical_outputs
                    .iter()
                    .copied()
                    .collect::<Vec<_>>();
                outputs.sort();
                outputs
            })
            .unwrap_or_default();
        let output = match outputs.as_slice() {
            [] => return None,
            [only] => *only,
            _ => {
                let rect = self.surface_layout_rect(surface_id);
                outputs
                    .iter()
                    .copied()
                    .max_by_key(|output_id| {
                        let area = rect
                            .zip(self.logical_outputs.get(output_id))
                            .map_or(0, |(rect, output)| {
                                logical_output_intersection_area(output, rect)
                            });
                        // Earlier OutputId wins ties: max_by_key keeps the last
                        // maximum, so order by (area, reversed position).
                        (area, std::cmp::Reverse(*output_id))
                    })
                    .expect("non-empty output list")
            }
        };
        self.output_presentation_key(output)
    }

    /// Takes the visible pending frame callbacks owned by the presenting output.
    pub(in crate::compositor) fn take_presenting_output_frame_callbacks(
        &mut self,
    ) -> Vec<wl_callback::WlCallback> {
        if !self.frame_callbacks_are_output_scoped() {
            self.visible_pending_frame_callback_count = 0;
            return std::mem::take(&mut self.visible_pending_frame_callbacks);
        }
        let callbacks = std::mem::take(&mut self.visible_pending_frame_callbacks);
        let mut taken = Vec::new();
        let mut kept = Vec::new();
        for callback in callbacks {
            let output = self
                .pending_frame_callback_surfaces
                .get(&callback.id())
                .copied()
                .and_then(|surface_id| self.surface_frame_output(surface_id));
            if output == self.presenting_output {
                taken.push(callback);
            } else {
                kept.push(callback);
            }
        }
        self.visible_pending_frame_callback_count = kept.len();
        self.visible_pending_frame_callbacks = kept;
        taken
    }
}

fn logical_output_intersection_area(output: &LogicalOutput, rect: (i64, i64, i64, i64)) -> i64 {
    let (left, top, right, bottom) = rect;
    let (x, y) = (i64::from(output.position.0), i64::from(output.position.1));
    let (width, height) = output.logical_size();
    let overlap_x = right.min(x + i64::from(width)) - left.max(x);
    let overlap_y = bottom.min(y + i64::from(height)) - top.max(y);
    overlap_x.max(0) * overlap_y.max(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;
    use wayland_server::{Client, Display};

    struct Harness {
        display: Display<CompositorState>,
        client: Client,
        _peer: UnixStream,
        state: CompositorState,
        native: OutputId,
        second: OutputId,
    }

    impl Harness {
        fn new() -> Self {
            let display = Display::<CompositorState>::new().expect("display");
            let (server_end, peer) = UnixStream::pair().expect("client socket");
            let client = display
                .handle()
                .insert_client(server_end, Arc::new(()))
                .expect("client");
            let mut state = CompositorState::new(None);
            assert!(state.set_output_size(1920, 1080));
            let native = state.ensure_native_output_id().expect("native output");
            let second = state
                .add_logical_output(
                    &display.handle(),
                    LogicalOutput {
                        name: "HDMI-A-2".into(),
                        description: "test".into(),
                        position: (1920, 0),
                        size: crate::compositor::output::OutputSize::new(3840, 2160),
                        scale: crate::compositor::output::OutputScale::from_factor(2.0),
                        refresh: OutputRefreshRate::from_hz(60),
                        physical_size_mm: (0, 0),
                    },
                )
                .expect("second output");
            Self {
                display,
                client,
                _peer: peer,
                state,
                native,
                second,
            }
        }

        fn mapped_surface(&mut self, x: i32, width: u32) -> u32 {
            let surface = self.state.test_create_unmapped_surface_resource_at_version(
                &self.client,
                &self.display.handle(),
                6,
            );
            let surface_id = crate::compositor::state_data::compositor_surface_id(&surface);
            self.state.test_map_surface(
                surface_id,
                width,
                300,
                SurfacePlacement::absolute_root_at(x, 100),
            );
            surface_id
        }

        fn queue_callback(&mut self, surface_id: u32) -> wl_callback::WlCallback {
            let callback = self
                .client
                .create_resource::<wl_callback::WlCallback, (), CompositorState>(
                    &self.display.handle(),
                    1,
                    (),
                )
                .expect("callback");
            self.state
                .queue_frame_callbacks_for_surface(surface_id, vec![callback.clone()]);
            callback
        }
    }

    fn ids(callbacks: &[wl_callback::WlCallback]) -> HashSet<ObjectId> {
        callbacks.iter().map(|callback| callback.id()).collect()
    }

    #[test]
    fn frame_callbacks_follow_their_surface_primary_output() {
        let mut harness = Harness::new();
        let left = harness.mapped_surface(100, 400);
        let right = harness.mapped_surface(2200, 400);
        // 220 px on the native output, 180 px on the second: native owns it.
        let spanning = harness.mapped_surface(1700, 400);
        harness.state.reconcile_all_surface_output_memberships();
        let left_callback = harness.queue_callback(left);
        let right_callback = harness.queue_callback(right);
        let spanning_callback = harness.queue_callback(spanning);
        assert_eq!(harness.state.visible_pending_frame_callback_count, 3);

        harness.state.enter_output_presentation(harness.second);
        let second_taken = harness.state.take_visible_pending_frame_callbacks();
        assert_eq!(ids(&second_taken), ids(&[right_callback]));
        assert_eq!(harness.state.visible_pending_frame_callback_count, 2);

        harness.state.enter_output_presentation(harness.native);
        let native_taken = harness.state.take_visible_pending_frame_callbacks();
        assert_eq!(ids(&native_taken), ids(&[left_callback, spanning_callback]));
        assert_eq!(harness.state.visible_pending_frame_callback_count, 0);
    }

    #[test]
    fn single_output_takes_every_visible_callback() {
        let display = Display::<CompositorState>::new().expect("display");
        let (server_end, _peer) = UnixStream::pair().expect("client socket");
        let client = display
            .handle()
            .insert_client(server_end, Arc::new(()))
            .expect("client");
        let mut state = CompositorState::new(None);
        assert!(state.set_output_size(1920, 1080));
        let surface =
            state.test_create_unmapped_surface_resource_at_version(&client, &display.handle(), 6);
        let surface_id = crate::compositor::state_data::compositor_surface_id(&surface);
        // Off every output: still owned by the only (native) presentation.
        state.test_map_surface(
            surface_id,
            100,
            100,
            SurfacePlacement::absolute_root_at(5000, 5000),
        );
        let callback = client
            .create_resource::<wl_callback::WlCallback, (), CompositorState>(
                &display.handle(),
                1,
                (),
            )
            .expect("callback");
        state.queue_frame_callbacks_for_surface(surface_id, vec![callback.clone()]);
        assert_eq!(
            ids(&state.take_visible_pending_frame_callbacks()),
            ids(&[callback])
        );
    }

    #[test]
    fn entering_an_output_swaps_its_batch_slots() {
        let mut harness = Harness::new();
        let native_batch = CompositorFrameBatchId::new(std::num::NonZeroU64::new(1).unwrap());
        harness.state.enter_output_presentation(harness.native);
        harness.state.legacy_prepared_frame_batch = Some(native_batch);
        harness.state.enter_output_presentation(harness.second);
        assert_eq!(harness.state.legacy_prepared_frame_batch, None);
        assert_eq!(harness.state.legacy_submitted_frame_batch, None);
        harness.state.enter_output_presentation(harness.native);
        assert_eq!(
            harness.state.legacy_prepared_frame_batch,
            Some(native_batch)
        );
        assert!(harness.state.parked_output_frame_slots.is_empty());
    }

    #[test]
    fn frame_batch_capacity_scales_with_outputs() {
        let harness = Harness::new();
        assert_eq!(harness.state.frame_batch_capacity(), 4);
    }
}
