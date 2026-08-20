use std::num::NonZeroU64;

/// Stable identity for a live compositor window.
///
/// Values are allocated monotonically by the compositor and are never reused
/// during a compositor session. The type itself deliberately owns no
/// compositor or protocol state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(NonZeroU64);

impl WindowId {
    pub(crate) const fn new(value: NonZeroU64) -> Self {
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

    pub const fn raw(self) -> u64 {
        self.get()
    }
}
