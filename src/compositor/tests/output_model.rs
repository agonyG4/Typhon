use super::*;
use std::{
    collections::{HashMap, HashSet},
    io::{self, Read},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};
use wayland_server::{Client, Display, Resource};

const OUTPUT_WIDTH: i64 = 100;
const OUTPUT_HEIGHT: i64 = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReferenceSurface {
    mapped: bool,
    published: bool,
    width: i64,
    height: i64,
    x: i64,
    y: i64,
    parent: Option<u32>,
    version: u32,
    entered: HashSet<u32>,
    physical: bool,
    preference: Option<SurfaceBufferPreference>,
    enter_events: u64,
    leave_events: u64,
    scale_events: u64,
    transform_events: u64,
}

#[derive(Debug)]
struct ReferenceOutputModel {
    surfaces: HashMap<u32, ReferenceSurface>,
    outputs: HashSet<u32>,
    next_surface: u32,
    next_output: u32,
    scale: i32,
    transform: wl_output::Transform,
    enter_events_total: u64,
    leave_events_total: u64,
    scale_events_total: u64,
    transform_events_total: u64,
}

impl Default for ReferenceOutputModel {
    fn default() -> Self {
        Self {
            surfaces: HashMap::new(),
            outputs: HashSet::new(),
            next_surface: 0,
            next_output: 0,
            scale: 1,
            transform: wl_output::Transform::Normal,
            enter_events_total: 0,
            leave_events_total: 0,
            scale_events_total: 0,
            transform_events_total: 0,
        }
    }
}

impl ReferenceOutputModel {
    fn global_rect(&self, surface_id: u32) -> Option<(i64, i64, i64, i64)> {
        let surface = self.surfaces.get(&surface_id)?;
        let mut x = surface.x;
        let mut y = surface.y;
        let mut parent = surface.parent;
        while let Some(parent_id) = parent {
            let parent_surface = self.surfaces.get(&parent_id)?;
            x = x.saturating_add(parent_surface.x);
            y = y.saturating_add(parent_surface.y);
            parent = parent_surface.parent;
        }
        Some((x, y, surface.width, surface.height))
    }

    fn effectively_mapped(&self, surface_id: u32) -> bool {
        let Some(surface) = self.surfaces.get(&surface_id) else {
            return false;
        };
        surface.published
            && surface
                .parent
                .is_none_or(|parent| self.effectively_mapped(parent))
    }

    fn overlaps_output(&self, surface_id: u32) -> bool {
        let Some((x, y, width, height)) = self.global_rect(surface_id) else {
            return false;
        };
        self.effectively_mapped(surface_id)
            && x < OUTPUT_WIDTH
            && y < OUTPUT_HEIGHT
            && x.saturating_add(width) > 0
            && y.saturating_add(height) > 0
    }

    fn reconcile(&mut self, surface_id: u32) {
        let overlaps = self.overlaps_output(surface_id);
        let outputs = if overlaps {
            self.outputs.clone()
        } else {
            HashSet::new()
        };
        let preference = SurfaceBufferPreference {
            scale: self.scale,
            transform: self.transform,
        };
        let surface = self
            .surfaces
            .get_mut(&surface_id)
            .expect("reference surface");
        let old_entered = std::mem::replace(&mut surface.entered, outputs);
        surface.enter_events += surface.entered.difference(&old_entered).count() as u64;
        surface.leave_events += old_entered.difference(&surface.entered).count() as u64;
        surface.physical = overlaps;
        if overlaps && surface.version >= 6 {
            let old = surface.preference.replace(preference);
            if old != Some(preference) {
                if old.map_or(preference.scale != 1, |value| {
                    value.scale != preference.scale
                }) {
                    surface.scale_events += 1;
                }
                if old.map_or(
                    preference.transform != wl_output::Transform::Normal,
                    |value| value.transform != preference.transform,
                ) {
                    surface.transform_events += 1;
                }
            }
        }
    }

    fn reconcile_all(&mut self) {
        let ids = self.surfaces.keys().copied().collect::<Vec<_>>();
        for id in ids {
            self.reconcile(id);
        }
    }

    fn create_surface(&mut self, random: u64) -> u32 {
        let id = self.next_surface;
        self.next_surface += 1;
        self.surfaces.insert(
            id,
            ReferenceSurface {
                mapped: false,
                published: false,
                width: i64::from((random as u32 % 80).max(1)),
                height: i64::from(((random >> 16) as u32 % 80).max(1)),
                x: (random as i16 as i64) % 180 - 40,
                y: ((random >> 8) as i16 as i64) % 180 - 40,
                parent: None,
                version: if random & 2 == 0 { 5 } else { 6 },
                entered: HashSet::new(),
                physical: false,
                preference: None,
                enter_events: 0,
                leave_events: 0,
                scale_events: 0,
                transform_events: 0,
            },
        );
        id
    }

