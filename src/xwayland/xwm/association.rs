use std::{collections::HashMap, num::NonZeroU64};

use super::{X11WindowHandle, XwaylandGeneration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaylandAssociation {
    pub(crate) generation: XwaylandGeneration,
    pub(crate) surface_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssociatedSurface {
    pub(crate) generation: XwaylandGeneration,
    pub(crate) serial: NonZeroU64,
    pub(crate) surface_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceAssociationJoinError {
    InvalidSerial,
    DuplicateWaylandSerial,
    DuplicateWaylandSurface,
    DuplicateX11Serial,
    X11Reassociation,
    GenerationMismatch,
}

impl std::fmt::Display for SurfaceAssociationJoinError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSerial => "surface association serial must be nonzero",
            Self::DuplicateWaylandSerial => "Wayland serial already has an association",
            Self::DuplicateWaylandSurface => "Wayland surface already has an association",
            Self::DuplicateX11Serial => "X11 serial is already owned by another window",
            Self::X11Reassociation => "X11 window attempted reassociation",
            Self::GenerationMismatch => "surface association belongs to another generation",
        })
    }
}

impl std::error::Error for SurfaceAssociationJoinError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XwmAssociationEvent {
    Associated {
        generation: XwaylandGeneration,
        window: X11WindowHandle,
        surface_id: u32,
    },
    Removed {
        generation: XwaylandGeneration,
        window: X11WindowHandle,
        surface_id: u32,
    },
}

#[derive(Debug, Default)]
pub struct SurfaceAssociationJoin {
    pub(crate) wayland_by_serial: HashMap<NonZeroU64, WaylandAssociation>,
    pub(crate) x11_by_serial: HashMap<NonZeroU64, X11WindowHandle>,
    pub(crate) serial_by_x11: HashMap<X11WindowHandle, NonZeroU64>,
    pub(crate) completed: HashMap<X11WindowHandle, AssociatedSurface>,
    events: Vec<XwmAssociationEvent>,
}

impl SurfaceAssociationJoin {
    pub(crate) fn commit_wayland(
        &mut self,
        generation: XwaylandGeneration,
        serial: NonZeroU64,
        surface_id: u32,
    ) -> Result<(), SurfaceAssociationJoinError> {
        if serial == NonZeroU64::MIN {
            return Err(SurfaceAssociationJoinError::InvalidSerial);
        }
        if self.wayland_by_serial.contains_key(&serial) {
            return Err(SurfaceAssociationJoinError::DuplicateWaylandSerial);
        }
        if self
            .wayland_by_serial
            .values()
            .any(|association| association.surface_id == surface_id)
        {
            return Err(SurfaceAssociationJoinError::DuplicateWaylandSurface);
        }
        self.wayland_by_serial.insert(
            serial,
            WaylandAssociation {
                generation,
                surface_id,
            },
        );
        self.complete_if_ready(serial)
    }

    pub(crate) fn note_x11_serial(
        &mut self,
        window: X11WindowHandle,
        serial: NonZeroU64,
    ) -> Result<(), SurfaceAssociationJoinError> {
        if serial == NonZeroU64::MIN {
            return Err(SurfaceAssociationJoinError::InvalidSerial);
        }
        if let Some(previous) = self.serial_by_x11.get(&window) {
            let _ = previous;
            return Err(SurfaceAssociationJoinError::X11Reassociation);
        }
        if self.x11_by_serial.contains_key(&serial) {
            return Err(SurfaceAssociationJoinError::DuplicateX11Serial);
        }
        self.x11_by_serial.insert(serial, window);
        self.serial_by_x11.insert(window, serial);
        self.complete_if_ready(serial)
    }

