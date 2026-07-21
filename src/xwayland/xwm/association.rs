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
    pub(crate) map_serial: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct X11Association {
    pub(crate) window: X11WindowHandle,
    pub(crate) map_serial: u64,
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
    pub(crate) x11_by_serial: HashMap<NonZeroU64, X11Association>,
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

    #[cfg(test)]
    pub(crate) fn note_x11_serial(
        &mut self,
        window: X11WindowHandle,
        serial: NonZeroU64,
    ) -> Result<(), SurfaceAssociationJoinError> {
        self.note_x11_serial_for_map(window, serial, 0)
    }

    pub(crate) fn note_x11_serial_for_map(
        &mut self,
        window: X11WindowHandle,
        serial: NonZeroU64,
        map_serial: u64,
    ) -> Result<(), SurfaceAssociationJoinError> {
        if self.x11_by_serial.contains_key(&serial) {
            if self.serial_by_x11.get(&window) == Some(&serial) {
                return self.complete_if_ready(serial);
            }
            return Err(SurfaceAssociationJoinError::DuplicateX11Serial);
        }
        if let Some(previous) = self.serial_by_x11.insert(window, serial) {
            self.completed.remove(&window);
            debug_assert!(self.x11_by_serial.contains_key(&previous));
        }
        self.x11_by_serial
            .insert(serial, X11Association { window, map_serial });
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
        if let Some(x11) = self.x11_by_serial.remove(&serial) {
            let current = self.serial_by_x11.get(&x11.window).copied();
            if current == Some(serial) {
                self.serial_by_x11.remove(&x11.window);
            }
            if current == Some(serial)
                && let Some(association) = self.completed.remove(&x11.window)
            {
                self.events.push(XwmAssociationEvent::Removed {
                    generation: association.generation,
                    window: x11.window,
                    surface_id: association.surface_id,
                });
            }
        }
    }

    pub(crate) fn remove_x11_window(&mut self, window: X11WindowHandle) {
        self.serial_by_x11.remove(&window);
        let serials = self
            .x11_by_serial
            .iter()
            .filter_map(|(serial, association)| (association.window == window).then_some(*serial))
            .collect::<Vec<_>>();
        for serial in serials {
            self.x11_by_serial.remove(&serial);
            self.wayland_by_serial.remove(&serial);
        }
        if let Some(association) = self.completed.remove(&window) {
            self.events.push(XwmAssociationEvent::Removed {
                generation: association.generation,
                window,
                surface_id: association.surface_id,
            });
        }
    }

    pub(crate) fn clear_generation(&mut self, generation: XwaylandGeneration) {
        let windows = self
            .serial_by_x11
            .keys()
            .filter(|window| window.generation() == generation)
            .copied()
            .collect::<Vec<_>>();
        for window in windows {
            self.remove_x11_window(window);
        }
        let wayland_serials = self
            .wayland_by_serial
            .iter()
            .filter_map(|(serial, association)| {
                (association.generation == generation).then_some(*serial)
            })
            .collect::<Vec<_>>();
        for serial in wayland_serials {
            self.wayland_by_serial.remove(&serial);
            self.x11_by_serial.remove(&serial);
        }
    }

    pub(crate) fn take_events(&mut self) -> Vec<XwmAssociationEvent> {
        std::mem::take(&mut self.events)
    }

    fn complete_if_ready(&mut self, serial: NonZeroU64) -> Result<(), SurfaceAssociationJoinError> {
        let (Some(wayland), Some(x11)) = (
            self.wayland_by_serial.get(&serial).copied(),
            self.x11_by_serial.get(&serial).copied(),
        ) else {
            return Ok(());
        };
        let window = x11.window;
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
            map_serial: x11.map_serial,
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
    fn serial_one_is_valid_wayland_first() {
        let generation = generation(1);
        let handle = window(generation, 10);
        let serial = NonZeroU64::new(1).unwrap();
        let mut join = SurfaceAssociationJoin::default();

        join.commit_wayland(generation, serial, 42).unwrap();
        join.note_x11_serial(handle, serial).unwrap();

        assert_eq!(join.completed[&handle].serial, serial);
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
    fn serial_one_is_valid_x11_first() {
        let generation = generation(1);
        let handle = window(generation, 10);
        let serial = NonZeroU64::new(1).unwrap();
        let mut join = SurfaceAssociationJoin::default();

        join.note_x11_serial(handle, serial).unwrap();
        join.commit_wayland(generation, serial, 43).unwrap();

        assert_eq!(join.completed[&handle].surface_id, 43);
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
    fn x11_reassociation_replaces_current_serial_without_disturbing_old_pending() {
        let generation = generation(1);
        let handle = window(generation, 1);
        let first = NonZeroU64::new(10).unwrap();
        let second = NonZeroU64::new(11).unwrap();
        let mut join = SurfaceAssociationJoin::default();
        join.note_x11_serial(handle, first).unwrap();

        join.note_x11_serial(handle, second)
            .expect("reassociation replaces current serial");
        assert_eq!(join.serial_by_x11.get(&handle), Some(&second));
        assert_eq!(
            join.x11_by_serial.get(&first).map(|value| value.window),
            Some(handle)
        );
    }

    #[test]
    fn new_serial_before_old_surface_removed_reassociates_same_xid() {
        let generation = generation(1);
        let handle = window(generation, 1);
        let first = NonZeroU64::new(10).unwrap();
        let second = NonZeroU64::new(11).unwrap();
        let mut join = SurfaceAssociationJoin::default();
        join.note_x11_serial(handle, first).unwrap();
        join.commit_wayland(generation, first, 41).unwrap();
        join.take_events();

        join.note_x11_serial(handle, second)
            .expect("new map epoch may arrive before old surface removal");

        assert_eq!(join.serial_by_x11.get(&handle), Some(&second));
        assert_eq!(
            join.x11_by_serial.get(&first).map(|value| value.window),
            Some(handle)
        );
        assert_eq!(
            join.x11_by_serial.get(&second).map(|value| value.window),
            Some(handle)
        );
    }

    #[test]
    fn old_surface_removed_after_new_association_does_not_clear_replacement() {
        let generation = generation(1);
        let handle = window(generation, 1);
        let first = NonZeroU64::new(10).unwrap();
        let second = NonZeroU64::new(11).unwrap();
        let mut join = SurfaceAssociationJoin::default();
        join.note_x11_serial(handle, first).unwrap();
        join.commit_wayland(generation, first, 41).unwrap();
        join.take_events();
        join.note_x11_serial(handle, second).unwrap();
        join.commit_wayland(generation, second, 42).unwrap();
        join.take_events();

        join.remove_wayland_surface(41);

        assert_eq!(join.serial_by_x11.get(&handle), Some(&second));
        assert_eq!(join.completed[&handle].serial, second);
        assert_eq!(join.completed[&handle].surface_id, 42);
        assert!(join.take_events().is_empty());
    }

    #[test]
    fn new_serial_then_old_surface_removal_still_allows_new_wayland_commit() {
        let generation = generation(1);
        let handle = window(generation, 1);
        let first = NonZeroU64::new(10).unwrap();
        let second = NonZeroU64::new(11).unwrap();
        let mut join = SurfaceAssociationJoin::default();
        join.note_x11_serial(handle, first).unwrap();
        join.commit_wayland(generation, first, 41).unwrap();
        join.take_events();
        join.note_x11_serial(handle, second).unwrap();

        join.remove_wayland_surface(41);
        join.commit_wayland(generation, second, 42).unwrap();

        assert_eq!(join.completed[&handle].serial, second);
        assert_eq!(join.completed[&handle].surface_id, 42);
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
