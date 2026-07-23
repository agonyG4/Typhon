use crate::compositor::DesktopWindowKind;

use super::{X11Geometry, X11WindowHandle, Xwm, XwmError};

impl Xwm {
    pub fn observe_window(&mut self, handle: X11WindowHandle) -> Result<bool, XwmError> {
        self.observe_window_with_kind(handle, DesktopWindowKind::Managed, X11Geometry::default())
    }

    pub(crate) fn observe_window_with_kind(
        &mut self,
        handle: X11WindowHandle,
        kind: DesktopWindowKind,
        geometry: X11Geometry,
    ) -> Result<bool, XwmError> {
        if handle.generation() != self.generation {
            return Err(XwmError::StaleGeneration);
        }
        let inserted = self
            .windows
            .insert_observed_with_kind(handle, kind, geometry);
        if inserted {
            super::properties::begin_initial(self, handle)?;
        }
        Ok(inserted)
    }

    pub(crate) fn begin_map_to_association_wait(
        &mut self,
        handle: X11WindowHandle,
    ) -> Result<(), XwmError> {
        if handle.generation() != self.generation {
            return Err(XwmError::StaleGeneration);
        }
        if self.windows.get(handle).is_none_or(|record| {
            record.kind != DesktopWindowKind::Managed
                || !record.map_requested
                || record.association.is_some()
                || matches!(
                    record.lifecycle,
                    super::window::X11WindowLifecycle::Withdrawn
                        | super::window::X11WindowLifecycle::Destroyed
                )
        }) {
            return Ok(());
        }
        let deadline = crate::native::event_loop::monotonic_now_ns()
            .unwrap_or_default()
            .saturating_add(super::adoption::ADOPTION_TIMEOUT_NS);
        self.adoption.observe(
            handle,
            super::adoption::AdoptionWait::MapToAssociation,
            deadline,
        );
        Ok(())
    }
}
