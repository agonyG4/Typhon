use std::time::Instant;

use wayland_server::{
    Resource,
    protocol::{wl_pointer, wl_surface},
};

use super::selection::SelectionMutationEpoch;

const WL_POINTER_FRAME_SINCE: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InputSerialKind {
    PointerEnter,
    PointerButtonPress {
        button: u32,
    },
    KeyboardKeyPress {
        key: u32,
    },
    #[allow(dead_code)]
    TouchDown {
        touch_id: i32,
    },
}

#[derive(Debug, Clone)]
pub(super) struct InputSerial {
    pub(super) serial: u32,
    pub(super) epoch: SelectionMutationEpoch,
    pub(super) surface: wl_surface::WlSurface,
    pub(super) client_id: Option<wayland_server::backend::ClientId>,
    pub(super) root_surface_id: u32,
    pub(super) kind: InputSerialKind,
    pub(super) focus_generation: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OutputPosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl OutputRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Option<Self> {
        if !x.is_finite()
            || !y.is_finite()
            || !width.is_finite()
            || !height.is_finite()
            || width <= 0.0
            || height <= 0.0
        {
            return None;
        }
        Some(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub fn closest_point(self, position: OutputPosition) -> OutputPosition {
        let max_x = self.x + self.width - 1.0;
        let max_y = self.y + self.height - 1.0;
        OutputPosition {
            x: position.x.clamp(self.x, max_x),
            y: position.y.clamp(self.y, max_y),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputRegion {
    pub rects: Vec<OutputRect>,
}

impl OutputRegion {
    pub fn from_rect(rect: OutputRect) -> Self {
        Self { rects: vec![rect] }
    }

    pub fn closest_point(&self, position: OutputPosition) -> OutputPosition {
        let Some(first) = self.rects.first().copied() else {
            return position;
        };
        let mut closest = first.closest_point(position);
        let mut closest_distance = output_distance_squared(position, closest);
        for rect in self.rects.iter().copied().skip(1) {
            let candidate = rect.closest_point(position);
            let distance = output_distance_squared(position, candidate);
            if distance < closest_distance {
                closest = candidate;
                closest_distance = distance;
            }
        }
        closest
    }
}

fn output_distance_squared(left: OutputPosition, right: OutputPosition) -> f64 {
    let dx = left.x - right.x;
    let dy = left.y - right.y;
    dx.mul_add(dx, dy * dy)
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RelativePointerMotion {
    pub dx: f64,
    pub dy: f64,
    pub dx_unaccelerated: f64,
    pub dy_unaccelerated: f64,
}

impl RelativePointerMotion {
    pub fn is_zero(self) -> bool {
        self.dx == 0.0
            && self.dy == 0.0
            && self.dx_unaccelerated == 0.0
            && self.dy_unaccelerated == 0.0
    }

    pub fn from_absolute_delta(dx: f64, dy: f64) -> Option<Self> {
        if dx == 0.0 && dy == 0.0 {
            return None;
        }
        Some(Self {
            dx,
            dy,
            dx_unaccelerated: dx,
            dy_unaccelerated: dy,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PointerMotionSample {
    pub timestamp_usec: u64,
    pub absolute: Option<OutputPosition>,
    pub relative: Option<RelativePointerMotion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PointerConstraintMode {
    None,
    Confined,
    Locked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerConstraintLifetime {
    Oneshot,
    Persistent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PointerConstraintBackendId {
    pub constraint_id: u64,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerConstraintRegionResolutionTiming {
    pub duration_ns: u64,
    pub thread_cpu_ns: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PointerConstraintBackendRequest {
    ActivateLocked {
        id: PointerConstraintBackendId,
    },
    ActivateConfined {
        id: PointerConstraintBackendId,
        region: OutputRegion,
        region_resolution_timing: Option<PointerConstraintRegionResolutionTiming>,
    },
    UpdateConfinedRegion {
        id: PointerConstraintBackendId,
        region: OutputRegion,
    },
    Deactivate {
        id: PointerConstraintBackendId,
        restore_position: Option<OutputPosition>,
    },
    WarpPointer {
        position: OutputPosition,
    },
    ApplyCursorVisibility {
        visible: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPointerConstraintBackendRequest {
    pub request: PointerConstraintBackendRequest,
    pub locked_anchor: Option<OutputPosition>,
    pub region_resolution_timing: Option<PointerConstraintRegionResolutionTiming>,
}

impl PointerConstraintBackendRequest {
    pub const fn id(&self) -> Option<PointerConstraintBackendId> {
        match self {
            Self::ActivateLocked { id, .. }
            | Self::ActivateConfined { id, .. }
            | Self::UpdateConfinedRegion { id, .. } => Some(*id),
            Self::Deactivate { id, .. } => Some(*id),
            Self::WarpPointer { .. } | Self::ApplyCursorVisibility { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub struct PointerConstraintState {
    mode: PointerConstraintMode,
    surface_id: Option<u32>,
}

impl Default for PointerConstraintState {
    fn default() -> Self {
        Self {
            mode: PointerConstraintMode::None,
            surface_id: None,
        }
    }
}

impl PointerConstraintState {
    #[allow(dead_code)]
    pub fn activate(&mut self, mode: PointerConstraintMode, surface_id: u32) {
        self.mode = mode;
        self.surface_id = Some(surface_id);
    }

    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.mode = PointerConstraintMode::None;
        self.surface_id = None;
    }

    #[allow(dead_code)]
    pub const fn mode(self) -> PointerConstraintMode {
        self.mode
    }

    #[allow(dead_code)]
    pub fn filters_absolute_motion(self, surface_id: u32) -> bool {
        self.surface_id == Some(surface_id) && matches!(self.mode, PointerConstraintMode::Locked)
    }
}

pub(super) fn send_pointer_frame_if_supported(pointer: &wl_pointer::WlPointer) {
    if pointer.version() >= WL_POINTER_FRAME_SINCE {
        let _ = pointer.send_event(wl_pointer::Event::Frame);
    }
}

pub(super) fn wayland_event_time() -> u32 {
    static CLOCK_START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let elapsed = CLOCK_START.get_or_init(Instant::now).elapsed().as_millis();
    u32::try_from(elapsed & u128::from(u32::MAX)).unwrap_or_default()
}

pub(super) fn wayland_event_time_from_usec(timestamp_usec: u64) -> u32 {
    u32::try_from((timestamp_usec / 1_000) & u64::from(u32::MAX)).unwrap_or_default()
}

/// The metadata that accompanies one logical pointer scroll frame.
///
/// This lives at the compositor boundary so native input backends can preserve
/// libinput's source, discrete-step, stop, and timestamp information all the
/// way to wl_pointer dispatch.  A missing continuous value means that axis was
/// not present in the native event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerAxisComponent {
    pub continuous: Option<f64>,
    pub value120: Option<i32>,
    pub discrete: Option<i32>,
    pub stopped: bool,
}

impl PointerAxisComponent {
    pub const fn continuous(value: f64) -> Self {
        Self {
            continuous: Some(value),
            value120: None,
            discrete: None,
            stopped: false,
        }
    }

    pub const fn absent() -> Self {
        Self {
            continuous: None,
            value120: None,
            discrete: None,
            stopped: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerAxisSource {
    Wheel,
    Finger,
    Continuous,
    WheelTilt,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerAxisFrame {
    pub timestamp_usec: u64,
    pub source: PointerAxisSource,
    pub horizontal: PointerAxisComponent,
    pub vertical: PointerAxisComponent,
}

impl PointerAxisFrame {
    pub const fn unknown(timestamp_usec: u64, horizontal: f64, vertical: f64) -> Self {
        Self {
            timestamp_usec,
            source: PointerAxisSource::Unknown,
            horizontal: PointerAxisComponent::continuous(horizontal),
            vertical: PointerAxisComponent::continuous(vertical),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_pointer_motion_uses_absolute_delta_for_both_tracks() {
        let motion = RelativePointerMotion::from_absolute_delta(4.0, -2.5).unwrap();

        assert_eq!(motion.dx, 4.0);
        assert_eq!(motion.dy, -2.5);
        assert_eq!(motion.dx_unaccelerated, 4.0);
        assert_eq!(motion.dy_unaccelerated, -2.5);
        assert!(RelativePointerMotion::from_absolute_delta(0.0, 0.0).is_none());
    }

    #[test]
    fn relative_pointer_motion_detects_zero_across_both_tracks() {
        assert!(RelativePointerMotion::default().is_zero());
        assert!(
            !RelativePointerMotion {
                dx: 0.0,
                dy: 0.0,
                dx_unaccelerated: 0.25,
                dy_unaccelerated: 0.0,
            }
            .is_zero()
        );
    }

    #[test]
    fn pointer_constraint_locked_surface_filters_absolute_motion() {
        let mut state = PointerConstraintState::default();

        state.activate(PointerConstraintMode::Confined, 42);
        assert!(!state.filters_absolute_motion(42));
        state.activate(PointerConstraintMode::Locked, 42);

        assert!(state.filters_absolute_motion(42));
        assert!(!state.filters_absolute_motion(7));
        state.clear();
        assert_eq!(state.mode(), PointerConstraintMode::None);
    }
}
