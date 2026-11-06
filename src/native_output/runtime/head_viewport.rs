//! Per-head scene projection.
//!
//! The compositor resolves one scene in global logical layout coordinates.
//! Each output head shows the part of that layout its logical rectangle
//! covers: the frame resolution projects the scene into head-local
//! coordinates (translate by the head origin) and culls windows the head does
//! not show. Culling matters beyond cost: presentation feedback and surface
//! damage are taken by the surfaces a head renders, so a head must not sample
//! windows that live on another head.
//!
//! With a single logical output there is no viewport and the scene is
//! untouched. The native head at the layout origin still culls, but its
//! translation is the identity.

use std::{borrow::Cow, collections::HashMap, collections::HashSet};

use oblivion_one::compositor::{
    EffectAnchor, PresentationSceneSample, PresentedWindowGeometry, RenderableSurface,
    ResolvedEffectInstance,
};
use oblivion_one::effects::{EffectRect, EffectRegion};
use oblivion_one::presentation_animation::{
    PresentationClip, PresentationClipRect, PresentationRect,
};

/// Logical margin kept around a head when culling, so window shadows and
/// decorations that reach past their surfaces do not pop at head edges.
const CULL_MARGIN: i64 = 96;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NativeHeadViewport {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl NativeHeadViewport {
    pub(crate) fn from_logical_rect((x, y, width, height): (i32, i32, i32, i32)) -> Option<Self> {
        Some(Self {
            x,
            y,
            width: u32::try_from(width).ok().filter(|width| *width > 0)?,
            height: u32::try_from(height).ok().filter(|height| *height > 0)?,
        })
    }

    /// Whether projection moves content; the head at the layout origin only
    /// culls.
    pub(crate) fn is_translated(self) -> bool {
        self.x != 0 || self.y != 0
    }

    pub(crate) fn translation(self) -> (i32, i32) {
        (self.x.saturating_neg(), self.y.saturating_neg())
    }

    pub(crate) fn dimensions(self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Whether a head-local rectangle overlaps the head, margin included.
    fn shows_local(self, x: i64, y: i64, width: i64, height: i64) -> bool {
        width > 0
            && height > 0
            && x < i64::from(self.width) + CULL_MARGIN
            && x + width > -CULL_MARGIN
            && y < i64::from(self.height) + CULL_MARGIN
            && y + height > -CULL_MARGIN
    }

    /// Whether a global logical rectangle overlaps the head, margin included.
    fn shows_global(self, rect: PresentationRect) -> bool {
        self.shows_local(
            rect.x().floor() as i64 - i64::from(self.x),
            rect.y().floor() as i64 - i64::from(self.y),
            rect.width().ceil() as i64,
            rect.height().ceil() as i64,
        )
    }

    fn project_rect(self, rect: PresentationRect) -> PresentationRect {
        PresentationRect::new(
            rect.x() - f64::from(self.x),
            rect.y() - f64::from(self.y),
            rect.width(),
            rect.height(),
        )
        .unwrap_or(rect)
    }

    fn project_clip_rect(self, rect: PresentationClipRect) -> PresentationClipRect {
        PresentationClipRect::new(
            rect.x() - f64::from(self.x),
            rect.y() - f64::from(self.y),
            rect.width(),
            rect.height(),
        )
        .unwrap_or(rect)
    }

    /// Moves window samples, transforms, and clips into head-local space.
    pub(crate) fn project_presentation(self, sample: &mut PresentationSceneSample) {
        if !self.is_translated() {
            return;
        }
        for window in &mut sample.windows {
            window.rect = self.project_rect(window.rect);
        }
        for transform in &mut sample.transforms {
            transform.canonical_rect = self.project_rect(transform.canonical_rect);
            transform.presented_rect = self.project_rect(transform.presented_rect);
        }
        for clip in &mut sample.clips {
            if let PresentationClip::Rect(rect) = clip.clip {
                clip.clip = PresentationClip::Rect(self.project_clip_rect(rect));
            }
            clip.presented_clip = clip.presented_clip.map(|rect| self.project_clip_rect(rect));
        }
    }

    pub(crate) fn project_presented_windows(
        self,
        windows: Vec<PresentedWindowGeometry>,
    ) -> Vec<PresentedWindowGeometry> {
        if !self.is_translated() {
            return windows;
        }
        windows
            .into_iter()
            .map(|window| {
                PresentedWindowGeometry::with_scene_node(
                    window.scene_node_id(),
                    window.root_surface_id(),
                    self.project_rect(window.presented_rect()),
                )
            })
            .collect()
    }

    /// Projects an effect into head-local space; `None` when the head does
    /// not show it or its anchor surface was culled.
    pub(crate) fn project_effect(
        self,
        mut instance: ResolvedEffectInstance,
        culled_surface_ids: &HashSet<u32>,
    ) -> Option<ResolvedEffectInstance> {
        let anchor_surface_id = match instance.anchor {
            EffectAnchor::BeforeSurface(surface_id)
            | EffectAnchor::ReplaceSurface(surface_id)
            | EffectAnchor::AfterSurface(surface_id) => Some(surface_id),
            EffectAnchor::OutputPostProcess => None,
        };
        if anchor_surface_id.is_some_and(|surface_id| culled_surface_ids.contains(&surface_id)) {
            return None;
        }
        if self.is_translated() {
            let (dx, dy) = self.translation();
            instance.target_bounds = translate_effect_rect(instance.target_bounds, dx, dy)?;
            instance.region = translate_effect_region(&instance.region, dx, dy);
            instance.coverage = instance
                .coverage
                .as_ref()
                .and_then(|coverage| coverage.translated(f64::from(dx), f64::from(dy)));
            instance.signature = instance.signature.wrapping_mul(0x0000_0100_0000_01b3)
                ^ ((u64::from(self.x as u32) << 32) | u64::from(self.y as u32));
        }
        if anchor_surface_id.is_some() {
            let head = EffectRect::new(0, 0, self.width, self.height)?;
            instance.target_bounds.intersect(head)?;
        }
        Some(instance)
    }

    /// Presentation owner roots this head does not show. `surfaces` are
    /// already head-local and aligned with `owner_roots`; transforms in
    /// `presentation` are still global.
    pub(crate) fn culled_owner_roots(
        self,
        surfaces: &[RenderableSurface],
        owner_roots: &[u32],
        presentation: &PresentationSceneSample,
    ) -> HashSet<u32> {
        let origins = oblivion_one::compositor::surface_origins(surfaces);
        let mut shown = HashSet::new();
        let mut owners = HashSet::new();
        for ((surface, &owner_root), (origin_x, origin_y)) in
            surfaces.iter().zip(owner_roots).zip(origins)
        {
            owners.insert(owner_root);
            if shown.contains(&owner_root) {
                continue;
            }
            let (width, height) =
                surface
                    .render_target_size
                    .map_or((surface.width, surface.height), |size| {
                        (
                            size.width.max(surface.width),
                            size.height.max(surface.height),
                        )
                    });
            if self.shows_local(
                i64::from(origin_x),
                i64::from(origin_y),
                i64::from(width),
                i64::from(height),
            ) {
                shown.insert(owner_root);
            }
        }
        for transform in &presentation.transforms {
            if self.shows_global(transform.presented_rect)
                || self.shows_global(transform.canonical_rect)
            {
                shown.insert(transform.root_surface_id);
            }
        }
        owners.retain(|owner_root| !shown.contains(owner_root));
        owners
    }
}

/// Moves every surface by `(dx, dy)`: roots become absolute at their moved
/// origins, children stay relative to their parents.
pub(crate) fn translate_frame_surfaces<'a>(
    surfaces: Cow<'a, [RenderableSurface]>,
    (dx, dy): (i32, i32),
) -> Cow<'a, [RenderableSurface]> {
    if (dx, dy) == (0, 0) || surfaces.is_empty() {
        return surfaces;
    }
    let ids = surfaces
        .iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    let translations = ids
        .iter()
        .map(|&surface_id| (surface_id, (f64::from(dx), f64::from(dy))))
        .collect::<HashMap<_, _>>();
    super::frame::translate_workspace_frame_surfaces(surfaces, &ids, &translations)
}

