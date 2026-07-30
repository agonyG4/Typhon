#![allow(dead_code)]

use std::{
    fmt,
    num::NonZeroU64,
    ops::{BitOr, BitOrAssign},
};

use oblivion_one::native::kms::{AtomicCursorVisualState, PageFlipToken};

use super::pipeline::ConfirmedPrimaryState;

macro_rules! plane_identity {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub(crate) struct $name(NonZeroU64);

        impl $name {
            pub(crate) const fn new(value: NonZeroU64) -> Self {
                Self(value)
            }

            pub(crate) const fn get(self) -> u64 {
                self.0.get()
            }

            const fn next(self) -> Self {
                match NonZeroU64::new(self.0.get().wrapping_add(1)) {
                    Some(next) => Self(next),
                    None => Self(NonZeroU64::MIN),
                }
            }
        }
    };
}

plane_identity!(PlaneStateRevision);
plane_identity!(CursorImageEpoch);
plane_identity!(CursorMotionEpoch);
plane_identity!(CursorVisibilityEpoch);
plane_identity!(CursorSidecarId);
plane_identity!(KmsCommitBundleId);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorRevision {
    pub(crate) image: CursorImageEpoch,
    pub(crate) motion: CursorMotionEpoch,
    pub(crate) visibility: CursorVisibilityEpoch,
}

impl CursorRevision {
    pub(crate) const fn initial() -> Self {
        Self {
            image: CursorImageEpoch::new(NonZeroU64::MIN),
            motion: CursorMotionEpoch::new(NonZeroU64::MIN),
            visibility: CursorVisibilityEpoch::new(NonZeroU64::MIN),
        }
    }

    pub(crate) const fn from_legacy_epoch(epoch: NonZeroU64) -> Self {
        Self {
            image: CursorImageEpoch::new(epoch),
            motion: CursorMotionEpoch::new(epoch),
            visibility: CursorVisibilityEpoch::new(epoch),
        }
    }

    pub(crate) const fn advance_image(self) -> Self {
        Self {
            image: self.image.next(),
            ..self
        }
    }

    pub(crate) const fn advance_motion(self) -> Self {
        Self {
            motion: self.motion.next(),
            ..self
        }
    }

    pub(crate) const fn advance_visibility(self) -> Self {
        Self {
            visibility: self.visibility.next(),
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlaneWriteSet(u8);

impl PlaneWriteSet {
    pub(crate) const PRIMARY: Self = Self(0b0001);
    pub(crate) const CURSOR: Self = Self(0b0010);
    pub(crate) const OUTPUT: Self = Self(0b0100);

    pub(crate) const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub(crate) const fn validate_cursor_delta(self) -> Result<(), PlaneWriteSetError> {
        if !self.contains(Self::CURSOR) {
            Err(PlaneWriteSetError::MissingCursor)
        } else if self.contains(Self::PRIMARY) {
            Err(PlaneWriteSetError::CursorDeltaChangesPrimary)
        } else {
            Ok(())
        }
    }
}

impl BitOr for PlaneWriteSet {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for PlaneWriteSet {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlaneWriteSetError {
    MissingCursor,
    CursorDeltaChangesPrimary,
}

impl fmt::Display for PlaneWriteSetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for PlaneWriteSetError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorCoupling {
    IndependentPlane,
    EmbeddedInPrimary,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorPlanePoint {
    pub(crate) x: i32,
    pub(crate) y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PresentedCursorState {
    pub(crate) revision: CursorRevision,
    pub(crate) coupling: CursorCoupling,
    pub(crate) framebuffer_id: Option<u32>,
    pub(crate) visible: bool,
    pub(crate) output_position: CursorPlanePoint,
    pub(crate) hotspot: CursorPlanePoint,
}

impl PresentedCursorState {
    pub(crate) fn from_atomic(
        revision: CursorRevision,
        coupling: CursorCoupling,
        state: &AtomicCursorVisualState,
    ) -> Self {
        Self {
            revision,
            coupling,
            framebuffer_id: state.framebuffer_id,
            visible: state.visible,
            output_position: CursorPlanePoint {
                x: state.x,
                y: state.y,
            },
            hotspot: CursorPlanePoint {
                x: state.hotspot_x,
                y: state.hotspot_y,
            },
        }
    }

    pub(crate) fn kms_equivalent_to(self, state: &AtomicCursorVisualState) -> bool {
        if !self.visible && !state.visible {
            return true;
        }
        self.visible == state.visible
            && self.framebuffer_id == state.framebuffer_id
            && self.output_position.x == state.x
            && self.output_position.y == state.y
            && self.hotspot.x == state.hotspot_x
            && self.hotspot.y == state.hotspot_y
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PresentedPlaneSnapshot {
    pub(crate) revision: PlaneStateRevision,
    pub(crate) primary: Option<ConfirmedPrimaryState>,
    pub(crate) cursor: PresentedCursorState,
}

impl PresentedPlaneSnapshot {
    pub(crate) const fn initial(cursor: PresentedCursorState) -> Self {
        Self {
            revision: PlaneStateRevision::new(NonZeroU64::MIN),
            primary: None,
            cursor,
        }
    }

    pub(crate) const fn legacy(primary: Option<ConfirmedPrimaryState>) -> Self {
        Self {
            revision: PlaneStateRevision::new(NonZeroU64::MIN),
            primary,
            cursor: PresentedCursorState {
                revision: CursorRevision::initial(),
                coupling: CursorCoupling::Hidden,
                framebuffer_id: None,
                visible: false,
                output_position: CursorPlanePoint { x: 0, y: 0 },
                hotspot: CursorPlanePoint { x: 0, y: 0 },
            },
        }
    }

    pub(crate) fn promote_cursor(
        &mut self,
        promotion: &PresentedCursorPromotion,
        pageflip: PlanePageflipIdentity,
    ) -> bool {
        if promotion.identity != pageflip {
            return false;
        }
        self.cursor = promotion.cursor;
        self.revision = self.revision.next();
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlanePageflipIdentity {
    pub(crate) bundle_id: KmsCommitBundleId,
    pub(crate) token: PageFlipToken,
    pub(crate) output_generation: u64,
    pub(crate) crtc_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PresentedCursorPromotion {
    pub(crate) identity: PlanePageflipIdentity,
    pub(crate) cursor: PresentedCursorState,
}
