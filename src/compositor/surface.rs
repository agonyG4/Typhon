use super::ViewportSourceRect;
use crate::render_backend::buffer::{
    BufferId, BufferIdentity, BufferSize, CommittedSurfaceBuffer, DmabufBufferHandle,
    SurfaceBufferSource,
};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct SurfaceCommitCounter(pub u64);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SurfaceCommitSequence(pub u64);

impl SurfaceCommitSequence {
    pub const fn initial() -> Self {
        Self(0)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DamageSince {
    Empty,
    Known(RenderableSurfaceDamage),
    HistoryLost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SurfaceDamageEntry {
    commit: SurfaceCommitCounter,
    damage: RenderableSurfaceDamage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceDamageJournal {
    current_commit: SurfaceCommitCounter,
    entries: VecDeque<SurfaceDamageEntry>,
    capacity: usize,
}

impl SurfaceDamageJournal {
    pub fn new(capacity: usize) -> Self {
        Self {
            current_commit: SurfaceCommitCounter::default(),
            entries: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    pub const fn current_commit(&self) -> SurfaceCommitCounter {
        self.current_commit
    }

    pub fn record(
        &mut self,
        damage: RenderableSurfaceDamage,
        width: u32,
        height: u32,
    ) -> SurfaceCommitCounter {
        self.current_commit = SurfaceCommitCounter(self.current_commit.0.wrapping_add(1).max(1));
        self.entries.push_back(SurfaceDamageEntry {
            commit: self.current_commit,
            damage: damage.normalized_for_surface(width, height),
        });
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
        self.current_commit
    }

    pub fn damage_since(
        &self,
        last_seen: SurfaceCommitCounter,
        width: u32,
        height: u32,
    ) -> DamageSince {
        if last_seen == self.current_commit {
            return DamageSince::Empty;
        }
        if last_seen > self.current_commit {
            return DamageSince::HistoryLost;
        }
        if self
            .entries
            .front()
            .is_some_and(|oldest| last_seen.0.saturating_add(1) < oldest.commit.0)
        {
            return DamageSince::HistoryLost;
        }
        let damage = self
            .entries
            .iter()
            .filter(|entry| entry.commit > last_seen)
            .fold(RenderableSurfaceDamage::Empty, |damage, entry| {
                damage.union(entry.damage.clone(), width, height)
            });
        if damage.is_empty() {
            DamageSince::Empty
        } else {
            DamageSince::Known(damage)
        }
    }
}

#[derive(Debug, Clone)]
pub struct RenderableSurface {
    pub surface_id: u32,
    pub x: i32,
    pub y: i32,
    // Logical extent of the currently committed wl_surface content.
    // Pointer resize, configure ACK, and visual window changes must not mutate it.
    pub width: u32,
    // Logical extent of the currently committed wl_surface content.
    // Pointer resize, configure ACK, and visual window changes must not mutate it.
    pub height: u32,
    pub placement: SurfacePlacement,
    pub render_placement: Option<SurfacePlacement>,
    pub visual_clip: Option<super::render::SurfaceTargetRect>,
    pub generation: u64,
    pub commit_sequence: SurfaceCommitSequence,
    pub buffer: CommittedSurfaceBuffer,
    pub viewport_source: Option<ViewportSourceRect>,
    pub damage: RenderableSurfaceDamage,
}

impl RenderableSurface {
    pub const fn buffer_id(&self) -> BufferId {
        self.buffer.buffer_id()
    }

    pub fn buffer_identity(&self) -> &BufferIdentity {
        self.buffer.buffer_identity()
    }

    pub const fn buffer_source(&self) -> SurfaceBufferSource {
        self.buffer.source()
    }

    pub fn cpu_pixels(&self) -> Option<&[u32]> {
        self.buffer.cpu_pixels()
    }

    pub fn dmabuf_handle(&self) -> Option<&DmabufBufferHandle> {
        self.buffer.dmabuf_handle_ref()
    }

    pub const fn buffer_size(&self) -> BufferSize {
        self.buffer.size()
    }

    pub(super) fn shm_pixels_mut(&mut self) -> Option<&mut Vec<u32>> {
        self.buffer.shm_pixels_mut()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderableSurfaceDamage {
    Empty,
    Full,
    Partial(Vec<SurfaceDamageRect>),
}

impl RenderableSurfaceDamage {
    pub const fn empty() -> Self {
        Self::Empty
    }

    pub const fn full() -> Self {
        Self::Full
    }

    pub fn from_rects(rects: Vec<SurfaceDamageRect>) -> Self {
        if rects.is_empty() {
            Self::Empty
        } else {
            Self::Partial(rects)
        }
    }

    pub const fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    pub const fn is_full(&self) -> bool {
        matches!(self, Self::Full)
    }

    pub fn normalized_for_surface(self, surface_width: u32, surface_height: u32) -> Self {
        if self.covers_surface(surface_width, surface_height) {
            return Self::Full;
        }

        match self {
            Self::Empty => Self::Empty,
            Self::Full => Self::Full,
            Self::Partial(rects) => {
                let clipped_rects = rects
                    .into_iter()
                    .filter_map(|rect| rect.clipped_to_surface(surface_width, surface_height))
                    .collect::<Vec<_>>();
                Self::from_rects(clipped_rects)
            }
        }
    }

    pub fn covers_surface(&self, surface_width: u32, surface_height: u32) -> bool {
        if surface_width == 0 || surface_height == 0 {
            return false;
        }

        match self {
            Self::Empty => false,
            Self::Full => true,
            Self::Partial(rects) => {
                let surface_pixels = u64::from(surface_width) * u64::from(surface_height);
                rects.iter().any(|rect| {
                    rect.clipped_to_surface(surface_width, surface_height)
                        .is_some_and(|rect| rect.covers_surface(surface_width, surface_height))
                }) || rects
                    .iter()
                    .filter_map(|rect| rect.clipped_to_surface(surface_width, surface_height))
                    .fold(0u64, |pixels, rect| {
                        pixels.saturating_add(u64::from(rect.width) * u64::from(rect.height))
                    })
                    >= surface_pixels
            }
        }
    }

    pub fn clipped_rects(&self, surface_width: u32, surface_height: u32) -> Vec<SurfaceDamageRect> {
        match self {
            Self::Empty => Vec::new(),
            Self::Full => vec![SurfaceDamageRect::full(surface_width, surface_height)],
            Self::Partial(rects) => rects
                .iter()
                .filter_map(|rect| rect.clipped_to_surface(surface_width, surface_height))
                .collect(),
        }
    }

    pub fn union(self, other: Self, surface_width: u32, surface_height: u32) -> Self {
        match (self, other) {
            (Self::Full, _) | (_, Self::Full) => Self::Full,
            (Self::Empty, damage) | (damage, Self::Empty) => {
                damage.normalized_for_surface(surface_width, surface_height)
            }
            (Self::Partial(mut older), Self::Partial(newer)) => {
                older.extend(newer);
                Self::Partial(older).normalized_for_surface(surface_width, surface_height)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceDamageRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl SurfaceDamageRect {
    pub const fn full(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    pub fn from_wayland_rect(x: i32, y: i32, width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }

        let left = i64::from(x);
        let top = i64::from(y);
        let right = left.saturating_add(i64::from(width));
        let bottom = top.saturating_add(i64::from(height));
        if right <= 0 || bottom <= 0 {
            return None;
        }

        let clipped_left = left.max(0);
        let clipped_top = top.max(0);
        let clipped_right = right.max(clipped_left);
        let clipped_bottom = bottom.max(clipped_top);

        Some(Self {
            x: clipped_left.try_into().ok()?,
            y: clipped_top.try_into().ok()?,
            width: (clipped_right - clipped_left).try_into().ok()?,
            height: (clipped_bottom - clipped_top).try_into().ok()?,
        })
    }

    fn clipped_to_surface(self, surface_width: u32, surface_height: u32) -> Option<Self> {
        let left = self.x.min(surface_width);
        let top = self.y.min(surface_height);
        let right = self.x.saturating_add(self.width).min(surface_width);
        let bottom = self.y.saturating_add(self.height).min(surface_height);
        (right > left && bottom > top).then_some(Self {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        })
    }

    fn covers_surface(self, surface_width: u32, surface_height: u32) -> bool {
        self.x == 0 && self.y == 0 && self.width == surface_width && self.height == surface_height
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootPlacementMode {
    CascadedWindow,
    Absolute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfacePlacement {
    pub parent_surface_id: Option<u32>,
    pub local_x: i32,
    pub local_y: i32,
    pub root_mode: RootPlacementMode,
}

impl SurfacePlacement {
    pub const fn root() -> Self {
        Self {
            parent_surface_id: None,
            local_x: 0,
            local_y: 0,
            root_mode: RootPlacementMode::CascadedWindow,
        }
    }

    pub const fn root_at(local_x: i32, local_y: i32) -> Self {
        Self {
            parent_surface_id: None,
            local_x,
            local_y,
            root_mode: RootPlacementMode::CascadedWindow,
        }
    }

    pub const fn absolute_root_at(local_x: i32, local_y: i32) -> Self {
        Self {
            parent_surface_id: None,
            local_x,
            local_y,
            root_mode: RootPlacementMode::Absolute,
        }
    }

    pub const fn subsurface(parent_surface_id: u32, local_x: i32, local_y: i32) -> Self {
        Self {
            parent_surface_id: Some(parent_surface_id),
            local_x,
            local_y,
            root_mode: RootPlacementMode::CascadedWindow,
        }
    }
}

impl Default for SurfacePlacement {
    fn default() -> Self {
        Self::root()
    }
}
