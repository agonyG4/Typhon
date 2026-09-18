use super::{DecorationRenderInstance, RenderableSurface, SurfaceTargetRect, WindowVisualGroup};
use crate::compositor::surface::SurfaceRenderBackend;
use crate::render_backend::buffer::{BufferSize, DrmFormat, DrmModifier, SurfaceBufferSource};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PresentationCoverageOpacity {
    OpaqueRgb8888,
    #[default]
    Unknown,
}

impl PresentationCoverageOpacity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpaqueRgb8888 => "opaque_rgb8888",
            Self::Unknown => "unknown",
        }
    }

    pub const fn is_proven_opaque(self) -> bool {
        matches!(self, Self::OpaqueRgb8888)
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
pub struct PresentationCoverageSurface {
    pub surface_id: u32,
    pub target: SurfaceTargetRect,
    pub opacity: PresentationCoverageOpacity,
    pub backend: SurfaceRenderBackend,
    pub buffer_source: SurfaceBufferSource,
    pub format: Option<DrmFormat>,
    pub modifier: Option<DrmModifier>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationCoverageApplicationGroup {
    pub root_surface_id: u32,
    pub surface_ids: Vec<u32>,
    pub surface_details: Vec<PresentationCoverageSurface>,
    pub covering_surface: Option<PresentationCoverageSurface>,
    pub visible_surface_ids_above_covering: Vec<u32>,
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
    pub fn geometrically_covers_output(&self) -> bool {
        self.covering_application_group
            .as_ref()
            .is_some_and(|group| group.covering_surface.is_some())
    }

    pub const fn can_occlude_behind_content(&self) -> bool {
        self.opacity.is_proven_opaque()
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn analyze_presentation_coverage(
    surfaces: &[RenderableSurface],
    decorations: &[DecorationRenderInstance],
    render_targets: &[SurfaceTargetRect],
    popup_surface_ids: &[u32],
    output_size: BufferSize,
    is_application_group: impl Fn(u32) -> bool,
    is_layer_group: impl Fn(u32) -> bool,
    surface_opacity: impl Fn(&RenderableSurface, SurfaceTargetRect) -> PresentationCoverageOpacity,
) -> PresentationCoverageAnalysis {
    let output_rect = SurfaceTargetRect::new(0, 0, output_size.width, output_size.height);
    let groups =
        WindowVisualGroup::stack_order_with_popups(surfaces, decorations, popup_surface_ids);
    let covering_group_index = groups.iter().enumerate().rev().find_map(|(index, group)| {
        (is_application_group(group.root_surface_id())
            && !popup_surface_ids.contains(&group.root_surface_id())
            && group.surface_indices().iter().any(|surface_index| {
                render_targets
                    .get(*surface_index)
                    .is_some_and(|target| target.intersection(output_rect) == Some(output_rect))
            }))
        .then_some(index)
    });

    let Some(covering_group_index) = covering_group_index else {
        return PresentationCoverageAnalysis::default();
    };
    let covering_group = &groups[covering_group_index];
    let covering_root_surface_id = covering_group.root_surface_id();
    let surface_details = covering_group
        .surface_indices()
        .iter()
        .filter_map(|index| {
            let surface = surfaces.get(*index)?;
            let target = render_targets.get(*index).copied()?;
            Some(PresentationCoverageSurface {
                surface_id: surface.surface_id,
                target,
                opacity: surface_opacity(surface, target),
                backend: surface.render_backend,
                buffer_source: surface.buffer_source(),
                format: surface.dmabuf_handle().map(|buffer| buffer.format()),
                modifier: surface
                    .dmabuf_handle()
                    .and_then(|buffer| buffer.planes().first())
                    .map(|plane| plane.descriptor().modifier),
            })
        })
        .collect::<Vec<_>>();
    let covering_order =
        surface_details
            .iter()
            .enumerate()
            .rev()
            .find_map(|(group_order, surface)| {
                surface
                    .target
                    .intersection(output_rect)
                    .is_some_and(|intersection| intersection == output_rect)
                    .then_some(group_order)
            });
    let covering_surface = covering_order.and_then(|order| surface_details.get(order).cloned());
    let visible_surface_ids_above_covering = covering_order
        .map(|covering_order| {
            surface_details
                .iter()
                .skip(covering_order.saturating_add(1))
                .filter(|surface| surface.target.intersects(output_rect))
                .map(|surface| surface.surface_id)
                .collect()
        })
        .unwrap_or_default();
    let mut analysis = PresentationCoverageAnalysis {
        covering_application_group: Some(PresentationCoverageApplicationGroup {
            root_surface_id: covering_root_surface_id,
            surface_ids: covering_group
                .surface_indices()
                .iter()
                .filter_map(|index| surfaces.get(*index).map(|surface| surface.surface_id))
                .collect(),
            surface_details,
            covering_surface: covering_surface.clone(),
            visible_surface_ids_above_covering,
        }),
        opacity: covering_surface
            .as_ref()
            .map_or(PresentationCoverageOpacity::Unknown, |surface| {
                surface.opacity
            }),
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
            render_targets.get(*index).is_some_and(|target| {
                target.intersects(SurfaceTargetRect::new(
                    0,
                    0,
                    output_size.width,
                    output_size.height,
                ))
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
        if intersects_output && let Some(kind) = kind {
            analysis
                .visible_content_above
                .push(PresentationCoverageContent {
                    root_surface_id: group.root_surface_id(),
                    kind,
                });
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
    use crate::compositor::render;
    use crate::compositor::{
        DecorationRenderInstance, RenderableSurfaceDamage, SurfaceCommitSequence, SurfacePlacement,
        SurfaceRenderBackend, WindowId,
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

    fn analyze(
        surfaces: &[RenderableSurface],
        apps: &[u32],
        layers: &[u32],
    ) -> PresentationCoverageAnalysis {
        let origins = render::surface_origins(surfaces);
        let render_targets = render::surface_render_space_targets(surfaces, &origins, 1.0);
        analyze_presentation_coverage(
            surfaces,
            &[],
            &render_targets,
            &[],
            BufferSize::new(1280, 800).expect("test output size"),
            |root| apps.contains(&root),
            |root| layers.contains(&root),
            |surface, _target| {
                if surface.surface_id == 10 {
                    PresentationCoverageOpacity::OpaqueRgb8888
                } else {
                    PresentationCoverageOpacity::Unknown
                }
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
                surface_details: vec![PresentationCoverageSurface {
                    surface_id: 10,
                    target: SurfaceTargetRect::new(0, 0, 1280, 800),
                    opacity: PresentationCoverageOpacity::OpaqueRgb8888,
                    backend: SurfaceRenderBackend::NativeWayland,
                    buffer_source: SurfaceBufferSource::Shm,
                    format: None,
                    modifier: None,
                }],
                covering_surface: Some(PresentationCoverageSurface {
                    surface_id: 10,
                    target: SurfaceTargetRect::new(0, 0, 1280, 800),
                    opacity: PresentationCoverageOpacity::OpaqueRgb8888,
                    backend: SurfaceRenderBackend::NativeWayland,
                    buffer_source: SurfaceBufferSource::Shm,
                    format: None,
                    modifier: None,
                }),
                visible_surface_ids_above_covering: Vec::new(),
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
    fn rendered_target_bounds_keep_intersecting_content_above_visible() {
        let mut above = test_surface(20, -40, 0, 40, 40);
        above.render_target_size = Some(BufferSize::new(200, 40).expect("render target size"));
        let surfaces = vec![test_surface(10, 0, 0, 1280, 800), above];

        let analysis = analyze(&surfaces, &[10, 20], &[]);

        assert!(
            analysis
                .visible_content_above
                .contains(&PresentationCoverageContent {
                    root_surface_id: 20,
                    kind: PresentationCoverageContentKind::Application,
                })
        );
    }

    #[test]
    fn selects_highest_full_output_surface_as_visual_scanout_source() {
        let root = test_surface(10, 0, 0, 1280, 800);
        let mut source = test_surface(11, 0, 0, 1280, 800);
        source.placement = SurfacePlacement::subsurface(10, 0, 0);
        let mut above = test_surface(12, 0, 0, 32, 32);
        above.placement = SurfacePlacement::subsurface(10, 0, 0);
        let surfaces = vec![root, source, above];
        let origins = render::surface_origins(&surfaces);
        let render_targets = render::surface_render_space_targets(&surfaces, &origins, 1.0);
        let analysis = analyze_presentation_coverage(
            &surfaces,
            &[],
            &render_targets,
            &[],
            BufferSize::new(1280, 800).expect("test output size"),
            |root| root == 10,
            |_| false,
            |surface, _target| {
                if surface.surface_id == 11 {
                    PresentationCoverageOpacity::OpaqueRgb8888
                } else {
                    PresentationCoverageOpacity::Unknown
                }
            },
        );

        let group = analysis
            .covering_application_group
            .expect("covering application group");
        assert_eq!(
            group
                .covering_surface
                .as_ref()
                .map(|surface| surface.surface_id),
            Some(11)
        );
        assert_eq!(group.visible_surface_ids_above_covering, vec![12]);
        assert_eq!(
            group
                .surface_details
                .iter()
                .map(|surface| surface.surface_id)
                .collect::<Vec<_>>(),
            vec![10, 11, 12]
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
            &render::surface_render_space_targets(
                &surfaces,
                &render::surface_origins(&surfaces),
                1.0,
            ),
            &[40],
            BufferSize::new(1280, 800).expect("test output size"),
            |root| root == 10 || root == 40,
            |root| root == 50,
            |surface, _target| {
                if surface.surface_id == 10 {
                    PresentationCoverageOpacity::OpaqueRgb8888
                } else {
                    PresentationCoverageOpacity::Unknown
                }
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
            &render::surface_render_space_targets(
                &surfaces,
                &render::surface_origins(&surfaces),
                1.0,
            ),
            &[],
            BufferSize::new(1280, 800).expect("test output size"),
            |root| root == 1 || root == 10,
            |_| false,
            |_, _| PresentationCoverageOpacity::Unknown,
        );

        assert!(analysis.geometrically_covers_output());
        assert!(!analysis.can_occlude_behind_content());
    }

    #[test]
    fn visible_server_decoration_is_content_above_the_client_surface() {
        let surfaces = [test_surface(10, 0, 0, 1280, 800)];
        let decoration = DecorationRenderInstance::test_solid(
            WindowId::from_raw(1).expect("test window id"),
            10,
            0,
            0,
            1280,
            24,
            [0xff, 0, 0, 0xff],
        );
        let analysis = analyze_presentation_coverage(
            &surfaces,
            &[decoration],
            &render::surface_render_space_targets(
                &surfaces,
                &render::surface_origins(&surfaces),
                1.0,
            ),
            &[],
            BufferSize::new(1280, 800).expect("test output size"),
            |root| root == 10,
            |_| false,
            |_, _| PresentationCoverageOpacity::OpaqueRgb8888,
        );

        assert_eq!(
            analysis.visible_content_above,
            vec![PresentationCoverageContent {
                root_surface_id: 10,
                kind: PresentationCoverageContentKind::ServerSideDecoration,
            }]
        );
    }
}
