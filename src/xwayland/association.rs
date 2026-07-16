use std::{collections::HashMap, num::NonZeroU64};

use super::XwaylandGeneration;

pub type SurfaceId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceAssociation {
    pub generation: XwaylandGeneration,
    pub serial: NonZeroU64,
    pub surface_id: SurfaceId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssociationError {
    SerialAlreadyAssociated,
    SurfaceAlreadyAssociated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XwaylandAssociationEvent {
    Committed {
        generation: XwaylandGeneration,
        serial: NonZeroU64,
        surface_id: SurfaceId,
    },
    Removed {
        generation: XwaylandGeneration,
        serial: NonZeroU64,
        surface_id: SurfaceId,
    },
}

#[derive(Debug, Default)]
pub struct AssociationRegistry {
    by_serial: HashMap<(XwaylandGeneration, NonZeroU64), SurfaceAssociation>,
    by_surface: HashMap<SurfaceId, (XwaylandGeneration, NonZeroU64)>,
    events: Vec<XwaylandAssociationEvent>,
}

impl AssociationRegistry {
    pub fn commit_surface_serial(
        &mut self,
        generation: XwaylandGeneration,
        serial: NonZeroU64,
        surface_id: SurfaceId,
    ) -> Result<(), AssociationError> {
        if self.by_serial.contains_key(&(generation, serial)) {
            return Err(AssociationError::SerialAlreadyAssociated);
        }
        if self.by_surface.contains_key(&surface_id) {
            return Err(AssociationError::SurfaceAlreadyAssociated);
        }
        let association = SurfaceAssociation {
            generation,
            serial,
            surface_id,
        };
        self.by_serial.insert((generation, serial), association);
        self.by_surface.insert(surface_id, (generation, serial));
        self.events.push(XwaylandAssociationEvent::Committed {
            generation,
            serial,
            surface_id,
        });
        Ok(())
    }

    pub fn surface_for_serial(
        &self,
        generation: XwaylandGeneration,
        serial: NonZeroU64,
    ) -> Option<SurfaceId> {
        self.by_serial
            .get(&(generation, serial))
            .map(|entry| entry.surface_id)
    }

    pub fn serial_for_surface(
        &self,
        surface_id: SurfaceId,
    ) -> Option<(XwaylandGeneration, NonZeroU64)> {
        self.by_surface.get(&surface_id).copied()
    }

    pub fn remove_surface(&mut self, surface_id: SurfaceId) {
        let Some((generation, serial)) = self.by_surface.remove(&surface_id) else {
            return;
        };
        self.by_serial.remove(&(generation, serial));
        self.events.push(XwaylandAssociationEvent::Removed {
            generation,
            serial,
            surface_id,
        });
    }

    pub fn clear_generation(&mut self, generation: XwaylandGeneration) {
        let surfaces: Vec<_> = self
            .by_serial
            .keys()
            .filter(|(entry_generation, _)| *entry_generation == generation)
            .filter_map(|key| self.by_serial.get(key).map(|entry| entry.surface_id))
            .collect();
        for surface_id in surfaces {
            self.remove_surface(surface_id);
        }
    }

    pub fn take_events(&mut self) -> Vec<XwaylandAssociationEvent> {
        std::mem::take(&mut self.events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generation(value: u64) -> XwaylandGeneration {
        XwaylandGeneration::new(NonZeroU64::new(value).unwrap())
    }

    #[test]
    fn commit_populates_both_indexes_and_emits_normalized_event() {
        let mut registry = AssociationRegistry::default();
        let generation = generation(1);
        let serial = NonZeroU64::new(0x1_0000_0001).unwrap();

        registry
            .commit_surface_serial(generation, serial, 42)
            .unwrap();

        assert_eq!(registry.surface_for_serial(generation, serial), Some(42));
        assert_eq!(registry.serial_for_surface(42), Some((generation, serial)));
        assert_eq!(
            registry.take_events(),
            vec![XwaylandAssociationEvent::Committed {
                generation,
                serial,
                surface_id: 42,
            }]
        );
    }

    #[test]
    fn duplicate_serial_or_surface_is_rejected() {
        let mut registry = AssociationRegistry::default();
        let first_generation = generation(1);
        let second_generation = generation(2);
        let serial = NonZeroU64::new(7).unwrap();
        registry
            .commit_surface_serial(first_generation, serial, 1)
            .unwrap();

        assert_eq!(
            registry.commit_surface_serial(first_generation, serial, 2),
            Err(AssociationError::SerialAlreadyAssociated)
        );
        assert_eq!(
            registry.commit_surface_serial(second_generation, NonZeroU64::new(8).unwrap(), 1),
            Err(AssociationError::SurfaceAlreadyAssociated)
        );
    }

    #[test]
    fn same_serial_is_allowed_for_a_later_generation() {
        let mut registry = AssociationRegistry::default();
        let serial = NonZeroU64::new(9).unwrap();
        registry
            .commit_surface_serial(generation(1), serial, 1)
            .unwrap();
        registry
            .commit_surface_serial(generation(2), serial, 2)
            .unwrap();

        assert_eq!(registry.surface_for_serial(generation(1), serial), Some(1));
        assert_eq!(registry.surface_for_serial(generation(2), serial), Some(2));
    }

    #[test]
    fn removing_surface_and_clearing_generation_emit_removals() {
        let mut registry = AssociationRegistry::default();
        let serial_one = NonZeroU64::new(1).unwrap();
        let serial_two = NonZeroU64::new(2).unwrap();
        registry
            .commit_surface_serial(generation(1), serial_one, 1)
            .unwrap();
        registry
            .commit_surface_serial(generation(1), serial_two, 2)
            .unwrap();
        registry
            .commit_surface_serial(generation(2), serial_one, 3)
            .unwrap();
        registry.take_events();

        registry.remove_surface(1);
        registry.clear_generation(generation(1));

        assert_eq!(registry.serial_for_surface(1), None);
        assert_eq!(registry.serial_for_surface(2), None);
        assert_eq!(
            registry.serial_for_surface(3),
            Some((generation(2), serial_one))
        );
        assert_eq!(
            registry.take_events(),
            vec![
                XwaylandAssociationEvent::Removed {
                    generation: generation(1),
                    serial: serial_one,
                    surface_id: 1,
                },
                XwaylandAssociationEvent::Removed {
                    generation: generation(1),
                    serial: serial_two,
                    surface_id: 2,
                },
            ]
        );
    }
}
