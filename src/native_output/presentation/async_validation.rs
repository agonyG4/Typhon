use oblivion_one::compositor::{DrmContentType, OutputPresentationMode};
use oblivion_one::native::kms::DrmFormatModifierPair;

/// Exact state that makes a composited Async TEST_ONLY result reusable.
///
/// Keep this key deliberately structural.  A successful test is not a general
/// capability bit: it only proves the exact output generation, plane,
/// framebuffer layout, acquire strategy, cursor state, and connector content
/// type that were tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CompositedAsyncValidationKey {
    pub(crate) output_generation: u64,
    pub(crate) crtc_id: u32,
    pub(crate) primary_plane_id: u32,
    pub(crate) format_modifier: DrmFormatModifierPair,
    pub(crate) presentation_mode: OutputPresentationMode,
    pub(crate) acquire_strategy: u8,
    pub(crate) cursor_visible: bool,
    pub(crate) content_type: DrmContentType,
}

impl CompositedAsyncValidationKey {
    pub(crate) const fn new(
        output_generation: u64,
        crtc_id: u32,
        primary_plane_id: u32,
        format_modifier: DrmFormatModifierPair,
        acquire_strategy: u8,
        cursor_visible: bool,
        content_type: DrmContentType,
    ) -> Self {
        Self {
            output_generation,
            crtc_id,
            primary_plane_id,
            format_modifier,
            presentation_mode: OutputPresentationMode::Async,
            acquire_strategy,
            cursor_visible,
            content_type,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> CompositedAsyncValidationKey {
        CompositedAsyncValidationKey::new(
            7,
            42,
            43,
            DrmFormatModifierPair {
                fourcc: 0x3432_5258,
                modifier: 0,
            },
            0,
            false,
            DrmContentType::Graphics,
        )
    }

    #[test]
    fn exact_key_changes_when_any_qualification_input_changes() {
        let base = key();
        let mut variants = [base; 7];
        variants[0].output_generation += 1;
        variants[1].crtc_id += 1;
        variants[2].primary_plane_id += 1;
        variants[3].format_modifier.modifier = 1;
        variants[4].acquire_strategy = 1;
        variants[5].cursor_visible = true;
        variants[6].content_type = DrmContentType::Game;
        for variant in variants {
            assert_ne!(base, variant);
        }
    }
}
