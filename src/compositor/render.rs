use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::WindowId;
use super::decoration::render_plan::{DecorationRenderPlan, DecorationRenderPrimitive};
use super::decoration::types::DecorationRect;
use super::{
    ClientCursorRenderState, RenderableSurface, RenderableSurfaceDamage, RootPlacementMode,
    SurfaceDamageRect, SurfaceRenderBackend,
};
use crate::cursor_theme::{CompositorCursorImage, shared_compositor_cursor_image};
use crate::render_backend::buffer::{BufferSize, SurfaceBufferSource};
#[cfg(test)]
use wayland_server::protocol::wl_output;

pub const OUTPUT_BACKGROUND: u32 = 0xff08_0a0e;
#[cfg(test)]
pub const CURSOR_FILL: u32 = 0xffff_ffff;
#[cfg(test)]
pub const CURSOR_OUTLINE: u32 = 0xff10_1116;
pub const FIRST_SURFACE_OFFSET: (i32, i32) = (72, 72);
pub const SURFACE_CASCADE_STEP: i32 = 32;
pub const SERVER_FRAME_BORDER_THICKNESS: i32 = 6;
pub const SERVER_FRAME_BORDER_COLOR: u32 = 0xff0a_0d12;
pub const SERVER_FRAME_TITLEBAR_COLOR: u32 = 0xff1a_2029;
pub const SERVER_FRAME_SEPARATOR_COLOR: u32 = 0xff2e_3644;
pub const OUTPUT_SCALE_DENOMINATOR: u32 = 120;
pub const MAX_BUFFER_AGE: u32 = 4;

