use std::collections::HashMap;

use super::shell::{ShellOverlayImage, blend_shell_overlay_argb};
use super::{RenderableSurface, RenderableSurfaceDamage, SurfaceDamageRect};

pub const NESTED_OUTPUT_BACKGROUND: u32 = 0xff08_0a0e;
pub const CURSOR_FILL: u32 = 0xffff_ffff;
pub const CURSOR_OUTLINE: u32 = 0xff10_1116;
pub const FIRST_SURFACE_OFFSET: (i32, i32) = (72, 72);
pub const SURFACE_CASCADE_STEP: i32 = 32;
pub const SERVER_FRAME_BORDER_THICKNESS: i32 = 6;
pub const SERVER_FRAME_BORDER_COLOR: u32 = 0xff0a_0d12;
pub const SERVER_FRAME_TITLEBAR_COLOR: u32 = 0xff1a_2029;
pub const SERVER_FRAME_SEPARATOR_COLOR: u32 = 0xff2e_3644;
pub const OUTPUT_SCALE_DENOMINATOR: u32 = 120;

const WALLPAPER_TOP_LEFT: Rgb = Rgb::new(18, 21, 28);
const WALLPAPER_TOP_RIGHT: Rgb = Rgb::new(20, 58, 54);
const WALLPAPER_BOTTOM_LEFT: Rgb = Rgb::new(58, 34, 49);
const WALLPAPER_BOTTOM_RIGHT: Rgb = Rgb::new(68, 51, 28);

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
    pub content_generation: u64,
    pub visual_state: DesktopVisualState,
    pub shell_overlay: Option<&'a ShellOverlayImage>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SceneSurfaceSnapshot {
    surface_id: u32,
    generation: u64,
    target: SurfaceTargetRect,
    buffer_width: u32,
    buffer_height: u32,
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

#[derive(Debug, Default)]
pub struct DesktopSceneRenderer {
    wallpaper: Vec<u32>,
    wallpaper_width: u32,
    wallpaper_height: u32,
    wallpaper_generation: u64,
    scene: Vec<u32>,
    scene_width: u32,
    scene_height: u32,
    scene_output_scale_key: u32,
    scene_content_generation: u64,
    scene_generation: u64,
    scene_surface_snapshots: Vec<SceneSurfaceSnapshot>,
    last_rebuild_damage_rects: Vec<OutputRect>,
    last_rebuild_kind: DesktopSceneRebuildKind,
    last_frame_copy_kind: DesktopFrameCopyKind,
    reusable_frame_key: Option<ReusableFrameKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReusableFrameKey {
    width: u32,
    height: u32,
    output_scale_key: u32,
    shell_overlay_generation: Option<u64>,
    visual_state: DesktopVisualState,
}

impl DesktopSceneRenderer {
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
        );
        self.copy_scene_to_frame(frame, frame_width, frame_height);
        if let Some((cursor_x, cursor_y)) = visual_state.cursor {
            draw_cursor(frame, frame_width, frame_height, cursor_x, cursor_y);
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
        self.rebuild_scene(frame_width, frame_height, surfaces, content_generation, 1.0);
        self.copy_scene_to_frame(frame, frame_width, frame_height);
        if let Some((cursor_x, cursor_y)) = visual_state.cursor {
            draw_cursor(frame, frame_width, frame_height, cursor_x, cursor_y);
        }
    }

    pub fn compose_request(&mut self, request: DesktopComposeRequest<'_>) {
        self.compose_request_internal(request, false);
    }

    pub fn compose_reusing_frame(&mut self, request: DesktopComposeRequest<'_>) {
        self.compose_request_internal(request, true);
    }

    fn compose_request_internal(&mut self, request: DesktopComposeRequest<'_>, reuse_frame: bool) {
        let DesktopComposeRequest {
            frame,
            frame_width,
            frame_height,
            output_scale,
            surfaces,
            content_generation,
            visual_state,
            shell_overlay,
        } = request;

        self.rebuild_scene(
            frame_width,
            frame_height,
            surfaces,
            content_generation,
            output_scale,
        );
        let output_scale_key = output_scale_key(output_scale);
        let shell_overlay_generation = shell_overlay.map(|overlay| overlay.generation);
        let scaled_visual_state = scale_desktop_visual_state(visual_state, output_scale);
        let frame_key = ReusableFrameKey {
            width: frame_width,
            height: frame_height,
            output_scale_key,
            shell_overlay_generation,
            visual_state: scaled_visual_state,
        };
        let partial_frame_copy = reuse_frame
            && self.reusable_frame_key == Some(frame_key)
            && scaled_visual_state.cursor.is_none()
            && self.last_rebuild_kind == DesktopSceneRebuildKind::Partial
            && !self.last_rebuild_damage_rects.is_empty()
            && frame.len() == self.scene.len();
        let no_frame_copy = reuse_frame
            && self.reusable_frame_key == Some(frame_key)
            && scaled_visual_state.cursor.is_none()
            && self.last_rebuild_kind == DesktopSceneRebuildKind::None
            && frame.len() == self.scene.len();
        if partial_frame_copy {
            self.copy_scene_damage_to_frame(frame, frame_width, frame_height);
        } else if no_frame_copy {
            self.last_frame_copy_kind = DesktopFrameCopyKind::None;
        } else {
            self.copy_scene_to_frame(frame, frame_width, frame_height);
        }
        if let Some(shell_overlay) = shell_overlay {
            if partial_frame_copy {
                blend_shell_overlay_in_rects(
                    frame,
                    frame_width,
                    frame_height,
                    shell_overlay,
                    &self.last_rebuild_damage_rects,
                );
            } else if !no_frame_copy {
                blend_shell_overlay(frame, frame_width, frame_height, shell_overlay);
            }
        }
        if let Some((cursor_x, cursor_y)) = scaled_visual_state.cursor {
            draw_cursor(frame, frame_width, frame_height, cursor_x, cursor_y);
        }
        self.reusable_frame_key = reuse_frame.then_some(frame_key);
    }

