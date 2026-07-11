use std::os::fd::{AsRawFd, BorrowedFd, RawFd};

use super::{
    AtomicKmsError, AtomicKmsErrorKind, ConnectorId, CrtcId, FramebufferId, PageFlipToken,
};

#[derive(Debug)]
pub struct LegacyKmsBackend {
    fd: RawFd,
    crtc: CrtcId,
    connector: ConnectorId,
    mode: drm_sys::drm_mode_modeinfo,
    original: Option<drm_sys::drm_mode_crtc>,
    restored: bool,
    restore_on_drop: bool,
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
        .map(|_| ())
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
            mode,
            original,
            restored: false,
            restore_on_drop: true,
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

    pub fn recover(&self, framebuffer: FramebufferId) -> Result<(), AtomicKmsError> {
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        let mut crtcs = Vec::new();
        let mut connectors = Vec::new();
        drm_ffi::mode::get_resources(fd, None, Some(&mut crtcs), Some(&mut connectors), None)
            .map_err(|error| {
                AtomicKmsError::new(
                    AtomicKmsErrorKind::DeviceLost,
                    format!("revalidate legacy KMS resources before recovery failed: {error}"),
                )
            })?;
        let mut modes = Vec::new();
        let connector = drm_ffi::mode::get_connector(
            fd,
            self.connector.get(),
            None,
            None,
            Some(&mut modes),
            None,
            true,
        )
        .map_err(|error| {
            AtomicKmsError::new(
                AtomicKmsErrorKind::DeviceLost,
                format!("revalidate legacy connector mode failed: {error}"),
            )
        })?;
        let mode_available = modes.iter().any(|candidate| {
            candidate.hdisplay == self.mode.hdisplay
                && candidate.vdisplay == self.mode.vdisplay
                && candidate.vrefresh == self.mode.vrefresh
        });
        if !crtcs.contains(&self.crtc.get())
            || !connectors.contains(&self.connector.get())
            || connector.connection != 1
            || !mode_available
        {
            return Err(AtomicKmsError::new(
                AtomicKmsErrorKind::DeviceLost,
                format!(
                    "legacy recovery target disappeared (connector {}, CRTC {})",
                    self.connector.get(),
                    self.crtc.get()
                ),
            ));
        }
        drm_ffi::mode::set_crtc(
            fd,
            self.crtc.get(),
            framebuffer.get(),
            0,
            0,
            &[self.connector.get()],
            Some(self.mode),
        )
        .map(|_| ())
        .map_err(|error| {
            AtomicKmsError::new(
                AtomicKmsErrorKind::InitialCommitRejected,
                format!("legacy native-session recovery modeset failed: {error}"),
            )
        })
    }

    pub fn disarm_drm_io(&mut self) {
        self.restore_on_drop = false;
        self.restored = true;
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
        if self.restore_on_drop
            && !self.restored
            && let Err(error) = self.restore()
        {
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
