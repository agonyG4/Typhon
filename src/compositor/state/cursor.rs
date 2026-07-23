use super::*;

impl CompositorState {
    pub(in crate::compositor) fn note_cursor_generation(&mut self, cause: RenderGenerationCause) {
        if matches!(
            cause,
            RenderGenerationCause::CursorCommit | RenderGenerationCause::CursorState
        ) {
            self.cursor_generation = self.cursor_generation.saturating_add(1);
        }
    }

    pub(in crate::compositor) fn advance_cursor_generation(&mut self) -> u64 {
        self.cursor_generation = self.cursor_generation.saturating_add(1);
        self.cursor_generation
    }
}