    pub(crate) fn remove_wayland_surface(&mut self, surface_id: u32) {
        let Some(serial) = self
            .wayland_by_serial
            .iter()
            .find_map(|(serial, association)| {
                (association.surface_id == surface_id).then_some(*serial)
            })
        else {
            return;
        };
        self.wayland_by_serial.remove(&serial);
        if let Some(window) = self.x11_by_serial.remove(&serial) {
            self.serial_by_x11.remove(&window);
            if let Some(association) = self.completed.remove(&window) {
                self.events.push(XwmAssociationEvent::Removed {
                    generation: association.generation,
                    window,
                    surface_id: association.surface_id,
                });
            }
        }
    }

    pub(crate) fn remove_x11_window(&mut self, window: X11WindowHandle) {
        let Some(serial) = self.serial_by_x11.remove(&window) else {
            return;
        };
        self.x11_by_serial.remove(&serial);
        if let Some(association) = self.completed.remove(&window) {
            self.wayland_by_serial.remove(&serial);
            self.events.push(XwmAssociationEvent::Removed {
                generation: association.generation,
                window,
                surface_id: association.surface_id,
            });
        }
    }

    pub(crate) fn clear_generation(&mut self, generation: XwaylandGeneration) {
        let wayland_serials = self
            .wayland_by_serial
            .iter()
            .filter_map(|(serial, association)| {
                (association.generation == generation).then_some(*serial)
            })
            .collect::<Vec<_>>();
        for serial in wayland_serials {
            if let Some(association) = self.wayland_by_serial.remove(&serial)
                && let Some(window) = self.x11_by_serial.remove(&serial)
            {
                self.serial_by_x11.remove(&window);
                if self.completed.remove(&window).is_some() {
                    self.events.push(XwmAssociationEvent::Removed {
                        generation,
                        window,
                        surface_id: association.surface_id,
                    });
                }
            }
        }
        let windows = self
            .serial_by_x11
            .keys()
            .filter(|window| window.generation() == generation)
            .copied()
            .collect::<Vec<_>>();
        for window in windows {
            self.remove_x11_window(window);
        }
    }

    pub(crate) fn take_events(&mut self) -> Vec<XwmAssociationEvent> {
        std::mem::take(&mut self.events)
    }

