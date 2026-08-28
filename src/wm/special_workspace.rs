use std::num::NonZeroU32;

use super::WorkspaceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpecialWorkspaceId(NonZeroU32);

impl SpecialWorkspaceId {
    pub const DEFAULT: Self = Self(NonZeroU32::new(1).expect("special workspace id is non-zero"));

    pub const fn new(value: u32) -> Option<Self> {
        match NonZeroU32::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WorkspaceLocation {
    Regular(WorkspaceId),
    Special(SpecialWorkspaceId),
}
