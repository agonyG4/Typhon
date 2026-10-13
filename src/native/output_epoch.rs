/// Lifetime of the currently published physical output presentation pipeline.
///
/// Structural modesets and successful session recovery advance this ID;
/// Settings changes and buffer-pool replacement retain their own identities.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutputEpochId(u64);

impl OutputEpochId {
    pub const fn initial() -> Self {
        Self(1)
    }

    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn next(self) -> Self {
        let next = self.0.wrapping_add(1);
        Self(if next == 0 { 1 } else { next })
    }

    pub fn advance(&mut self) -> Self {
        *self = self.next();
        *self
    }
}
