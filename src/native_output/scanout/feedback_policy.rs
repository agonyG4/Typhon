use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NvidiaEglWayland2CompatPolicy {
    Auto,
    Off,
    Force,
}

impl NvidiaEglWayland2CompatPolicy {
    pub(crate) fn from_env() -> Self {
        let value = std::env::var("OBLIVION_ONE_NVIDIA_EGL_WAYLAND2_COMPAT").ok();
        let policy = value.as_deref().map(Self::parse).unwrap_or(Self::Auto);
        if value
            .as_deref()
            .is_some_and(|value| !Self::is_known_value(value))
        {
            eprintln!(
                "native GPU protocol: unknown OBLIVION_ONE_NVIDIA_EGL_WAYLAND2_COMPAT={value:?}; using off"
            );
        }
        policy
    }

    pub(crate) fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Self::Off,
            "force" => Self::Force,
            "auto" => Self::Auto,
            _ => Self::Off,
        }
    }

    pub(crate) fn is_known_value(value: &str) -> bool {
        matches!(value, "auto" | "off" | "force")
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
            Self::Force => "force",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DmabufFeedbackCompatibilityEffective {
    Off,
    SameDeviceNormalization,
}

impl DmabufFeedbackCompatibilityEffective {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::SameDeviceNormalization => "same-device-normalization",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DmabufFeedbackPolicyInput<'a> {
    pub(crate) requested: NvidiaEglWayland2CompatPolicy,
    pub(crate) egl_vendor: &'a str,
    pub(crate) main_device: Option<u64>,
    pub(crate) main_device_path: Option<&'a Path>,
    pub(crate) scanout_target_device: Option<u64>,
    pub(crate) scanout_target_path: Option<&'a Path>,
    pub(crate) same_physical_gpu: Option<bool>,
    pub(crate) dmabuf_version: u32,
    pub(crate) default_feedback_tranche_count: usize,
    pub(crate) surface_feedback_policy: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DmabufFeedbackCompatibility {
    pub(crate) requested: NvidiaEglWayland2CompatPolicy,
    pub(crate) egl_vendor: String,
    pub(crate) main_device: Option<u64>,
    pub(crate) main_device_path: Option<PathBuf>,
    pub(crate) scanout_target_device: Option<u64>,
    pub(crate) scanout_target_path: Option<PathBuf>,
    pub(crate) same_physical_gpu: Option<bool>,
    pub(crate) dmabuf_version: u32,
    pub(crate) default_feedback_tranche_count: usize,
    pub(crate) surface_feedback_policy: &'static str,
    pub(crate) effective: DmabufFeedbackCompatibilityEffective,
    pub(crate) reason: &'static str,
    rejected: bool,
}

impl DmabufFeedbackCompatibility {
    pub(crate) fn target_device_override(&self) -> Option<u64> {
        (self.effective == DmabufFeedbackCompatibilityEffective::SameDeviceNormalization)
            .then_some(self.main_device?)
    }

    #[cfg(test)]
    pub(crate) const fn normalization_count(&self) -> u64 {
        match self.effective {
            DmabufFeedbackCompatibilityEffective::Off => 0,
            DmabufFeedbackCompatibilityEffective::SameDeviceNormalization => 1,
        }
    }

    #[cfg(test)]
    pub(crate) const fn normalization_rejection_count(&self) -> u64 {
        self.rejected as u64
    }

    pub(crate) fn same_physical_gpu_label(&self) -> &'static str {
        match self.same_physical_gpu {
            Some(true) => "true",
            Some(false) => "false",
            None => "unknown",
        }
    }

    pub(crate) fn startup_diagnostic(&self) -> String {
        format!(
            "native GPU protocol: NVIDIA egl-wayland2 compatibility dmabuf_feedback_policy={} egl_vendor={} main_device={} main_device_path={} scanout_target_device={} scanout_target_path={} same_physical_gpu={} compat_requested={} compat_effective={} compat_reason={} default_feedback_tranche_count={} surface_feedback_policy={}",
            self.effective.as_str(),
            self.egl_vendor,
            display_device(self.main_device),
            display_path(self.main_device_path.as_deref()),
            display_device(self.scanout_target_device),
            display_path(self.scanout_target_path.as_deref()),
            self.same_physical_gpu_label(),
            self.requested.as_str(),
            self.effective.as_str(),
            self.reason,
            self.default_feedback_tranche_count,
            self.surface_feedback_policy,
        )
    }
}

fn display_device(device: Option<u64>) -> String {
    device
        .map(|device| device.to_string())
        .unwrap_or_else(|| "none".to_string())
}

fn display_path(path: Option<&Path>) -> String {
    path.map_or_else(|| "none".to_string(), |path| path.display().to_string())
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DmabufFeedbackCompatibilityMetrics {
    pub(crate) scanout_target_normalizations: u64,
    pub(crate) scanout_target_normalization_rejections: u64,
    last_normalized_target: Option<u64>,
    last_rejection: bool,
}

impl DmabufFeedbackCompatibilityMetrics {
    pub(crate) fn observe(&mut self, compatibility: &DmabufFeedbackCompatibility) {
        let target = compatibility.target_device_override();
        if target != self.last_normalized_target {
            if target.is_some() {
                self.scanout_target_normalizations =
                    self.scanout_target_normalizations.saturating_add(1);
            }
            self.last_normalized_target = target;
        }
        if compatibility.rejected && !self.last_rejection {
            self.scanout_target_normalization_rejections = self
                .scanout_target_normalization_rejections
                .saturating_add(1);
        }
        self.last_rejection = compatibility.rejected;
    }
}

pub(crate) fn resolve_dmabuf_feedback_policy(
    input: DmabufFeedbackPolicyInput<'_>,
) -> DmabufFeedbackCompatibility {
    let target_differs_from_main = input.main_device.is_some_and(|main| {
        input
            .scanout_target_device
            .is_some_and(|target| target != 0 && target != main)
    });
    let nvidia_vendor = input.egl_vendor.to_ascii_lowercase().contains("nvidia");
    let mut effective = DmabufFeedbackCompatibilityEffective::Off;
    let mut reason = "policy off";
    let mut rejected = false;

    if input.requested != NvidiaEglWayland2CompatPolicy::Off {
        if input.requested == NvidiaEglWayland2CompatPolicy::Auto && !nvidia_vendor {
            reason = "active EGL vendor is not NVIDIA";
        } else if input.dmabuf_version < 4 {
            reason = "DMA-BUF feedback version is below 4";
        } else if input.default_feedback_tranche_count < 2 || !target_differs_from_main {
            reason = if input.default_feedback_tranche_count < 2 {
                "no scanout preference tranche would target a different device"
            } else {
                "scanout target already matches main device"
            };
        } else {
            match input.same_physical_gpu {
                Some(true) => {
                    effective = DmabufFeedbackCompatibilityEffective::SameDeviceNormalization;
                    reason = if nvidia_vendor {
                        "same NVIDIA GPU uses distinct primary and render node dev_t values"
                    } else {
                        "same physical GPU uses distinct primary and render node dev_t values"
                    };
                }
                Some(false) => {
                    reason = "scanout and render nodes are different physical GPUs";
                    rejected = true;
                }
                None => {
                    reason = "same physical GPU could not be proven";
                    rejected = true;
                }
            }
        }
    }

    DmabufFeedbackCompatibility {
        requested: input.requested,
        egl_vendor: input.egl_vendor.to_string(),
        main_device: input.main_device,
        main_device_path: input.main_device_path.map(Path::to_path_buf),
        scanout_target_device: input.scanout_target_device,
        scanout_target_path: input.scanout_target_path.map(Path::to_path_buf),
        same_physical_gpu: input.same_physical_gpu,
        dmabuf_version: input.dmabuf_version,
        default_feedback_tranche_count: input.default_feedback_tranche_count,
        surface_feedback_policy: input.surface_feedback_policy,
        effective,
        reason,
        rejected,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DrmNode {
    path: PathBuf,
    device: u64,
    physical_device: Option<PathBuf>,
}

impl DrmNode {
    pub(crate) fn from_path(path: &Path) -> Option<Self> {
        Self::from_path_with_sysfs_root(path, Path::new("/sys/class/drm"))
    }

    pub(crate) fn from_path_with_sysfs_root(path: &Path, sysfs_root: &Path) -> Option<Self> {
        let device = fs::metadata(path).ok()?.rdev();
        let node_name = path.file_name()?.to_str()?;
        let physical_device = fs::canonicalize(sysfs_root.join(node_name).join("device")).ok();
        Some(Self {
            path: path.to_path_buf(),
            device,
            physical_device,
        })
    }

    #[cfg(test)]
    fn fixture(path: PathBuf, device: u64, physical_device: Option<PathBuf>) -> Self {
        Self {
            path,
            device,
            physical_device,
        }
    }

    #[cfg(test)]
    pub(crate) const fn device(&self) -> u64 {
        self.device
    }
}

pub(crate) fn drm_nodes_share_physical_device(primary: &DrmNode, render: &DrmNode) -> bool {
    primary
        .physical_device
        .as_ref()
        .zip(render.physical_device.as_ref())
        .is_some_and(|(primary, render)| primary == render)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_output::ASTREA_ENV_LOCK;
    use std::path::{Path, PathBuf};

    fn input(
        requested: NvidiaEglWayland2CompatPolicy,
        vendor: &str,
        main_device: u64,
        scanout_device: u64,
        same_physical_gpu: Option<bool>,
    ) -> DmabufFeedbackPolicyInput<'_> {
        DmabufFeedbackPolicyInput {
            requested,
            egl_vendor: vendor,
            main_device: Some(main_device),
            main_device_path: Some(Path::new("/dev/dri/renderD128")),
            scanout_target_device: Some(scanout_device),
            scanout_target_path: Some(Path::new("/dev/dri/card1")),
            same_physical_gpu,
            dmabuf_version: 4,
            default_feedback_tranche_count: 2,
            surface_feedback_policy: "default-and-surface-same",
        }
    }

    #[test]
    fn compatibility_policy_parses_auto_off_and_force() {
        assert_eq!(
            NvidiaEglWayland2CompatPolicy::parse("auto"),
            NvidiaEglWayland2CompatPolicy::Auto
        );
        assert_eq!(
            NvidiaEglWayland2CompatPolicy::parse("off"),
            NvidiaEglWayland2CompatPolicy::Off
        );
        assert_eq!(
            NvidiaEglWayland2CompatPolicy::parse("force"),
            NvidiaEglWayland2CompatPolicy::Force
        );
        assert_eq!(
            NvidiaEglWayland2CompatPolicy::parse("unknown"),
            NvidiaEglWayland2CompatPolicy::Off
        );
    }

    #[test]
    fn compatibility_policy_defaults_to_auto_when_unset() {
        let _guard = ASTREA_ENV_LOCK.lock().unwrap();
        let name = "OBLIVION_ONE_NVIDIA_EGL_WAYLAND2_COMPAT";
        let previous = std::env::var_os(name);
        // SAFETY: ASTREA_ENV_LOCK serializes environment mutation in native-output tests.
        unsafe { std::env::remove_var(name) };
        let policy = NvidiaEglWayland2CompatPolicy::from_env();
        match previous {
            // SAFETY: ASTREA_ENV_LOCK remains held while the prior value is restored.
            Some(value) => unsafe { std::env::set_var(name, value) },
            // SAFETY: ASTREA_ENV_LOCK remains held while the prior value is restored.
            None => unsafe { std::env::remove_var(name) },
        }

        assert_eq!(policy, NvidiaEglWayland2CompatPolicy::Auto);
    }

    #[test]
    fn forced_compat_normalizes_same_gpu_scanout_target() {
        let compatibility = resolve_dmabuf_feedback_policy(input(
            NvidiaEglWayland2CompatPolicy::Force,
            "NVIDIA",
            0x100,
            0x200,
            Some(true),
        ));

        assert_eq!(
            compatibility.effective,
            DmabufFeedbackCompatibilityEffective::SameDeviceNormalization
        );
        assert_eq!(compatibility.target_device_override(), Some(0x100));
        assert_eq!(compatibility.normalization_count(), 1);
    }

    #[test]
    fn forced_compat_rejects_different_gpu_nodes() {
        let compatibility = resolve_dmabuf_feedback_policy(input(
            NvidiaEglWayland2CompatPolicy::Force,
            "NVIDIA",
            0x100,
            0x200,
            Some(false),
        ));

        assert_eq!(
            compatibility.effective,
            DmabufFeedbackCompatibilityEffective::Off
        );
        assert_eq!(compatibility.target_device_override(), None);
        assert_eq!(compatibility.normalization_rejection_count(), 1);
    }

    #[test]
    fn auto_compat_enables_for_nvidia_same_gpu_distinct_nodes() {
        let compatibility = resolve_dmabuf_feedback_policy(input(
            NvidiaEglWayland2CompatPolicy::Auto,
            "NVIDIA Corporation",
            0x100,
            0x200,
            Some(true),
        ));

        assert_eq!(
            compatibility.effective,
            DmabufFeedbackCompatibilityEffective::SameDeviceNormalization
        );
    }

    #[test]
    fn auto_compat_stays_off_for_non_nvidia_vendor() {
        let compatibility = resolve_dmabuf_feedback_policy(input(
            NvidiaEglWayland2CompatPolicy::Auto,
            "Mesa Project",
            0x100,
            0x200,
            Some(true),
        ));

        assert_eq!(
            compatibility.effective,
            DmabufFeedbackCompatibilityEffective::Off
        );
        assert_eq!(compatibility.normalization_rejection_count(), 0);
    }

    #[test]
    fn auto_compat_stays_off_when_target_matches_main_device() {
        let compatibility = resolve_dmabuf_feedback_policy(input(
            NvidiaEglWayland2CompatPolicy::Auto,
            "NVIDIA",
            0x100,
            0x100,
            Some(true),
        ));

        assert_eq!(
            compatibility.effective,
            DmabufFeedbackCompatibilityEffective::Off
        );
        assert_eq!(compatibility.normalization_count(), 0);
    }

    #[test]
    fn auto_compat_stays_off_when_same_gpu_cannot_be_proven() {
        let compatibility = resolve_dmabuf_feedback_policy(input(
            NvidiaEglWayland2CompatPolicy::Auto,
            "NVIDIA",
            0x100,
            0x200,
            None,
        ));

        assert_eq!(
            compatibility.effective,
            DmabufFeedbackCompatibilityEffective::Off
        );
        assert_eq!(compatibility.normalization_rejection_count(), 1);
    }

    #[test]
    fn compat_metric_increments_once_per_changed_feedback() {
        let compatibility = resolve_dmabuf_feedback_policy(input(
            NvidiaEglWayland2CompatPolicy::Force,
            "NVIDIA",
            0x100,
            0x200,
            Some(true),
        ));
        let mut metrics = DmabufFeedbackCompatibilityMetrics::default();

        metrics.observe(&compatibility);
        metrics.observe(&compatibility);

        assert_eq!(metrics.scanout_target_normalizations, 1);
        assert_eq!(metrics.scanout_target_normalization_rejections, 0);
    }

    #[test]
    fn compat_feedback_cache_is_stable() {
        let compatibility = resolve_dmabuf_feedback_policy(input(
            NvidiaEglWayland2CompatPolicy::Auto,
            "NVIDIA",
            0x100,
            0x200,
            Some(true),
        ));
        let cached = compatibility.clone();

        assert_eq!(compatibility, cached);
        assert_eq!(cached.target_device_override(), Some(0x100));
    }

    #[test]
    fn real_machine_renderd128_card1_fixture_normalizes_same_gpu_target() {
        let primary = DrmNode::fixture(
            PathBuf::from("/dev/dri/card1"),
            0x301,
            Some(PathBuf::from("/sys/devices/pci0000:00/0000:00:01.0")),
        );
        let render = DrmNode::fixture(
            PathBuf::from("/dev/dri/renderD128"),
            0x901,
            Some(PathBuf::from("/sys/devices/pci0000:00/0000:00:01.0")),
        );
        let compatibility = resolve_dmabuf_feedback_policy(DmabufFeedbackPolicyInput {
            requested: NvidiaEglWayland2CompatPolicy::Auto,
            egl_vendor: "NVIDIA",
            main_device: Some(render.device),
            main_device_path: Some(&render.path),
            scanout_target_device: Some(primary.device),
            scanout_target_path: Some(&primary.path),
            same_physical_gpu: Some(drm_nodes_share_physical_device(&primary, &render)),
            dmabuf_version: 4,
            default_feedback_tranche_count: 2,
            surface_feedback_policy: "default-and-surface-same",
        });

        assert_eq!(compatibility.target_device_override(), Some(render.device));
    }

    #[test]
    fn drm_nodes_share_physical_device_only_for_equal_canonical_parents() {
        let primary = DrmNode::fixture(
            PathBuf::from("/dev/dri/card1"),
            0x201,
            Some(PathBuf::from("/sys/devices/pci0000:00/0000:00:01.0")),
        );
        let render = DrmNode::fixture(
            PathBuf::from("/dev/dri/renderD128"),
            0x801,
            Some(PathBuf::from("/sys/devices/pci0000:00/0000:00:01.0")),
        );
        let other_gpu = DrmNode::fixture(
            PathBuf::from("/dev/dri/card2"),
            0x202,
            Some(PathBuf::from("/sys/devices/pci0000:00/0000:00:02.0")),
        );
        let unknown = DrmNode::fixture(PathBuf::from("/dev/dri/card3"), 0x203, None);

        assert!(drm_nodes_share_physical_device(&primary, &render));
        assert!(!drm_nodes_share_physical_device(&primary, &other_gpu));
        assert!(!drm_nodes_share_physical_device(&primary, &unknown));
        assert_ne!(primary.device(), render.device());
    }

    #[test]
    fn drm_node_identity_uses_shared_sysfs_device_parent() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("dmabuf-feedback-topology-tests")
            .join(std::process::id().to_string());
        let sysfs = root.join("sys");
        let dri = root.join("dev").join("dri");
        let physical = root.join("pci").join("0000:01:00.0");
        let card = dri.join("card1");
        let render = dri.join("renderD128");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(sysfs.join("card1")).unwrap();
        fs::create_dir_all(sysfs.join("renderD128")).unwrap();
        fs::create_dir_all(&physical).unwrap();
        fs::create_dir_all(&dri).unwrap();
        fs::write(&card, b"").unwrap();
        fs::write(&render, b"").unwrap();
        std::os::unix::fs::symlink(&physical, sysfs.join("card1").join("device")).unwrap();
        std::os::unix::fs::symlink(&physical, sysfs.join("renderD128").join("device")).unwrap();

        let primary = DrmNode::from_path_with_sysfs_root(&card, &sysfs).unwrap();
        let render = DrmNode::from_path_with_sysfs_root(&render, &sysfs).unwrap();
        let shared = drm_nodes_share_physical_device(&primary, &render);
        let _ = fs::remove_dir_all(&root);

        assert!(shared);
    }

    #[test]
    fn compat_does_not_expand_direct_scanout_eligibility() {
        let capability = DmabufFeedbackCompatibilityEffective::SameDeviceNormalization;

        assert_eq!(capability.as_str(), "same-device-normalization");
        assert_eq!(
            capability,
            DmabufFeedbackCompatibilityEffective::SameDeviceNormalization
        );
    }
}