/// Keeps the entries of an aligned slice whose `keep` flag is set.
pub(crate) fn retain_aligned<'a, T: Clone>(items: Cow<'a, [T]>, keep: &[bool]) -> Cow<'a, [T]> {
    assert_eq!(items.len(), keep.len(), "culled slices must stay aligned");
    if keep.iter().all(|keep| *keep) {
        return items;
    }
    Cow::Owned(
        items
            .iter()
            .zip(keep)
            .filter_map(|(item, keep)| keep.then(|| item.clone()))
            .collect(),
    )
}

fn translate_effect_rect(rect: EffectRect, dx: i32, dy: i32) -> Option<EffectRect> {
    let x = i32::try_from(i64::from(rect.x) + i64::from(dx)).ok()?;
    let y = i32::try_from(i64::from(rect.y) + i64::from(dy)).ok()?;
    EffectRect::new(x, y, rect.width, rect.height)
}

fn translate_effect_region(region: &EffectRegion, dx: i32, dy: i32) -> EffectRegion {
    // A conservative full region has no rectangles and stays full.
    if region.rects().is_empty() {
        return region.clone();
    }
    let mut translated = EffectRegion::empty();
    for rect in region.rects() {
        if let Some(rect) = translate_effect_rect(*rect, dx, dy) {
            translated.push(rect);
        }
    }
    translated
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
        PresentationRect::new(x, y, width, height).expect("finite rect")
    }

    #[test]
    fn origin_head_only_culls() {
        let head = NativeHeadViewport::from_logical_rect((0, 0, 1920, 1080)).unwrap();
        assert!(!head.is_translated());
        assert_eq!(head.translation(), (0, 0));
    }

    #[test]
    fn rejects_empty_heads() {
        assert!(NativeHeadViewport::from_logical_rect((0, 0, 0, 1080)).is_none());
        assert!(NativeHeadViewport::from_logical_rect((0, 0, 1920, -1)).is_none());
    }

    #[test]
    fn shows_global_rects_on_the_head_only() {
        let tv = NativeHeadViewport::from_logical_rect((1920, 0, 3840, 2160)).unwrap();
        assert!(tv.shows_global(rect(2000.0, 100.0, 800.0, 600.0)));
        assert!(!tv.shows_global(rect(100.0, 100.0, 800.0, 600.0)));
        // A window spanning the boundary shows on both heads.
        assert!(tv.shows_global(rect(1500.0, 100.0, 800.0, 600.0)));
        // Within the margin of the edge still counts.
        assert!(tv.shows_global(rect(1000.0, 100.0, 900.0, 600.0)));
        assert!(!tv.shows_global(rect(1000.0, 100.0, 800.0, 600.0)));
    }

    #[test]
    fn projects_rects_into_head_space() {
        let tv = NativeHeadViewport::from_logical_rect((1920, 0, 3840, 2160)).unwrap();
        let projected = tv.project_rect(rect(2000.0, 50.0, 10.0, 20.0));
        assert_eq!(
            (
                projected.x(),
                projected.y(),
                projected.width(),
                projected.height()
            ),
            (80.0, 50.0, 10.0, 20.0)
        );
    }

    #[test]
    fn translates_effect_regions_and_keeps_full_regions() {
        let region = EffectRegion::from_rect(EffectRect::new(2000, 10, 100, 50).unwrap());
        let translated = translate_effect_region(&region, -1920, 0);
        assert_eq!(
            translated.rects(),
            &[EffectRect::new(80, 10, 100, 50).unwrap()]
        );
        let empty = EffectRegion::empty();
        assert_eq!(translate_effect_region(&empty, -1920, 0), empty);
    }

    #[test]
    fn retain_aligned_borrows_when_nothing_is_culled() {
        let items = [1, 2, 3];
        assert!(matches!(
            retain_aligned(Cow::Borrowed(&items[..]), &[true, true, true]),
            Cow::Borrowed(_)
        ));
        assert_eq!(
            retain_aligned(Cow::Borrowed(&items[..]), &[true, false, true]).as_ref(),
            &[1, 3]
        );
    }
}
