use std::num::NonZeroU64;

/// Identity of one atomic semantic presentation operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PresentationTransactionId(NonZeroU64);

impl PresentationTransactionId {
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    pub const fn from_raw(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Exact version of one SceneNode/property presentation track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PresentationRevisionId(NonZeroU64);

impl PresentationRevisionId {
    pub const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    pub const fn from_raw(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Compatibility name for the pre-v2 exact transition identity.
pub type TransitionId = PresentationRevisionId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PresentationPropertyKind {
    Geometry,
    Opacity,
}
