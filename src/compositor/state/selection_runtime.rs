use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct SelectionPublicationKey {
    pub(in crate::compositor) kind: SelectionKind,
    pub(in crate::compositor) generation: u64,
}

impl SelectionPublicationKey {
    pub(in crate::compositor) const fn new(kind: SelectionKind, generation: u64) -> Self {
        Self { kind, generation }
    }
}