    pub fn scene_generation(&self) -> u64 {
        self.scene_generation
    }

    pub fn wallpaper_generation(&self) -> u64 {
        self.wallpaper_generation
    }

    pub fn last_rebuild_kind(&self) -> DesktopSceneRebuildKind {
        self.last_rebuild_kind
    }

    pub fn last_frame_copy_kind(&self) -> DesktopFrameCopyKind {
        self.last_frame_copy_kind
    }

    fn rebuild_scene(
        &mut self,
        frame_width: u32,
        frame_height: u32,
        surfaces: &[RenderableSurface],
        content_generation: u64,
        output_scale: f64,
    ) {
        self.ensure_wallpaper(frame_width, frame_height);
        let output_scale_key = output_scale_key(output_scale);

        let pixel_count = frame_width.saturating_mul(frame_height) as usize;
        let scene_ready = self.scene_width == frame_width
            && self.scene_height == frame_height
            && self.scene_output_scale_key == output_scale_key
            && self.scene.len() == pixel_count;
        if scene_ready && self.scene_content_generation == content_generation {
            self.last_rebuild_damage_rects.clear();
            self.last_rebuild_kind = DesktopSceneRebuildKind::None;
            return;
        }

        let snapshots = scene_surface_snapshots(surfaces, output_scale);
        if scene_ready
            && self.rebuild_scene_from_damage(
                frame_width,
                frame_height,
                surfaces,
                content_generation,
                output_scale,
                &snapshots,
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

    fn rebuild_scene_from_damage(
        &mut self,
        frame_width: u32,
        frame_height: u32,
        surfaces: &[RenderableSurface],
        content_generation: u64,
        output_scale: f64,
        snapshots: &[SceneSurfaceSnapshot],
    ) -> bool {
        let Some(damage_rects) = partial_scene_damage_rects(
            &self.scene_surface_snapshots,
            surfaces,
            snapshots,
            frame_width,
            frame_height,
        ) else {
            return false;
        };

        if damage_rects.is_empty() {
            self.scene_content_generation = content_generation;
            self.scene_surface_snapshots = snapshots.to_vec();
            self.last_rebuild_damage_rects.clear();
            self.last_rebuild_kind = DesktopSceneRebuildKind::None;
            return true;
        }

        for damage_rect in damage_rects.iter().copied() {
            copy_wallpaper_rect_to_scene(
                &mut self.scene,
                &self.wallpaper,
                frame_width,
                damage_rect,
            );
            draw_client_surfaces_scaled_with_snapshots(
                &mut self.scene,
                frame_width,
                frame_height,
                surfaces,
                snapshots,
                output_scale,
                Some(damage_rect),
            );
        }

        self.scene_content_generation = content_generation;
        self.scene_surface_snapshots = snapshots.to_vec();
        self.last_rebuild_damage_rects = damage_rects;
        self.scene_generation = self.scene_generation.saturating_add(1);
        self.last_rebuild_kind = DesktopSceneRebuildKind::Partial;
        true
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
        if self.scene.len() == self.wallpaper.len() {
            self.scene.copy_from_slice(&self.wallpaper);
        } else {
            self.scene.resize(pixel_count, NESTED_OUTPUT_BACKGROUND);
            draw_wallpaper(&mut self.scene, frame_width, frame_height);
        }

        draw_client_surfaces_scaled(
            &mut self.scene,
            frame_width,
            frame_height,
            surfaces,
            output_scale,
        );
        self.scene_surface_snapshots = snapshots;
        self.last_rebuild_damage_rects.clear();
        self.scene_generation = self.scene_generation.saturating_add(1);
        self.last_rebuild_kind = DesktopSceneRebuildKind::Full;
    }

    fn copy_scene_to_frame(&mut self, frame: &mut [u32], frame_width: u32, frame_height: u32) {
        if frame.len() == self.scene.len() {
            frame.copy_from_slice(&self.scene);
        } else {
            draw_wallpaper(frame, frame_width, frame_height);
        }
        self.last_frame_copy_kind = DesktopFrameCopyKind::Full;
    }

    fn copy_scene_damage_to_frame(
        &mut self,
        frame: &mut [u32],
        frame_width: u32,
        frame_height: u32,
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

    fn ensure_wallpaper(&mut self, frame_width: u32, frame_height: u32) {
        let pixel_count = frame_width.saturating_mul(frame_height) as usize;
        if self.wallpaper_width == frame_width
            && self.wallpaper_height == frame_height
            && self.wallpaper.len() == pixel_count
        {
            return;
        }

        self.wallpaper_width = frame_width;
        self.wallpaper_height = frame_height;
        self.wallpaper.resize(pixel_count, NESTED_OUTPUT_BACKGROUND);
        draw_wallpaper(&mut self.wallpaper, frame_width, frame_height);
        self.wallpaper_generation = self.wallpaper_generation.saturating_add(1);
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rgb {
    red: u8,
    green: u8,
    blue: u8,
}

impl Rgb {
    const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    fn to_pixel(self) -> u32 {
        0xff00_0000
            | (u32::from(self.red) << 16)
            | (u32::from(self.green) << 8)
            | u32::from(self.blue)
    }
}

pub fn compose_nested_output(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    surfaces: &[RenderableSurface],
    visual_state: DesktopVisualState,
) {
    draw_wallpaper(frame, frame_width, frame_height);
    draw_client_surfaces(frame, frame_width, frame_height, surfaces);

    if let Some((cursor_x, cursor_y)) = visual_state.cursor {
        draw_cursor(frame, frame_width, frame_height, cursor_x, cursor_y);
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
    output_scale: f64,
    clip: Option<OutputRect>,
) {
    for (surface, snapshot) in surfaces.iter().zip(snapshots) {
        for rect in server_frame_rects_for_surface(surface) {
            let rect = scale_server_frame_rect(*rect, output_scale);
            match clip {
                Some(clip) => fill_rect_clipped(frame, frame_width, frame_height, rect, clip),
                None => fill_rect(frame, frame_width, frame_height, rect),
            }
        }

        if clip.is_some_and(|clip| !snapshot.target.output_rect().intersects(clip)) {
            continue;
        }
        blit_surface_to_rect_clipped(
            frame,
            frame_width,
            frame_height,
            surface,
            snapshot.target,
            clip,
        );
    }
}

fn scene_surface_snapshots(
    surfaces: &[RenderableSurface],
    output_scale: f64,
) -> Vec<SceneSurfaceSnapshot> {
    let targets = surface_target_rects(surfaces, output_scale);
    surfaces
        .iter()
        .zip(targets)
        .map(|(surface, target)| {
            let buffer_size = surface.buffer_size();
            SceneSurfaceSnapshot {
                surface_id: surface.surface_id,
                generation: surface.generation,
                target,
                buffer_width: buffer_size.width,
                buffer_height: buffer_size.height,
            }
        })
        .collect()
}

fn surface_target_rects(
    surfaces: &[RenderableSurface],
    output_scale: f64,
) -> Vec<SurfaceTargetRect> {
    let output_scale = normalized_output_scale(output_scale);
    let origins = surface_origins(surfaces);
    surfaces
        .iter()
        .zip(origins)
        .map(|(surface, (origin_x, origin_y))| SurfaceTargetRect {
            x: scale_logical_coordinate(origin_x, output_scale),
            y: scale_logical_coordinate(origin_y, output_scale),
            width: scale_logical_extent(surface.width, output_scale),
            height: scale_logical_extent(surface.height, output_scale),
        })
        .collect()
}

fn partial_scene_damage_rects(
    previous_snapshots: &[SceneSurfaceSnapshot],
    surfaces: &[RenderableSurface],
    snapshots: &[SceneSurfaceSnapshot],
    frame_width: u32,
    frame_height: u32,
) -> Option<Vec<OutputRect>> {
    if previous_snapshots.len() != snapshots.len() || surfaces.len() != snapshots.len() {
        return None;
    }

    let mut damage_rects = Vec::new();
    for ((previous, surface), snapshot) in previous_snapshots
        .iter()
        .copied()
        .zip(surfaces)
        .zip(snapshots.iter().copied())
    {
        if previous.surface_id != snapshot.surface_id {
            return None;
        }

        if previous.target != snapshot.target {
            if let Some(rects) = resize_preview_target_damage_rects(
                surface,
                previous.target,
                snapshot.target,
                frame_width,
                frame_height,
            ) {
                damage_rects.extend(rects);
            } else {
                if let Some(rect) = previous
                    .target
                    .output_rect()
                    .clipped_to_output(frame_width, frame_height)
                {
                    damage_rects.push(rect);
                }
                if let Some(rect) = snapshot
                    .target
                    .output_rect()
                    .clipped_to_output(frame_width, frame_height)
                {
                    damage_rects.push(rect);
                }
            }
            continue;
        }

        if previous.buffer_width != snapshot.buffer_width
            || previous.buffer_height != snapshot.buffer_height
        {
            if let Some(rect) = snapshot
                .target
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

        match &surface.damage {
            RenderableSurfaceDamage::Full => {
                if let Some(rect) = snapshot
                    .target
                    .output_rect()
                    .clipped_to_output(frame_width, frame_height)
                {
                    damage_rects.push(rect);
                }
            }
            RenderableSurfaceDamage::Partial(_) => {
                let buffer_size = surface.buffer_size();
                for rect in surface
                    .damage
                    .clipped_rects(buffer_size.width, buffer_size.height)
                {
                    let Some(rect) = output_damage_rect_for_surface(surface, snapshot.target, rect)
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

fn resize_preview_target_damage_rects(
    surface: &RenderableSurface,
    previous: SurfaceTargetRect,
    current: SurfaceTargetRect,
    frame_width: u32,
    frame_height: u32,
) -> Option<Vec<OutputRect>> {
    let preview = surface.resize_preview?;
    if preview.anchor_right || preview.anchor_bottom {
        return None;
    }
    if previous.x != current.x || previous.y != current.y {
        return None;
    }

    let previous = previous.output_rect();
    let current = current.output_rect();
    let output = OutputRect::full(frame_width, frame_height);
    let rects = rect_symmetric_difference(previous, current)
        .into_iter()
        .filter_map(|rect| rect.intersection(output))
        .collect::<Vec<_>>();
    Some(rects)
}

fn rect_symmetric_difference(left: OutputRect, right: OutputRect) -> Vec<OutputRect> {
    let mut rects = rect_difference(left, right);
    rects.extend(rect_difference(right, left));
    rects
}

fn rect_difference(source: OutputRect, cut: OutputRect) -> Vec<OutputRect> {
    let Some(overlap) = source.intersection(cut) else {
        return vec![source];
    };

    let mut rects = Vec::new();
    push_output_rect(
        &mut rects,
        source.left(),
        source.top(),
        source.right(),
        overlap.top(),
    );
    push_output_rect(
        &mut rects,
        source.left(),
        overlap.bottom(),
        source.right(),
        source.bottom(),
    );
    push_output_rect(
        &mut rects,
        source.left(),
        overlap.top(),
        overlap.left(),
        overlap.bottom(),
    );
    push_output_rect(
        &mut rects,
        overlap.right(),
        overlap.top(),
        source.right(),
        overlap.bottom(),
    );
    rects
}

fn push_output_rect(rects: &mut Vec<OutputRect>, left: i64, top: i64, right: i64, bottom: i64) {
    if right <= left || bottom <= top {
        return;
    }
    rects.push(OutputRect {
        x: left.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        y: top.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        width: u32::try_from(right.saturating_sub(left)).unwrap_or(u32::MAX),
        height: u32::try_from(bottom.saturating_sub(top)).unwrap_or(u32::MAX),
    });
}

fn output_damage_rect_for_surface(
    surface: &RenderableSurface,
    target: SurfaceTargetRect,
    rect: SurfaceDamageRect,
) -> Option<OutputRect> {
    if target.width == 0 || target.height == 0 {
        return None;
    }

    let buffer_size = surface.buffer_size();
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
    Border,
    Titlebar,
    Separator,
}

impl ServerFrameColor {
    pub const ALL: [Self; 3] = [Self::Border, Self::Titlebar, Self::Separator];

    pub const fn pixel(self) -> u32 {
        match self {
            Self::Border => SERVER_FRAME_BORDER_COLOR,
            Self::Titlebar => SERVER_FRAME_TITLEBAR_COLOR,
            Self::Separator => SERVER_FRAME_SEPARATOR_COLOR,
        }
    }
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
struct SurfaceTargetRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl SurfaceTargetRect {
    const fn output_rect(self) -> OutputRect {
        OutputRect {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
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
        .map(|surface| server_frame_rects_for_surface(surface).to_vec())
        .collect()
}

pub fn server_frame_rects_for_surface(_surface: &RenderableSurface) -> &'static [ServerFrameRect] {
    &[]
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
    let origin = surface
        .placement
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
                parent_origin.0 + surface.placement.local_x + surface.x,
                parent_origin.1 + surface.placement.local_y + surface.y,
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
    let cascade = root_index as i32 * SURFACE_CASCADE_STEP;
    (
        FIRST_SURFACE_OFFSET.0 + cascade + surface.placement.local_x + surface.x,
        FIRST_SURFACE_OFFSET.1 + cascade + surface.placement.local_y + surface.y,
    )
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

pub fn draw_wallpaper(frame: &mut [u32], frame_width: u32, frame_height: u32) {
    if frame_width == 0 || frame_height == 0 {
        frame.fill(NESTED_OUTPUT_BACKGROUND);
        return;
    }

    for y in 0..frame_height {
        for x in 0..frame_width {
            let pixel_index = (y * frame_width + x) as usize;
            if let Some(pixel) = frame.get_mut(pixel_index) {
                *pixel = wallpaper_pixel(x, y, frame_width, frame_height);
            }
        }
    }
}

pub fn cursor_texture_size() -> (u32, u32) {
    let width = CURSOR_PATTERN
        .iter()
        .map(|line| line.len() as u32)
        .max()
        .unwrap_or(0);
    (width, CURSOR_PATTERN.len() as u32)
}

pub fn cursor_texture_pixels() -> Vec<u32> {
    let (width, height) = cursor_texture_size();
    let mut pixels = vec![0; width.saturating_mul(height) as usize];
    for (row, line) in CURSOR_PATTERN.iter().enumerate() {
        for (column, marker) in line.bytes().enumerate() {
            let color = match marker {
                b'X' => CURSOR_OUTLINE,
                b'O' => CURSOR_FILL,
                _ => continue,
            };
            let index = row * width as usize + column;
            if let Some(pixel) = pixels.get_mut(index) {
                *pixel = color;
            }
        }
    }

    pixels
}

fn wallpaper_pixel(x: u32, y: u32, width: u32, height: u32) -> u32 {
    let horizontal = gradient_step(x, width);
    let vertical = gradient_step(y, height);
    let top = mix_rgb(WALLPAPER_TOP_LEFT, WALLPAPER_TOP_RIGHT, horizontal);
    let bottom = mix_rgb(WALLPAPER_BOTTOM_LEFT, WALLPAPER_BOTTOM_RIGHT, horizontal);
    let base = mix_rgb(top, bottom, vertical);
    let diagonal = gradient_step(x.saturating_add(y), width.saturating_add(height).max(1));

    Rgb::new(
        base.red.saturating_add((diagonal / 12) as u8),
        base.green
            .saturating_add((u32::from(255u8.saturating_sub(diagonal as u8)) / 18) as u8),
        base.blue.saturating_add((vertical / 14) as u8),
    )
    .to_pixel()
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

fn gradient_step(position: u32, extent: u32) -> u32 {
    let last = extent.saturating_sub(1).max(1);
    position.min(last) * 255 / last
}

fn mix_rgb(start: Rgb, end: Rgb, step: u32) -> Rgb {
    Rgb::new(
        mix_channel(start.red, end.red, step),
        mix_channel(start.green, end.green, step),
        mix_channel(start.blue, end.blue, step),
    )
}

fn mix_channel(start: u8, end: u8, step: u32) -> u8 {
    let inverse = 255 - step;
    ((u32::from(start) * inverse + u32::from(end) * step) / 255) as u8
}

fn draw_cursor(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    cursor_x: i32,
    cursor_y: i32,
) {
    for (row, line) in CURSOR_PATTERN.iter().enumerate() {
        for (column, marker) in line.bytes().enumerate() {
            let color = match marker {
                b'X' => CURSOR_OUTLINE,
                b'O' => CURSOR_FILL,
                _ => continue,
            };

            let target_x = cursor_x + column as i32;
            let target_y = cursor_y + row as i32;
            if !(0..frame_width as i32).contains(&target_x)
                || !(0..frame_height as i32).contains(&target_y)
            {
                continue;
            }

            let pixel_index = (target_y as u32 * frame_width + target_x as u32) as usize;
            if let Some(pixel) = frame.get_mut(pixel_index) {
                *pixel = color;
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
    let Some(surface_pixels) = surface.cpu_pixels() else {
        return;
    };
    let target = resize_preview_content_target(surface, target);
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

    if buffer_size.width == target.width && buffer_size.height == target.height {
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
    for row_y in start_y..end_y {
        let local_y = row_y - i64::from(target.y);
        let source_y = ((local_y * i64::from(buffer_size.height)) / target_height)
            .clamp(0, i64::from(buffer_size.height.saturating_sub(1)))
            as usize;
        let target_start = row_y as usize * frame_width + start_x as usize;
        let Some(target_row) =
            frame.get_mut(target_start..target_start + (end_x - start_x) as usize)
        else {
            continue;
        };
        for (column, target_pixel) in target_row.iter_mut().enumerate() {
            let local_x = (start_x - i64::from(target.x)) + column as i64;
            let source_x = ((local_x * i64::from(buffer_size.width)) / target_width)
                .clamp(0, i64::from(buffer_size.width.saturating_sub(1)))
                as usize;
            let source_index = source_y * buffer_width + source_x;
            if let Some(source) = surface_pixels.get(source_index).copied() {
                *target_pixel = blend_premultiplied_argb_over_opaque(source, *target_pixel);
            }
        }
    }
}

fn resize_preview_content_target(
    surface: &RenderableSurface,
    target: SurfaceTargetRect,
) -> SurfaceTargetRect {
    let Some(preview) = surface.resize_preview else {
        return target;
    };
    let width = preview_content_extent(preview.committed_width, surface.width, target.width);
    let height = preview_content_extent(preview.committed_height, surface.height, target.height);
    SurfaceTargetRect {
        x: if preview.anchor_right && width < target.width {
            i32_saturating_add_u32(target.x, target.width - width)
        } else {
            target.x
        },
        y: if preview.anchor_bottom && height < target.height {
            i32_saturating_add_u32(target.y, target.height - height)
        } else {
            target.y
        },
        width,
        height,
    }
}

fn preview_content_extent(committed: u32, current: u32, target_extent: u32) -> u32 {
    if committed >= current || current == 0 {
        return target_extent;
    }
    let scaled = u64::from(committed).saturating_mul(u64::from(target_extent)) / u64::from(current);
    u32::try_from(scaled)
        .unwrap_or(u32::MAX)
        .clamp(1, target_extent)
}

fn copy_wallpaper_rect_to_scene(
    scene: &mut [u32],
    wallpaper: &[u32],
    frame_width: u32,
    rect: OutputRect,
) {
    if frame_width == 0 {
        return;
    }

    let frame_width = frame_width as usize;
    let left = rect.x.max(0) as usize;
    let top = rect.y.max(0) as usize;
    let row_width = rect.width as usize;
    for output_y in top..top.saturating_add(rect.height as usize) {
        let row_start = output_y.saturating_mul(frame_width).saturating_add(left);
        let row_end = row_start.saturating_add(row_width);
        let Some(wallpaper_row) = wallpaper.get(row_start..row_end) else {
            continue;
        };
        let Some(scene_row) = scene.get_mut(row_start..row_end) else {
            continue;
        };
        scene_row.copy_from_slice(wallpaper_row);
    }
}

fn scale_server_frame_rect(rect: ServerFrameRect, output_scale: f64) -> ServerFrameRect {
    ServerFrameRect {
        color: rect.color,
        x: scale_logical_coordinate(rect.x, output_scale),
        y: scale_logical_coordinate(rect.y, output_scale),
        width: scale_logical_extent(rect.width, output_scale),
        height: scale_logical_extent(rect.height, output_scale),
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

fn blend_shell_overlay(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    shell_overlay: &ShellOverlayImage,
) {
    blend_shell_overlay_with_clip(frame, frame_width, frame_height, shell_overlay, None);
}

fn blend_shell_overlay_in_rects(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    shell_overlay: &ShellOverlayImage,
    clip_rects: &[OutputRect],
) {
    if clip_rects.is_empty() {
        return;
    }
    blend_shell_overlay_with_clip(
        frame,
        frame_width,
        frame_height,
        shell_overlay,
        Some(clip_rects),
    );
}

fn blend_shell_overlay_with_clip(
    frame: &mut [u32],
    frame_width: u32,
    frame_height: u32,
    shell_overlay: &ShellOverlayImage,
    clip_rects: Option<&[OutputRect]>,
) {
    if shell_overlay.width == 0 || shell_overlay.height == 0 {
        return;
    }

    for region in shell_overlay.regions() {
        let bounds = region.output;
        let texture = region.texture;
        let region_rect = OutputRect {
            x: i32::try_from(bounds.x).unwrap_or(i32::MAX),
            y: i32::try_from(bounds.y).unwrap_or(i32::MAX),
            width: bounds.width.min(texture.width),
            height: bounds.height.min(texture.height),
        };
        let output_rect = OutputRect::full(frame_width, frame_height);
        let Some(region_rect) = region_rect.intersection(output_rect) else {
            continue;
        };
        if let Some(clip_rects) = clip_rects {
            for clip_rect in clip_rects {
                let Some(clipped_rect) = region_rect.intersection(*clip_rect) else {
                    continue;
                };
                blend_shell_overlay_region_rect(
                    frame,
                    frame_width,
                    shell_overlay,
                    bounds,
                    texture,
                    clipped_rect,
                );
            }
        } else {
            blend_shell_overlay_region_rect(
                frame,
                frame_width,
                shell_overlay,
                bounds,
                texture,
                region_rect,
            );
        }
    }
}

fn blend_shell_overlay_region_rect(
    frame: &mut [u32],
    frame_width: u32,
    shell_overlay: &ShellOverlayImage,
    bounds: super::shell::ShellOverlayBounds,
    texture: super::shell::ShellOverlayBounds,
    rect: OutputRect,
) {
    let left = rect.x.max(0) as u32;
    let top = rect.y.max(0) as u32;
    let right = rect
        .x
        .saturating_add(i32::try_from(rect.width).unwrap_or(i32::MAX))
        .max(0) as u32;
    let bottom = rect
        .y
        .saturating_add(i32::try_from(rect.height).unwrap_or(i32::MAX))
        .max(0) as u32;
    if left >= right || top >= bottom {
        return;
    }

    let row_width = (right - left) as usize;
    let source_width = shell_overlay.width as usize;
    let source_x = texture.x.saturating_add(left.saturating_sub(bounds.x)) as usize;
    let source_y = texture.y.saturating_add(top.saturating_sub(bounds.y)) as usize;
    for output_y in top..bottom {
        let source_row_index = source_y + output_y.saturating_sub(top) as usize;
        let source_start = source_row_index
            .saturating_mul(source_width)
            .saturating_add(source_x);
        let target_start = output_y as usize * frame_width as usize + left as usize;
        let Some(source_row) = shell_overlay
            .pixels
            .get(source_start..source_start + row_width)
        else {
            continue;
        };
        let Some(target_row) = frame.get_mut(target_start..target_start + row_width) else {
            continue;
        };
        for (source, target) in source_row.iter().copied().zip(target_row.iter_mut()) {
            if source >> 24 != 0 {
                *target = blend_shell_overlay_argb(source, *target);
            }
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

const CURSOR_PATTERN: [&str; 17] = [
    "X",
    "XX",
    "XOX",
    "XOOX",
    "XOOOX",
    "XOOOOX",
    "XOOOOOX",
    "XOOOOOOX",
    "XOOOOOOOX",
    "XOOOOOOOOX",
    "XOOOOXXXXX",
    "XOOXOOX",
    "XOX XOOX",
    "XX  XOOX",
    "X    XOOX",
    "     XOOX",
    "      XX",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::SurfacePlacement;
    use crate::render_backend::buffer::{BufferSize, CommittedSurfaceBuffer};

    fn shm_buffer(width: u32, height: u32, pixels: Vec<u32>) -> CommittedSurfaceBuffer {
        CommittedSurfaceBuffer::shm_snapshot(
            BufferSize::new(width, height).expect("test surfaces use non-zero sizes"),
            pixels,
        )
    }

    #[test]
    fn desktop_scene_renderer_reuses_wallpaper_cache_until_size_changes() {
        let mut renderer = DesktopSceneRenderer::default();
        let mut frame = vec![0; 16 * 12];

        renderer.compose(
            &mut frame,
            16,
            12,
            &[],
            DesktopVisualState::wallpaper_only(),
        );
        let first_generation = renderer.wallpaper_generation();

        renderer.compose(
            &mut frame,
            16,
            12,
            &[],
            DesktopVisualState::with_cursor(4, 4),
        );
        assert_eq!(renderer.wallpaper_generation(), first_generation);

        let mut resized = vec![0; 20 * 12];
        renderer.compose(
            &mut resized,
            20,
            12,
            &[],
            DesktopVisualState::wallpaper_only(),
        );
        assert_eq!(renderer.wallpaper_generation(), first_generation + 1);
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
            resize_preview: None,
            generation: 0,
            buffer: shm_buffer(
                2,
                2,
                vec![0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff],
            ),
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
    fn resize_preview_does_not_upscale_undersized_committed_buffer() {
        let surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 4,
            height: 2,
            placement: SurfacePlacement::root(),
            resize_preview: Some(crate::compositor::ResizePreview {
                committed_width: 2,
                committed_height: 2,
                anchor_right: false,
                anchor_bottom: false,
            }),
            generation: 1,
            buffer: shm_buffer(2, 2, vec![0xffff_0000; 4]),
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let mut frame = vec![0; 96 * 96];
        let mut wallpaper = vec![0; 96 * 96];
        compose_nested_output(
            &mut wallpaper,
            96,
            96,
            &[],
            DesktopVisualState::wallpaper_only(),
        );

        compose_nested_output(
            &mut frame,
            96,
            96,
            std::slice::from_ref(&surface),
            DesktopVisualState::wallpaper_only(),
        );

        let row = FIRST_SURFACE_OFFSET.1 as usize * 96 + FIRST_SURFACE_OFFSET.0 as usize;
        assert_eq!(frame[row], 0xffff_0000);
        assert_eq!(frame[row + 1], 0xffff_0000);
        assert_eq!(frame[row + 2], wallpaper[row + 2]);
        assert_eq!(frame[row + 3], wallpaper[row + 3]);
    }

    #[test]
    fn desktop_scene_renderer_repairs_old_and_new_bounds_when_surface_target_changes() {
        let mut renderer = DesktopSceneRenderer::default();
        let mut frame = vec![0; 96 * 96];
        let initial_surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 4,
            height: 4,
            placement: SurfacePlacement::root(),
            resize_preview: None,
            generation: 1,
            buffer: shm_buffer(4, 4, vec![0xffff_0000; 4 * 4]),
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        renderer.compose_with_generation(
            &mut frame,
            96,
            96,
            std::slice::from_ref(&initial_surface),
            1,
            DesktopVisualState::wallpaper_only(),
        );

        let resized_surface = RenderableSurface {
            width: 8,
            height: 4,
            generation: 2,
            resize_preview: Some(crate::compositor::ResizePreview {
                committed_width: 4,
                committed_height: 4,
                anchor_right: false,
                anchor_bottom: false,
            }),
            ..initial_surface
        };
        renderer.compose_with_generation(
            &mut frame,
            96,
            96,
            std::slice::from_ref(&resized_surface),
            2,
            DesktopVisualState::wallpaper_only(),
        );

        assert_eq!(
            renderer.last_rebuild_kind(),
            DesktopSceneRebuildKind::Partial
        );
    }

    #[test]
    fn desktop_scene_renderer_resize_growth_repairs_only_exposed_edges() {
        let mut renderer = DesktopSceneRenderer::default();
        let mut frame = vec![0; 96 * 96];
        let initial_surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 4,
            height: 4,
            placement: SurfacePlacement::root(),
            resize_preview: None,
            generation: 1,
            buffer: shm_buffer(4, 4, vec![0xffff_0000; 4 * 4]),
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        renderer.compose_reusing_frame(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 96,
            frame_height: 96,
            output_scale: 1.0,
            surfaces: std::slice::from_ref(&initial_surface),
            content_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            shell_overlay: None,
        });

        let resized_surface = RenderableSurface {
            width: 8,
            height: 6,
            generation: 2,
            resize_preview: Some(crate::compositor::ResizePreview {
                committed_width: 4,
                committed_height: 4,
                anchor_right: false,
                anchor_bottom: false,
            }),
            ..initial_surface
        };
        renderer.compose_reusing_frame(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 96,
            frame_height: 96,
            output_scale: 1.0,
            surfaces: std::slice::from_ref(&resized_surface),
            content_generation: 2,
            visual_state: DesktopVisualState::wallpaper_only(),
            shell_overlay: None,
        });

        assert_eq!(
            renderer.last_rebuild_kind(),
            DesktopSceneRebuildKind::Partial
        );
        assert_eq!(
            renderer.last_frame_copy_kind(),
            DesktopFrameCopyKind::Partial
        );
        assert_eq!(renderer.last_rebuild_damage_rects.len(), 2);
        assert_eq!(
            renderer.last_rebuild_damage_rects,
            vec![
                OutputRect {
                    x: FIRST_SURFACE_OFFSET.0,
                    y: FIRST_SURFACE_OFFSET.1 + 4,
                    width: 8,
                    height: 2,
                },
                OutputRect {
                    x: FIRST_SURFACE_OFFSET.0 + 4,
                    y: FIRST_SURFACE_OFFSET.1,
                    width: 4,
                    height: 4,
                },
            ]
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
            resize_preview: None,
            generation: 1,
            buffer: shm_buffer(4, 4, vec![0xffff_0000; 4 * 4]),
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
            resize_preview: None,
            generation: 2,
            buffer: shm_buffer(4, 4, updated_pixels),
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
            resize_preview: None,
            generation: 1,
            buffer: shm_buffer(4, 4, vec![0xffff_0000; 4 * 4]),
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        renderer.compose_reusing_frame(DesktopComposeRequest {
            frame: &mut frame,
            frame_width: 96,
            frame_height: 96,
            output_scale: 1.0,
            surfaces: &[initial_surface],
            content_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            shell_overlay: None,
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
            resize_preview: None,
            generation: 2,
            buffer: shm_buffer(4, 4, updated_pixels),
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
            content_generation: 2,
            visual_state: DesktopVisualState::wallpaper_only(),
            shell_overlay: None,
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
            resize_preview: None,
            generation: 1,
            buffer: shm_buffer(4, 4, vec![0xffff_0000; 4 * 4]),
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let top = RenderableSurface {
            surface_id: 8,
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            placement: SurfacePlacement::subsurface(7, 1, 1),
            resize_preview: None,
            generation: 1,
            buffer: shm_buffer(2, 2, vec![0xff00_ff00; 2 * 2]),
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
            resize_preview: None,
            generation: 2,
            buffer: shm_buffer(4, 4, updated_pixels),
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
            resize_preview: None,
            generation: 1,
            buffer: shm_buffer(4, 4, vec![0xffff_0000; 4 * 4]),
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
            resize_preview: None,
            generation: 2,
            buffer: shm_buffer(4, 4, vec![0xff00_00ff; 4 * 4]),
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

        let mut wallpaper = vec![0; 96 * 96];
        draw_wallpaper(&mut wallpaper, 96, 96);
        assert_eq!(frame[72 * 96 + 72], wallpaper[72 * 96 + 72]);
        assert_eq!(frame[72 * 96 + 74], 0xff00_00ff);
    }

    #[test]
    fn desktop_scene_renderer_places_cropped_shell_overlay_at_bounds() {
        use crate::compositor::shell::{ShellOverlayRenderer, ShellOverlayState, ShellTopbarModel};

        let mut overlay_renderer = ShellOverlayRenderer::default();
        let overlay = overlay_renderer
            .render(
                320,
                200,
                &ShellOverlayState {
                    topbar: ShellTopbarModel::visible("Oblivion One"),
                    dock_items: Vec::new(),
                    spotlight: Default::default(),
                    generation: 1,
                },
            )
            .clone();
        let bounds = overlay.content_bounds().expect("topbar should draw");
        let sample_x = bounds.x + bounds.width / 2;
        let sample_y = bounds.y + bounds.height / 2;
        let sample_index = sample_y as usize * 320 + sample_x as usize;

        let mut base_frame = vec![0; 320 * 200];
        let mut overlay_frame = vec![0; 320 * 200];
        let mut renderer = DesktopSceneRenderer::default();
        renderer.compose_request(DesktopComposeRequest {
            frame: &mut base_frame,
            frame_width: 320,
            frame_height: 200,
            output_scale: 1.0,
            surfaces: &[],
            content_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            shell_overlay: None,
        });
        renderer.compose_request(DesktopComposeRequest {
            frame: &mut overlay_frame,
            frame_width: 320,
            frame_height: 200,
            output_scale: 1.0,
            surfaces: &[],
            content_generation: 1,
            visual_state: DesktopVisualState::wallpaper_only(),
            shell_overlay: Some(&overlay),
        });

        assert_ne!(overlay_frame[sample_index], base_frame[sample_index]);
    }

    #[test]
    fn compose_nested_output_draws_desktop_wallpaper_when_empty() {
        let mut frame = vec![0; 12 * 8];

        compose_nested_output(&mut frame, 12, 8, &[], DesktopVisualState::wallpaper_only());

        assert_eq!(frame[0] >> 24, 0xff);
        assert_ne!(frame[0], frame[11]);
        assert_ne!(frame[0], frame[7 * 12]);
    }

    #[test]
    fn compose_nested_output_draws_cursor_over_scene() {
        let mut frame = vec![0; 48 * 48];

        compose_nested_output(
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
    fn compose_nested_output_draws_surface_pixels() {
        let surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            placement: SurfacePlacement::root(),
            resize_preview: None,
            generation: 0,
            buffer: shm_buffer(
                2,
                2,
                vec![0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff],
            ),
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let mut frame = vec![0; 96 * 96];

        compose_nested_output(
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
            resize_preview: None,
            generation: 0,
            buffer: shm_buffer(
                2,
                2,
                vec![0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff],
            ),
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
    fn compose_nested_output_keeps_server_frame_hidden() {
        let surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 12,
            height: 8,
            placement: SurfacePlacement::root(),
            resize_preview: None,
            generation: 0,
            buffer: shm_buffer(12, 8, vec![0xffff_ffff; 12 * 8]),
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let mut frame = vec![0; 120 * 120];

        compose_nested_output(
            &mut frame,
            120,
            120,
            &[surface],
            DesktopVisualState::wallpaper_only(),
        );

        let titlebar_pixel = ((72 - 12) * 120 + 76) as usize;
        let mut wallpaper = vec![0; 120 * 120];
        draw_wallpaper(&mut wallpaper, 120, 120);
        assert_eq!(frame[titlebar_pixel], wallpaper[titlebar_pixel]);
    }

    #[test]
    fn compose_nested_output_preserves_scene_under_transparent_surface_pixels() {
        let transparent_surface = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            placement: SurfacePlacement::root(),
            resize_preview: None,
            generation: 0,
            buffer: shm_buffer(1, 1, vec![0x0000_0000]),
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let mut wallpaper = vec![0; 96 * 96];
        let mut with_surface = vec![0; 96 * 96];

        compose_nested_output(
            &mut wallpaper,
            96,
            96,
            &[],
            DesktopVisualState::wallpaper_only(),
        );
        compose_nested_output(
            &mut with_surface,
            96,
            96,
            &[transparent_surface],
            DesktopVisualState::wallpaper_only(),
        );

        let origin = (72 * 96 + 72) as usize;
        assert_eq!(with_surface[origin], wallpaper[origin]);
    }

    #[test]
    fn compose_nested_output_blends_premultiplied_alpha_surface_pixels() {
        let half_red_premultiplied = RenderableSurface {
            surface_id: 7,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            placement: SurfacePlacement::root(),
            resize_preview: None,
            generation: 0,
            buffer: shm_buffer(1, 1, vec![0x8080_0000]),
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
            resize_preview: None,
            generation: 0,
            buffer: shm_buffer(100, 80, vec![0; 100 * 80]),
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
            resize_preview: None,
            generation: 0,
            buffer: shm_buffer(100, 80, vec![0; 100 * 80]),
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let child = RenderableSurface {
            surface_id: 2,
            x: 0,
            y: 0,
            width: 20,
            height: 10,
            placement: SurfacePlacement::subsurface(1, 10, 12),
            resize_preview: None,
            generation: 0,
            buffer: shm_buffer(20, 10, vec![0; 20 * 10]),
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
            resize_preview: None,
            generation: 0,
            buffer: shm_buffer(100, 80, vec![0; 100 * 80]),
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };
        let second = RenderableSurface {
            surface_id: 2,
            x: 7,
            y: 8,
            width: 20,
            height: 10,
            placement: SurfacePlacement::root_at(9, 10),
            resize_preview: None,
            generation: 0,
            buffer: shm_buffer(20, 10, vec![0; 20 * 10]),
            damage: crate::compositor::RenderableSurfaceDamage::full(),
        };

        let origins = surface_origins(&[first, second]);

        assert_eq!(origins, vec![(80, 82), (120, 122)]);
    }
}