    fn destroy_surface(&mut self, surface_id: u32) {
        if let Some(surface) = self.surfaces.remove(&surface_id) {
            self.enter_events_total += surface.enter_events;
            self.leave_events_total += surface.leave_events;
            self.scale_events_total += surface.scale_events;
            self.transform_events_total += surface.transform_events;
        }
        let children = self
            .surfaces
            .iter()
            .filter_map(|(id, surface)| (surface.parent == Some(surface_id)).then_some(*id))
            .collect::<Vec<_>>();
        for child in children {
            self.unmap_subtree(child);
            if let Some(surface) = self.surfaces.get_mut(&child) {
                surface.parent = None;
            }
        }
    }

    fn unmap_subtree(&mut self, surface_id: u32) {
        if let Some(surface) = self.surfaces.get_mut(&surface_id) {
            surface.mapped = false;
            surface.published = false;
        }
        let children = self
            .surfaces
            .iter()
            .filter_map(|(id, surface)| (surface.parent == Some(surface_id)).then_some(*id))
            .collect::<Vec<_>>();
        for child in children {
            self.unmap_subtree(child);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProductionSurfaceSnapshot {
    published: bool,
    physical: bool,
    entered: HashSet<u32>,
    preference: Option<SurfaceBufferPreference>,
}

struct ProductionOutputClient {
    client: Client,
    peer: UnixStream,
}

struct ProductionOutputModel {
    display: Display<CompositorState>,
    state: CompositorState,
    client: ProductionOutputClient,
    surfaces: HashMap<u32, wl_surface::WlSurface>,
    outputs: HashMap<u32, wl_output::WlOutput>,
    expected_enter_events: u64,
    expected_leave_events: u64,
    expected_scale_events: u64,
    expected_transform_events: u64,
}

impl ProductionOutputModel {
    fn new() -> Self {
        let display = Display::<CompositorState>::new().expect("output model display");
        let (server_end, peer) = UnixStream::pair().expect("output model client");
        peer.set_nonblocking(true)
            .expect("output model nonblocking peer");
        let client = display
            .handle()
            .insert_client(server_end, Arc::new(()))
            .expect("output model insert client");
        let mut state = CompositorState::new(None);
        assert!(state.set_output_size(OUTPUT_WIDTH as u32, OUTPUT_HEIGHT as u32));
        Self {
            display,
            state,
            client: ProductionOutputClient { client, peer },
            surfaces: HashMap::new(),
            outputs: HashMap::new(),
            expected_enter_events: 0,
            expected_leave_events: 0,
            expected_scale_events: 0,
            expected_transform_events: 0,
        }
    }

    fn create_surface(&mut self, id: u32, reference: ReferenceSurface) {
        let surface = self.state.test_create_unmapped_surface_resource_at_version(
            &self.client.client,
            &self.display.handle(),
            reference.version,
        );
        self.surfaces.insert(id, surface);
    }

    fn placement(&self, id: u32, reference: &ReferenceOutputModel) -> SurfacePlacement {
        let surface = reference.surfaces.get(&id).expect("reference placement");
        if let Some(parent) = surface.parent {
            let parent_surface = self.surfaces.get(&parent).expect("production parent");
            SurfacePlacement::subsurface(
                compositor_surface_id(parent_surface),
                surface.x as i32,
                surface.y as i32,
            )
        } else {
            SurfacePlacement::absolute_root_at(surface.x as i32, surface.y as i32)
        }
    }

    fn map_surface(&mut self, id: u32, reference: &ReferenceOutputModel) {
        let surface = self.surfaces.get(&id).expect("production map surface");
        let state_id = compositor_surface_id(surface);
        let reference_surface = reference.surfaces.get(&id).expect("reference map surface");
        self.state.test_map_surface(
            state_id,
            reference_surface.width as u32,
            reference_surface.height as u32,
            self.placement(id, reference),
        );
    }

    fn unmap_surface(&mut self, id: u32) {
        let surface = self.surfaces.get(&id).expect("production unmap surface");
        self.state
            .test_unmap_surface(compositor_surface_id(surface));
    }

    fn move_or_resize_surface(&mut self, id: u32, reference: &ReferenceOutputModel, resize: bool) {
        let surface = self.surfaces.get(&id).expect("production geometry surface");
        let state_id = compositor_surface_id(surface);
        if resize {
            let reference_surface = reference
                .surfaces
                .get(&id)
                .expect("reference resize surface");
            self.state.test_resize_surface(
                state_id,
                reference_surface.width as u32,
                reference_surface.height as u32,
            );
        }
        self.state
            .test_set_surface_placement(state_id, self.placement(id, reference));
    }

    fn destroy_surface(&mut self, id: u32) {
        if let Some(surface) = self.surfaces.remove(&id) {
            self.state
                .test_destroy_surface_resource(compositor_surface_id(&surface));
        }
    }

    fn bind_output(&mut self, id: u32) {
        let output = self
            .client
            .client
            .create_resource::<wl_output::WlOutput, (), CompositorState>(
                &self.display.handle(),
                4,
                (),
            )
            .expect("output resource");
        self.state.register_output_resource(output.clone());
        self.outputs.insert(id, output);
    }

    fn release_output(&mut self, id: u32) {
        if let Some(output) = self.outputs.remove(&id) {
            self.state.unregister_output_resource(&output);
        }
    }

    fn snapshot(&self, id: u32) -> Option<ProductionSurfaceSnapshot> {
        let surface = self.surfaces.get(&id)?;
        let membership = self
            .state
            .surface_output_memberships
            .get(&compositor_surface_id(surface))
            .cloned()
            .unwrap_or_default();
        let entered = membership
            .entered_resources
            .iter()
            .filter_map(|resource_id| {
                self.outputs.iter().find_map(|(logical_id, output)| {
                    (output.id().protocol_id() == *resource_id).then_some(*logical_id)
                })
            })
            .collect();
        Some(ProductionSurfaceSnapshot {
            published: self
                .state
                .renderable_surfaces
                .iter()
                .any(|renderable| renderable.surface_id == compositor_surface_id(surface)),
            physical: !membership.physical_outputs.is_empty(),
            entered,
            preference: membership.last_preference,
        })
    }

    fn drain_events(&mut self) {
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            match self.client.peer.read(&mut buffer) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("output model event drain failed: {error}"),
            }
        }
    }

    fn verify_counters(&self, reference: &ReferenceOutputModel, context: &str) {
        for (id, expected) in &reference.surfaces {
            let actual = self.snapshot(*id).expect("production snapshot");
            assert_output_snapshot_matches(expected, &actual, *id, context);
        }
        assert_eq!(
            self.state.compliance_metrics.surface_enter_events, self.expected_enter_events,
            "{context} enter events"
        );
        assert_eq!(
            self.state.compliance_metrics.surface_leave_events, self.expected_leave_events,
            "{context} leave events"
        );
        assert_eq!(
            self.state.compliance_metrics.preferred_scale_events, self.expected_scale_events,
            "{context} preferred scale events"
        );
        assert_eq!(
            self.state.compliance_metrics.preferred_transform_events,
            self.expected_transform_events,
            "{context} preferred transform events"
        );
        assert!(self.state.check_surface_output_membership_invariants());
    }
}

fn assert_output_snapshot_matches(
    expected: &ReferenceSurface,
    actual: &ProductionSurfaceSnapshot,
    surface_id: u32,
    context: &str,
) {
    assert_eq!(
        expected.published, actual.published,
        "{context} surface={surface_id} published expected={expected:?} actual={actual:?}"
    );
    assert_eq!(
        expected.physical, actual.physical,
        "{context} surface={surface_id} physical expected={expected:?} actual={actual:?}"
    );
    assert_eq!(
        expected.entered, actual.entered,
        "{context} surface={surface_id} entered expected={expected:?} actual={actual:?}"
    );
    assert_eq!(
        expected.preference, actual.preference,
        "{context} surface={surface_id} preference expected={expected:?} actual={actual:?}"
    );
}

#[test]
fn output_model_comparison_rejects_intentional_divergence() {
    let expected = ReferenceSurface {
        mapped: true,
        width: 10,
        height: 10,
        x: 0,
        y: 0,
        parent: None,
        version: 6,
        published: true,
        entered: HashSet::new(),
        physical: false,
        preference: None,
        enter_events: 0,
        leave_events: 0,
        scale_events: 0,
        transform_events: 0,
    };
    let actual = ProductionSurfaceSnapshot {
        published: true,
        physical: true,
        entered: HashSet::from([1]),
        preference: None,
    };
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            assert_output_snapshot_matches(&expected, &actual, 7, "intentional divergence");
        }))
        .is_err()
    );
}

