use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeDamageSummary {
    pub(crate) kind: NativeDamageKind,
    pub(crate) rects: usize,
    pub(crate) pixels: u64,
}

impl NativeDamageSummary {
    pub(crate) fn fields(self) -> [NativePerfField; 3] {
        [
            NativePerfField::str("damage_kind", self.kind.as_str()),
            NativePerfField::usize("damage_rects", self.rects),
            NativePerfField::u64("damaged_pixels", self.pixels),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeOutputDamage {
    pub(crate) kind: NativeDamageKind,
    pub(crate) rects: Vec<NativeDamageRect>,
    pub(crate) pixels: u64,
}

impl NativeOutputDamage {
    pub(crate) const fn empty() -> Self {
        Self {
            kind: NativeDamageKind::Empty,
            rects: Vec::new(),
            pixels: 0,
        }
    }

    pub(crate) fn full_output(width: u32, height: u32) -> Self {
        let pixels = u64::from(width).saturating_mul(u64::from(height));
        Self {
            kind: NativeDamageKind::FullOutput,
            rects: if width > 0 && height > 0 {
                vec![NativeDamageRect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                }]
            } else {
                Vec::new()
            },
            pixels,
        }
    }

    pub(crate) fn surface_damage(rects: Vec<NativeDamageRect>) -> Self {
        let rects = coalesce_native_damage_rects(rects);
        if rects.is_empty() {
            return Self::empty();
        }
        let pixels = rects
            .iter()
            .fold(0u64, |pixels, rect| pixels.saturating_add(rect.pixels()));
        Self {
            kind: NativeDamageKind::SurfaceDamage,
            rects,
            pixels,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.kind == NativeDamageKind::Empty || self.rects.is_empty() || self.pixels == 0
    }

    pub(crate) fn summary(&self) -> NativeDamageSummary {
        NativeDamageSummary {
            kind: self.kind,
            rects: self.rects.len(),
            pixels: self.pixels,
        }
    }

    pub(crate) fn fields(&self) -> [NativePerfField; 3] {
        self.summary().fields()
    }

    pub(crate) fn as_renderer_damage(&self, width: u32, height: u32) -> OutputDamage {
        match self.kind {
            NativeDamageKind::Empty => OutputDamage::Empty,
            NativeDamageKind::FullOutput => OutputDamage::Full,
            NativeDamageKind::SurfaceDamage => OutputDamage::rects(
                width,
                height,
                self.rects
                    .iter()
                    .map(|rect| RendererOutputRect::new(rect.x, rect.y, rect.width, rect.height)),
            ),
        }
    }

    pub(crate) fn frame_copy_damage(&self) -> NativeFrameCopyDamage<'_> {
        match self.kind {
            NativeDamageKind::FullOutput => NativeFrameCopyDamage::Full,
            NativeDamageKind::Empty | NativeDamageKind::SurfaceDamage => {
                NativeFrameCopyDamage::Rects(&self.rects)
            }
        }
    }

    pub(crate) fn frame_copy_damage_for_scene(
        &self,
        scene_rebuild: DesktopSceneRebuildKind,
    ) -> NativeFrameCopyDamage<'_> {
        if scene_rebuild == DesktopSceneRebuildKind::Full {
            NativeFrameCopyDamage::Full
        } else {
            self.frame_copy_damage()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeDamageKind {
    Empty,
    SurfaceDamage,
    FullOutput,
}

impl NativeDamageKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::SurfaceDamage => "surface_damage",
            Self::FullOutput => "full",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeDamageRect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl NativeDamageRect {
    pub(crate) fn from_render_element_bounds(element: &RenderSceneElement) -> Option<Self> {
        let target = element.visible_target();
        (target.width() > 0 && target.height() > 0).then_some(Self {
            x: target.x(),
            y: target.y(),
            width: target.width(),
            height: target.height(),
        })
    }

    #[cfg(test)]
    pub(crate) fn from_surface_damage(
        surface: &RenderableSurface,
        origin: (i32, i32),
        rect: oblivion_one::compositor::SurfaceDamageRect,
    ) -> Option<Self> {
        if surface.width == 0 || surface.height == 0 {
            return None;
        }

        let buffer_size = surface.buffer_size();
        let left = scale_damage_floor(rect.x, buffer_size.width, surface.width)?;
        let top = scale_damage_floor(rect.y, buffer_size.height, surface.height)?;
        let right = scale_damage_ceil(
            rect.x.saturating_add(rect.width),
            buffer_size.width,
            surface.width,
        )?;
        let bottom = scale_damage_ceil(
            rect.y.saturating_add(rect.height),
            buffer_size.height,
            surface.height,
        )?;
        if right <= left || bottom <= top {
            return None;
        }

        Some(Self {
            x: i32_saturating_add_u32(origin.0, left),
            y: i32_saturating_add_u32(origin.1, top),
            width: right - left,
            height: bottom - top,
        })
    }

    pub(crate) fn from_render_element_damage(
        element: &RenderSceneElement,
        rect: oblivion_one::compositor::SurfaceDamageRect,
    ) -> Option<Self> {
        let target = element.visible_target();
        if target.width() == 0 || target.height() == 0 {
            return None;
        }

        let buffer_size = element.buffer_size();
        let left = scale_damage_floor(rect.x, buffer_size.width, target.width())?;
        let top = scale_damage_floor(rect.y, buffer_size.height, target.height())?;
        let right = scale_damage_ceil(
            rect.x.saturating_add(rect.width),
            buffer_size.width,
            target.width(),
        )?;
        let bottom = scale_damage_ceil(
            rect.y.saturating_add(rect.height),
            buffer_size.height,
            target.height(),
        )?;
        if right <= left || bottom <= top {
            return None;
        }

        Some(Self {
            x: i32_saturating_add_u32(target.x(), left),
            y: i32_saturating_add_u32(target.y(), top),
            width: right - left,
            height: bottom - top,
        })
    }

    pub(crate) fn clipped_to_output(self, output_width: u32, output_height: u32) -> Option<Self> {
        let left = i64::from(self.x).clamp(0, i64::from(output_width));
        let top = i64::from(self.y).clamp(0, i64::from(output_height));
        let right = i64::from(self.x)
            .saturating_add(i64::from(self.width))
            .clamp(0, i64::from(output_width));
        let bottom = i64::from(self.y)
            .saturating_add(i64::from(self.height))
            .clamp(0, i64::from(output_height));
        (right > left && bottom > top).then_some(Self {
            x: left as i32,
            y: top as i32,
            width: (right - left) as u32,
            height: (bottom - top) as u32,
        })
    }

    pub(crate) const fn pixels(self) -> u64 {
        (self.width as u64).saturating_mul(self.height as u64)
    }

    pub(crate) fn left(self) -> i64 {
        i64::from(self.x)
    }

    pub(crate) fn top(self) -> i64 {
        i64::from(self.y)
    }

    pub(crate) fn right(self) -> i64 {
        self.left().saturating_add(i64::from(self.width))
    }

    pub(crate) fn bottom(self) -> i64 {
        self.top().saturating_add(i64::from(self.height))
    }

    pub(crate) fn union(self, other: Self) -> Self {
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

pub(crate) fn coalesce_native_damage_rects(rects: Vec<NativeDamageRect>) -> Vec<NativeDamageRect> {
    let mut coalesced = Vec::<NativeDamageRect>::new();
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

#[derive(Debug, Clone)]
pub(crate) struct NativeDamageAccumulator {
    pub(crate) output_width: u32,
    pub(crate) output_height: u32,
    pub(crate) rects: Vec<NativeDamageRect>,
}

impl NativeDamageAccumulator {
    pub(crate) const fn for_output(output_width: u32, output_height: u32) -> Self {
        Self {
            output_width,
            output_height,
            rects: Vec::new(),
        }
    }

    pub(crate) fn from_surfaces(
        output_width: u32,
        output_height: u32,
        surfaces: &[RenderableSurface],
    ) -> Self {
        let elements = render_scene_elements_for_surfaces(surfaces, 1.0);
        Self::from_render_elements(output_width, output_height, &elements)
    }

    pub(crate) fn from_render_elements(
        output_width: u32,
        output_height: u32,
        elements: &[RenderSceneElement],
    ) -> Self {
        let mut accumulator = Self::for_output(output_width, output_height);
        for element in elements {
            accumulator.add_render_element(element);
        }
        accumulator
    }

    pub(crate) fn from_surface_bounds_changes(
        output_width: u32,
        output_height: u32,
        previous_surfaces: &[RenderableSurface],
        current_surfaces: &[RenderableSurface],
    ) -> Self {
        let previous_elements = render_scene_elements_for_surfaces(previous_surfaces, 1.0);
        let current_elements = render_scene_elements_for_surfaces(current_surfaces, 1.0);
        Self::from_render_element_bounds_changes(
            output_width,
            output_height,
            &previous_elements,
            &current_elements,
        )
    }

    pub(crate) fn from_render_element_bounds_changes(
        output_width: u32,
        output_height: u32,
        previous_elements: &[RenderSceneElement],
        current_elements: &[RenderSceneElement],
    ) -> Self {
        let previous_rects =
            native_element_bounds_by_id(output_width, output_height, previous_elements);
        let current_rects =
            native_element_bounds_by_id(output_width, output_height, current_elements);

        let mut accumulator = Self::for_output(output_width, output_height);
        for (surface_id, previous_rect) in &previous_rects {
            let current_rect = current_rects.get(surface_id).copied();
            if current_rect != Some(*previous_rect) {
                if let Some(current_rect) = current_rect {
                    accumulator.rects.push(*previous_rect);
                    accumulator.rects.push(current_rect);
                } else {
                    accumulator.rects.push(*previous_rect);
                }
            }
        }
        for (surface_id, current_rect) in current_rects {
            if !previous_rects.contains_key(&surface_id) {
                accumulator.rects.push(current_rect);
            }
        }
        accumulator
    }

    pub(crate) fn extend(&mut self, other: Self) {
        debug_assert_eq!(self.output_width, other.output_width);
        debug_assert_eq!(self.output_height, other.output_height);
        self.rects.extend(other.rects);
    }

    #[cfg(test)]
    pub(crate) fn add_surface(&mut self, surface: &RenderableSurface, origin: (i32, i32)) {
        let buffer_size = surface.buffer_size();
        for rect in surface
            .damage
            .clipped_rects(buffer_size.width, buffer_size.height)
        {
            let Some(rect) = NativeDamageRect::from_surface_damage(surface, origin, rect)
                .and_then(|rect| rect.clipped_to_output(self.output_width, self.output_height))
            else {
                continue;
            };
            self.rects.push(rect);
        }
    }

    pub(crate) fn add_render_element(&mut self, element: &RenderSceneElement) {
        let buffer_size = element.buffer_size();
        for rect in element
            .damage()
            .clipped_rects(buffer_size.width, buffer_size.height)
        {
            let Some(rect) = NativeDamageRect::from_render_element_damage(element, rect)
                .and_then(|rect| rect.clipped_to_output(self.output_width, self.output_height))
            else {
                continue;
            };
            self.rects.push(rect);
        }
    }

    #[cfg(test)]
    pub(crate) fn rects(&self) -> &[NativeDamageRect] {
        &self.rects
    }

    #[cfg(test)]
    pub(crate) fn summary(&self) -> NativeDamageSummary {
        if self.rects.is_empty() {
            return NativeDamageSummary {
                kind: NativeDamageKind::Empty,
                rects: 0,
                pixels: 0,
            };
        }

        NativeDamageSummary {
            kind: NativeDamageKind::SurfaceDamage,
            rects: self.rects.len(),
            pixels: self
                .rects
                .iter()
                .fold(0u64, |pixels, rect| pixels.saturating_add(rect.pixels())),
        }
    }

    pub(crate) fn into_output_damage(self) -> NativeOutputDamage {
        NativeOutputDamage::surface_damage(self.rects)
    }
}

pub(crate) fn native_element_bounds_by_id(
    output_width: u32,
    output_height: u32,
    elements: &[RenderSceneElement],
) -> HashMap<RenderSceneElementId, NativeDamageRect> {
    elements
        .iter()
        .filter_map(|element| {
            let rect = NativeDamageRect::from_render_element_bounds(element)?
                .clipped_to_output(output_width, output_height)?;
            Some((element.id(), rect))
        })
        .collect()
}

pub(crate) fn native_output_damage_for_repaint(
    width: u32,
    height: u32,
    previous_surfaces: &[RenderableSurface],
    surfaces: &[RenderableSurface],
    cause: RenderGenerationCause,
    render_generation_changed: bool,
) -> NativeOutputDamage {
    if render_generation_changed && cause.uses_surface_damage() {
        let mut damage = NativeDamageAccumulator::from_surfaces(width, height, surfaces);
        damage.extend(NativeDamageAccumulator::from_surface_bounds_changes(
            width,
            height,
            previous_surfaces,
            surfaces,
        ));
        damage.into_output_damage()
    } else if render_generation_changed
        && matches!(
            cause,
            RenderGenerationCause::WindowMove
                | RenderGenerationCause::WindowResize
                | RenderGenerationCause::SurfacePlacement
        )
    {
        let damage = NativeDamageAccumulator::from_surface_bounds_changes(
            width,
            height,
            previous_surfaces,
            surfaces,
        )
        .into_output_damage();
        if damage.rects.is_empty() {
            NativeOutputDamage::full_output(width, height)
        } else {
            damage
        }
    } else {
        NativeOutputDamage::full_output(width, height)
    }
}

pub(crate) fn native_repaint_cause_label(
    render_generation_cause: RenderGenerationCause,
    render_generation_changed: bool,
    accepted_clients: usize,
    pending_frame_work: bool,
    redraw_requested: bool,
) -> &'static str {
    if render_generation_changed {
        return render_generation_cause.as_str();
    }
    if redraw_requested {
        return "redraw_requested";
    }
    if pending_frame_work {
        return "pending_frame_work";
    }
    if accepted_clients > 0 {
        return "accepted_client";
    }
    "unknown"
}

pub(crate) fn scale_damage_floor(value: u32, from_extent: u32, to_extent: u32) -> Option<u32> {
    if from_extent == 0 {
        return None;
    }
    let scaled = u64::from(value).saturating_mul(u64::from(to_extent)) / u64::from(from_extent);
    Some(scaled.min(u64::from(u32::MAX)) as u32)
}

pub(crate) fn scale_damage_ceil(value: u32, from_extent: u32, to_extent: u32) -> Option<u32> {
    if from_extent == 0 {
        return None;
    }
    let numerator = u64::from(value).saturating_mul(u64::from(to_extent));
    let scaled =
        numerator.saturating_add(u64::from(from_extent).saturating_sub(1)) / u64::from(from_extent);
    Some(scaled.min(u64::from(u32::MAX)) as u32)
}

pub(crate) fn i32_saturating_add_u32(value: i32, addend: u32) -> i32 {
    i64::from(value)
        .saturating_add(i64::from(addend))
        .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}
