use crate::compositor::{
    CapturedSurfacePresentation, SurfaceContentType, SurfacePresentationHint,
    SurfacePresentationMetadata,
};

use super::SurfaceData;

impl SurfaceData {
    pub(super) fn set_pending_presentation_hint(&self, hint: SurfacePresentationHint) {
        if let Ok(mut presentation) = self.presentation.lock() {
            *presentation = std::mem::take(&mut *presentation).set_pending_hint(hint);
        }
    }

    pub(super) fn set_pending_content_type(&self, content_type: SurfaceContentType) {
        if let Ok(mut presentation) = self.presentation.lock() {
            *presentation =
                std::mem::take(&mut *presentation).set_pending_content_type(content_type);
        }
    }

    pub(super) fn revert_pending_presentation_hint(&self) {
        if let Ok(mut presentation) = self.presentation.lock() {
            *presentation = std::mem::take(&mut *presentation).destroy_tearing_object();
        }
    }

    pub(super) fn revert_pending_content_type(&self) {
        if let Ok(mut presentation) = self.presentation.lock() {
            *presentation = std::mem::take(&mut *presentation).destroy_content_type_object();
        }
    }

    pub(super) fn take_pending_presentation(&self) -> CapturedSurfacePresentation {
        self.presentation
            .lock()
            .map(|mut presentation| {
                let (next, captured) =
                    std::mem::take(&mut *presentation).capture_pending_and_reset();
                *presentation = next;
                captured
            })
            .unwrap_or_default()
    }

    pub(super) fn current_presentation(&self) -> SurfacePresentationMetadata {
        self.presentation
            .lock()
            .map(|presentation| presentation.current())
            .unwrap_or_default()
    }

    #[allow(dead_code)]
    pub(super) fn pending_presentation(&self) -> SurfacePresentationMetadata {
        self.presentation
            .lock()
            .map(|presentation| presentation.pending())
            .unwrap_or_default()
    }

    pub(super) fn apply_presentation(&self, captured: CapturedSurfacePresentation) {
        if let Ok(mut presentation) = self.presentation.lock() {
            *presentation = std::mem::take(&mut *presentation).apply_captured(captured);
        }
    }
}
