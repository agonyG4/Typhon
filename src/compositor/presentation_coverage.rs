use super::{DecorationRenderInstance, RenderableSurface, WindowVisualGroup};
use crate::render_backend::buffer::BufferSize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PresentationCoverageOpacity {
    OpaqueXrgb8888,
    #[default]
    Unknown,
}

impl PresentationCoverageOpacity {
    pub const fn is_proven_opaque(self) -> bool {
        matches!(self, Self::OpaqueXrgb8888)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationCoverageContentKind {
    Application,
    Popup,
    LayerShell,
    ServerSideDecoration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationCoverageApplicationGroup {
    pub root_surface_id: u32,
    pub surface_ids: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationCoverageContent {
    pub root_surface_id: u32,
    pub kind: PresentationCoverageContentKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresentationCoverageAnalysis {
    pub covering_application_group: Option<PresentationCoverageApplicationGroup>,
    pub opacity: PresentationCoverageOpacity,
    pub visible_content_above: Vec<PresentationCoverageContent>,
}

impl PresentationCoverageAnalysis {
    pub const fn geometrically_covers_output(&self) -> bool {
        self.covering_application_group.is_some()
    }

    pub const fn can_occlude_behind_content(&self) -> bool {
        self.opacity.is_proven_opaque()
    }
}

pub(crate) fn analyze_presentation_coverage(
    surfaces: &[RenderableSurface],
    decorations: &[DecorationRenderInstance],
    popup_surface_ids: &[u32],
    output_size: BufferSize,
    is_application_group: impl Fn(u32) -> bool,
    is_layer_group: impl Fn(u32) -> bool,
    covers_output: impl Fn(u32) -> bool,
    group_opacity: impl Fn(u32) -> PresentationCoverageOpacity,
) -> PresentationCoverageAnalysis {
    let groups = WindowVisualGroup::stack_order_with_popups(
        surfaces,
        decorations,
        popup_surface_ids,
    );
    let origins = super::render::surface_origins(surfaces);
    let covering_group_index = groups.iter().enumerate().rev().find_map(|(index, group)| {
        (is_application_group(group.root_surface_id())
            && !popup_surface_ids.contains(&group.root_surface_id())
            && covers_output(group.root_surface_id()))
        .then_some(index)
    });

    let Some(covering_group_index) = covering_group_index else {
        return PresentationCoverageAnalysis::default();
    };
    let covering_group = &groups[covering_group_index];
    let covering_root_surface_id = covering_group.root_surface_id();
    let mut analysis = PresentationCoverageAnalysis {
        covering_application_group: Some(PresentationCoverageApplicationGroup {
            root_surface_id: covering_root_surface_id,
            surface_ids: covering_group
                .surface_indices()
                .iter()
                .filter_map(|index| surfaces.get(*index).map(|surface| surface.surface_id))
                .collect(),
        }),
        opacity: group_opacity(covering_root_surface_id),
        visible_content_above: Vec::new(),
    };

    record_visible_decoration(
        &mut analysis.visible_content_above,
        covering_group,
        decorations,
        output_size,
    );
    for group in groups.iter().skip(covering_group_index + 1) {
        let intersects_output = group.surface_indices().iter().any(|index| {
            surfaces
                .get(*index)
                .and_then(|surface| origins.get(*index).map(|origin| (surface, *origin)))
                .is_some_and(|(surface, origin)| {
                    rect_intersects_output(
                        origin.0,
                        origin.1,
                        surface.width,
                        surface.height,
                        output_size,
                    )
                })
        });
        let kind = if popup_surface_ids.contains(&group.root_surface_id()) {
            Some(PresentationCoverageContentKind::Popup)
        } else if is_layer_group(group.root_surface_id()) {
            Some(PresentationCoverageContentKind::LayerShell)
        } else if is_application_group(group.root_surface_id()) {
            Some(PresentationCoverageContentKind::Application)
        } else {
            None
        };
        if intersects_output {
            if let Some(kind) = kind {
                analysis.visible_content_above.push(PresentationCoverageContent {
                    root_surface_id: group.root_surface_id(),
                    kind,
                });
            }
        }
        record_visible_decoration(
            &mut analysis.visible_content_above,
            group,
            decorations,
            output_size,
        );
    }

    analysis
}

fn record_visible_decoration(
    visible_content_above: &mut Vec<PresentationCoverageContent>,
    group: &WindowVisualGroup,
    decorations: &[DecorationRenderInstance],
    output_size: BufferSize,
) {
    let Some(decoration_index) = group.decoration_index() else {
        return;
    };
    let Some(decoration) = decorations.get(decoration_index) else {
        return;
    };
    if decoration.primitives().is_empty() {
        return;
    }
    let (x, y, width, height) = decoration.scene_snapshot().bounds();
    if rect_intersects_output(x, y, width, height, output_size) {
        visible_content_above.push(PresentationCoverageContent {
            root_surface_id: group.root_surface_id(),
            kind: PresentationCoverageContentKind::ServerSideDecoration,
        });
    }
}

fn rect_intersects_output(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    output_size: BufferSize,
) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    let right = i64::from(x).saturating_add(i64::from(width));
    let bottom = i64::from(y).saturating_add(i64::from(height));
    right > 0
        && bottom > 0
        && i64::from(x) < i64::from(output_size.width)
        && i64::from(y) < i64::from(output_size.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::{
        RenderableSurfaceDamage, SurfaceCommitSequence, SurfacePlacement, SurfaceRenderBackend,
    };
    use crate::render_backend::buffer::{
        BufferIdAllocator, BufferIdentity, CommittedSurfaceBuffer,
    };
    use std::sync::{Mutex, OnceLock};
    use wayland_server::protocol::wl_output;

    fn test_surface(surface_id: u32, x: i32, y: i32, width: u32, height: u32) -> RenderableSurface {
        static IDS: OnceLock<Mutex<BufferIdAllocator>> = OnceLock::new();
        let identity: BufferIdentity = IDS
            .get_or_init(|| Mutex::new(BufferIdAllocator::default()))
            .lock()
            .expect("test buffer identity allocator")
            .allocate()
            .expect("test buffer identity");
        RenderableSurface {
            surface_id,
            x: 0,
            y: 0,
            width,
            height,
            placement: SurfacePlacement::absolute_root_at(x, y),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: CommittedSurfaceBuffer::shm_snapshot(
                identity,
                BufferSize::new(width, height).expect("test surface size"),
                vec![0; (width * height) as usize],
            ),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: RenderableSurfaceDamage::full(),
        }
    }

    fn analyze(surfaces: &[RenderableSurface], apps: &[u32], layers: &[u32]) -> PresentationCoverageAnalysis {
        analyze_presentation_coverage(
            surfaces,
            &[],
            &[],
            BufferSize::new(1280, 800).expect("test output size"),
            |root| apps.contains(&root),
            |root| layers.contains(&root),
            |root| root == 10,
            |root| {
                (root == 10)
                    .then_some(PresentationCoverageOpacity::OpaqueXrgb8888)
                    .unwrap_or(PresentationCoverageOpacity::Unknown)
            },
        )
    }

    #[test]
    fn selects_covering_group_and_ignores_behind_or_outside_content() {
        let surfaces = vec![
            test_surface(1, 0, 0, 40, 40),
            test_surface(10, 0, 0, 1280, 800),
            test_surface(20, 100, 100, 40, 40),
            test_surface(30, 2000, 0, 40, 40),
        ];
        let analysis = analyze(&surfaces, &[1, 10, 20, 30], &[]);

        assert_eq!(
            analysis.covering_application_group,
            Some(PresentationCoverageApplicationGroup {
                root_surface_id: 10,
                surface_ids: vec![10],
            })
        );
        assert!(analysis.can_occlude_behind_content());
        assert_eq!(
            analysis.visible_content_above,
            vec![PresentationCoverageContent {
                root_surface_id: 20,
                kind: PresentationCoverageContentKind::Application,
            }]
        );
    }

    #[test]
    fn popup_and_layer_content_above_are_recorded_separately() {
        let surfaces = vec![
            test_surface(10, 0, 0, 1280, 800),
            test_surface(40, 1, 1, 20, 20),
            test_surface(50, 2, 2, 20, 20),
        ];
        let analysis = analyze_presentation_coverage(
            &surfaces,
            &[],
            &[40],
            BufferSize::new(1280, 800).expect("test output size"),
            |root| root == 10 || root == 40,
            |root| root == 50,
            |root| root == 10,
            |root| {
                (root == 10)
                    .then_some(PresentationCoverageOpacity::OpaqueXrgb8888)
                    .unwrap_or(PresentationCoverageOpacity::Unknown)
            },
        );

        assert_eq!(
            analysis.visible_content_above,
            vec![
                PresentationCoverageContent {
                    root_surface_id: 40,
                    kind: PresentationCoverageContentKind::Popup,
                },
                PresentationCoverageContent {
                    root_surface_id: 50,
                    kind: PresentationCoverageContentKind::LayerShell,
                },
            ]
        );
    }

    #[test]
    fn unknown_opacity_cannot_occlude_behind_content() {
        let surfaces = vec![
            test_surface(1, 0, 0, 40, 40),
            test_surface(10, 0, 0, 1280, 800),
        ];
        let analysis = analyze_presentation_coverage(
            &surfaces,
            &[],
            &[],
            BufferSize::new(1280, 800).expect("test output size"),
            |root| root == 1 || root == 10,
            |_| false,
            |root| root == 10,
            |_| PresentationCoverageOpacity::Unknown,
        );

        assert!(analysis.geometrically_covers_output());
        assert!(!analysis.can_occlude_behind_content());
    }
}
