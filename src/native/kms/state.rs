use std::time::Instant;

use super::{AtomicKmsError, AtomicKmsErrorKind, FramebufferId, PageFlipToken};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AtomicCommitState {
    #[default]
    Idle,
    Pending {
        token: PageFlipToken,
        framebuffer: FramebufferId,
        backend_generation: u64,
        submitted_at: Instant,
    },
}

impl AtomicCommitState {
    pub fn begin(
        &mut self,
        token: PageFlipToken,
        framebuffer: FramebufferId,
        backend_generation: u64,
        submitted_at: Instant,
    ) -> Result<(), AtomicKmsError> {
        if self.is_pending() {
            return Err(AtomicKmsError::new(
                AtomicKmsErrorKind::AlreadyPending,
                "an atomic display commit is already pending",
            ));
        }
        *self = Self::Pending {
            token,
            framebuffer,
            backend_generation,
            submitted_at,
        };
        Ok(())
    }

    pub fn submission_failed(&mut self, token: PageFlipToken) -> bool {
        if matches!(self, Self::Pending { token: pending, .. } if *pending == token) {
            *self = Self::Idle;
            true
        } else {
            false
        }
    }

    /// Forget a flip after libseat has revoked the DRM fd; the corresponding
    /// kernel event must be treated as stale after recovery.
    pub fn abandon(&mut self) {
        *self = Self::Idle;
    }

    pub fn complete(&mut self, token: PageFlipToken, backend_generation: u64) -> AtomicCompletion {
        let Self::Pending {
            token: expected,
            framebuffer,
            backend_generation: expected_generation,
            ..
        } = *self
        else {
            return AtomicCompletion::Stale;
        };
        if expected_generation != backend_generation {
            return AtomicCompletion::StaleGeneration;
        }
        if expected != token {
            return AtomicCompletion::Mismatched;
        }
        *self = Self::Idle;
        AtomicCompletion::Completed { framebuffer }
    }

    pub const fn is_pending(&self) -> bool {
        matches!(self, Self::Pending { .. })
    }

    pub const fn pending_token(&self) -> Option<PageFlipToken> {
        match self {
            Self::Idle => None,
            Self::Pending { token, .. } => Some(*token),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicCompletion {
    Completed { framebuffer: FramebufferId },
    Mismatched,
    StaleGeneration,
    Stale,
}