pub const fn cascaded_root_position(root_index: usize) -> (i32, i32) {
    let cascade = root_index as i32 * SURFACE_CASCADE_STEP;
    (
        FIRST_SURFACE_OFFSET.0 + cascade,
        FIRST_SURFACE_OFFSET.1 + cascade,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesktopVisualState {
    pub cursor: Option<(i32, i32)>,
}

impl DesktopVisualState {
    pub const fn wallpaper_only() -> Self {
        Self { cursor: None }
    }

    pub const fn with_cursor(cursor_x: i32, cursor_y: i32) -> Self {
        Self {
            cursor: Some((cursor_x, cursor_y)),
        }
    }
}

impl Default for DesktopVisualState {
    fn default() -> Self {
        Self::with_cursor(640, 400)
    }
}

pub struct DesktopComposeRequest<'a> {
    pub frame: &'a mut [u32],
    pub frame_width: u32,
    pub frame_height: u32,
    pub output_scale: f64,
    pub surfaces: &'a [RenderableSurface],
    pub external_overlay_surface_ids: Vec<u32>,
    pub content_generation: u64,
    pub visual_state: DesktopVisualState,
    pub client_cursor: Option<ClientCursorRenderState<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecorationSceneSnapshot {
    window_id: WindowId,
    root_surface_id: u32,
    bounds: DecorationRect,
    visual_signature: u64,
}

impl DecorationSceneSnapshot {
    pub fn from_bounds(
        window_id: WindowId,
        root_surface_id: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        visual_signature: u64,
    ) -> Self {
        Self {
            window_id,
            root_surface_id,
            bounds: DecorationRect::new(x, y, width, height),
            visual_signature,
        }
    }

    pub fn identity(&self) -> (WindowId, u32) {
        (self.window_id, self.root_surface_id)
    }

    pub fn bounds(&self) -> (i32, i32, u32, u32) {
        (
            self.bounds.x,
            self.bounds.y,
            self.bounds.width,
            self.bounds.height,
        )
    }

    pub fn visual_signature(&self) -> u64 {
        self.visual_signature
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecorationRenderInstance {
    pub(crate) plan: DecorationRenderPlan,
    pub(crate) origin_x: i32,
    pub(crate) origin_y: i32,
    pub(crate) window_id: WindowId,
    pub(crate) root_surface_id: u32,
}

impl DecorationRenderInstance {
    pub fn primitives(&self) -> &[DecorationRenderPrimitive] {
        &self.plan.primitives
    }

    pub fn origin(&self) -> (i32, i32) {
        (self.origin_x, self.origin_y)
    }

    pub fn scene_snapshot(&self) -> DecorationSceneSnapshot {
        DecorationSceneSnapshot::from_bounds(
            self.window_id,
            self.root_surface_id,
            self.origin_x.saturating_add(self.plan.layout.outer.x),
            self.origin_y.saturating_add(self.plan.layout.outer.y),
            self.plan.layout.outer.width,
            self.plan.layout.outer.height,
            self.plan.visual_signature(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualStackGroup {
    root_surface_id: u32,
    root_surface_index: usize,
    surface_indices: Vec<usize>,
    popup: bool,
}

impl VisualStackGroup {
    pub fn root_surface_id(&self) -> u32 {
        self.root_surface_id
    }

    pub fn root_surface_index(&self) -> usize {
        self.root_surface_index
    }

    pub fn surface_indices(&self) -> &[usize] {
        &self.surface_indices
    }

    pub fn is_popup(&self) -> bool {
        self.popup
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowVisualGroup {
    visual: VisualStackGroup,
    decoration_index: Option<usize>,
}

impl WindowVisualGroup {
    pub fn root_surface_id(&self) -> u32 {
        self.visual.root_surface_id()
    }

    pub fn surface_indices(&self) -> &[usize] {
        self.visual.surface_indices()
    }

    pub fn decoration_index(&self) -> Option<usize> {
        self.decoration_index
    }
    pub fn orphan_decoration_count(
        surfaces: &[RenderableSurface],
        decorations: &[DecorationRenderInstance],
    ) -> u32 {
        let root_indices = surface_root_indices(surfaces);
        let mut orphan_decoration_count = 0u32;
        for decoration in decorations {
            let has_live_root = surfaces.iter().enumerate().any(|(index, surface)| {
                root_indices.get(index) == Some(&index)
                    && surface.surface_id == decoration.root_surface_id
            });
            if !has_live_root {
                orphan_decoration_count = orphan_decoration_count.saturating_add(1);
            }
        }
        orphan_decoration_count
    }

    pub fn stack_order_with_popups(
        surfaces: &[RenderableSurface],
        decorations: &[DecorationRenderInstance],
        popup_surface_ids: &[u32],
    ) -> Vec<Self> {
        window_visual_stack_order_with_popups(surfaces, decorations, popup_surface_ids)
    }
}

/// Return the shared back-to-front visual ownership groups used by rendering
/// and input. Ordinary client subsurfaces stay with their root window; popup
/// roots split into their own group so they remain above the parent SSD.
pub fn visual_stack_groups(
    surfaces: &[RenderableSurface],
    popup_surface_ids: &[u32],
) -> Vec<VisualStackGroup> {
    let root_indices = surface_root_indices(surfaces);
    let popup_surface_ids = popup_surface_ids.iter().copied().collect::<HashSet<_>>();
    let index_by_id = surfaces
        .iter()
        .enumerate()
        .map(|(index, surface)| (surface.surface_id, index))
        .collect::<HashMap<_, _>>();
    let visual_roots = surfaces
        .iter()
        .enumerate()
        .map(|(index, _)| {
            visual_group_root_index(
                index,
                surfaces,
                &index_by_id,
                &root_indices,
                &popup_surface_ids,
            )
        })
        .collect::<Vec<_>>();
    let mut group_roots = Vec::new();
    let mut seen_group_roots = HashSet::new();
    for root_index in visual_roots.iter().copied() {
        if seen_group_roots.insert(root_index) {
            group_roots.push(root_index);
        }
    }
    group_roots
        .into_iter()
        .map(|root_index| VisualStackGroup {
            root_surface_id: surfaces[root_index].surface_id,
            root_surface_index: root_index,
            surface_indices: visual_roots
                .iter()
                .enumerate()
                .filter_map(|(index, root)| (*root == root_index).then_some(index))
                .collect(),
            popup: popup_surface_ids.contains(&surfaces[root_index].surface_id),
        })
        .collect()
}

/// Return the authoritative back-to-front normal-window visual order.
///
/// A group owns its root and every descendant surface, plus the optional SSD
/// for that root.  Renderers may choose a different command representation,
/// but they must consume this ownership order rather than appending all SSD
/// primitives after unrelated windows.
pub fn window_visual_stack_order(
    surfaces: &[RenderableSurface],
    decorations: &[DecorationRenderInstance],
) -> Vec<WindowVisualGroup> {
    window_visual_stack_order_with_popups(surfaces, decorations, &[])
}

/// Return visual ownership order while keeping XDG popups above their parent
/// window's server-side decoration. Ordinary client subsurfaces remain in
/// their normal window group.
fn window_visual_stack_order_with_popups(
    surfaces: &[RenderableSurface],
    decorations: &[DecorationRenderInstance],
    popup_surface_ids: &[u32],
) -> Vec<WindowVisualGroup> {
    visual_stack_groups(surfaces, popup_surface_ids)
        .into_iter()
        .map(|visual| WindowVisualGroup {
            decoration_index: (!visual.is_popup())
                .then(|| {
                    decorations.iter().position(|decoration| {
                        decoration.root_surface_id == visual.root_surface_id()
                    })
                })
                .flatten(),
            visual,
        })
        .collect()
}

fn visual_group_root_index(
    index: usize,
    surfaces: &[RenderableSurface],
    index_by_id: &HashMap<u32, usize>,
    root_indices: &[usize],
    popup_surface_ids: &HashSet<u32>,
) -> usize {
    let mut current = index;
    let mut visited = HashSet::new();
    loop {
        if popup_surface_ids.contains(&surfaces[current].surface_id) {
            return current;
        }
        if !visited.insert(current) {
            break;
        }
        let placement = surfaces[current]
            .render_placement
            .unwrap_or(surfaces[current].placement);
        let Some(parent_index) = placement
            .parent_surface_id
            .and_then(|parent_id| index_by_id.get(&parent_id).copied())
            .filter(|parent_index| *parent_index != current)
        else {
            break;
        };
        current = parent_index;
    }
    root_indices.get(index).copied().unwrap_or(index)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DesktopSceneRebuildKind {
    #[default]
    None,
    Full,
    Partial,
}

impl DesktopSceneRebuildKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Full => "full",
            Self::Partial => "partial",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BufferAge {
    #[default]
    Reset,
    Age(u32),
    Unknown,
}

impl BufferAge {
    pub const fn normalized(self) -> Self {
        match self {
            Self::Age(0) => Self::Reset,
            Self::Age(age) if age > MAX_BUFFER_AGE => Self::Age(MAX_BUFFER_AGE),
            age => age,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DesktopFrameCopyKind {
    #[default]
    None,
    Full,
    Partial,
}

impl DesktopFrameCopyKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Full => "full",
            Self::Partial => "partial",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DamageDebugStats {
    pub kind: DesktopSceneRebuildKind,
    pub rect_count: u32,
    pub damaged_area: u32,
    pub frame_area: u32,
}

impl DamageDebugStats {
    pub fn full(frame_width: u32, frame_height: u32) -> Self {
        let frame_area = frame_width.saturating_mul(frame_height);
        Self {
            kind: DesktopSceneRebuildKind::Full,
            rect_count: (frame_area > 0) as u32,
            damaged_area: frame_area,
            frame_area,
        }
    }

    pub fn partial<const N: usize>(
        frame_width: u32,
        frame_height: u32,
        rects: [Option<SurfaceDamageRect>; N],
    ) -> Self {
        let mut rect_count = 0;
        let mut damaged_area = 0u32;
        for rect in rects.into_iter().flatten() {
            rect_count += 1;
            damaged_area = damaged_area.saturating_add(rect.width.saturating_mul(rect.height));
        }
        Self {
            kind: if rect_count == 0 {
                DesktopSceneRebuildKind::None
            } else {
                DesktopSceneRebuildKind::Partial
            },
            rect_count,
            damaged_area,
            frame_area: frame_width.saturating_mul(frame_height),
        }
    }

    pub fn coverage_percent(self) -> u32 {
        if self.frame_area == 0 {
            return 0;
        }
        self.damaged_area.saturating_mul(100) / self.frame_area
    }

    fn from_output_rects(frame_width: u32, frame_height: u32, rects: &[OutputRect]) -> Self {
        let mut rect_count = 0;
        let mut damaged_area = 0u32;
        for rect in rects {
            if rect.width == 0 || rect.height == 0 {
                continue;
            }
            rect_count += 1;
            damaged_area = damaged_area.saturating_add(rect.width.saturating_mul(rect.height));
        }
        Self {
            kind: if rect_count == 0 {
                DesktopSceneRebuildKind::None
            } else {
                DesktopSceneRebuildKind::Partial
            },
            rect_count,
            damaged_area,
            frame_area: frame_width.saturating_mul(frame_height),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct SceneSurfaceSnapshot {
    surface_id: u32,
    generation: u64,
    target: SurfaceTargetRect,
    visible_target: SurfaceTargetRect,
    backing_target: Option<SurfaceTargetRect>,
    content_regions: Vec<SurfaceRenderPlan>,
    buffer_width: u32,
    buffer_height: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderSceneElement {
    id: RenderSceneElementId,
    kind: RenderSceneElementKind,
    target: SurfaceTargetRect,
    visible_target: SurfaceTargetRect,
    backing_target: Option<SurfaceTargetRect>,
    content_regions: Vec<SurfaceRenderPlan>,
    content_uv: SurfaceUvRect,
    generation: u64,
    buffer_size: BufferSize,
    buffer_source: SurfaceBufferSource,
    damage: RenderableSurfaceDamage,
}

impl RenderSceneElement {
    pub fn from_surface(surface: &RenderableSurface, target: SurfaceTargetRect) -> Self {
        Self::from_surface_with_aperture(surface, target, surface.visual_clip.clone())
    }

    pub fn from_surface_with_clip(
        surface: &RenderableSurface,
        target: SurfaceTargetRect,
        visual_clip: Option<SurfaceTargetRect>,
    ) -> Self {
        Self::from_surface_with_aperture(
            surface,
            target,
            visual_clip.map(SurfaceVisualAperture::logical_only),
        )
    }

    pub fn from_surface_with_aperture(
        surface: &RenderableSurface,
        target: SurfaceTargetRect,
        visual_aperture: Option<SurfaceVisualAperture>,
    ) -> Self {
        let content_regions =
            surface_render_plans_with_aperture(surface, target, visual_aperture.as_ref());
        let visible_target = content_regions
            .iter()
            .map(|plan| plan.content_target)
            .reduce(SurfaceTargetRect::union)
            .unwrap_or(target);
        Self {
            id: RenderSceneElementId::Surface(surface.surface_id),
            kind: RenderSceneElementKind::ClientSurface,
            target,
            visible_target,
            backing_target: xwayland_visual_backing_target(surface, visual_aperture.as_ref()),
            content_uv: content_regions
                .first()
                .map_or(SurfaceUvRect::FULL, |plan| plan.content_uv),
            content_regions,
            generation: surface.generation,
            buffer_size: surface.buffer_size(),
            buffer_source: surface.buffer_source(),
            damage: surface.damage.clone(),
        }
    }

    pub const fn id(&self) -> RenderSceneElementId {
        self.id
    }

    pub const fn kind(&self) -> RenderSceneElementKind {
        self.kind
    }

    pub const fn target(&self) -> SurfaceTargetRect {
        self.target
    }

    pub const fn visible_target(&self) -> SurfaceTargetRect {
        self.visible_target
    }

    pub const fn backing_target(&self) -> Option<SurfaceTargetRect> {
        self.backing_target
    }

    pub fn content_regions(&self) -> &[SurfaceRenderPlan] {
        &self.content_regions
    }

    pub const fn content_uv(&self) -> SurfaceUvRect {
        self.content_uv
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn buffer_size(&self) -> BufferSize {
        self.buffer_size
    }

    pub const fn buffer_source(&self) -> SurfaceBufferSource {
        self.buffer_source
    }

    pub const fn damage(&self) -> &RenderableSurfaceDamage {
        &self.damage
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RenderSceneElementId {
    Surface(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderSceneElementKind {
    ClientSurface,
}

struct SceneFullRebuild<'a> {
    frame_width: u32,
    frame_height: u32,
    surfaces: &'a [RenderableSurface],
    content_generation: u64,
    output_scale_key: u32,
    output_scale: f64,
    snapshots: Vec<SceneSurfaceSnapshot>,
}

#[derive(Debug)]
pub struct DesktopSceneRenderer {
    cursor_image: Arc<CompositorCursorImage>,
    scene: Vec<u32>,
    scene_width: u32,
    scene_height: u32,
    scene_output_scale_key: u32,
    scene_content_generation: u64,
    scene_generation: u64,
    scene_surface_snapshots: Vec<SceneSurfaceSnapshot>,
    scene_popup_surface_ids: Vec<u32>,
    last_rebuild_damage_rects: Vec<OutputRect>,
    last_rebuild_kind: DesktopSceneRebuildKind,
    last_frame_copy_kind: DesktopFrameCopyKind,
    last_damage_debug_stats: DamageDebugStats,
    last_orphan_decoration_count: u32,
    reusable_frame_key: Option<ReusableFrameKey>,
    reusable_frame_had_client_cursor: bool,
    decoration_instances: Vec<DecorationRenderInstance>,
    decoration_damage_rects: Vec<DecorationRect>,
    popup_surface_ids: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReusableFrameKey {
    width: u32,
    height: u32,
    output_scale_key: u32,
    visual_state: DesktopVisualState,
}

impl DesktopSceneRenderer {
    pub fn with_cursor_image(cursor_image: Arc<CompositorCursorImage>) -> Self {
        Self {
            cursor_image,
            scene: Vec::new(),
            scene_width: 0,
            scene_height: 0,
            scene_output_scale_key: 0,
            scene_content_generation: 0,
            scene_generation: 0,
            scene_surface_snapshots: Vec::new(),
            scene_popup_surface_ids: Vec::new(),
            last_rebuild_damage_rects: Vec::new(),
            last_rebuild_kind: DesktopSceneRebuildKind::default(),
            last_frame_copy_kind: DesktopFrameCopyKind::default(),
            last_damage_debug_stats: DamageDebugStats::default(),
            last_orphan_decoration_count: 0,
            reusable_frame_key: None,
            reusable_frame_had_client_cursor: false,
            decoration_instances: Vec::new(),
            decoration_damage_rects: Vec::new(),
            popup_surface_ids: Vec::new(),
        }
    }

    pub fn set_cursor_image(&mut self, cursor_image: Arc<CompositorCursorImage>) {
        self.cursor_image = cursor_image;
        self.reusable_frame_key = None;
    }

    pub fn set_decoration_instances(&mut self, instances: &[DecorationRenderInstance]) {
        if self.decoration_instances == instances {
            return;
        }
        let previous = self
            .decoration_instances
            .iter()
            .map(DecorationRenderInstance::scene_snapshot)
            .collect::<Vec<_>>();
        let current = instances
            .iter()
            .map(DecorationRenderInstance::scene_snapshot)
            .collect::<Vec<_>>();
        self.decoration_damage_rects = previous
            .into_iter()
            .chain(current)
            .map(|snapshot| {
                let (x, y, width, height) = snapshot.bounds();
                DecorationRect::new(x, y, width, height)
            })
            .collect();
        self.decoration_instances.clear();
        self.decoration_instances.extend_from_slice(instances);
    }

    pub fn set_popup_surface_ids(&mut self, popup_surface_ids: &[u32]) {
        let mut normalized = popup_surface_ids.to_vec();
        normalized.sort_unstable();
        normalized.dedup();
        if self.popup_surface_ids == normalized {
            return;
        }
        self.popup_surface_ids = normalized;
        self.reusable_frame_key = None;
    }

    pub fn compose(
        &mut self,
        frame: &mut [u32],
        frame_width: u32,
        frame_height: u32,
        surfaces: &[RenderableSurface],
        visual_state: DesktopVisualState,
    ) {
        self.rebuild_scene(
            frame_width,
            frame_height,
            surfaces,
            self.scene_content_generation + 1,
            1.0,
            BufferAge::Age(1),
        );
        self.copy_scene_to_frame(frame, frame_width, frame_height);
        self.decoration_damage_rects.clear();
        if let Some((cursor_x, cursor_y)) = visual_state.cursor {
            draw_cursor(
                frame,
                frame_width,
                frame_height,
                cursor_x,
                cursor_y,
                &self.cursor_image,
            );
        }
    }

    pub fn compose_with_generation(
        &mut self,
        frame: &mut [u32],
        frame_width: u32,
        frame_height: u32,
        surfaces: &[RenderableSurface],
        content_generation: u64,
        visual_state: DesktopVisualState,
    ) {
        self.rebuild_scene(
            frame_width,
            frame_height,
            surfaces,
            content_generation,
            1.0,
            BufferAge::Age(1),
        );
        self.copy_scene_to_frame(frame, frame_width, frame_height);
        self.decoration_damage_rects.clear();
        if let Some((cursor_x, cursor_y)) = visual_state.cursor {
            draw_cursor(
                frame,
                frame_width,
                frame_height,
                cursor_x,
                cursor_y,
                &self.cursor_image,
            );
        }
    }

    pub fn compose_request(&mut self, request: DesktopComposeRequest<'_>) {
        self.compose_request_internal(request, false, BufferAge::Age(1));
    }

    pub fn compose_reusing_frame(&mut self, request: DesktopComposeRequest<'_>) {
        self.compose_request_internal(request, true, BufferAge::Age(1));
    }

    pub fn compose_request_with_buffer_age(
        &mut self,
        request: DesktopComposeRequest<'_>,
        buffer_age: BufferAge,
    ) {
        self.compose_request_internal(request, true, buffer_age);
    }

    fn compose_request_internal(
        &mut self,
        request: DesktopComposeRequest<'_>,
        reuse_frame: bool,
        buffer_age: BufferAge,
    ) {
        let DesktopComposeRequest {
            frame,
            frame_width,
            frame_height,
            output_scale,
            surfaces,
            external_overlay_surface_ids,
            content_generation,
            visual_state,
            client_cursor,
        } = request;
        let (base_surfaces, overlay_surfaces) =
            split_external_overlay_surfaces(surfaces, &external_overlay_surface_ids);
        let scene_surfaces = if external_overlay_surface_ids.is_empty() {
            surfaces
        } else {
            base_surfaces.as_slice()
        };

        self.rebuild_scene(
            frame_width,
            frame_height,
            scene_surfaces,
            content_generation,
            output_scale,
            if external_overlay_surface_ids.is_empty() {
                buffer_age
            } else {
                BufferAge::Reset
            },
        );
        let output_scale_key = output_scale_key(output_scale);
        let scaled_visual_state = scale_desktop_visual_state(visual_state, output_scale);
        let frame_key = ReusableFrameKey {
            width: frame_width,
            height: frame_height,
            output_scale_key,
            visual_state: scaled_visual_state,
        };
        let has_partial_damage =
            !self.last_rebuild_damage_rects.is_empty() || !self.decoration_damage_rects.is_empty();
        let partial_frame_copy = reuse_frame
            && self.reusable_frame_key == Some(frame_key)
            && scaled_visual_state.cursor.is_none()
            && client_cursor.is_none()
            && !self.reusable_frame_had_client_cursor
            && self.last_rebuild_kind != DesktopSceneRebuildKind::Full
            && has_partial_damage
            && frame.len() == self.scene.len();
        let no_frame_copy = reuse_frame
            && self.reusable_frame_key == Some(frame_key)
            && scaled_visual_state.cursor.is_none()
            && client_cursor.is_none()
            && !self.reusable_frame_had_client_cursor
            && self.last_rebuild_kind == DesktopSceneRebuildKind::None
            && frame.len() == self.scene.len();
        if partial_frame_copy {
            self.copy_scene_damage_to_frame(frame, frame_width, frame_height, output_scale);
        } else if no_frame_copy {
            self.last_frame_copy_kind = DesktopFrameCopyKind::None;
        } else {
            self.copy_scene_to_frame(frame, frame_width, frame_height);
        }
        if !overlay_surfaces.is_empty() {
            draw_client_surfaces_scaled(
                frame,
                frame_width,
                frame_height,
                &overlay_surfaces,
                output_scale,
            );
        }
        if client_cursor.is_none()
            && let Some((cursor_x, cursor_y)) = scaled_visual_state.cursor
        {
            draw_cursor(
                frame,
                frame_width,
                frame_height,
                cursor_x,
                cursor_y,
                &self.cursor_image,
            );
        }
        if let Some(cursor) = client_cursor {
            draw_client_cursor(frame, frame_width, frame_height, cursor, output_scale);
        }
        self.decoration_damage_rects.clear();
        self.reusable_frame_key = reuse_frame.then_some(frame_key);
        self.reusable_frame_had_client_cursor = reuse_frame && client_cursor.is_some();
    }

    pub fn scene_generation(&self) -> u64 {
        self.scene_generation
    }

    pub fn last_rebuild_kind(&self) -> DesktopSceneRebuildKind {
        self.last_rebuild_kind
    }

    pub fn last_frame_copy_kind(&self) -> DesktopFrameCopyKind {
        self.last_frame_copy_kind
    }

    pub fn last_damage_debug_stats(&self) -> DamageDebugStats {
        self.last_damage_debug_stats
    }

    pub fn last_orphan_decoration_count(&self) -> u32 {
        self.last_orphan_decoration_count
    }

    fn rebuild_scene(
        &mut self,
        frame_width: u32,
        frame_height: u32,
        surfaces: &[RenderableSurface],
        content_generation: u64,
        output_scale: f64,
        buffer_age: BufferAge,
    ) {
        self.last_orphan_decoration_count =
            WindowVisualGroup::orphan_decoration_count(surfaces, &self.decoration_instances);
        let output_scale_key = output_scale_key(output_scale);

        let pixel_count = frame_width.saturating_mul(frame_height) as usize;
        let scene_ready = self.scene_width == frame_width
            && self.scene_height == frame_height
            && self.scene_output_scale_key == output_scale_key
            && self.scene.len() == pixel_count
            && self.scene_popup_surface_ids == self.popup_surface_ids;
        let decoration_dirty = !self.decoration_damage_rects.is_empty();
        if scene_ready && self.scene_content_generation == content_generation && !decoration_dirty {
            self.last_rebuild_damage_rects.clear();
            self.last_rebuild_kind = DesktopSceneRebuildKind::None;
            self.last_damage_debug_stats = DamageDebugStats::partial(frame_width, frame_height, []);
            return;
        }

        let elements = render_scene_elements_for_surfaces(surfaces, output_scale);
        let snapshots = scene_surface_snapshots_from_elements(&elements);
        if scene_ready
            && self.rebuild_scene_from_age(
                frame_width,
                frame_height,
                surfaces,
                content_generation,
                output_scale,
                &elements,
                &snapshots,
                buffer_age,
            )
        {
            return;
        }

        self.rebuild_full_scene(SceneFullRebuild {
            frame_width,
            frame_height,
            surfaces,
            content_generation,
            output_scale_key,
            output_scale,
            snapshots,
        });
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "hot scene rebuild path passes borrowed frame state directly to avoid transient config allocation"
    )]
    fn rebuild_scene_from_old_snapshots(
        &mut self,
        frame_width: u32,
        frame_height: u32,
        surfaces: &[RenderableSurface],
        content_generation: u64,
        output_scale: f64,
        elements: &[RenderSceneElement],
        snapshots: &[SceneSurfaceSnapshot],
    ) -> bool {
        let Some(mut damage_rects) = partial_scene_damage_rects(
            &self.scene_surface_snapshots,
            elements,
            snapshots,
            frame_width,
            frame_height,
        ) else {
            return false;
        };
        damage_rects.extend(self.decoration_damage_rects.iter().filter_map(|rect| {
            decoration_damage_output_rect(*rect, output_scale, frame_width, frame_height)
        }));
        damage_rects = coalesce_output_rects(damage_rects);

        if damage_rects.is_empty() {
            self.scene_content_generation = content_generation;
            self.scene_surface_snapshots = snapshots.to_vec();
            self.last_rebuild_damage_rects.clear();
            self.last_rebuild_kind = DesktopSceneRebuildKind::None;
            self.last_damage_debug_stats = DamageDebugStats::partial(frame_width, frame_height, []);
            return true;
        }

        for damage_rect in damage_rects.iter().copied() {
            fill_output_background_rect(&mut self.scene, frame_width, frame_height, damage_rect);
            draw_window_visual_groups(WindowVisualDrawRequest {
                frame: &mut self.scene,
                frame_width,
                frame_height,
                surfaces,
                snapshots,
                output_scale,
                decorations: &self.decoration_instances,
                popup_surface_ids: &self.popup_surface_ids,
                clip: Some(damage_rect),
            });
        }

        self.scene_content_generation = content_generation;
        self.scene_surface_snapshots = snapshots.to_vec();
        self.scene_popup_surface_ids = self.popup_surface_ids.clone();
        self.last_rebuild_damage_rects = damage_rects;
        self.scene_generation = self.scene_generation.saturating_add(1);
        self.last_rebuild_kind = DesktopSceneRebuildKind::Partial;
        self.last_damage_debug_stats = DamageDebugStats::from_output_rects(
            frame_width,
            frame_height,
            &self.last_rebuild_damage_rects,
        );
        true
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "hot scene rebuild path passes borrowed frame state directly to avoid transient config allocation"
    )]
    fn rebuild_scene_from_age(
        &mut self,
        frame_width: u32,
        frame_height: u32,
        surfaces: &[RenderableSurface],
        content_generation: u64,
        output_scale: f64,
        elements: &[RenderSceneElement],
        snapshots: &[SceneSurfaceSnapshot],
        buffer_age: BufferAge,
    ) -> bool {
        match buffer_age.normalized() {
            BufferAge::Reset | BufferAge::Unknown => false,
            BufferAge::Age(_) => self.rebuild_scene_from_old_snapshots(
                frame_width,
                frame_height,
                surfaces,
                content_generation,
                output_scale,
                elements,
                snapshots,
            ),
        }
    }

    fn rebuild_full_scene(&mut self, rebuild: SceneFullRebuild<'_>) {
        let SceneFullRebuild {
            frame_width,
            frame_height,
            surfaces,
            content_generation,
            output_scale_key,
            output_scale,
            snapshots,
        } = rebuild;
        let pixel_count = frame_width.saturating_mul(frame_height) as usize;
        self.scene_width = frame_width;
        self.scene_height = frame_height;
        self.scene_output_scale_key = output_scale_key;
        self.scene_content_generation = content_generation;
        self.scene.resize(pixel_count, OUTPUT_BACKGROUND);
        self.scene.fill(OUTPUT_BACKGROUND);

        draw_window_visual_groups(WindowVisualDrawRequest {
            frame: &mut self.scene,
            frame_width,
            frame_height,
            surfaces,
            snapshots: &snapshots,
            output_scale,
            decorations: &self.decoration_instances,
            popup_surface_ids: &self.popup_surface_ids,
            clip: None,
        });
        self.scene_surface_snapshots = snapshots;
        self.scene_popup_surface_ids = self.popup_surface_ids.clone();
        self.last_rebuild_damage_rects.clear();
        self.scene_generation = self.scene_generation.saturating_add(1);
        self.last_rebuild_kind = DesktopSceneRebuildKind::Full;
        self.last_damage_debug_stats = DamageDebugStats::full(frame_width, frame_height);
    }

    fn copy_scene_to_frame(&mut self, frame: &mut [u32], frame_width: u32, frame_height: u32) {
        if frame.len() == self.scene.len() {
            frame.copy_from_slice(&self.scene);
        } else {
            let _ = (frame_width, frame_height);
            frame.fill(OUTPUT_BACKGROUND);
        }
        self.last_frame_copy_kind = DesktopFrameCopyKind::Full;
    }

    fn copy_scene_damage_to_frame(
        &mut self,
        frame: &mut [u32],
        frame_width: u32,
        frame_height: u32,
        _output_scale: f64,
    ) {
        if frame.len() != self.scene.len() {
            self.copy_scene_to_frame(frame, frame_width, frame_height);
            return;
        }
        for rect in &self.last_rebuild_damage_rects {
            copy_scene_rect_to_frame(&self.scene, frame, frame_width, *rect);
        }
        self.last_frame_copy_kind = DesktopFrameCopyKind::Partial;
    }
}

fn draw_client_cursor(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    cursor: ClientCursorRenderState<'_>,
    output_scale: f64,
) {
    let target = SurfaceTargetRect {
        x: scale_logical_coordinate(
            cursor.logical_x.saturating_add(cursor.surface.x),
            output_scale,
        ),
        y: scale_logical_coordinate(
            cursor.logical_y.saturating_add(cursor.surface.y),
            output_scale,
        ),
        width: scale_logical_extent(cursor.surface.width, output_scale),
        height: scale_logical_extent(cursor.surface.height, output_scale),
    };
    blit_surface_to_rect_clipped(
        frame,
        frame_width,
        frame_height,
        cursor.surface,
        target,
        None,
    );
}

fn draw_decoration_instance(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    instance: &DecorationRenderInstance,
    output_scale: f64,
    clip: Option<OutputRect>,
) {
    let output_clip = clip.map(|clip| ServerFrameRect {
        color: ServerFrameColor::Border,
        x: clip.x,
        y: clip.y,
        width: clip.width,
        height: clip.height,
    });
    for primitive in &instance.plan.primitives {
        match primitive {
            DecorationRenderPrimitive::SolidRect { rect, color } => {
                let output_rect = decoration_output_rect(instance, *rect, output_scale);
                if let Some(clip) = clip {
                    fill_decoration_rect_clipped(
                        frame,
                        frame_width,
                        frame_height,
                        output_rect,
                        *color,
                        clip,
                    );
                } else {
                    fill_decoration_rect(frame, frame_width, frame_height, output_rect, *color);
                }
            }
            DecorationRenderPrimitive::Image { rect, asset } => {
                draw_decoration_raster_asset(
                    frame,
                    frame_width,
                    frame_height,
                    decoration_output_rect(instance, *rect, output_scale),
                    asset,
                    output_clip,
                );
            }
            DecorationRenderPrimitive::Text {
                rect, clip, asset, ..
            } => {
                let primitive_clip = decoration_output_rect(instance, *clip, output_scale);
                let Some(effective_clip) = (match output_clip {
                    Some(scene_clip) => intersect_server_frame_rect(primitive_clip, scene_clip),
                    None => Some(primitive_clip),
                }) else {
                    continue;
                };
                draw_decoration_text(
                    frame,
                    frame_width,
                    frame_height,
                    decoration_output_rect(instance, *rect, output_scale),
                    effective_clip,
                    asset,
                    output_scale,
                );
            }
        }
    }
}

fn fill_decoration_rect_clipped(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    rect: ServerFrameRect,
    color: [u8; 4],
    clip: OutputRect,
) {
    let Some(clipped) = (OutputRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    })
    .intersection(clip)
    .and_then(|rect| rect.clipped_to_output(frame_width, frame_height)) else {
        return;
    };
    fill_decoration_rect(
        frame,
        frame_width,
        frame_height,
        ServerFrameRect {
            color: ServerFrameColor::Border,
            x: clipped.x,
            y: clipped.y,
            width: clipped.width,
            height: clipped.height,
        },
        color,
    );
}

fn intersect_server_frame_rect(
    left: ServerFrameRect,
    right: ServerFrameRect,
) -> Option<ServerFrameRect> {
    let intersection = (OutputRect {
        x: left.x,
        y: left.y,
        width: left.width,
        height: left.height,
    })
    .intersection(OutputRect {
        x: right.x,
        y: right.y,
        width: right.width,
        height: right.height,
    })?;
    Some(ServerFrameRect {
        color: left.color,
        x: intersection.x,
        y: intersection.y,
        width: intersection.width,
        height: intersection.height,
    })
}

fn decoration_output_rect(
    instance: &DecorationRenderInstance,
    rect: DecorationRect,
    output_scale: f64,
) -> ServerFrameRect {
    ServerFrameRect {
        color: ServerFrameColor::Border,
        x: scale_logical_coordinate(instance.origin_x.saturating_add(rect.x), output_scale),
        y: scale_logical_coordinate(instance.origin_y.saturating_add(rect.y), output_scale),
        width: scale_logical_extent(rect.width, output_scale),
        height: scale_logical_extent(rect.height, output_scale),
    }
}

fn fill_decoration_rect(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    rect: ServerFrameRect,
    color: [u8; 4],
) {
    let start_x = i64::from(rect.x).max(0);
    let start_y = i64::from(rect.y).max(0);
    let end_x = i64::from(rect.x)
        .saturating_add(i64::from(rect.width))
        .min(i64::from(frame_width));
    let end_y = i64::from(rect.y)
        .saturating_add(i64::from(rect.height))
        .min(i64::from(frame_height));
    if start_x >= end_x || start_y >= end_y {
        return;
    }
    let pixel = rgba_to_pixel(color);
    let frame_width = frame_width as usize;
    for target_y in start_y..end_y {
        let row_start = target_y as usize * frame_width + start_x as usize;
        let row_end = row_start + (end_x - start_x) as usize;
        if let Some(row) = frame.get_mut(row_start..row_end) {
            row.fill(pixel);
        }
    }
}

fn draw_decoration_raster_asset(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    rect: ServerFrameRect,
    asset: &super::decoration::raster::DecorationRasterAsset,
    clip: Option<ServerFrameRect>,
) {
    let clip_left = clip.map_or(rect.x, |clip| clip.x).max(rect.x).max(0);
    let clip_top = clip.map_or(rect.y, |clip| clip.y).max(rect.y).max(0);
    let clip_right = clip.map_or_else(
        || rect.x.saturating_add(rect.width as i32),
        |clip| clip.x.saturating_add(clip.width as i32),
    );
    let clip_bottom = clip.map_or_else(
        || rect.y.saturating_add(rect.height as i32),
        |clip| clip.y.saturating_add(clip.height as i32),
    );
    let start_x = i64::from(clip_left);
    let start_y = i64::from(clip_top);
    let end_x = i64::from(rect.x)
        .saturating_add(i64::from(rect.width))
        .min(i64::from(frame_width))
        .min(i64::from(clip_right));
    let end_y = i64::from(rect.y)
        .saturating_add(i64::from(rect.height))
        .min(i64::from(frame_height))
        .min(i64::from(clip_bottom));
    if start_x >= end_x || start_y >= end_y || rect.width == 0 || rect.height == 0 {
        return;
    }
    let pixels = asset.rgba_premultiplied();
    let asset_width = asset.width() as usize;
    for target_y in start_y..end_y {
        let local_y = target_y.saturating_sub(i64::from(rect.y)) as u32;
        let source_y = (u64::from(local_y) * u64::from(asset.height()) / u64::from(rect.height))
            .min(u64::from(asset.height().saturating_sub(1))) as usize;
        for target_x in start_x..end_x {
            let local_x = target_x.saturating_sub(i64::from(rect.x)) as u32;
            let source_x = (u64::from(local_x) * u64::from(asset.width()) / u64::from(rect.width))
                .min(u64::from(asset.width().saturating_sub(1)))
                as usize;
            let source_index = (source_y * asset_width + source_x) * 4;
            let Some(source) = pixels.get(source_index..source_index + 4) else {
                continue;
            };
            if source[3] == 0 {
                continue;
            }
            let index = target_y as usize * frame_width as usize + target_x as usize;
            let Some(destination) = frame.get_mut(index) else {
                continue;
            };
            *destination = blend_premultiplied_rgba(*destination, source);
        }
    }
}

fn blend_premultiplied_rgba(destination: u32, source: &[u8]) -> u32 {
    let source_alpha = u32::from(source[3]);
    let inverse_alpha = 255 - source_alpha;
    let destination_alpha = destination >> 24;
    let output_alpha =
        source_alpha.saturating_add(destination_alpha.saturating_mul(inverse_alpha) / 255);
    let destination_red = (destination >> 16) & 0xff;
    let destination_green = (destination >> 8) & 0xff;
    let destination_blue = destination & 0xff;
    let red = u32::from(source[0])
        .saturating_add(destination_red.saturating_mul(inverse_alpha) / 255)
        .min(255);
    let green = u32::from(source[1])
        .saturating_add(destination_green.saturating_mul(inverse_alpha) / 255)
        .min(255);
    let blue = u32::from(source[2])
        .saturating_add(destination_blue.saturating_mul(inverse_alpha) / 255)
        .min(255);
    (output_alpha << 24) | (red << 16) | (green << 8) | blue
}

fn draw_decoration_text(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    rect: ServerFrameRect,
    clip: ServerFrameRect,
    asset: &super::decoration::raster::DecorationRasterAsset,
    _output_scale: f64,
) {
    draw_decoration_raster_asset(frame, frame_width, frame_height, rect, asset, Some(clip));
}

fn rgba_to_pixel(color: [u8; 4]) -> u32 {
    (u32::from(color[3]) << 24)
        | (u32::from(color[0]) << 16)
        | (u32::from(color[1]) << 8)
        | u32::from(color[2])
}

fn split_external_overlay_surfaces(
    surfaces: &[RenderableSurface],
    external_overlay_surface_ids: &[u32],
) -> (Vec<RenderableSurface>, Vec<RenderableSurface>) {
    if external_overlay_surface_ids.is_empty() {
        return (Vec::new(), Vec::new());
    }
    surfaces
        .iter()
        .cloned()
        .partition(|surface| !external_overlay_surface_ids.contains(&surface.surface_id))
}

fn copy_scene_rect_to_frame(scene: &[u32], frame: &mut [u32], frame_width: u32, rect: OutputRect) {
    let frame_width = frame_width as usize;
    let left = rect.x.max(0) as usize;
    let top = rect.y.max(0) as usize;
    let width = rect.width as usize;
    let height = rect.height as usize;
    for y in top..top.saturating_add(height) {
        let start = y.saturating_mul(frame_width).saturating_add(left);
        let end = start.saturating_add(width);
        let Some(source_row) = scene.get(start..end) else {
            continue;
        };
        let Some(target_row) = frame.get_mut(start..end) else {
            continue;
        };
        target_row.copy_from_slice(source_row);
    }
}

fn decoration_damage_output_rect(
    rect: DecorationRect,
    output_scale: f64,
    frame_width: u32,
    frame_height: u32,
) -> Option<OutputRect> {
    OutputRect {
        x: scale_logical_coordinate(rect.x, output_scale),
        y: scale_logical_coordinate(rect.y, output_scale),
        width: scale_logical_extent(rect.width, output_scale),
        height: scale_logical_extent(rect.height, output_scale),
    }
    .clipped_to_output(frame_width, frame_height)
}

pub fn compose_output(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    surfaces: &[RenderableSurface],
    visual_state: DesktopVisualState,
) {
    frame.fill(OUTPUT_BACKGROUND);
    draw_client_surfaces(frame, frame_width, frame_height, surfaces);

    if let Some((cursor_x, cursor_y)) = visual_state.cursor {
        let cursor_image = shared_compositor_cursor_image();
        draw_cursor(
            frame,
            frame_width,
            frame_height,
            cursor_x,
            cursor_y,
            &cursor_image,
        );
    }
}

fn draw_client_surfaces(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    surfaces: &[RenderableSurface],
) {
    draw_client_surfaces_scaled(frame, frame_width, frame_height, surfaces, 1.0);
}

fn draw_client_surfaces_scaled(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    surfaces: &[RenderableSurface],
    output_scale: f64,
) {
    let snapshots = scene_surface_snapshots(surfaces, output_scale);
    draw_client_surfaces_scaled_with_snapshots(
        frame,
        frame_width,
        frame_height,
        surfaces,
        &snapshots,
        output_scale,
        None,
    );
}

fn draw_client_surfaces_scaled_with_snapshots(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    surfaces: &[RenderableSurface],
    snapshots: &[SceneSurfaceSnapshot],
    _output_scale: f64,
    clip: Option<OutputRect>,
) {
    for (surface, snapshot) in surfaces.iter().zip(snapshots) {
        draw_client_surface_with_snapshot(
            frame,
            frame_width,
            frame_height,
            surface,
            snapshot,
            clip,
        );
    }
}

struct WindowVisualDrawRequest<'a> {
    frame: &'a mut [u32],
    frame_width: u32,
    frame_height: u32,
    surfaces: &'a [RenderableSurface],
    snapshots: &'a [SceneSurfaceSnapshot],
    output_scale: f64,
    decorations: &'a [DecorationRenderInstance],
    popup_surface_ids: &'a [u32],
    clip: Option<OutputRect>,
}

fn draw_window_visual_groups(request: WindowVisualDrawRequest<'_>) {
    let WindowVisualDrawRequest {
        frame,
        frame_width,
        frame_height,
        surfaces,
        snapshots,
        output_scale,
        decorations,
        popup_surface_ids,
        clip,
    } = request;
    for group in window_visual_stack_order_with_popups(surfaces, decorations, popup_surface_ids) {
        for &surface_index in group.surface_indices() {
            let Some((surface, snapshot)) = surfaces
                .get(surface_index)
                .zip(snapshots.get(surface_index))
            else {
                continue;
            };
            draw_client_surface_with_snapshot(
                frame,
                frame_width,
                frame_height,
                surface,
                snapshot,
                clip,
            );
        }
        if let Some(decoration_index) = group.decoration_index()
            && let Some(decoration) = decorations.get(decoration_index)
        {
            draw_decoration_instance(
                frame,
                frame_width,
                frame_height,
                decoration,
                output_scale,
                clip,
            );
        }
    }
}

fn draw_client_surface_with_snapshot(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    surface: &RenderableSurface,
    snapshot: &SceneSurfaceSnapshot,
    clip: Option<OutputRect>,
) {
    if let Some(backing) = snapshot.backing_target {
        let rect = ServerFrameRect {
            color: ServerFrameColor::XwaylandBacking,
            x: backing.x(),
            y: backing.y(),
            width: backing.width(),
            height: backing.height(),
        };
        match clip {
            Some(clip) => fill_rect_clipped(frame, frame_width, frame_height, rect, clip),
            None => fill_rect(frame, frame_width, frame_height, rect),
        }
    }

    for plan in &snapshot.content_regions {
        if clip.is_some_and(|clip| !plan.content_target.output_rect().intersects(clip)) {
            continue;
        }
        blit_surface_with_plan(frame, frame_width, frame_height, surface, *plan, clip);
    }
}

fn scene_surface_snapshots(
    surfaces: &[RenderableSurface],
    output_scale: f64,
) -> Vec<SceneSurfaceSnapshot> {
    let elements = render_scene_elements_for_surfaces(surfaces, output_scale);
    scene_surface_snapshots_from_elements(&elements)
}

pub fn render_scene_elements_for_surfaces(
    surfaces: &[RenderableSurface],
    output_scale: f64,
) -> Vec<RenderSceneElement> {
    let assignments = surface_render_space_assignments(surfaces, output_scale);
    surfaces
        .iter()
        .zip(assignments)
        .map(|(surface, assignment)| {
            RenderSceneElement::from_surface_with_aperture(
                surface,
                assignment.target,
                assignment.visual_clip,
            )
        })
        .collect()
}

fn scene_surface_snapshots_from_elements(
    elements: &[RenderSceneElement],
) -> Vec<SceneSurfaceSnapshot> {
    elements
        .iter()
        .map(|element| {
            let RenderSceneElementId::Surface(surface_id) = element.id;
            SceneSurfaceSnapshot {
                surface_id,
                generation: element.generation,
                target: element.target,
                visible_target: element.visible_target,
                backing_target: element.backing_target,
                content_regions: element.content_regions.clone(),
                buffer_width: element.buffer_size.width,
                buffer_height: element.buffer_size.height,
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceRenderSpaceAssignment {
    pub target: SurfaceTargetRect,
    pub visual_clip: Option<SurfaceVisualAperture>,
}

pub fn surface_render_space_assignments(
    surfaces: &[RenderableSurface],
    output_scale: f64,
) -> Vec<SurfaceRenderSpaceAssignment> {
    let output_scale = normalized_output_scale(output_scale);
    let origins = surface_origins(surfaces);
    let root_indices = surface_root_indices(surfaces);
    surfaces
        .iter()
        .enumerate()
        .zip(origins.iter().copied())
        .map(|((index, surface), (origin_x, origin_y))| {
            let root_index = root_indices.get(index).copied().unwrap_or(index);
            let root = &surfaces[root_index];
            let root_origin = origins
                .get(root_index)
                .copied()
                .unwrap_or((origin_x, origin_y));
            let root_placement = root.render_placement.unwrap_or(root.placement);
            let root_clip_base = (
                root_origin.0.saturating_sub(root_placement.local_x),
                root_origin.1.saturating_sub(root_placement.local_y),
            );
            // An explicit target is a compositor policy override used by
            // ordinary client-driven X11 geometry. Interactive XWayland
            // previews clear it so stale content remains at its committed
            // extent and the backing rectangle supplies only uncovered space.
            let (target_width, target_height) = surface
                .render_target_size
                .map(|size| (size.width, size.height))
                .unwrap_or((surface.width, surface.height));
            SurfaceRenderSpaceAssignment {
                target: render_space_rect_from_logical(
                    (origin_x, origin_y),
                    target_width,
                    target_height,
                    output_scale,
                ),
                visual_clip: surface
                    .visual_clip
                    .clone()
                    .map(|aperture| aperture.map_logical(root_clip_base, output_scale)),
            }
        })
        .collect()
}

fn render_space_rect_from_logical(
    origin: (i32, i32),
    width: u32,
    height: u32,
    output_scale: f64,
) -> SurfaceTargetRect {
    SurfaceTargetRect {
        x: scale_logical_coordinate(origin.0, output_scale),
        y: scale_logical_coordinate(origin.1, output_scale),
        width: scale_logical_extent(width, output_scale),
        height: scale_logical_extent(height, output_scale),
    }
}

fn partial_scene_damage_rects(
    previous_snapshots: &[SceneSurfaceSnapshot],
    elements: &[RenderSceneElement],
    snapshots: &[SceneSurfaceSnapshot],
    frame_width: u32,
    frame_height: u32,
) -> Option<Vec<OutputRect>> {
    if previous_snapshots.len() != snapshots.len() || elements.len() != snapshots.len() {
        return None;
    }

    let mut damage_rects = Vec::new();
    for ((previous, element), snapshot) in previous_snapshots
        .iter()
        .zip(elements)
        .zip(snapshots.iter())
    {
        if previous.surface_id != snapshot.surface_id {
            return None;
        }

        if previous.target != snapshot.target
            || previous.visible_target != snapshot.visible_target
            || previous.backing_target != snapshot.backing_target
            || previous.content_regions != snapshot.content_regions
        {
            if let Some(rect) = previous
                .visible_target
                .output_rect()
                .clipped_to_output(frame_width, frame_height)
            {
                damage_rects.push(rect);
            }
            if let Some(rect) = snapshot
                .visible_target
                .output_rect()
                .clipped_to_output(frame_width, frame_height)
            {
                damage_rects.push(rect);
            }
            if let Some(rect) = previous.backing_target.and_then(|target| {
                target
                    .output_rect()
                    .clipped_to_output(frame_width, frame_height)
            }) {
                damage_rects.push(rect);
            }
            if let Some(rect) = snapshot.backing_target.and_then(|target| {
                target
                    .output_rect()
                    .clipped_to_output(frame_width, frame_height)
            }) {
                damage_rects.push(rect);
            }
            continue;
        }

        if previous.buffer_width != snapshot.buffer_width
            || previous.buffer_height != snapshot.buffer_height
        {
            if let Some(rect) = snapshot
                .visible_target
                .output_rect()
                .clipped_to_output(frame_width, frame_height)
            {
                damage_rects.push(rect);
            }
            continue;
        }

        if previous.generation == snapshot.generation {
            continue;
        }

        match &element.damage {
            RenderableSurfaceDamage::Empty => {}
            RenderableSurfaceDamage::Full => {
                if let Some(rect) = snapshot
                    .visible_target
                    .output_rect()
                    .clipped_to_output(frame_width, frame_height)
                {
                    damage_rects.push(rect);
                }
            }
            RenderableSurfaceDamage::Partial(_) => {
                for rect in element
                    .damage
                    .clipped_rects(element.buffer_size.width, element.buffer_size.height)
                {
                    let Some(rect) =
                        output_damage_rect_for_element(element, snapshot.visible_target, rect)
                            .and_then(|rect| rect.clipped_to_output(frame_width, frame_height))
                    else {
                        continue;
                    };
                    damage_rects.push(rect);
                }
            }
        }
    }

    Some(coalesce_output_rects(damage_rects))
}

fn output_damage_rect_for_element(
    element: &RenderSceneElement,
    target: SurfaceTargetRect,
    rect: SurfaceDamageRect,
) -> Option<OutputRect> {
    if target.width == 0 || target.height == 0 {
        return None;
    }

    let buffer_size = element.buffer_size;
    let left = scale_damage_floor(rect.x, buffer_size.width, target.width)?;
    let top = scale_damage_floor(rect.y, buffer_size.height, target.height)?;
    let right = scale_damage_ceil(
        rect.x.saturating_add(rect.width),
        buffer_size.width,
        target.width,
    )?;
    let bottom = scale_damage_ceil(
        rect.y.saturating_add(rect.height),
        buffer_size.height,
        target.height,
    )?;
    if right <= left || bottom <= top {
        return None;
    }

    Some(OutputRect {
        x: i32_saturating_add_u32(target.x, left),
        y: i32_saturating_add_u32(target.y, top),
        width: right - left,
        height: bottom - top,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServerFrameColor {
    OutputBackground,
    XwaylandBacking,
    Border,
    Titlebar,
    Separator,
}

impl ServerFrameColor {
    pub const ALL: [Self; 5] = [
        Self::OutputBackground,
        Self::XwaylandBacking,
        Self::Border,
        Self::Titlebar,
        Self::Separator,
    ];

    pub const fn pixel(self) -> u32 {
        match self {
            Self::OutputBackground => OUTPUT_BACKGROUND,
            Self::XwaylandBacking => 0xff00_0000,
            Self::Border => SERVER_FRAME_BORDER_COLOR,
            Self::Titlebar => SERVER_FRAME_TITLEBAR_COLOR,
            Self::Separator => SERVER_FRAME_SEPARATOR_COLOR,
        }
    }
}

pub fn xwayland_visual_backing_target(
    surface: &RenderableSurface,
    visual_aperture: Option<&SurfaceVisualAperture>,
) -> Option<SurfaceTargetRect> {
    if surface.render_backend != SurfaceRenderBackend::Xwayland {
        return None;
    }
    let placement = surface.render_placement.unwrap_or(surface.placement);
    (surface.placement.parent_surface_id.is_none()
        && placement.root_mode == RootPlacementMode::Absolute)
        .then_some(())
        .and(visual_aperture)
        .map(SurfaceVisualAperture::bounds)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerFrameRect {
    pub color: ServerFrameColor,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceTargetRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl SurfaceTargetRect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn x(self) -> i32 {
        self.x
    }

    pub const fn y(self) -> i32 {
        self.y
    }

    pub const fn width(self) -> u32 {
        self.width
    }

    pub const fn height(self) -> u32 {
        self.height
    }

    pub fn intersects(self, other: Self) -> bool {
        self.intersection(other).is_some()
    }

    pub fn intersection(self, other: Self) -> Option<Self> {
        let left = i64::from(self.x).max(i64::from(other.x));
        let top = i64::from(self.y).max(i64::from(other.y));
        let right = i64::from(self.right()).min(i64::from(other.right()));
        let bottom = i64::from(self.bottom()).min(i64::from(other.bottom()));
        (right > left && bottom > top).then_some(Self::new(
            i32::try_from(left).unwrap_or(if left.is_negative() {
                i32::MIN
            } else {
                i32::MAX
            }),
            i32::try_from(top).unwrap_or(if top.is_negative() {
                i32::MIN
            } else {
                i32::MAX
            }),
            u32::try_from(right.saturating_sub(left)).unwrap_or(u32::MAX),
            u32::try_from(bottom.saturating_sub(top)).unwrap_or(u32::MAX),
        ))
    }

    pub fn union(self, other: Self) -> Self {
        let left = self.x.min(other.x);
        let top = self.y.min(other.y);
        let right = i64::from(self.right()).max(i64::from(other.right()));
        let bottom = i64::from(self.bottom()).max(i64::from(other.bottom()));
        Self::new(
            left,
            top,
            u32::try_from(right.saturating_sub(i64::from(left))).unwrap_or(u32::MAX),
            u32::try_from(bottom.saturating_sub(i64::from(top))).unwrap_or(u32::MAX),
        )
    }

    fn right(self) -> i32 {
        self.x
            .saturating_add(i32::try_from(self.width).unwrap_or(i32::MAX))
    }

    fn bottom(self) -> i32 {
        self.y
            .saturating_add(i32::try_from(self.height).unwrap_or(i32::MAX))
    }

    fn right_i32(self) -> i32 {
        self.right()
    }

    fn bottom_i32(self) -> i32 {
        self.bottom()
    }

    const fn output_rect(self) -> OutputRect {
        OutputRect {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceVisualAperture {
    logical_target: SurfaceTargetRect,
    committed_content_target: Option<SurfaceTargetRect>,
    committed_extent_regions: Vec<SurfaceTargetRect>,
}

impl SurfaceVisualAperture {
    pub fn logical_only(logical_target: SurfaceTargetRect) -> Self {
        Self {
            logical_target,
            committed_content_target: Some(logical_target),
            committed_extent_regions: Vec::new(),
        }
    }

    pub fn for_root_window_preview(
        root_origin: (i32, i32),
        root_buffer: BufferSize,
        visual_extents: (u32, u32, u32, u32),
        logical_target: SurfaceTargetRect,
    ) -> Self {
        let buffer_rect = SurfaceTargetRect::new(
            root_origin.0,
            root_origin.1,
            root_buffer.width,
            root_buffer.height,
        );
        let committed_rect = SurfaceTargetRect::new(
            root_origin
                .0
                .saturating_add(i32::try_from(visual_extents.0).unwrap_or(i32::MAX)),
            root_origin
                .1
                .saturating_add(i32::try_from(visual_extents.1).unwrap_or(i32::MAX)),
            root_buffer
                .width
                .saturating_sub(visual_extents.0)
                .saturating_sub(visual_extents.2),
            root_buffer
                .height
                .saturating_sub(visual_extents.1)
                .saturating_sub(visual_extents.3),
        );
        let committed_content_target = buffer_rect
            .intersection(committed_rect)
            .and_then(|rect| rect.intersection(logical_target));
        let committed_extent_regions = subtract_rect_from_rect(buffer_rect, committed_rect)
            .into_iter()
            .flat_map(|region| subtract_rect_from_rect(region, logical_target))
            .collect();
        Self {
            logical_target,
            committed_content_target,
            committed_extent_regions,
        }
    }

    pub const fn logical_target(&self) -> SurfaceTargetRect {
        self.logical_target
    }

    pub const fn x(&self) -> i32 {
        self.logical_target.x()
    }

    pub const fn y(&self) -> i32 {
        self.logical_target.y()
    }

    pub const fn width(&self) -> u32 {
        self.logical_target.width()
    }

    pub const fn height(&self) -> u32 {
        self.logical_target.height()
    }

    pub const fn committed_content_target(&self) -> Option<SurfaceTargetRect> {
        self.committed_content_target
    }

    pub fn committed_extent_regions(&self) -> &[SurfaceTargetRect] {
        &self.committed_extent_regions
    }

    pub fn content_regions(&self) -> Vec<SurfaceTargetRect> {
        self.committed_content_target
            .into_iter()
            .chain(self.committed_extent_regions.iter().copied())
            .collect()
    }

    pub fn bounds(&self) -> SurfaceTargetRect {
        self.content_regions()
            .into_iter()
            .chain(std::iter::once(self.logical_target))
            .reduce(SurfaceTargetRect::union)
            .unwrap_or(self.logical_target)
    }

    pub fn map_logical(self, root_clip_base: (i32, i32), output_scale: f64) -> Self {
        let map = |rect: SurfaceTargetRect| {
            render_space_rect_from_logical(
                (
                    root_clip_base.0.saturating_add(rect.x()),
                    root_clip_base.1.saturating_add(rect.y()),
                ),
                rect.width(),
                rect.height(),
                output_scale,
            )
        };
        Self {
            logical_target: map(self.logical_target),
            committed_content_target: self.committed_content_target.map(map),
            committed_extent_regions: self.committed_extent_regions.into_iter().map(map).collect(),
        }
    }
}

fn subtract_rect_from_rect(
    source: SurfaceTargetRect,
    excluded: SurfaceTargetRect,
) -> Vec<SurfaceTargetRect> {
    let Some(intersection) = source.intersection(excluded) else {
        return vec![source];
    };
    let mut pieces = Vec::with_capacity(4);
    if source.y() < intersection.y() {
        pieces.push(SurfaceTargetRect::new(
            source.x(),
            source.y(),
            source.width(),
            u32::try_from(i64::from(intersection.y()) - i64::from(source.y())).unwrap_or(u32::MAX),
        ));
    }
    if intersection.bottom() < source.bottom() {
        pieces.push(SurfaceTargetRect::new(
            source.x(),
            intersection.bottom_i32(),
            source.width(),
            u32::try_from(source.bottom() - intersection.bottom()).unwrap_or(u32::MAX),
        ));
    }
    let middle_top = intersection.y();
    let middle_bottom = intersection.bottom_i32();
    if source.x() < intersection.x() {
        pieces.push(SurfaceTargetRect::new(
            source.x(),
            middle_top,
            u32::try_from(i64::from(intersection.x()) - i64::from(source.x())).unwrap_or(u32::MAX),
            u32::try_from(i64::from(middle_bottom) - i64::from(middle_top)).unwrap_or(u32::MAX),
        ));
    }
    if intersection.right_i32() < source.right_i32() {
        pieces.push(SurfaceTargetRect::new(
            intersection.right_i32(),
            middle_top,
            u32::try_from(source.right() - intersection.right()).unwrap_or(u32::MAX),
            u32::try_from(i64::from(middle_bottom) - i64::from(middle_top)).unwrap_or(u32::MAX),
        ));
    }
    pieces
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceUvRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl SurfaceUvRect {
    pub const FULL: Self = Self {
        left: 0.0,
        top: 0.0,
        right: 1.0,
        bottom: 1.0,
    };
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceRenderPlan {
    pub visual_target: SurfaceTargetRect,
    pub content_target: SurfaceTargetRect,
    pub content_uv: SurfaceUvRect,
    pub clip: Option<SurfaceTargetRect>,
}

pub fn surface_render_plan(
    surface: &RenderableSurface,
    visual_target: SurfaceTargetRect,
) -> SurfaceRenderPlan {
    let visual_aperture = surface.visual_clip.as_ref();
    surface_render_plans_with_aperture(surface, visual_target, visual_aperture)
        .into_iter()
        .next()
        .unwrap_or_else(|| SurfaceRenderPlan {
            visual_target,
            content_target: SurfaceTargetRect::new(visual_target.x(), visual_target.y(), 0, 0),
            content_uv: SurfaceUvRect::FULL,
            clip: visual_aperture.map(SurfaceVisualAperture::logical_target),
        })
}

pub fn surface_render_plans_with_aperture(
    surface: &RenderableSurface,
    visual_target: SurfaceTargetRect,
    visual_aperture: Option<&SurfaceVisualAperture>,
) -> Vec<SurfaceRenderPlan> {
    match visual_aperture {
        Some(aperture) => aperture
            .content_regions()
            .into_iter()
            .map(|clip| surface_render_plan_with_clip(surface, visual_target, Some(clip)))
            .filter(|plan| plan.content_target.width() > 0 && plan.content_target.height() > 0)
            .collect(),
        None => vec![surface_render_plan_with_clip(surface, visual_target, None)],
    }
}

pub fn surface_render_plan_with_clip(
    surface: &RenderableSurface,
    visual_target: SurfaceTargetRect,
    visual_clip: Option<SurfaceTargetRect>,
) -> SurfaceRenderPlan {
    let mut plan = SurfaceRenderPlan {
        visual_target,
        content_target: visual_target,
        content_uv: surface_base_uv(surface),
        clip: visual_clip,
    };
    if let Some(clip) = visual_clip {
        plan = clip_surface_render_plan(plan, clip);
    }
    plan
}

fn surface_base_uv(surface: &RenderableSurface) -> SurfaceUvRect {
    let Some(source) = surface.viewport_source else {
        return SurfaceUvRect::FULL;
    };
    let buffer_size = surface.buffer_size();
    if buffer_size.width == 0 || buffer_size.height == 0 {
        return SurfaceUvRect::FULL;
    }
    let buffer_width = f64::from(buffer_size.width);
    let buffer_height = f64::from(buffer_size.height);
    SurfaceUvRect {
        left: (source.x / buffer_width) as f32,
        top: (source.y / buffer_height) as f32,
        right: ((source.x + source.width) / buffer_width) as f32,
        bottom: ((source.y + source.height) / buffer_height) as f32,
    }
}

pub fn clip_surface_render_plan(
    plan: SurfaceRenderPlan,
    clip: SurfaceTargetRect,
) -> SurfaceRenderPlan {
    let Some(intersection) = plan
        .content_target
        .output_rect()
        .intersection(clip.output_rect())
    else {
        return SurfaceRenderPlan {
            content_target: SurfaceTargetRect {
                x: plan.content_target.x,
                y: plan.content_target.y,
                width: 0,
                height: 0,
            },
            ..plan
        };
    };
    let target = SurfaceTargetRect {
        x: intersection.x,
        y: intersection.y,
        width: intersection.width,
        height: intersection.height,
    };
    let original = plan.content_target;
    let left_trim = (target.x - original.x) as f32 / original.width.max(1) as f32;
    let top_trim = (target.y - original.y) as f32 / original.height.max(1) as f32;
    let right_trim = (original.right() - target.right()) as f32 / original.width.max(1) as f32;
    let bottom_trim = (original.bottom() - target.bottom()) as f32 / original.height.max(1) as f32;
    let uv_width = plan.content_uv.right - plan.content_uv.left;
    let uv_height = plan.content_uv.bottom - plan.content_uv.top;
    SurfaceRenderPlan {
        content_target: target,
        content_uv: SurfaceUvRect {
            left: (plan.content_uv.left + uv_width * left_trim).clamp(0.0, 1.0),
            top: (plan.content_uv.top + uv_height * top_trim).clamp(0.0, 1.0),
            right: (plan.content_uv.right - uv_width * right_trim).clamp(0.0, 1.0),
            bottom: (plan.content_uv.bottom - uv_height * bottom_trim).clamp(0.0, 1.0),
        },
        ..plan
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OutputRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl OutputRect {
    const fn full(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    fn clipped_to_output(self, output_width: u32, output_height: u32) -> Option<Self> {
        self.intersection(Self::full(output_width, output_height))
    }

    fn intersects(self, other: Self) -> bool {
        self.intersection(other).is_some()
    }

    fn intersection(self, other: Self) -> Option<Self> {
        let left = self.left().max(other.left());
        let top = self.top().max(other.top());
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > left && bottom > top).then_some(Self {
            x: left.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
            y: top.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
            width: (right - left).min(i64::from(u32::MAX)) as u32,
            height: (bottom - top).min(i64::from(u32::MAX)) as u32,
        })
    }

    fn left(self) -> i64 {
        i64::from(self.x)
    }

    fn top(self) -> i64 {
        i64::from(self.y)
    }

    fn right(self) -> i64 {
        self.left().saturating_add(i64::from(self.width))
    }

    fn bottom(self) -> i64 {
        self.top().saturating_add(i64::from(self.height))
    }

    fn pixels(self) -> u64 {
        u64::from(self.width).saturating_mul(u64::from(self.height))
    }

    fn union(self, other: Self) -> Self {
        let left = self.x.min(other.x);
        let top = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self {
            x: left,
            y: top,
            width: u32::try_from(right.saturating_sub(i64::from(left))).unwrap_or(u32::MAX),
            height: u32::try_from(bottom.saturating_sub(i64::from(top))).unwrap_or(u32::MAX),
        }
    }
}

fn coalesce_output_rects(rects: Vec<OutputRect>) -> Vec<OutputRect> {
    let mut coalesced = Vec::<OutputRect>::new();
    'next_rect: for rect in rects {
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        let mut pending = rect;
        let mut index = 0;
        while index < coalesced.len() {
            let existing = coalesced[index];
            let union = existing.union(pending);
            let separate_pixels = existing.pixels().saturating_add(pending.pixels());
            if union.pixels() <= separate_pixels {
                pending = union;
                coalesced.swap_remove(index);
                index = 0;
                continue;
            }
            if existing == pending {
                continue 'next_rect;
            }
            index += 1;
        }
        coalesced.push(pending);
    }
    coalesced
}

pub fn server_frame_rects_by_surface(surfaces: &[RenderableSurface]) -> Vec<Vec<ServerFrameRect>> {
    surfaces
        .iter()
        .map(server_frame_rects_for_surface)
        .collect()
}

pub fn server_frame_rects_for_surface(surface: &RenderableSurface) -> Vec<ServerFrameRect> {
    if let Some(bounds) = xwayland_visual_backing_target(surface, surface.visual_clip.as_ref()) {
        return vec![ServerFrameRect {
            color: ServerFrameColor::XwaylandBacking,
            x: 0,
            y: 0,
            width: bounds.width(),
            height: bounds.height(),
        }];
    }
    Vec::new()
}

pub fn surface_origins(surfaces: &[RenderableSurface]) -> Vec<(i32, i32)> {
    if surfaces
        .iter()
        .all(|surface| surface.placement.parent_surface_id.is_none())
    {
        return surfaces
            .iter()
            .enumerate()
            .map(|(index, surface)| root_surface_origin_for_ordinal(index, surface))
            .collect();
    }

    let index_by_id: HashMap<u32, usize> = surfaces
        .iter()
        .enumerate()
        .map(|(index, surface)| (surface.surface_id, index))
        .collect();
    let root_ordinals = root_surface_ordinals(surfaces, &index_by_id);
    let mut origins = vec![None; surfaces.len()];
    let mut resolving = vec![false; surfaces.len()];

    for index in 0..surfaces.len() {
        let origin = resolve_surface_origin(
            index,
            surfaces,
            &index_by_id,
            &root_ordinals,
            &mut origins,
            &mut resolving,
        );
        origins[index] = Some(origin);
    }

    origins
        .into_iter()
        .enumerate()
        .map(|(index, origin)| origin.unwrap_or_else(|| surface_origin(index, &surfaces[index])))
        .collect()
}

fn surface_root_indices(surfaces: &[RenderableSurface]) -> Vec<usize> {
    if surfaces
        .iter()
        .all(|surface| surface.placement.parent_surface_id.is_none())
    {
        return (0..surfaces.len()).collect();
    }

    let index_by_id: HashMap<u32, usize> = surfaces
        .iter()
        .enumerate()
        .map(|(index, surface)| (surface.surface_id, index))
        .collect();
    let mut roots = vec![None; surfaces.len()];
    let mut resolving = vec![false; surfaces.len()];

    for index in 0..surfaces.len() {
        let root =
            resolve_surface_root_index(index, surfaces, &index_by_id, &mut roots, &mut resolving);
        roots[index] = Some(root);
    }

    roots
        .into_iter()
        .enumerate()
        .map(|(index, root)| root.unwrap_or(index))
        .collect()
}

fn resolve_surface_root_index(
    index: usize,
    surfaces: &[RenderableSurface],
    index_by_id: &HashMap<u32, usize>,
    roots: &mut [Option<usize>],
    resolving: &mut [bool],
) -> usize {
    if let Some(root) = roots[index] {
        return root;
    }
    if resolving[index] {
        return index;
    }

    resolving[index] = true;
    let placement = surfaces[index]
        .render_placement
        .unwrap_or(surfaces[index].placement);
    let root = placement
        .parent_surface_id
        .and_then(|parent_id| index_by_id.get(&parent_id).copied())
        .filter(|parent_index| *parent_index != index)
        .map(|parent_index| {
            resolve_surface_root_index(parent_index, surfaces, index_by_id, roots, resolving)
        })
        .unwrap_or(index);
    resolving[index] = false;
    roots[index] = Some(root);
    root
}

fn root_surface_ordinals(
    surfaces: &[RenderableSurface],
    index_by_id: &HashMap<u32, usize>,
) -> HashMap<u32, usize> {
    let mut root_ordinals = HashMap::new();
    let mut root_count = 0;

    for surface in surfaces {
        let has_visible_parent = surface
            .placement
            .parent_surface_id
            .is_some_and(|parent_id| index_by_id.contains_key(&parent_id));
        if has_visible_parent {
            continue;
        }

        root_ordinals.insert(surface.surface_id, root_count);
        root_count += 1;
    }

    root_ordinals
}

fn resolve_surface_origin(
    index: usize,
    surfaces: &[RenderableSurface],
    index_by_id: &HashMap<u32, usize>,
    root_ordinals: &HashMap<u32, usize>,
    origins: &mut [Option<(i32, i32)>],
    resolving: &mut [bool],
) -> (i32, i32) {
    if let Some(origin) = origins[index] {
        return origin;
    }
    if resolving[index] {
        return root_surface_origin(index, &surfaces[index], root_ordinals);
    }

    resolving[index] = true;
    let surface = &surfaces[index];
    let placement = surface.render_placement.unwrap_or(surface.placement);
    let origin = placement
        .parent_surface_id
        .and_then(|parent_id| index_by_id.get(&parent_id).copied())
        .filter(|parent_index| *parent_index != index)
        .map(|parent_index| {
            let parent_origin = resolve_surface_origin(
                parent_index,
                surfaces,
                index_by_id,
                root_ordinals,
                origins,
                resolving,
            );
            (
                parent_origin.0 + placement.local_x + surface.x,
                parent_origin.1 + placement.local_y + surface.y,
            )
        })
        .unwrap_or_else(|| root_surface_origin(index, surface, root_ordinals));
    resolving[index] = false;
    origins[index] = Some(origin);
    origin
}

fn root_surface_origin(
    fallback_index: usize,
    surface: &RenderableSurface,
    root_ordinals: &HashMap<u32, usize>,
) -> (i32, i32) {
    let root_index = root_ordinals
        .get(&surface.surface_id)
        .copied()
        .unwrap_or(fallback_index);
    root_surface_origin_for_ordinal(root_index, surface)
}

fn root_surface_origin_for_ordinal(root_index: usize, surface: &RenderableSurface) -> (i32, i32) {
    let placement = surface.render_placement.unwrap_or(surface.placement);
    match placement.root_mode {
        RootPlacementMode::CascadedWindow => {
            let (root_x, root_y) = cascaded_root_position(root_index);
            (
                root_x + placement.local_x + surface.x,
                root_y + placement.local_y + surface.y,
            )
        }
        RootPlacementMode::Absolute => (
            placement.local_x.saturating_add(surface.x),
            placement.local_y.saturating_add(surface.y),
        ),
    }
}

pub fn surface_origin(index: usize, surface: &RenderableSurface) -> (i32, i32) {
    let cascade = index as i32 * SURFACE_CASCADE_STEP;
    (
        FIRST_SURFACE_OFFSET.0 + cascade + surface.x,
        FIRST_SURFACE_OFFSET.1 + cascade + surface.y,
    )
}

pub fn surface_local_point_at_origin(
    surface: &RenderableSurface,
    origin: (i32, i32),
    output_x: f64,
    output_y: f64,
) -> Option<(f64, f64)> {
    let (origin_x, origin_y) = origin;
    let local_x = output_x - f64::from(origin_x);
    let local_y = output_y - f64::from(origin_y);

    if local_x >= 0.0
        && local_y >= 0.0
        && local_x < f64::from(surface.width)
        && local_y < f64::from(surface.height)
    {
        Some((local_x, local_y))
    } else {
        None
    }
}

pub fn cursor_damage_rect(
    cursor_x: i32,
    cursor_y: i32,
    frame_width: u32,
    frame_height: u32,
    cursor_image: &CompositorCursorImage,
) -> Option<SurfaceDamageRect> {
    let (top_left_x, top_left_y) = cursor_image.top_left(cursor_x, cursor_y);
    let left = i64::from(top_left_x).clamp(0, i64::from(frame_width));
    let top = i64::from(top_left_y).clamp(0, i64::from(frame_height));
    let right = i64::from(top_left_x)
        .checked_add(i64::from(cursor_image.width))?
        .clamp(0, i64::from(frame_width));
    let bottom = i64::from(top_left_y)
        .checked_add(i64::from(cursor_image.height))?
        .clamp(0, i64::from(frame_height));
    (right > left && bottom > top).then_some(SurfaceDamageRect {
        x: u32::try_from(left).ok()?,
        y: u32::try_from(top).ok()?,
        width: u32::try_from(right - left).ok()?,
        height: u32::try_from(bottom - top).ok()?,
    })
}

pub fn normalized_output_scale(output_scale: f64) -> f64 {
    if output_scale.is_finite() && output_scale > 0.0 {
        output_scale
    } else {
        1.0
    }
}

pub fn output_scale_key(output_scale: f64) -> u32 {
    (normalized_output_scale(output_scale) * f64::from(OUTPUT_SCALE_DENOMINATOR))
        .round()
        .max(1.0) as u32
}

pub fn scale_logical_coordinate(value: i32, output_scale: f64) -> i32 {
    (f64::from(value) * normalized_output_scale(output_scale)).round() as i32
}

pub fn scale_logical_extent(value: u32, output_scale: f64) -> u32 {
    if value == 0 {
        0
    } else {
        (f64::from(value) * normalized_output_scale(output_scale))
            .round()
            .max(1.0) as u32
    }
}

fn scale_damage_floor(value: u32, from_extent: u32, to_extent: u32) -> Option<u32> {
    if from_extent == 0 {
        return None;
    }
    let scaled = u64::from(value).saturating_mul(u64::from(to_extent)) / u64::from(from_extent);
    Some(scaled.min(u64::from(u32::MAX)) as u32)
}

fn scale_damage_ceil(value: u32, from_extent: u32, to_extent: u32) -> Option<u32> {
    if from_extent == 0 {
        return None;
    }
    let numerator = u64::from(value).saturating_mul(u64::from(to_extent));
    let scaled =
        numerator.saturating_add(u64::from(from_extent).saturating_sub(1)) / u64::from(from_extent);
    Some(scaled.min(u64::from(u32::MAX)) as u32)
}

fn i32_saturating_add_u32(value: i32, addend: u32) -> i32 {
    i64::from(value)
        .saturating_add(i64::from(addend))
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

pub fn scale_desktop_visual_state(
    visual_state: DesktopVisualState,
    output_scale: f64,
) -> DesktopVisualState {
    let Some((cursor_x, cursor_y)) = visual_state.cursor else {
        return visual_state;
    };
    DesktopVisualState::with_cursor(
        scale_logical_coordinate(cursor_x, output_scale),
        scale_logical_coordinate(cursor_y, output_scale),
    )
}

fn draw_cursor(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    cursor_x: i32,
    cursor_y: i32,
    cursor_image: &CompositorCursorImage,
) {
    let (top_left_x, top_left_y) = cursor_image.top_left(cursor_x, cursor_y);
    for row in 0..cursor_image.height {
        for column in 0..cursor_image.width {
            let source_index = row
                .saturating_mul(cursor_image.width)
                .saturating_add(column) as usize;
            let Some(&source) = cursor_image.pixels_argb8888.get(source_index) else {
                continue;
            };
            if source >> 24 == 0 {
                continue;
            }
            let target_x = top_left_x.saturating_add(column as i32);
            let target_y = top_left_y.saturating_add(row as i32);
            if !(0..frame_width as i32).contains(&target_x)
                || !(0..frame_height as i32).contains(&target_y)
            {
                continue;
            }

            let pixel_index = (target_y as u32 * frame_width + target_x as u32) as usize;
            if let Some(pixel) = frame.get_mut(pixel_index) {
                *pixel = blend_premultiplied_argb_over_opaque(source, *pixel);
            }
        }
    }
}

fn blit_surface_to_rect_clipped(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    surface: &RenderableSurface,
    target: SurfaceTargetRect,
    clip: Option<OutputRect>,
) {
    let plan = surface_render_plan(surface, target);
    blit_surface_with_plan(frame, frame_width, frame_height, surface, plan, clip);
}

fn blit_surface_with_plan(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    surface: &RenderableSurface,
    plan: SurfaceRenderPlan,
    clip: Option<OutputRect>,
) {
    let Some(surface_pixels) = surface.cpu_pixels() else {
        return;
    };
    let target = plan.content_target;
    let output_clip = match clip {
        Some(clip) => {
            let Some(clip) = clip.clipped_to_output(frame_width, frame_height) else {
                return;
            };
            clip
        }
        None => OutputRect::full(frame_width, frame_height),
    };
    let Some(target_clip) = target
        .output_rect()
        .intersection(output_clip)
        .and_then(|rect| rect.clipped_to_output(frame_width, frame_height))
    else {
        return;
    };

    let start_x = target_clip.left();
    let start_y = target_clip.top();
    let end_x = target_clip.right();
    let end_y = target_clip.bottom();

    let buffer_size = surface.buffer_size();
    let buffer_width = buffer_size.width as usize;
    let buffer_height = buffer_size.height as usize;
    let frame_width = frame_width as usize;
    if buffer_width == 0 || buffer_height == 0 || target.width == 0 || target.height == 0 {
        return;
    }

    if plan.content_uv == SurfaceUvRect::FULL
        && buffer_size.width == target.width
        && buffer_size.height == target.height
    {
        let row_width = (end_x - start_x) as usize;
        let source_x = (start_x - i64::from(target.x)) as usize;
        for row_y in start_y..end_y {
            let source_y = (row_y - i64::from(target.y)) as usize;
            let source_start = source_y * buffer_width + source_x;
            let target_start = row_y as usize * frame_width + start_x as usize;
            let Some(source_row) = surface_pixels.get(source_start..source_start + row_width)
            else {
                continue;
            };
            let Some(target_row) = frame.get_mut(target_start..target_start + row_width) else {
                continue;
            };
            if source_row_is_opaque(source_row) {
                target_row.copy_from_slice(source_row);
            } else {
                for (source, target) in source_row.iter().copied().zip(target_row.iter_mut()) {
                    *target = blend_premultiplied_argb_over_opaque(source, *target);
                }
            }
        }
        return;
    }

    let target_width = target.width as i64;
    let target_height = target.height as i64;
    let uv_left = plan.content_uv.left;
    let uv_top = plan.content_uv.top;
    let uv_width = plan.content_uv.right - plan.content_uv.left;
    let uv_height = plan.content_uv.bottom - plan.content_uv.top;
    for row_y in start_y..end_y {
        let local_y = row_y - i64::from(target.y);
        let source_y = ((uv_top * buffer_size.height as f32)
            + (local_y as f32 / target_height as f32) * uv_height * buffer_size.height as f32)
            .floor() as i64;
        let source_y = source_y.clamp(0, i64::from(buffer_size.height.saturating_sub(1))) as usize;
        let target_start = row_y as usize * frame_width + start_x as usize;
        let Some(target_row) =
            frame.get_mut(target_start..target_start + (end_x - start_x) as usize)
        else {
            continue;
        };
        for (column, target_pixel) in target_row.iter_mut().enumerate() {
            let local_x = (start_x - i64::from(target.x)) + column as i64;
            let source_x = ((uv_left * buffer_size.width as f32)
                + (local_x as f32 / target_width as f32) * uv_width * buffer_size.width as f32)
                .floor() as i64;
            let source_x =
                source_x.clamp(0, i64::from(buffer_size.width.saturating_sub(1))) as usize;
            let source_index = source_y * buffer_width + source_x;
            if let Some(source) = surface_pixels.get(source_index).copied() {
                *target_pixel = blend_premultiplied_argb_over_opaque(source, *target_pixel);
            }
        }
    }
}

fn fill_output_background_rect(
    scene: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    rect: OutputRect,
) {
    let Some(rect) = rect.clipped_to_output(frame_width, frame_height) else {
        return;
    };

    let frame_width = frame_width as usize;
    let left = rect.x as usize;
    let top = rect.y as usize;
    let row_width = rect.width as usize;
    for output_y in top..top.saturating_add(rect.height as usize) {
        let row_start = output_y.saturating_mul(frame_width).saturating_add(left);
        let row_end = row_start.saturating_add(row_width);
        let Some(scene_row) = scene.get_mut(row_start..row_end) else {
            continue;
        };
        scene_row.fill(OUTPUT_BACKGROUND);
    }
}

fn source_row_is_opaque(row: &[u32]) -> bool {
    row.iter().all(|pixel| pixel >> 24 == 0xff)
}

fn fill_rect_clipped(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    rect: ServerFrameRect,
    clip: OutputRect,
) {
    let Some(clipped) = (OutputRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    })
    .intersection(clip)
    .and_then(|rect| rect.clipped_to_output(frame_width, frame_height)) else {
        return;
    };

    fill_rect(
        frame,
        frame_width,
        frame_height,
        ServerFrameRect {
            color: rect.color,
            x: clipped.x,
            y: clipped.y,
            width: clipped.width,
            height: clipped.height,
        },
    );
}

fn fill_rect(frame: &mut [u32], frame_width: u32, frame_height: u32, rect: ServerFrameRect) {
    let start_x = i64::from(rect.x).max(0);
    let start_y = i64::from(rect.y).max(0);
    let end_x = i64::from(rect.x)
        .saturating_add(i64::from(rect.width))
        .min(i64::from(frame_width));
    let end_y = i64::from(rect.y)
        .saturating_add(i64::from(rect.height))
        .min(i64::from(frame_height));

    if start_x >= end_x || start_y >= end_y {
        return;
    }

    let frame_width = frame_width as usize;
    let color = rect.color.pixel();
    for target_y in start_y..end_y {
        let row_start = target_y as usize * frame_width + start_x as usize;
        let row_end = row_start + (end_x - start_x) as usize;
        if let Some(row) = frame.get_mut(row_start..row_end) {
            row.fill(color);
        }
    }
}

fn blend_premultiplied_argb_over_opaque(source: u32, target: u32) -> u32 {
    let alpha = (source >> 24) & 0xff;
    if alpha == 0 {
        return target;
    }
    if alpha == 0xff {
        return source;
    }

    let inverse_alpha = 255 - alpha;
    let source_red = (source >> 16) & 0xff;
    let source_green = (source >> 8) & 0xff;
    let source_blue = source & 0xff;
    let target_red = (target >> 16) & 0xff;
    let target_green = (target >> 8) & 0xff;
    let target_blue = target & 0xff;

    let red = blend_premultiplied_channel(source_red, target_red, inverse_alpha);
    let green = blend_premultiplied_channel(source_green, target_green, inverse_alpha);
    let blue = blend_premultiplied_channel(source_blue, target_blue, inverse_alpha);

    0xff00_0000 | (red << 16) | (green << 8) | blue
}

fn blend_premultiplied_channel(source: u32, target: u32, inverse_alpha: u32) -> u32 {
    source
        .saturating_add((target * inverse_alpha + 127) / 255)
        .min(255)
}

impl Default for DesktopSceneRenderer {
    fn default() -> Self {
        Self::with_cursor_image(shared_compositor_cursor_image())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::{SurfaceCommitSequence, SurfacePlacement, ViewportSourceRect};
    use crate::render_backend::buffer::{
        BufferIdAllocator, BufferIdentity, BufferSize, CommittedSurfaceBuffer,
    };
    use std::sync::{Arc, Mutex, OnceLock};

    fn test_buffer_identity() -> BufferIdentity {
        static IDS: OnceLock<Mutex<BufferIdAllocator>> = OnceLock::new();
        IDS.get_or_init(|| Mutex::new(BufferIdAllocator::default()))
            .lock()
            .expect("test buffer identity allocator")
            .allocate()
            .expect("test buffer identity")
    }

    fn shm_buffer(width: u32, height: u32, pixels: Vec<u32>) -> CommittedSurfaceBuffer {
        CommittedSurfaceBuffer::shm_snapshot(
            test_buffer_identity(),
            BufferSize::new(width, height).expect("test surfaces use non-zero sizes"),
            pixels,
        )
    }

    fn absolute_test_surface(surface_id: u32, x: i32, y: i32) -> RenderableSurface {
        RenderableSurface {
            surface_id,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            placement: SurfacePlacement::absolute_root_at(x, y),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 0,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(1, 1, vec![0xffff_ffff]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        }
    }

    fn solid_test_surface(
        surface_id: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        pixel: u32,
    ) -> RenderableSurface {
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
            generation: 0,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(width, height, vec![pixel; (width * height) as usize]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        }
    }

    fn solid_test_decoration(
        window_id: WindowId,
        root_surface_id: u32,
        width: u32,
        height: u32,
        pixel: [u8; 4],
    ) -> DecorationRenderInstance {
        let outer = DecorationRect::new(0, 0, width, height);
        let layout = super::super::decoration::layout::DecorationLayout {
            outer,
            client: outer,
            titlebar: DecorationRect::new(0, 0, width, height),
            title_safe: outer,
            resize_input: outer,
            visible_border: Vec::new(),
            buttons: Vec::new(),
            extents: Default::default(),
        };
        DecorationRenderInstance {
            plan: DecorationRenderPlan {
                layout,
                primitives: vec![DecorationRenderPrimitive::SolidRect {
                    rect: outer,
                    color: pixel,
                }],
                theme_generation: 1,
            },
            origin_x: 0,
            origin_y: 0,
            window_id,
            root_surface_id,
        }
    }

    #[test]
    fn absolute_root_origins_are_stable_when_renderable_stack_order_changes() {
        let first = absolute_test_surface(701, 72, 72);
        let second = absolute_test_surface(702, 104, 104);
        let initial = surface_origins(&[first.clone(), second.clone()]);
        let reordered = surface_origins(&[second, first]);

        assert_eq!(initial, vec![(72, 72), (104, 104)]);
        assert_eq!(reordered, vec![(104, 104), (72, 72)]);
    }

    #[test]
    fn window_visual_stack_order_keeps_each_decoration_with_its_window() {
        let surfaces = vec![
            solid_test_surface(701, 0, 0, 2, 2, 0xff00_0000),
            solid_test_surface(702, 2, 0, 2, 2, 0xff00_ff00),
            solid_test_surface(703, 4, 0, 2, 2, 0xff00_00ff),
        ];
        let decorations = vec![
            solid_test_decoration(
                WindowId::from_raw(1).expect("window id"),
                701,
                2,
                1,
                [0xff, 0x00, 0x00, 0xff],
            ),
            solid_test_decoration(
                WindowId::from_raw(3).expect("window id"),
                703,
                2,
                1,
                [0x00, 0x00, 0xff, 0xff],
            ),
        ];

        let groups = window_visual_stack_order(&surfaces, &decorations);

        assert_eq!(
            groups
                .iter()
                .map(WindowVisualGroup::root_surface_id)
                .collect::<Vec<_>>(),
            vec![701, 702, 703]
        );
        assert_eq!(groups[0].surface_indices(), &[0]);
        assert_eq!(groups[0].decoration_index(), Some(0));
        assert_eq!(groups[1].decoration_index(), None);
        assert_eq!(groups[2].decoration_index(), Some(1));
    }

    #[test]
    fn window_visual_stack_order_discards_orphan_decorations_and_counts_them() {
        let surfaces = vec![solid_test_surface(701, 0, 0, 4, 4, 0xffff_0000)];
        let decorations = vec![solid_test_decoration(
            WindowId::from_raw(99).expect("orphan window id"),
            999,
            4,
            4,
            [0x00, 0xff, 0x00, 0xff],
        )];

        let groups = window_visual_stack_order(&surfaces, &decorations);

        assert_eq!(
            groups
                .iter()
                .map(WindowVisualGroup::root_surface_id)
                .collect::<Vec<_>>(),
            vec![701]
        );
        assert_eq!(
            WindowVisualGroup::orphan_decoration_count(&surfaces, &decorations),
            1
        );

        let mut renderer = DesktopSceneRenderer::default();
        renderer.set_decoration_instances(&decorations);
        let mut frame = vec![0; 8 * 8];
        renderer.compose_request(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 8,
            frame_height: 8,
            output_scale: 1.0,
            surfaces: &surfaces,
            external_overlay_surface_ids: Vec::new(),
            content_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            client_cursor: None,
        });

        assert_eq!(frame[0], 0xffff_0000);
        assert_eq!(renderer.last_orphan_decoration_count(), 1);
    }

    #[test]
    fn popup_surfaces_paint_above_ssd_but_ordinary_subsurfaces_stay_with_the_window() {
        let root = solid_test_surface(701, 0, 0, 4, 4, 0xffff_0000);
        let mut ordinary_subsurface = solid_test_surface(702, 0, 0, 2, 2, 0xff00_00ff);
        ordinary_subsurface.placement = SurfacePlacement::subsurface(701, 0, 0);
        let mut popup = ordinary_subsurface.clone();
        popup.surface_id = 703;

        let decorations = vec![solid_test_decoration(
            WindowId::from_raw(99).expect("window id"),
            701,
            4,
            4,
            [0x00, 0xff, 0x00, 0xff],
        )];

        let ordinary_groups =
            window_visual_stack_order(&[root.clone(), ordinary_subsurface.clone()], &decorations);
        assert_eq!(ordinary_groups.len(), 1);
        assert_eq!(ordinary_groups[0].surface_indices(), &[0, 1]);
        assert_eq!(ordinary_groups[0].decoration_index(), Some(0));
        let ordinary_visual_groups = visual_stack_groups(&[root.clone(), ordinary_subsurface], &[]);
        assert_eq!(ordinary_visual_groups.len(), 1);
        assert_eq!(ordinary_visual_groups[0].root_surface_id(), 701);
        assert_eq!(ordinary_visual_groups[0].root_surface_index(), 0);
        assert_eq!(ordinary_visual_groups[0].surface_indices(), &[0, 1]);
        assert!(!ordinary_visual_groups[0].is_popup());

        let popup_surfaces = vec![root, popup];
        let popup_groups =
            window_visual_stack_order_with_popups(&popup_surfaces, &decorations, &[703]);
        assert_eq!(
            popup_groups
                .iter()
                .map(WindowVisualGroup::root_surface_id)
                .collect::<Vec<_>>(),
            vec![701, 703]
        );
        assert_eq!(popup_groups[0].surface_indices(), &[0]);
        assert_eq!(popup_groups[0].decoration_index(), Some(0));
        assert_eq!(popup_groups[1].surface_indices(), &[1]);
        assert_eq!(popup_groups[1].decoration_index(), None);
        let popup_visual_groups = visual_stack_groups(&popup_surfaces, &[703]);
        assert_eq!(popup_visual_groups.len(), 2);
        assert_eq!(popup_visual_groups[0].root_surface_id(), 701);
        assert_eq!(popup_visual_groups[0].root_surface_index(), 0);
        assert_eq!(popup_visual_groups[1].root_surface_id(), 703);
        assert_eq!(popup_visual_groups[1].root_surface_index(), 1);
        assert!(popup_visual_groups[1].is_popup());

        let mut renderer = DesktopSceneRenderer::default();
        renderer.set_decoration_instances(&decorations);
        renderer.set_popup_surface_ids(&[703]);
        let mut frame = vec![0; 4 * 4];
        renderer.compose_request(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 4,
            frame_height: 4,
            output_scale: 1.0,
            surfaces: &popup_surfaces,
            external_overlay_surface_ids: Vec::new(),
            content_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            client_cursor: None,
        });
        assert_eq!(frame[0], 0xff00_00ff);
    }

    #[test]
    fn desktop_scene_renderer_uses_neutral_background_on_full_rebuild() {
        let mut renderer = DesktopSceneRenderer::default();
        let mut frame = vec![0; 16 * 12];

        renderer.compose(
            &mut frame,
            16,
            12,
            &[],
            DesktopVisualState::wallpaper_only(),
        );
        assert!(frame.iter().all(|pixel| *pixel == OUTPUT_BACKGROUND));

        let mut resized = vec![0; 20 * 12];
        renderer.compose(
            &mut resized,
            20,
            12,
            &[],
            DesktopVisualState::wallpaper_only(),
        );
        assert!(resized.iter().all(|pixel| *pixel == OUTPUT_BACKGROUND));
    }

    #[test]
    fn lower_window_decoration_cannot_cover_higher_window_client_content() {
        let lower = solid_test_surface(701, 0, 0, 4, 4, 0xffff_0000);
        let higher = solid_test_surface(702, 0, 0, 4, 4, 0xff00_00ff);
        let lower_id = WindowId::from_raw(1).expect("lower window id");
        let mut renderer = DesktopSceneRenderer::default();
        renderer.set_decoration_instances(&[solid_test_decoration(
            lower_id,
            lower.surface_id,
            4,
            2,
            [0xff, 0x00, 0xff, 0xff],
        )]);
        let mut frame = vec![0; 8 * 8];

        renderer.compose_request(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 8,
            frame_height: 8,
            output_scale: 1.0,
            surfaces: &[lower, higher],
            external_overlay_surface_ids: Vec::new(),
            content_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            client_cursor: None,
        });

        assert_eq!(
            frame[0], 0xff00_00ff,
            "the higher client must win where it overlaps the lower titlebar"
        );
    }

    #[test]
    fn desktop_scene_renderer_reuses_composed_scene_when_only_cursor_moves() {
        let surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 0,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(
                2,
                2,
                vec![0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff],
            ),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let mut renderer = DesktopSceneRenderer::default();
        let mut frame = vec![0; 96 * 96];

        renderer.compose_with_generation(
            &mut frame,
            96,
            96,
            std::slice::from_ref(&surface),
            1,
            DesktopVisualState::with_cursor(4, 4),
        );
        let first_generation = renderer.scene_generation();

        renderer.compose_with_generation(
            &mut frame,
            96,
            96,
            &[surface],
            1,
            DesktopVisualState::with_cursor(20, 20),
        );

        assert_eq!(renderer.scene_generation(), first_generation);
    }

    #[test]
    fn cursor_image_change_invalidates_reusable_frame_and_draws_new_image() {
        let old_image =
            Arc::new(CompositorCursorImage::from_argb8888(vec![0xff00_0000], 1, 1, 0, 0).unwrap());
        let new_image =
            Arc::new(CompositorCursorImage::from_argb8888(vec![0xffff_0000], 1, 1, 0, 0).unwrap());
        let mut renderer = DesktopSceneRenderer::with_cursor_image(old_image);
        let mut frame = vec![0; 8 * 8];

        renderer.compose_request(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 8,
            frame_height: 8,
            output_scale: 1.0,
            surfaces: &[],
            external_overlay_surface_ids: Vec::new(),
            content_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            client_cursor: None,
        });
        assert!(renderer.reusable_frame_key.is_none());

        renderer.compose_reusing_frame(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 8,
            frame_height: 8,
            output_scale: 1.0,
            surfaces: &[],
            external_overlay_surface_ids: Vec::new(),
            content_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            client_cursor: None,
        });
        assert!(renderer.reusable_frame_key.is_some());

        renderer.set_cursor_image(new_image);
        assert!(renderer.reusable_frame_key.is_none());
        renderer.compose(&mut frame, 8, 8, &[], DesktopVisualState::with_cursor(3, 2));
        assert_eq!(frame[2 * 8 + 3], 0xffff_0000);
    }

    #[test]
    fn task_05_8_interactive_render_uses_full_uv_and_real_surface_size() {
        let surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 800,
            height: 600,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: Some(SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(
                0, 0, 1000, 700,
            ))),
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(800, 600, vec![0xffff_0000; 800 * 600]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        let plan = surface_render_plan(&surface, SurfaceTargetRect::new(0, 0, 800, 600));

        assert_eq!(plan.content_target.width(), 800);
        assert_eq!(plan.content_target.height(), 600);
        assert_eq!(plan.content_uv, SurfaceUvRect::FULL);
        assert_eq!(plan.clip, Some(SurfaceTargetRect::new(0, 0, 1000, 700)));
    }

    #[test]
    fn xwayland_grow_preview_renders_black_backing_without_scaling() {
        let surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 800,
            height: 600,
            placement: SurfacePlacement::absolute_root_at(100, 100),
            render_backend: SurfaceRenderBackend::Xwayland,
            render_placement: Some(SurfacePlacement::absolute_root_at(100, 100)),
            visual_clip: Some(SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(
                100, 100, 1100, 760,
            ))),
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(800, 600, vec![0xffff_0000; 800 * 600]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        let element = render_scene_elements_for_surfaces(std::slice::from_ref(&surface), 1.0)
            .pop()
            .expect("XWayland scene element");
        assert_eq!(
            element.backing_target(),
            Some(SurfaceTargetRect::new(100, 100, 1100, 760))
        );
        assert_eq!(
            element.target(),
            SurfaceTargetRect::new(100, 100, 800, 600),
            "the stale client texture remains at its committed size"
        );

        let mut frame = vec![0; 1300 * 900];
        compose_output(
            &mut frame,
            1300,
            900,
            std::slice::from_ref(&surface),
            DesktopVisualState::wallpaper_only(),
        );
        assert_eq!(frame[(100 * 1300 + 100) as usize], 0xffff_0000);
        assert_eq!(frame[(100 * 1300 + 100 + 800) as usize], 0xff00_0000);
        assert_eq!(frame[(100 + 600) * 1300 + 100], 0xff00_0000);
    }

    #[test]
    fn native_absolute_root_preview_does_not_get_xwayland_backing() {
        let surface = RenderableSurface {
            surface_id: 11,
            x: 0,
            y: 0,
            width: 800,
            height: 600,
            placement: SurfacePlacement::absolute_root_at(100, 100),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: Some(SurfacePlacement::absolute_root_at(100, 100)),
            visual_clip: Some(SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(
                100, 100, 1100, 760,
            ))),
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(800, 600, vec![0x80ff_0000; 800 * 600]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        let element = render_scene_elements_for_surfaces(std::slice::from_ref(&surface), 1.0)
            .pop()
            .expect("native scene element");
        assert_eq!(element.backing_target(), None);
        assert!(server_frame_rects_for_surface(&surface).is_empty());

        let mut frame = vec![0; 1300 * 900];
        let transparent_surface = RenderableSurface {
            buffer: shm_buffer(800, 600, vec![0x0000_0000; 800 * 600]),
            ..surface
        };
        compose_output(
            &mut frame,
            1300,
            900,
            std::slice::from_ref(&transparent_surface),
            DesktopVisualState::wallpaper_only(),
        );
        assert_eq!(frame[100 * 1300 + 100], OUTPUT_BACKGROUND);
    }

    #[test]
    fn non_absolute_root_preview_does_not_get_xwayland_backing() {
        let surface = RenderableSurface {
            surface_id: 8,
            x: 0,
            y: 0,
            width: 800,
            height: 600,
            placement: SurfacePlacement::root_at(100, 100),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: Some(SurfacePlacement::root_at(100, 100)),
            visual_clip: Some(SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(
                100, 100, 1100, 760,
            ))),
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(800, 600, vec![0xffff_0000; 800 * 600]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        let element = render_scene_elements_for_surfaces(std::slice::from_ref(&surface), 1.0)
            .pop()
            .expect("ordinary scene element");
        assert_eq!(element.backing_target(), None);
    }

    #[test]
    fn ordinary_xdg_and_managed_x11_surfaces_emit_no_server_frame_primitives() {
        let xdg = RenderableSurface {
            surface_id: 9,
            x: 0,
            y: 0,
            width: 80,
            height: 60,
            placement: SurfacePlacement::root_at(24, 32),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(80, 60, vec![0xffff_0000; 80 * 60]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let x11 = RenderableSurface {
            surface_id: 10,
            placement: SurfacePlacement::absolute_root_at(48, 40),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            ..xdg.clone()
        };

        for surface in [&xdg, &x11] {
            assert!(server_frame_rects_for_surface(surface).is_empty());
            let element = render_scene_elements_for_surfaces(std::slice::from_ref(surface), 1.0)
                .pop()
                .expect("ordinary scene element");
            assert_eq!(element.backing_target(), None);
        }
    }

    #[test]
    fn viewport_source_selects_source_uv_without_changing_target() {
        let surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 100,
            height: 50,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(200, 100, vec![0xffff_0000; 200 * 100]),
            viewport_source: ViewportSourceRect::new(20.0, 10.0, 100.0, 50.0),
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        let plan = surface_render_plan(&surface, SurfaceTargetRect::new(0, 0, 100, 50));

        assert_eq!(plan.content_target.width(), 100);
        assert_eq!(plan.content_target.height(), 50);
        assert_eq!(
            plan.content_uv,
            SurfaceUvRect {
                left: 0.1,
                top: 0.1,
                right: 0.6,
                bottom: 0.6,
            }
        );
    }

    #[test]
    fn task_05_8_shrink_clips_without_scaling() {
        let near_surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 1000,
            height: 700,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: Some(SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(
                0, 0, 800, 600,
            ))),
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(1000, 700, vec![0xffff_0000; 1000 * 700]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let near = surface_render_plan(&near_surface, SurfaceTargetRect::new(0, 0, 1000, 700));
        assert_eq!(
            (near.content_target.width(), near.content_target.height()),
            (800, 600)
        );
        assert_eq!(near.content_uv.left, 0.0);
        assert_eq!(near.content_uv.top, 0.0);
        assert_eq!(near.content_uv.right, 0.8);
        assert!((near.content_uv.bottom - (600.0 / 700.0)).abs() < f32::EPSILON);

        let far_surface = RenderableSurface {
            visual_clip: Some(SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(
                200, 100, 800, 600,
            ))),
            render_target_size: None,
            ..near_surface
        };
        let far = surface_render_plan(&far_surface, SurfaceTargetRect::new(0, 0, 1000, 700));
        assert_eq!((far.content_target.x(), far.content_target.y()), (200, 100));
        assert_eq!(
            (far.content_target.width(), far.content_target.height()),
            (800, 600)
        );
        assert_eq!(far.content_uv.left, 0.2);
        assert!((far.content_uv.top - (100.0 / 700.0)).abs() < f32::EPSILON);
        assert_eq!(far.content_uv.right, 1.0);
        assert_eq!(far.content_uv.bottom, 1.0);
    }

    #[test]
    fn active_resize_clip_uses_default_root_render_space_origin() {
        let surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 300,
            height: 200,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: Some(SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(
                0, 0, 340, 230,
            ))),
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(300, 200, vec![0xffff_0000; 300 * 200]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        let element = render_scene_elements_for_surfaces(&[surface], 1.0)
            .pop()
            .expect("render element");

        assert_eq!(
            element.target(),
            SurfaceTargetRect::new(FIRST_SURFACE_OFFSET.0, FIRST_SURFACE_OFFSET.1, 300, 200)
        );
        assert_eq!(
            element.visible_target(),
            SurfaceTargetRect::new(FIRST_SURFACE_OFFSET.0, FIRST_SURFACE_OFFSET.1, 300, 200)
        );
    }

    #[test]
    fn active_resize_clip_uses_cascaded_root_render_space_origin() {
        let first = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 120,
            height: 80,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(120, 80, vec![0xffff_0000; 120 * 80]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let second = RenderableSurface {
            surface_id: 8,
            width: 300,
            height: 200,
            visual_clip: Some(SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(
                0, 0, 340, 230,
            ))),
            render_target_size: None,
            buffer: shm_buffer(300, 200, vec![0xffff_0000; 300 * 200]),
            ..first.clone()
        };

        let elements = render_scene_elements_for_surfaces(&[first, second], 1.0);
        let second = &elements[1];
        let origin = (
            FIRST_SURFACE_OFFSET.0 + SURFACE_CASCADE_STEP,
            FIRST_SURFACE_OFFSET.1 + SURFACE_CASCADE_STEP,
        );

        assert_eq!(
            second.visible_target(),
            SurfaceTargetRect::new(origin.0, origin.1, 300, 200)
        );
    }

    #[test]
    fn active_resize_clip_preserves_csd_window_geometry_offset() {
        let surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 372,
            height: 272,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: Some(SurfacePlacement::root_at(-16, -10)),
            visual_clip: Some(SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(
                0, 0, 340, 230,
            ))),
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(372, 272, vec![0xffff_0000; 372 * 272]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        let element = render_scene_elements_for_surfaces(&[surface], 1.0)
            .pop()
            .expect("render element");

        assert_eq!(
            element.target(),
            SurfaceTargetRect::new(
                FIRST_SURFACE_OFFSET.0 - 16,
                FIRST_SURFACE_OFFSET.1 - 10,
                372,
                272
            )
        );
        assert_eq!(
            element.visible_target(),
            SurfaceTargetRect::new(FIRST_SURFACE_OFFSET.0, FIRST_SURFACE_OFFSET.1, 340, 230)
        );
    }

    #[test]
    fn active_resize_clip_uses_same_output_scale_rounding_as_target() {
        let surface = RenderableSurface {
            surface_id: 7,
            x: 1,
            y: 1,
            width: 301,
            height: 201,
            placement: SurfacePlacement::root_at(1, 1),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: Some(SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(
                1, 1, 301, 201,
            ))),
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(301, 201, vec![0xffff_0000; 301 * 201]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        let element = render_scene_elements_for_surfaces(&[surface], 1.25)
            .pop()
            .expect("render element");

        assert_eq!(element.visible_target(), element.target());
    }

    #[test]
    fn active_resize_clip_keeps_all_resize_edges_in_render_space() {
        for clip in [
            SurfaceTargetRect::new(0, 0, 260, 200),
            SurfaceTargetRect::new(40, 0, 260, 200),
            SurfaceTargetRect::new(0, 30, 300, 170),
            SurfaceTargetRect::new(40, 30, 260, 170),
            SurfaceTargetRect::new(0, 0, 300, 170),
        ] {
            let surface = RenderableSurface {
                surface_id: 7,
                x: 0,
                y: 0,
                width: 300,
                height: 200,
                placement: SurfacePlacement::root(),
                render_backend: SurfaceRenderBackend::NativeWayland,
                render_placement: None,
                visual_clip: Some(SurfaceVisualAperture::logical_only(clip)),
                render_target_size: None,
                generation: 1,
                commit_sequence: SurfaceCommitSequence::initial(),
                buffer: shm_buffer(300, 200, vec![0xffff_0000; 300 * 200]),
                viewport_source: None,
                viewport_destination: None,
                buffer_scale: 1,
                buffer_transform: wl_output::Transform::Normal,
                damage: crate::compositor::RenderableSurfaceDamage::full(),
            };

            let element = render_scene_elements_for_surfaces(&[surface], 1.0)
                .pop()
                .expect("render element");

            assert_eq!(
                element.visible_target(),
                SurfaceTargetRect::new(
                    FIRST_SURFACE_OFFSET.0 + clip.x(),
                    FIRST_SURFACE_OFFSET.1 + clip.y(),
                    clip.width(),
                    clip.height()
                )
            );
        }
    }

    #[test]
    fn active_resize_clip_applies_to_subsurface_in_root_render_space() {
        let root = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 300,
            height: 200,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: Some(SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(
                0, 0, 320, 220,
            ))),
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(300, 200, vec![0xffff_0000; 300 * 200]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let child = RenderableSurface {
            surface_id: 8,
            x: 0,
            y: 0,
            width: 40,
            height: 30,
            placement: SurfacePlacement::subsurface(7, 260, 20),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: Some(SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(
                0, 0, 280, 220,
            ))),
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(40, 30, vec![0xffff_0000; 40 * 30]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        let elements = render_scene_elements_for_surfaces(&[root, child], 1.0);
        let child = &elements[1];

        assert_eq!(
            child.visible_target(),
            SurfaceTargetRect::new(
                FIRST_SURFACE_OFFSET.0 + 260,
                FIRST_SURFACE_OFFSET.1 + 20,
                20,
                30
            )
        );
    }

    #[test]
    fn task_05_8_scene_snapshot_tracks_visual_clip_changes() {
        let previous = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 1000,
            height: 700,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: Some(SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(
                0, 0, 1000, 700,
            ))),
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(1000, 700, vec![0xffff_0000; 1000 * 700]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let mut current = previous.clone();
        current.visual_clip = Some(SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(
            0, 0, 800, 600,
        )));
        current.generation = 2;

        let previous_snapshot = scene_surface_snapshots(std::slice::from_ref(&previous), 1.0);
        let current_elements =
            render_scene_elements_for_surfaces(std::slice::from_ref(&current), 1.0);
        let current_snapshot = scene_surface_snapshots(std::slice::from_ref(&current), 1.0);
        let damage = partial_scene_damage_rects(
            &previous_snapshot,
            &current_elements,
            &current_snapshot,
            1200,
            900,
        )
        .expect("same surface can produce partial damage");

        assert_eq!(
            damage,
            vec![OutputRect {
                x: FIRST_SURFACE_OFFSET.0,
                y: FIRST_SURFACE_OFFSET.1,
                width: 1000,
                height: 700,
            }]
        );
    }

    #[test]
    fn xwayland_visual_backing_damage_covers_grow_preview_bounds() {
        let previous = RenderableSurface {
            surface_id: 17,
            x: 0,
            y: 0,
            width: 800,
            height: 600,
            placement: SurfacePlacement::absolute_root_at(100, 100),
            render_backend: SurfaceRenderBackend::Xwayland,
            render_placement: Some(SurfacePlacement::absolute_root_at(100, 100)),
            visual_clip: Some(SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(
                100, 100, 800, 600,
            ))),
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(800, 600, vec![0xffff_0000; 800 * 600]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let mut current = previous.clone();
        current.visual_clip = Some(SurfaceVisualAperture::logical_only(SurfaceTargetRect::new(
            100, 100, 1100, 760,
        )));
        current.generation = 2;

        let previous_snapshots = scene_surface_snapshots(std::slice::from_ref(&previous), 1.0);
        let current_elements =
            render_scene_elements_for_surfaces(std::slice::from_ref(&current), 1.0);
        let current_snapshots = scene_surface_snapshots(std::slice::from_ref(&current), 1.0);
        let damage = partial_scene_damage_rects(
            &previous_snapshots,
            &current_elements,
            &current_snapshots,
            1600,
            1000,
        )
        .expect("same surface can produce partial damage");

        assert!(
            damage.contains(&OutputRect {
                x: 100,
                y: 100,
                width: 1100,
                height: 760,
            }),
            "grow preview must damage the complete new black backing box: {damage:?}"
        );
    }

    #[test]
    fn task_3_root_aperture_uses_the_same_regions_for_cpu_and_gles_plans() {
        let aperture = SurfaceVisualAperture::for_root_window_preview(
            (84, 70),
            BufferSize::new(332, 242).expect("root buffer"),
            (16, 10, 16, 32),
            SurfaceTargetRect::new(100, 80, 300, 200),
        );
        let surface = RenderableSurface {
            surface_id: 17,
            x: 0,
            y: 0,
            width: 332,
            height: 242,
            placement: SurfacePlacement::absolute_root_at(84, 70),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: Some(SurfacePlacement::absolute_root_at(84, 70)),
            visual_clip: Some(aperture),
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(332, 242, vec![0xffff_0000; 332 * 242]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        let assignment = surface_render_space_assignments(std::slice::from_ref(&surface), 1.0)
            .into_iter()
            .next()
            .expect("root render assignment");
        let gles_plans = surface_render_plans_with_aperture(
            &surface,
            assignment.target,
            assignment.visual_clip.as_ref(),
        );
        let cpu_element = render_scene_elements_for_surfaces(std::slice::from_ref(&surface), 1.0)
            .into_iter()
            .next()
            .expect("CPU scene element");
        let cpu_plans = cpu_element.content_regions().to_vec();

        assert_eq!(cpu_plans, gles_plans);
        assert_eq!(
            cpu_element.backing_target(),
            xwayland_visual_backing_target(&surface, surface.visual_clip.as_ref())
        );
        let mut xwayland_surface = surface.clone();
        xwayland_surface.render_backend = SurfaceRenderBackend::Xwayland;
        let xwayland_element =
            render_scene_elements_for_surfaces(std::slice::from_ref(&xwayland_surface), 1.0)
                .into_iter()
                .next()
                .expect("XWayland CPU scene element");
        assert_eq!(
            xwayland_element.backing_target(),
            xwayland_visual_backing_target(
                &xwayland_surface,
                xwayland_surface.visual_clip.as_ref(),
            )
        );
        assert!(xwayland_element.backing_target().is_some());
        assert!(
            cpu_plans
                .iter()
                .any(|plan| { plan.clip == Some(SurfaceTargetRect::new(84, 70, 332, 10)) })
        );
        assert!(cpu_plans.iter().all(|plan| {
            plan.clip
                .is_some_and(|clip| !clip.intersects(SurfaceTargetRect::new(100, 80, 300, 200)))
                || plan.content_target == SurfaceTargetRect::new(100, 80, 300, 200)
        }));
    }

    #[test]
    fn task_3_aperture_damage_covers_old_and_new_root_bounds() {
        let previous_aperture = SurfaceVisualAperture::for_root_window_preview(
            (100, 100),
            BufferSize::new(332, 242).expect("root buffer"),
            (16, 10, 16, 32),
            SurfaceTargetRect::new(116, 110, 300, 200),
        );
        let current_aperture = SurfaceVisualAperture::for_root_window_preview(
            (500, 400),
            BufferSize::new(332, 242).expect("root buffer"),
            (16, 10, 16, 32),
            SurfaceTargetRect::new(516, 410, 300, 200),
        );
        let previous = RenderableSurface {
            surface_id: 17,
            x: 0,
            y: 0,
            width: 332,
            height: 242,
            placement: SurfacePlacement::absolute_root_at(100, 100),
            render_backend: SurfaceRenderBackend::Xwayland,
            render_placement: Some(SurfacePlacement::absolute_root_at(100, 100)),
            visual_clip: Some(previous_aperture),
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(332, 242, vec![0xffff_0000; 332 * 242]),
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            viewport_source: None,
            viewport_destination: None,
            damage: RenderableSurfaceDamage::Full,
        };
        let current = RenderableSurface {
            placement: SurfacePlacement::absolute_root_at(500, 400),
            render_backend: SurfaceRenderBackend::Xwayland,
            render_placement: Some(SurfacePlacement::absolute_root_at(500, 400)),
            visual_clip: Some(current_aperture),
            generation: 2,
            ..previous.clone()
        };
        let previous_element =
            render_scene_elements_for_surfaces(std::slice::from_ref(&previous), 1.0)
                .pop()
                .expect("previous scene element");
        let current_element =
            render_scene_elements_for_surfaces(std::slice::from_ref(&current), 1.0)
                .pop()
                .expect("current scene element");
        let previous_bounds = previous_element.backing_target().expect("previous bounds");
        let current_bounds = current_element.backing_target().expect("current bounds");
        let damage = partial_scene_damage_rects(
            &scene_surface_snapshots(std::slice::from_ref(&previous), 1.0),
            std::slice::from_ref(&current_element),
            &scene_surface_snapshots(std::slice::from_ref(&current), 1.0),
            1000,
            800,
        )
        .expect("same root produces partial damage");

        assert!(damage.contains(&previous_bounds.output_rect()));
        assert!(damage.contains(&current_bounds.output_rect()));
    }

    #[test]
    fn desktop_scene_renderer_resize_growth_repairs_rescaled_bounds() {
        let mut renderer = DesktopSceneRenderer::default();
        let mut frame = vec![0; 96 * 96];
        let initial_surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 4,
            height: 4,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(4, 4, vec![0xffff_0000; 4 * 4]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        renderer.compose_reusing_frame(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 96,
            frame_height: 96,
            output_scale: 1.0,
            surfaces: std::slice::from_ref(&initial_surface),
            external_overlay_surface_ids: Vec::new(),
            content_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            client_cursor: None,
        });

        let resized_surface = RenderableSurface {
            width: 8,
            height: 6,
            generation: 2,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            ..initial_surface
        };
        renderer.compose_reusing_frame(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 96,
            frame_height: 96,
            output_scale: 1.0,
            surfaces: std::slice::from_ref(&resized_surface),
            external_overlay_surface_ids: Vec::new(),
            content_generation: 2,
            visual_state: DesktopVisualState::wallpaper_only(),
            client_cursor: None,
        });

        assert_eq!(
            renderer.last_rebuild_kind(),
            DesktopSceneRebuildKind::Partial
        );
        assert_eq!(
            renderer.last_frame_copy_kind(),
            DesktopFrameCopyKind::Partial
        );
        assert_eq!(renderer.last_rebuild_damage_rects.len(), 1);
        assert_eq!(
            renderer.last_rebuild_damage_rects,
            vec![OutputRect {
                x: FIRST_SURFACE_OFFSET.0,
                y: FIRST_SURFACE_OFFSET.1,
                width: 8,
                height: 6,
            }]
        );
    }

    #[test]
    fn desktop_scene_renderer_repaints_only_partial_surface_damage() {
        let mut renderer = DesktopSceneRenderer::default();
        let mut frame = vec![0; 96 * 96];
        let initial_surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 4,
            height: 4,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(4, 4, vec![0xffff_0000; 4 * 4]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        renderer.compose_with_generation(
            &mut frame,
            96,
            96,
            &[initial_surface],
            1,
            DesktopVisualState::wallpaper_only(),
        );

        let mut updated_pixels = vec![0xff00_00ff; 4 * 4];
        updated_pixels[5] = 0xff00_ff00;
        let updated_surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 4,
            height: 4,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 2,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(4, 4, updated_pixels),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::Partial(vec![
                crate::compositor::SurfaceDamageRect {
                    x: 1,
                    y: 1,
                    width: 1,
                    height: 1,
                },
            ]),
        };

        renderer.compose_with_generation(
            &mut frame,
            96,
            96,
            &[updated_surface],
            2,
            DesktopVisualState::wallpaper_only(),
        );

        assert_eq!(frame[73 * 96 + 73], 0xff00_ff00);
        assert_eq!(frame[72 * 96 + 72], 0xffff_0000);
        assert_eq!(frame[72 * 96 + 73], 0xffff_0000);
        let stats = renderer.last_damage_debug_stats();
        assert_eq!(stats.kind, DesktopSceneRebuildKind::Partial);
        assert_eq!(stats.rect_count, 1);
        assert!(stats.damaged_area < stats.frame_area);
    }

    #[test]
    fn desktop_scene_renderer_reusing_frame_copies_only_partial_damage() {
        let mut renderer = DesktopSceneRenderer::default();
        let mut frame = vec![0; 96 * 96];
        let initial_surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 4,
            height: 4,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(4, 4, vec![0xffff_0000; 4 * 4]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        renderer.compose_reusing_frame(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 96,
            frame_height: 96,
            output_scale: 1.0,
            surfaces: &[initial_surface],
            external_overlay_surface_ids: Vec::new(),
            content_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            client_cursor: None,
        });
        assert_eq!(renderer.last_frame_copy_kind(), DesktopFrameCopyKind::Full);

        let mut updated_pixels = vec![0xffff_0000; 4 * 4];
        updated_pixels[5] = 0xff00_ff00;
        let updated_surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 4,
            height: 4,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 2,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(4, 4, updated_pixels),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::Partial(vec![
                crate::compositor::SurfaceDamageRect {
                    x: 1,
                    y: 1,
                    width: 1,
                    height: 1,
                },
            ]),
        };

        renderer.compose_reusing_frame(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 96,
            frame_height: 96,
            output_scale: 1.0,
            surfaces: &[updated_surface],
            external_overlay_surface_ids: Vec::new(),
            content_generation: 2,
            visual_state: DesktopVisualState::wallpaper_only(),
            client_cursor: None,
        });

        assert_eq!(
            renderer.last_rebuild_kind(),
            DesktopSceneRebuildKind::Partial
        );
        assert_eq!(
            renderer.last_frame_copy_kind(),
            DesktopFrameCopyKind::Partial
        );
        assert_eq!(frame[73 * 96 + 73], 0xff00_ff00);
        assert_eq!(frame[72 * 96 + 72], 0xffff_0000);
    }

    #[test]
    fn desktop_scene_renderer_repairs_exposed_region_from_lower_background_surface() {
        let mut renderer = DesktopSceneRenderer::default();
        let mut frame = vec![0; 16 * 16];
        let lower = solid_test_surface(701, 0, 0, 8, 8, 0xff00_ff00);
        let upper = solid_test_surface(702, 0, 0, 4, 4, 0xffff_0000);

        renderer.compose_reusing_frame(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 16,
            frame_height: 16,
            output_scale: 1.0,
            surfaces: &[lower.clone(), upper],
            external_overlay_surface_ids: Vec::new(),
            content_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            client_cursor: None,
        });
        assert_eq!(frame[0], 0xffff_0000);

        renderer.compose_reusing_frame(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 16,
            frame_height: 16,
            output_scale: 1.0,
            surfaces: std::slice::from_ref(&lower),
            external_overlay_surface_ids: Vec::new(),
            content_generation: 2,
            visual_state: DesktopVisualState::wallpaper_only(),
            client_cursor: None,
        });

        assert_eq!(frame[0], 0xff00_ff00);
        assert_eq!(frame[3 * 16 + 3], 0xff00_ff00);
        assert_eq!(frame[7 * 16 + 7], 0xff00_ff00);
    }

    fn test_decoration_instance(origin_x: i32, origin_y: i32) -> DecorationRenderInstance {
        let layout = crate::compositor::decoration::layout::DecorationLayout::for_window(
            20,
            20,
            crate::compositor::decoration::types::DecorationMode::ServerSide,
            false,
            false,
            crate::compositor::decoration::types::DecorationMetrics::mac_tahoe(),
        )
        .expect("test decoration layout");
        let plan = crate::compositor::decoration::render_plan::DecorationRenderPlan {
            layout: layout.clone(),
            primitives: vec![DecorationRenderPrimitive::SolidRect {
                rect: layout.titlebar,
                color: [51, 51, 51, 255],
            }],
            theme_generation: 1,
        };
        DecorationRenderInstance {
            plan,
            origin_x,
            origin_y,
            window_id: WindowId::from_raw(1).expect("test window id"),
            root_surface_id: 1,
        }
    }

    #[test]
    fn decoration_move_reuses_cpu_frame_with_bounded_old_and_new_copy_damage() {
        let mut renderer = DesktopSceneRenderer::default();
        let mut frame = vec![0; 96 * 96];
        let root = absolute_test_surface(1, 80, 80);
        let old = test_decoration_instance(20, 20);
        renderer.set_decoration_instances(std::slice::from_ref(&old));
        renderer.compose_reusing_frame(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 96,
            frame_height: 96,
            output_scale: 1.0,
            surfaces: std::slice::from_ref(&root),
            external_overlay_surface_ids: Vec::new(),
            content_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            client_cursor: None,
        });
        let titlebar_pixel = 0xff33_3333;
        assert_eq!(frame[21 * 96 + 21], titlebar_pixel);

        let new = test_decoration_instance(50, 20);
        renderer.set_decoration_instances(std::slice::from_ref(&new));
        renderer.compose_reusing_frame(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 96,
            frame_height: 96,
            output_scale: 1.0,
            surfaces: std::slice::from_ref(&root),
            external_overlay_surface_ids: Vec::new(),
            content_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            client_cursor: None,
        });

        assert_eq!(
            renderer.last_rebuild_kind(),
            DesktopSceneRebuildKind::Partial,
            "decoration movement is part of the complete scene damage"
        );
        assert_eq!(
            renderer.last_frame_copy_kind(),
            DesktopFrameCopyKind::Partial
        );
        assert_ne!(frame[21 * 96 + 21], titlebar_pixel);
        assert_eq!(frame[21 * 96 + 51], titlebar_pixel);
    }

    #[test]
    fn buffer_age_zero_normalizes_to_reset() {
        assert_eq!(BufferAge::Age(0).normalized(), BufferAge::Reset);
        assert_eq!(
            BufferAge::Age(99).normalized(),
            BufferAge::Age(MAX_BUFFER_AGE)
        );
    }

    #[test]
    fn render_scene_elements_for_surfaces_preserve_damage_and_buffer_source() {
        let surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 4,
            height: 2,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: Some(BufferSize::new(6, 3).unwrap()),
            generation: 3,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(8, 4, vec![0xffff_0000; 8 * 4]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::Partial(vec![
                crate::compositor::SurfaceDamageRect {
                    x: 2,
                    y: 1,
                    width: 2,
                    height: 1,
                },
            ]),
        };

        let elements = render_scene_elements_for_surfaces(std::slice::from_ref(&surface), 1.0);

        assert_eq!(
            elements,
            vec![RenderSceneElement::from_surface(
                &surface,
                SurfaceTargetRect {
                    x: FIRST_SURFACE_OFFSET.0,
                    y: FIRST_SURFACE_OFFSET.1,
                    width: 6,
                    height: 3,
                },
            )]
        );
        assert_eq!(elements[0].content_uv(), SurfaceUvRect::FULL);
        assert_eq!(elements[0].buffer_size(), BufferSize::new(8, 4).unwrap());
    }

    #[test]
    fn damage_debug_stats_report_full_frame_area() {
        let stats = DamageDebugStats::full(1920, 1080);

        assert_eq!(stats.kind, DesktopSceneRebuildKind::Full);
        assert_eq!(stats.rect_count, 1);
        assert_eq!(stats.damaged_area, 1920 * 1080);
        assert_eq!(stats.frame_area, 1920 * 1080);
        assert_eq!(stats.coverage_percent(), 100);
    }

    #[test]
    fn damage_debug_stats_report_partial_coverage() {
        let stats = DamageDebugStats::partial(
            100,
            100,
            [
                Some(SurfaceDamageRect {
                    x: 0,
                    y: 0,
                    width: 20,
                    height: 10,
                }),
                Some(SurfaceDamageRect {
                    x: 80,
                    y: 80,
                    width: 10,
                    height: 10,
                }),
                None,
                None,
            ],
        );

        assert_eq!(stats.kind, DesktopSceneRebuildKind::Partial);
        assert_eq!(stats.rect_count, 2);
        assert_eq!(stats.damaged_area, 300);
        assert_eq!(stats.coverage_percent(), 3);
    }

    #[test]
    fn desktop_scene_renderer_buffer_age_reset_forces_full_rebuild() {
        let mut renderer = DesktopSceneRenderer::default();
        let mut frame = vec![0; 96 * 96];
        let initial_surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 4,
            height: 4,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(4, 4, vec![0xffff_0000; 4 * 4]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        renderer.compose_request_with_buffer_age(
            DesktopComposeRequest {
                frame: &mut frame,
                frame_width: 96,
                frame_height: 96,
                output_scale: 1.0,
                surfaces: std::slice::from_ref(&initial_surface),
                external_overlay_surface_ids: Vec::new(),
                content_generation: 1,
                visual_state: DesktopVisualState::wallpaper_only(),
                client_cursor: None,
            },
            BufferAge::Reset,
        );

        let updated_surface = RenderableSurface {
            generation: 2,
            damage: crate::compositor::RenderableSurfaceDamage::Partial(vec![
                crate::compositor::SurfaceDamageRect {
                    x: 1,
                    y: 1,
                    width: 1,
                    height: 1,
                },
            ]),
            ..initial_surface
        };
        renderer.compose_request_with_buffer_age(
            DesktopComposeRequest {
                frame: &mut frame,
                frame_width: 96,
                frame_height: 96,
                output_scale: 1.0,
                surfaces: &[updated_surface],
                external_overlay_surface_ids: Vec::new(),
                content_generation: 2,
                visual_state: DesktopVisualState::wallpaper_only(),
                client_cursor: None,
            },
            BufferAge::Reset,
        );

        assert_eq!(renderer.last_rebuild_kind(), DesktopSceneRebuildKind::Full);
        assert_eq!(renderer.last_frame_copy_kind(), DesktopFrameCopyKind::Full);
    }

    #[test]
    fn desktop_scene_renderer_partial_damage_redraws_overlapping_surfaces() {
        let mut renderer = DesktopSceneRenderer::default();
        let mut frame = vec![0; 96 * 96];
        let bottom = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 4,
            height: 4,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(4, 4, vec![0xffff_0000; 4 * 4]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let top = RenderableSurface {
            surface_id: 8,
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            placement: SurfacePlacement::subsurface(7, 1, 1),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(2, 2, vec![0xff00_ff00; 2 * 2]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        renderer.compose_with_generation(
            &mut frame,
            96,
            96,
            &[bottom, top.clone()],
            1,
            DesktopVisualState::wallpaper_only(),
        );

        let mut updated_pixels = vec![0xffff_0000; 4 * 4];
        updated_pixels[5] = 0xff00_00ff;
        let updated_bottom = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 4,
            height: 4,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 2,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(4, 4, updated_pixels),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::Partial(vec![
                crate::compositor::SurfaceDamageRect {
                    x: 1,
                    y: 1,
                    width: 1,
                    height: 1,
                },
            ]),
        };

        renderer.compose_with_generation(
            &mut frame,
            96,
            96,
            &[updated_bottom, top],
            2,
            DesktopVisualState::wallpaper_only(),
        );

        assert_eq!(frame[73 * 96 + 73], 0xff00_ff00);
    }

    #[test]
    fn desktop_scene_renderer_falls_back_to_full_when_surface_layout_changes() {
        let mut renderer = DesktopSceneRenderer::default();
        let mut frame = vec![0; 96 * 96];
        let initial_surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 4,
            height: 4,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(4, 4, vec![0xffff_0000; 4 * 4]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        renderer.compose_with_generation(
            &mut frame,
            96,
            96,
            &[initial_surface],
            1,
            DesktopVisualState::wallpaper_only(),
        );

        let moved_surface = RenderableSurface {
            surface_id: 7,
            x: 2,
            y: 0,
            width: 4,
            height: 4,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 2,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(4, 4, vec![0xff00_00ff; 4 * 4]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::Partial(vec![
                crate::compositor::SurfaceDamageRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
            ]),
        };

        renderer.compose_with_generation(
            &mut frame,
            96,
            96,
            &[moved_surface],
            2,
            DesktopVisualState::wallpaper_only(),
        );

        assert_eq!(frame[72 * 96 + 72], OUTPUT_BACKGROUND);
        assert_eq!(frame[72 * 96 + 74], 0xff00_00ff);
    }

    #[test]
    fn compose_output_draws_neutral_background_when_empty() {
        let mut frame = vec![0; 12 * 8];

        compose_output(&mut frame, 12, 8, &[], DesktopVisualState::wallpaper_only());

        assert!(frame.iter().all(|pixel| *pixel == OUTPUT_BACKGROUND));
    }

    #[test]
    fn compose_output_draws_cursor_over_scene() {
        let mut frame = vec![0; 48 * 48];

        compose_output(
            &mut frame,
            48,
            48,
            &[],
            DesktopVisualState::with_cursor(12, 10),
        );

        assert_eq!(frame[10 * 48 + 12], CURSOR_OUTLINE);
        assert_eq!(frame[14 * 48 + 14], CURSOR_FILL);
    }

    #[test]
    fn software_cursor_draws_at_hotspot_adjusted_position() {
        let image = Arc::new(CompositorCursorImage {
            pixels_argb8888: vec![0xff00_0000, 0xffff_0000, 0xff00_ff00, 0],
            width: 2,
            height: 2,
            hotspot_x: 1,
            hotspot_y: 1,
            requested_size: 2,
            theme: "test".to_string(),
            source: None,
        });
        let mut renderer = DesktopSceneRenderer::with_cursor_image(image);
        let mut frame = vec![0; 8 * 8];

        renderer.compose(&mut frame, 8, 8, &[], DesktopVisualState::with_cursor(4, 4));

        assert_eq!(frame[3 * 8 + 3], 0xff00_0000);
        assert_eq!(frame[3 * 8 + 4], 0xffff_0000);
        assert_eq!(frame[4 * 8 + 3], 0xff00_ff00);
        assert_ne!(frame[4 * 8 + 4], 0xff00_0000);
    }

    #[test]
    fn software_damage_uses_hotspot_adjusted_bounds() {
        let image =
            CompositorCursorImage::from_argb8888(vec![0xffff_ffff; 4 * 3], 4, 3, 2, 1).unwrap();

        assert_eq!(
            cursor_damage_rect(10, 10, 1280, 800, &image),
            Some(SurfaceDamageRect {
                x: 8,
                y: 9,
                width: 4,
                height: 3,
            })
        );
    }

    #[test]
    fn compose_output_draws_surface_pixels() {
        let surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 0,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(
                2,
                2,
                vec![0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff],
            ),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let mut frame = vec![0; 96 * 96];

        compose_output(
            &mut frame,
            96,
            96,
            &[surface],
            DesktopVisualState::wallpaper_only(),
        );

        let origin = (72 * 96 + 72) as usize;
        assert_eq!(frame[origin], 0xffff_0000);
        assert_eq!(frame[origin + 1], 0xff00_ff00);
        assert_eq!(frame[origin + 96], 0xff00_00ff);
        assert_eq!(frame[origin + 97], 0xffff_ffff);
    }

    #[test]
    fn scaled_client_surfaces_are_drawn_in_physical_output_space() {
        let surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 0,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(
                2,
                2,
                vec![0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff],
            ),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let background = 0xff00_0000;
        let mut frame = vec![background; 160 * 160];

        draw_client_surfaces_scaled(&mut frame, 160, 160, &[surface], 1.5);

        let scaled_origin = (108 * 160 + 108) as usize;
        assert_eq!(frame[scaled_origin], 0xffff_0000);
        assert_eq!(frame[(72 * 160 + 72) as usize], background);
    }

    #[test]
    fn compose_output_keeps_server_frame_hidden() {
        let surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 12,
            height: 8,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 0,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(12, 8, vec![0xffff_ffff; 12 * 8]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let mut frame = vec![0; 120 * 120];

        compose_output(
            &mut frame,
            120,
            120,
            &[surface],
            DesktopVisualState::wallpaper_only(),
        );

        let titlebar_pixel = ((72 - 12) * 120 + 76) as usize;
        assert_eq!(frame[titlebar_pixel], OUTPUT_BACKGROUND);
    }

    #[test]
    fn compose_output_preserves_scene_under_transparent_surface_pixels() {
        let transparent_surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 0,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(1, 1, vec![0x0000_0000]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let mut with_surface = vec![0; 96 * 96];

        compose_output(
            &mut with_surface,
            96,
            96,
            &[transparent_surface],
            DesktopVisualState::wallpaper_only(),
        );

        let origin = (72 * 96 + 72) as usize;
        assert_eq!(with_surface[origin], OUTPUT_BACKGROUND);
    }

    #[test]
    fn compose_output_blends_premultiplied_alpha_surface_pixels() {
        let half_red_premultiplied = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 0,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(1, 1, vec![0x8080_0000]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let blue_background = 0xff00_00ff;
        let mut frame = vec![blue_background; 96 * 96];

        draw_client_surfaces(
            &mut frame,
            96,
            96,
            std::slice::from_ref(&half_red_premultiplied),
        );

        let origin = (72 * 96 + 72) as usize;
        assert_eq!(frame[origin], 0xff80_007f);
    }

    #[test]
    fn decoration_raster_blend_preserves_premultiplied_alpha_composition() {
        let destination = 0xff20_4060;
        let source = [0x80, 0x00, 0x00, 0x80];

        assert_eq!(blend_premultiplied_rgba(destination, &source), 0xff8f_1f2f);
    }

    #[test]
    fn desktop_scene_renderer_draws_client_cursor_last_without_motion_trails() {
        let cursor_surface = RenderableSurface {
            surface_id: 99,
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(2, 2, vec![0xff00_ff00; 4]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let mut renderer = DesktopSceneRenderer::default();
        let mut frame = vec![0; 16 * 16];

        renderer.compose_reusing_frame(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 16,
            frame_height: 16,
            output_scale: 1.0,
            surfaces: &[],
            external_overlay_surface_ids: Vec::new(),
            content_generation: 1,
            visual_state: DesktopVisualState::with_cursor(0, 0),
            client_cursor: Some(crate::compositor::ClientCursorRenderState {
                surface: &cursor_surface,
                logical_x: 2,
                logical_y: 3,
                hotspot_x: 0,
                hotspot_y: 0,
            }),
        });
        assert_ne!(frame[0], CURSOR_OUTLINE);
        assert_eq!(frame[3 * 16 + 2], 0xff00_ff00);

        renderer.compose_reusing_frame(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 16,
            frame_height: 16,
            output_scale: 1.0,
            surfaces: &[],
            external_overlay_surface_ids: Vec::new(),
            content_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            client_cursor: Some(crate::compositor::ClientCursorRenderState {
                surface: &cursor_surface,
                logical_x: 8,
                logical_y: 9,
                hotspot_x: 0,
                hotspot_y: 0,
            }),
        });

        assert_ne!(frame[3 * 16 + 2], 0xff00_ff00);
        assert_eq!(frame[9 * 16 + 8], 0xff00_ff00);
        assert_eq!(renderer.last_rebuild_kind(), DesktopSceneRebuildKind::None);

        renderer.compose_reusing_frame(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 16,
            frame_height: 16,
            output_scale: 1.0,
            surfaces: &[],
            external_overlay_surface_ids: Vec::new(),
            content_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            client_cursor: None,
        });
        assert_ne!(frame[9 * 16 + 8], 0xff00_ff00);
    }

    #[test]
    fn opaque_source_rows_are_detected_for_fast_blits() {
        assert!(source_row_is_opaque(&[0xffff_0000, 0xff00_ff00]));
        assert!(!source_row_is_opaque(&[0xffff_0000, 0x80ff_0000]));
    }

    #[test]
    fn surface_local_point_subtracts_visual_surface_origin() {
        let surface = RenderableSurface {
            surface_id: 7,
            x: 4,
            y: 6,
            width: 100,
            height: 80,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 0,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(100, 80, vec![0; 100 * 80]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        assert_eq!(
            surface_local_point_at_origin(
                &surface,
                surface_origin(0, &surface),
                72.0 + 4.0 + 32.0,
                72.0 + 6.0 + 10.0
            ),
            Some((32.0, 10.0))
        );
    }

    #[test]
    fn subsurface_origin_uses_parent_origin_without_surface_cascade() {
        let parent = RenderableSurface {
            surface_id: 1,
            x: 0,
            y: 0,
            width: 100,
            height: 80,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 0,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(100, 80, vec![0; 100 * 80]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let child = RenderableSurface {
            surface_id: 2,
            x: 0,
            y: 0,
            width: 20,
            height: 10,
            placement: SurfacePlacement::subsurface(1, 10, 12),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 0,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(20, 10, vec![0; 20 * 10]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        let origins = surface_origins(&[parent, child]);

        assert_eq!(origins, vec![(72, 72), (82, 84)]);
    }

    #[test]
    fn surface_origins_fast_path_keeps_root_cascade_and_placements() {
        let first = RenderableSurface {
            surface_id: 1,
            x: 3,
            y: 4,
            width: 100,
            height: 80,
            placement: SurfacePlacement::root_at(5, 6),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 0,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(100, 80, vec![0; 100 * 80]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let second = RenderableSurface {
            surface_id: 2,
            x: 7,
            y: 8,
            width: 20,
            height: 10,
            placement: SurfacePlacement::root_at(9, 10),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 0,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: shm_buffer(20, 10, vec![0; 20 * 10]),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        let origins = surface_origins(&[first, second]);

        assert_eq!(origins, vec![(80, 82), (120, 122)]);
    }
}
