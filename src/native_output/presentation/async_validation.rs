use oblivion_one::compositor::{DrmContentType, OutputPresentationMode};
use oblivion_one::core::OutputId;
use oblivion_one::native::kms::{AtomicCursorVisualState, DrmFormatModifierPair};

/// Exact state that makes a non-default composited presentation TEST_ONLY
/// result reusable.
///
/// Keep this key deliberately structural.  A successful test is not a general
/// capability bit: it only proves the exact output generation, plane,
/// framebuffer layout, acquire strategy, cursor state, and connector content
/// type that were tested.
///
/// Cursor placement is part of the proof only for tearing (async) flips,
/// which may not move other planes. Synchronous modes, AdaptiveSync included,
/// move the cursor plane like any VSync commit, so keying them on the
/// pointer position made every cursor move an unproven state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CompositedPresentationValidationKey {
    pub(crate) output_id: OutputId,
    pub(crate) output_generation: u64,
    pub(crate) crtc_id: u32,
    pub(crate) primary_plane_id: u32,
    pub(crate) format_modifier: DrmFormatModifierPair,
    pub(crate) presentation_mode: OutputPresentationMode,
    pub(crate) acquire_strategy: u8,
    pub(crate) cursor_visible: bool,
    pub(crate) cursor_state: Option<AtomicCursorVisualState>,
    pub(crate) cursor_transition_pending: bool,
    pub(crate) non_primary_plane_active: bool,
    pub(crate) explicit_sync_ready: bool,
    pub(crate) content_type: DrmContentType,
}

impl CompositedPresentationValidationKey {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
        output_id: OutputId,
        output_generation: u64,
        crtc_id: u32,
        primary_plane_id: u32,
        format_modifier: DrmFormatModifierPair,
        presentation_mode: OutputPresentationMode,
        acquire_strategy: u8,
        cursor_visible: bool,
        cursor_state: Option<AtomicCursorVisualState>,
        cursor_transition_pending: bool,
        non_primary_plane_active: bool,
        explicit_sync_ready: bool,
        content_type: DrmContentType,
    ) -> Self {
        let cursor_state = if presentation_mode.is_async() {
            cursor_state
        } else {
            match cursor_state {
                Some(state) => Some(AtomicCursorVisualState {
                    x: 0,
                    y: 0,
                    hotspot_x: 0,
                    hotspot_y: 0,
                    ..state
                }),
                None => None,
            }
        };
        Self {
            output_id,
            output_generation,
            crtc_id,
            primary_plane_id,
            format_modifier,
            presentation_mode,
            acquire_strategy,
            cursor_visible,
            cursor_state,
            cursor_transition_pending,
            non_primary_plane_active,
            explicit_sync_ready,
            content_type,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> CompositedPresentationValidationKey {
        CompositedPresentationValidationKey::new(
            OutputId::from_raw(1).expect("test output id"),
            7,
            42,
            43,
            DrmFormatModifierPair {
                fourcc: 0x3432_5258,
                modifier: 0,
            },
            OutputPresentationMode::Async,
            0,
            false,
            None,
            false,
            false,
            true,
            DrmContentType::Graphics,
        )
    }

    #[test]
    fn exact_key_changes_when_any_qualification_input_changes() {
        let base = key();
        let mut variants = [base; 12];
        variants[0].output_id = OutputId::from_raw(2).expect("test output id");
        variants[1].output_generation += 1;
        variants[2].crtc_id += 1;
        variants[3].primary_plane_id += 1;
        variants[4].format_modifier.modifier = 1;
        variants[5].acquire_strategy = 1;
        variants[6].cursor_visible = true;
        variants[7].content_type = DrmContentType::Game;
        variants[8].presentation_mode = OutputPresentationMode::AdaptiveSync;
        variants[9].explicit_sync_ready = false;
        variants[10].cursor_transition_pending = true;
        variants[11].cursor_state = Some(AtomicCursorVisualState {
            visible: true,
            x: 15,
            y: 20,
            hotspot_x: 1,
            hotspot_y: 2,
            width: 32,
            height: 32,
            framebuffer_id: Some(88),
            image_generation: 5,
        });
        for variant in variants {
            assert_ne!(base, variant);
        }
    }

    #[test]
    fn all_presentation_modes_and_output_generations_have_distinct_proofs() {
        let base = key();
        let keys = [
            OutputPresentationMode::Vsync,
            OutputPresentationMode::AdaptiveSync,
            OutputPresentationMode::Async,
            OutputPresentationMode::AdaptiveAsync,
        ]
        .map(|mode| CompositedPresentationValidationKey {
            presentation_mode: mode,
            ..base
        });
        let unique = keys.into_iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), 4);
        let next_generation = CompositedPresentationValidationKey {
            output_generation: base.output_generation + 1,
            ..base
        };
        assert!(!unique.contains(&next_generation));
    }
    #[test]
    fn synchronous_modes_ignore_cursor_placement_but_async_keeps_it() {
        let cursor_at = |x, y| AtomicCursorVisualState {
            visible: true,
            x,
            y,
            hotspot_x: 3,
            hotspot_y: 4,
            width: 32,
            height: 32,
            framebuffer_id: Some(88),
            image_generation: 5,
        };
        let key_for = |mode, cursor| {
            CompositedPresentationValidationKey::new(
                OutputId::from_raw(1).expect("test output id"),
                7,
                42,
                43,
                DrmFormatModifierPair {
                    fourcc: 0x3432_5258,
                    modifier: 0,
                },
                mode,
                0,
                true,
                Some(cursor),
                false,
                false,
                true,
                DrmContentType::Graphics,
            )
        };
        for mode in [
            OutputPresentationMode::Vsync,
            OutputPresentationMode::AdaptiveSync,
        ] {
            assert_eq!(
                key_for(mode, cursor_at(10, 20)),
                key_for(mode, cursor_at(900, 5))
            );
            let mut other_image = cursor_at(10, 20);
            other_image.image_generation += 1;
            assert_ne!(key_for(mode, cursor_at(10, 20)), key_for(mode, other_image));
        }
        for mode in [
            OutputPresentationMode::Async,
            OutputPresentationMode::AdaptiveAsync,
        ] {
            assert_ne!(
                key_for(mode, cursor_at(10, 20)),
                key_for(mode, cursor_at(900, 5))
            );
        }
    }
}