    fn complete_if_ready(&mut self, serial: NonZeroU64) -> Result<(), SurfaceAssociationJoinError> {
        let (Some(wayland), Some(window)) = (
            self.wayland_by_serial.get(&serial).copied(),
            self.x11_by_serial.get(&serial).copied(),
        ) else {
            return Ok(());
        };
        if wayland.generation != window.generation() {
            self.wayland_by_serial.remove(&serial);
            self.x11_by_serial.remove(&serial);
            self.serial_by_x11.remove(&window);
            return Err(SurfaceAssociationJoinError::GenerationMismatch);
        }
        if self.completed.contains_key(&window) {
            return Ok(());
        }
        let association = AssociatedSurface {
            generation: wayland.generation,
            serial,
            surface_id: wayland.surface_id,
        };
        self.completed.insert(window, association);
        self.events.push(XwmAssociationEvent::Associated {
            generation: association.generation,
            window,
            surface_id: association.surface_id,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generation(value: u64) -> XwaylandGeneration {
        XwaylandGeneration::new(NonZeroU64::new(value).expect("nonzero"))
    }

    fn window(generation: XwaylandGeneration, xid: u32) -> X11WindowHandle {
        X11WindowHandle::new(generation, xid)
    }

    #[test]
    fn wayland_first_then_x11_serial_completes_once() {
        let generation = generation(1);
        let handle = window(generation, 10);
        let serial = NonZeroU64::new(7).unwrap();
        let mut join = SurfaceAssociationJoin::default();

        join.commit_wayland(generation, serial, 42).unwrap();
        join.note_x11_serial(handle, serial).unwrap();

        assert_eq!(join.completed.len(), 1);
        assert_eq!(join.take_events().len(), 1);
        assert!(join.take_events().is_empty());
    }

    #[test]
    fn x11_first_then_wayland_serial_completes_once() {
        let generation = generation(1);
        let handle = window(generation, 10);
        let serial = NonZeroU64::new(8).unwrap();
        let mut join = SurfaceAssociationJoin::default();

        join.note_x11_serial(handle, serial).unwrap();
        join.commit_wayland(generation, serial, 43).unwrap();

        assert_eq!(join.completed[&handle].surface_id, 43);
        assert_eq!(join.take_events().len(), 1);
    }

    #[test]
    fn duplicate_x11_serial_owner_is_rejected() {
        let generation = generation(1);
        let serial = NonZeroU64::new(9).unwrap();
        let mut join = SurfaceAssociationJoin::default();
        join.note_x11_serial(window(generation, 1), serial).unwrap();

        assert_eq!(
            join.note_x11_serial(window(generation, 2), serial),
            Err(SurfaceAssociationJoinError::DuplicateX11Serial)
        );
    }

    #[test]
    fn x11_reassociation_is_rejected_without_disturbing_original() {
        let generation = generation(1);
        let handle = window(generation, 1);
        let first = NonZeroU64::new(10).unwrap();
        let second = NonZeroU64::new(11).unwrap();
        let mut join = SurfaceAssociationJoin::default();
        join.note_x11_serial(handle, first).unwrap();

        assert_eq!(
            join.note_x11_serial(handle, second),
            Err(SurfaceAssociationJoinError::X11Reassociation)
        );
        assert_eq!(join.serial_by_x11.get(&handle), Some(&first));
    }

    #[test]
    fn destroying_either_side_before_join_clears_only_pending_half() {
        let generation = generation(1);
        let handle = window(generation, 1);
        let serial = NonZeroU64::new(12).unwrap();
        let mut join = SurfaceAssociationJoin::default();

        join.note_x11_serial(handle, serial).unwrap();
        join.remove_x11_window(handle);
        join.commit_wayland(generation, serial, 1).unwrap();
        assert!(join.completed.is_empty());
        join.remove_wayland_surface(1);

        join.commit_wayland(generation, NonZeroU64::new(13).unwrap(), 2)
            .unwrap();
        join.remove_wayland_surface(2);
        assert!(join.wayland_by_serial.is_empty());
    }

    #[test]
    fn completed_removal_emits_one_dissociation() {
        let generation = generation(1);
        let handle = window(generation, 1);
        let serial = NonZeroU64::new(14).unwrap();
        let mut join = SurfaceAssociationJoin::default();
        join.note_x11_serial(handle, serial).unwrap();
        join.commit_wayland(generation, serial, 3).unwrap();
        join.take_events();

        join.remove_x11_window(handle);
        assert_eq!(
            join.take_events(),
            vec![XwmAssociationEvent::Removed {
                generation,
                window: handle,
                surface_id: 3,
            }]
        );
    }

    #[test]
    fn old_generation_serial_never_matches_new_generation_window() {
        let old_generation = generation(1);
        let new_generation = generation(2);
        let serial = NonZeroU64::new(15).unwrap();
        let mut join = SurfaceAssociationJoin::default();
        join.note_x11_serial(window(new_generation, 1), serial)
            .unwrap();

        assert_eq!(
            join.commit_wayland(old_generation, serial, 4),
            Err(SurfaceAssociationJoinError::GenerationMismatch)
        );
        assert!(join.completed.is_empty());
    }

    #[test]
    fn clearing_generation_removes_completed_join() {
        let generation = generation(1);
        let handle = window(generation, 1);
        let serial = NonZeroU64::new(16).unwrap();
        let mut join = SurfaceAssociationJoin::default();
        join.note_x11_serial(handle, serial).unwrap();
        join.commit_wayland(generation, serial, 5).unwrap();
        join.take_events();

        join.clear_generation(generation);
        assert!(join.completed.is_empty());
        assert!(join.serial_by_x11.is_empty());
        assert!(matches!(
            join.take_events().as_slice(),
            [XwmAssociationEvent::Removed { .. }]
        ));
    }
}
