use std::os::fd::{AsRawFd, BorrowedFd, RawFd};

use super::{
    AtomicKmsError, AtomicKmsErrorKind, ConnectorId, CrtcId, FramebufferId, PageFlipToken,
};

#[derive(Debug)]
pub struct LegacyKmsBackend {
    fd: RawFd,
    crtc: CrtcId,
    connector: ConnectorId,
    original: Option<drm_sys::drm_mode_crtc>,
    restored: bool,
}

impl LegacyKmsBackend {
    pub fn initialize(
        fd: BorrowedFd<'_>,
        connector: ConnectorId,
        crtc: CrtcId,
        mode: drm_sys::drm_mode_modeinfo,
        framebuffer: FramebufferId,
    ) -> Result<Self, AtomicKmsError> {
        let original = drm_ffi::mode::get_crtc(fd, crtc.get()).ok();
        drm_ffi::mode::set_crtc(
            fd,
            crtc.get(),
            framebuffer.get(),
            0,
            0,
            &[connector.get()],
            Some(mode),
        )
        .map_err(|error| {
            AtomicKmsError::new(
                AtomicKmsErrorKind::Io,
                format!("legacy set_crtc failed: {error}"),
            )
        })?;
        Ok(Self {
            fd: fd.as_raw_fd(),
            crtc,
            connector,
            original,
            restored: false,
        })
    }

    pub fn submit_flip(
        &self,
        framebuffer: FramebufferId,
        token: PageFlipToken,
    ) -> Result<(), AtomicKmsError> {
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        crate::native::drm::submit_legacy_page_flip(
            fd,
            self.crtc.get(),
            framebuffer.get(),
            token.get(),
        )
        .map_err(|error| {
            AtomicKmsError::new(
                AtomicKmsErrorKind::FlipRejected,
                format!("legacy pageflip failed: {error}"),
            )
        })
    }

    pub fn restore(&mut self) -> Result<RestorationOutcome, AtomicKmsError> {
        if self.restored {
            return Ok(RestorationOutcome::AlreadyRestored);
        }
        let Some(original) = self.original else {
            self.restored = true;
            return Ok(RestorationOutcome::Unavailable);
        };
        let mode = (original.mode_valid != 0).then_some(original.mode);
        let connectors = if mode.is_some() {
            vec![self.connector.get()]
        } else {
            Vec::new()
        };
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        drm_ffi::mode::set_crtc(
            fd,
            self.crtc.get(),
            original.fb_id,
            original.x,
            original.y,
            &connectors,
            mode,
        )
        .map_err(|error| {
            AtomicKmsError::new(
                AtomicKmsErrorKind::RestoreFailed,
                format!("legacy CRTC restore failed: {error}"),
            )
        })?;
        self.restored = true;
        Ok(RestorationOutcome::Exact)
    }
}

impl Drop for LegacyKmsBackend {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            eprintln!("legacy KMS: {error}");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestorationOutcome {
    Exact,
    SafeDisable,
    Unavailable,
    AlreadyRestored,
}

impl RestorationOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::SafeDisable => "safe_disable",
            Self::Unavailable => "unavailable",
            Self::AlreadyRestored => "already_restored",
        }
    }
}