#[test]
fn output_production_model_runs_10_000_operations() {
    const SEED: u64 = 0x4f55_5450_5554_3130;
    let mut random = SEED;
    let mut reference = ReferenceOutputModel {
        scale: 1,
        transform: wl_output::Transform::Normal,
        ..ReferenceOutputModel::default()
    };
    let mut production = ProductionOutputModel::new();

    for operation in 0..10_000_u32 {
        random = random
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let choice = (random >> 32) % 12;
        let surface_id = if reference.surfaces.is_empty() {
            None
        } else {
            let mut ids = reference.surfaces.keys().copied().collect::<Vec<_>>();
            ids.sort_unstable();
            ids.get((random as usize) % ids.len()).copied()
        };
        match choice {
            0 => {
                let id = reference.create_surface(random);
                let surface = reference.surfaces[&id].clone();
                production.create_surface(id, surface);
            }
            1 => {
                if let Some(id) = surface_id {
                    production.destroy_surface(id);
                    reference.destroy_surface(id);
                }
            }
            2 => {
                if let Some(id) = surface_id {
                    reference
                        .surfaces
                        .get_mut(&id)
                        .expect("map reference")
                        .mapped = true;
                    reference
                        .surfaces
                        .get_mut(&id)
                        .expect("publish reference")
                        .published = true;
                    production.map_surface(id, &reference);
                }
            }
            3 => {
                if let Some(id) = surface_id {
                    reference.unmap_subtree(id);
                    production.unmap_surface(id);
                }
            }
            4 | 5 => {
                if let Some(id) = surface_id {
                    if choice == 5 {
                        let surface = reference.surfaces.get_mut(&id).expect("resize reference");
                        surface.width = i64::from((random as u32 % 100).max(1));
                        surface.height = i64::from(((random >> 16) as u32 % 100).max(1));
                        reference.reconcile_all();
                        production.move_or_resize_surface(id, &reference, true);
                    }
                    let surface = reference.surfaces.get_mut(&id).expect("move reference");
                    surface.x = (random as i16 as i64) % 220 - 60;
                    surface.y = ((random >> 8) as i16 as i64) % 220 - 60;
                    production.move_or_resize_surface(id, &reference, false);
                }
            }
            6 => {
                let id = reference.next_output;
                reference.next_output += 1;
                reference.outputs.insert(id);
                production.bind_output(id);
            }
            7 => {
                if let Some(id) = reference.outputs.iter().copied().min() {
                    production.release_output(id);
                    reference.outputs.remove(&id);
                }
            }
            8 => {
                let scale = [1, 2, 3][(random as usize) % 3];
                reference.scale = scale;
                production.state.set_output_scale_factor(f64::from(scale));
            }
            9 => {
                reference.transform = [
                    wl_output::Transform::Normal,
                    wl_output::Transform::_90,
                    wl_output::Transform::_180,
                    wl_output::Transform::_270,
                ][(random as usize) % 4];
                production
                    .state
                    .set_output_preferred_transform(reference.transform);
            }
            10 => {
                if let Some(child) = surface_id {
                    let mut candidates = reference.surfaces.keys().copied().collect::<Vec<_>>();
                    candidates.sort_unstable();
                    let parent = candidates.into_iter().find(|candidate| {
                        *candidate != child
                            && !would_reference_cycle(&reference.surfaces, child, *candidate)
                    });
                    let child_surface = reference.surfaces.get_mut(&child).expect("child");
                    child_surface.parent = parent;
                    if let Some(parent) = parent {
                        let offset_x = (random as i16 as i64) % 80 - 20;
                        let offset_y = ((random >> 8) as i16 as i64) % 80 - 20;
                        child_surface.x = offset_x;
                        child_surface.y = offset_y;
                        let _ = parent;
                    }
                    production.move_or_resize_surface(child, &reference, false);
                }
            }
            _ => {
                if let Some(id) = surface_id {
                    reference
                        .surfaces
                        .get_mut(&id)
                        .expect("root reference")
                        .parent = None;
                    production.move_or_resize_surface(id, &reference, false);
                }
            }
        }

        reference.reconcile_all();
        production.expected_enter_events = reference.enter_events_total
            + reference
                .surfaces
                .values()
                .map(|surface| surface.enter_events)
                .sum::<u64>();
        production.expected_leave_events = reference.leave_events_total
            + reference
                .surfaces
                .values()
                .map(|surface| surface.leave_events)
                .sum::<u64>();
        production.expected_scale_events = reference.scale_events_total
            + reference
                .surfaces
                .values()
                .map(|surface| surface.scale_events)
                .sum::<u64>();
        production.expected_transform_events = reference.transform_events_total
            + reference
                .surfaces
                .values()
                .map(|surface| surface.transform_events)
                .sum::<u64>();
        production.drain_events();
        production.verify_counters(
            &reference,
            &format!("seed={SEED:#x} operation={operation} choice={choice}"),
        );
    }

    for id in reference.surfaces.keys().copied().collect::<Vec<_>>() {
        production.destroy_surface(id);
    }
    for id in reference.outputs.iter().copied().collect::<Vec<_>>() {
        production.release_output(id);
    }
    assert!(production.state.surface_output_memberships.is_empty());
    assert!(
        production
            .state
            .check_surface_output_membership_invariants()
    );
}

fn would_reference_cycle(
    surfaces: &HashMap<u32, ReferenceSurface>,
    child: u32,
    parent: u32,
) -> bool {
    let mut current = Some(parent);
    while let Some(id) = current {
        if id == child {
            return true;
        }
        current = surfaces.get(&id).and_then(|surface| surface.parent);
    }
    false
}
