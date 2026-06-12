use khronos_egl as egl;
use oblivion_one::compositor::{DesktopVisualState, SurfaceDamageRect, cursor_texture_size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EglOutputDamage {
    Full { width: u32, height: u32 },
    Rects([Option<SurfaceDamageRect>; 8]),
}

impl EglOutputDamage {
    pub(super) const fn full(width: u32, height: u32) -> Self {
        Self::Full { width, height }
    }

    #[cfg(test)]
    pub(super) const fn two_rects(first: SurfaceDamageRect, second: SurfaceDamageRect) -> Self {
        Self::Rects([
            Some(first),
            Some(second),
            None,
            None,
            None,
            None,
            None,
            None,
        ])
    }

    fn from_damage_rects(rects: DamageRectSet) -> Self {
        Self::Rects(rects.values)
    }

    pub(super) fn to_egl_rects(self) -> Option<EglDamageRects> {
        match self {
            Self::Full { width, height } => Some(EglDamageRects::single([
                0,
                0,
                width as egl::Int,
                height as egl::Int,
            ])),
            Self::Rects(rects) => {
                let mut egl_rects = EglDamageRects::new();
                for rect in rects.into_iter().flatten() {
                    if rect.width == 0 || rect.height == 0 {
                        continue;
                    }
                    egl_rects.push([
                        rect.x as egl::Int,
                        rect.y as egl::Int,
                        rect.width as egl::Int,
                        rect.height as egl::Int,
                    ]);
                }
                (!egl_rects.is_empty()).then_some(egl_rects)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EglDamageRects {
    values: [egl::Int; 32],
    value_count: usize,
}

impl EglDamageRects {
    const fn new() -> Self {
        Self {
            values: [0; 32],
            value_count: 0,
        }
    }

    const fn single(rect: [egl::Int; 4]) -> Self {
        let mut values = [0; 32];
        values[0] = rect[0];
        values[1] = rect[1];
        values[2] = rect[2];
        values[3] = rect[3];
        Self {
            values,
            value_count: 4,
        }
    }

    fn push(&mut self, rect: [egl::Int; 4]) {
        if self.value_count + 4 > self.values.len() {
            return;
        }
        self.values[self.value_count..self.value_count + 4].copy_from_slice(&rect);
        self.value_count += 4;
    }

    const fn is_empty(self) -> bool {
        self.value_count == 0
    }

    pub(super) const fn rect_count(self) -> usize {
        self.value_count / 4
    }

    pub(super) fn as_ptr(&self) -> *const egl::Int {
        self.values.as_ptr()
    }

    #[cfg(test)]
    pub(super) fn as_slice(&self) -> &[egl::Int] {
        &self.values[..self.value_count]
    }
}

#[derive(Debug, Default)]
pub(super) struct EglOutputDamageTracker {
    output_size: (u32, u32),
    last_cursor_rect: Option<SurfaceDamageRect>,
    last_shell_overlay: Option<ShellOverlayDamageState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ShellOverlayDamageState {
    generation: u64,
    rects: [Option<SurfaceDamageRect>; 4],
}

impl ShellOverlayDamageState {
    pub(super) fn new(generation: u64, rects: impl IntoIterator<Item = SurfaceDamageRect>) -> Self {
        let mut values = [None, None, None, None];
        for (slot, rect) in values.iter_mut().zip(rects) {
            *slot = Some(rect);
        }
        Self {
            generation,
            rects: values,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct DamageRectSet {
    values: [Option<SurfaceDamageRect>; 8],
}

impl DamageRectSet {
    const fn new() -> Self {
        Self {
            values: [None, None, None, None, None, None, None, None],
        }
    }

    fn push(&mut self, rect: SurfaceDamageRect) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        if self
            .values
            .iter()
            .flatten()
            .any(|existing| *existing == rect)
        {
            return;
        }
        if let Some(slot) = self.values.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(rect);
        }
    }

    fn is_empty(self) -> bool {
        self.values.iter().all(Option::is_none)
    }
}

impl EglOutputDamageTracker {
    pub(super) fn damage_for_frame(
        &mut self,
        width: u32,
        height: u32,
        scene_changed: bool,
        visual_state: DesktopVisualState,
        shell_overlay: Option<ShellOverlayDamageState>,
    ) -> EglOutputDamage {
        let cursor_rect = visual_state
            .cursor
            .and_then(|(x, y)| cursor_damage_rect(x, y, width, height));
        let size_changed = self.output_size != (width, height);
        self.output_size = (width, height);

        if size_changed || scene_changed {
            self.last_cursor_rect = cursor_rect;
            self.last_shell_overlay = shell_overlay;
            return EglOutputDamage::full(width, height);
        }

        let mut damage = DamageRectSet::new();
        if self.last_shell_overlay != shell_overlay {
            if let Some(previous) = self.last_shell_overlay {
                for rect in previous.rects.into_iter().flatten() {
                    damage.push(rect);
                }
            }
            if let Some(current) = shell_overlay {
                for rect in current.rects.into_iter().flatten() {
                    damage.push(rect);
                }
            }
        }

        if self.last_cursor_rect != cursor_rect {
            if let Some(previous) = self.last_cursor_rect {
                damage.push(previous);
            }
            if let Some(current) = cursor_rect {
                damage.push(current);
            }
        }

        self.last_cursor_rect = cursor_rect;
        self.last_shell_overlay = shell_overlay;
        if damage.is_empty() {
            EglOutputDamage::full(width, height)
        } else {
            EglOutputDamage::from_damage_rects(damage)
        }
    }
}

pub(super) fn cursor_damage_rect(
    cursor_x: i32,
    cursor_y: i32,
    output_width: u32,
    output_height: u32,
) -> Option<SurfaceDamageRect> {
    let (cursor_width, cursor_height) = cursor_texture_size();
    let left = cursor_x.max(0) as u32;
    let top = cursor_y.max(0) as u32;
    if left >= output_width || top >= output_height {
        return None;
    }
    let right = (cursor_x + cursor_width as i32)
        .max(0)
        .min(output_width as i32) as u32;
    let bottom = (cursor_y + cursor_height as i32)
        .max(0)
        .min(output_height as i32) as u32;
    let width = right.saturating_sub(left);
    let height = bottom.saturating_sub(top);
    (width > 0 && height > 0).then_some(SurfaceDamageRect {
        x: left,
        y: top,
        width,
        height,
    })
}
