use super::transaction::OutputContentKey;
use std::os::fd::OwnedFd;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectScanoutQualification {
    NotQualified,
    Qualified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectReleaseMode {
    Pageflip,
    OutFence,
}

#[derive(Debug)]
pub(crate) enum DirectSyncReadiness {
    Qualified {
        in_fence: Option<OwnedFd>,
        release_mode: DirectReleaseMode,
    },
    Unsupported(&'static str),
}

impl DirectSyncReadiness {
    /// Classify the synchronization contract before a direct primary submit.
    ///
    /// Published compositor content has already passed acquire readiness, so
    /// the current direct path does not need to manufacture an input fence.
    /// The helper still models the stricter explicit-fence mode so that a
    /// future commit worker cannot accidentally submit without the required
    /// plane property.
    pub(crate) fn from_capabilities(
        acquire_ready: bool,
        buffer_device_compatible: bool,
        atomic_backend: bool,
        in_fence_property: bool,
        out_fence_property: bool,
        require_input_fence: bool,
    ) -> Self {
        if !acquire_ready {
            return Self::Unsupported("acquire_not_ready");
        }
        if !buffer_device_compatible {
            return Self::Unsupported("buffer_device_or_modifier_unproven");
        }
        if !atomic_backend {
            return Self::Unsupported("atomic_backend_unavailable");
        }
        if require_input_fence && !in_fence_property {
            return Self::Unsupported("primary_in_fence_property_missing");
        }

        Self::Qualified {
            in_fence: None,
            release_mode: if out_fence_property {
                DirectReleaseMode::OutFence
            } else {
                DirectReleaseMode::Pageflip
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectScanoutQualificationState {
    status: DirectScanoutQualification,
    last_qualified_content: Option<OutputContentKey>,
}

impl Default for DirectScanoutQualificationState {
    fn default() -> Self {
        Self {
            status: DirectScanoutQualification::NotQualified,
            last_qualified_content: None,
        }
    }
}

impl DirectScanoutQualificationState {
    pub(crate) const fn status_str(self) -> &'static str {
        match self.status {
            DirectScanoutQualification::NotQualified => "not_qualified",
            DirectScanoutQualification::Qualified => "qualified",
        }
    }

    pub(crate) const fn is_qualified(self) -> bool {
        matches!(self.status, DirectScanoutQualification::Qualified)
    }

    #[allow(dead_code)]
    pub(crate) fn qualify(&mut self, content: OutputContentKey) {
        self.status = DirectScanoutQualification::Qualified;
        self.last_qualified_content = Some(content);
    }

    #[allow(dead_code)]
    pub(crate) fn invalidate(&mut self) {
        self.status = DirectScanoutQualification::NotQualified;
        self.last_qualified_content = None;
    }
}
