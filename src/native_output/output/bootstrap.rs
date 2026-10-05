use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeOutputBootstrap {
    pub(crate) runtime_dir: Option<PathBuf>,
    pub(crate) kms_device: Option<PathBuf>,
    pub(crate) render_device: Option<PathBuf>,
    pub(crate) kms_resources: Result<Option<KmsResources>, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeConnector {
    pub(crate) name: String,
    pub(crate) connector_id: Option<u32>,
    pub(crate) vrr_capable: Option<bool>,
}

pub(crate) fn native_vrr_policy_from_environment() -> VrrPolicy {
    let Some(value) = std::env::var_os("OBLIVION_ONE_VRR") else {
        return VrrPolicy::Auto;
    };
    let value = value.to_string_lossy();
    let policy = VrrPolicy::from_environment(Some(&value));
    if should_warn_unknown_vrr_policy(&value, policy) {
        eprintln!("native KMS: unknown OBLIVION_ONE_VRR={value:?}; using auto");
    }
    policy
}

fn should_warn_unknown_vrr_policy(value: &str, policy: VrrPolicy) -> bool {
    policy == VrrPolicy::Auto && !value.trim().eq_ignore_ascii_case("auto")
}

pub(crate) const fn vrr_doctor_severity(
    requested: VrrPolicy,
    supported: bool,
) -> oblivion_one::control_snapshots::DoctorSeverity {
    match requested {
        VrrPolicy::On if !supported => oblivion_one::control_snapshots::DoctorSeverity::Warning,
        VrrPolicy::Auto | VrrPolicy::Off | VrrPolicy::On => {
            oblivion_one::control_snapshots::DoctorSeverity::Ok
        }
    }
}

#[cfg(test)]
mod doctor_tests {
    use super::*;
    use oblivion_one::control_snapshots::DoctorSeverity;

    #[test]
    fn doctor_severity_reports_only_hard_capability_unavailability_for_on() {
        for supported in [false, true] {
            assert_eq!(
                vrr_doctor_severity(VrrPolicy::Off, supported),
                DoctorSeverity::Ok
            );
            assert_eq!(
                vrr_doctor_severity(VrrPolicy::Auto, supported),
                DoctorSeverity::Ok
            );
        }
        assert_eq!(vrr_doctor_severity(VrrPolicy::On, true), DoctorSeverity::Ok);
        assert_eq!(
            vrr_doctor_severity(VrrPolicy::On, false),
            DoctorSeverity::Warning
        );
    }

    #[test]
    fn trimmed_auto_vrr_policy_is_not_diagnosed_as_unknown() {
        for value in ["auto", " AUTO ", " auto "] {
            let policy = VrrPolicy::from_environment(Some(value));
            assert_eq!(policy, VrrPolicy::Auto);
            assert!(!should_warn_unknown_vrr_policy(value, policy));
        }

        let policy = VrrPolicy::from_environment(Some(" automatic "));
        assert!(should_warn_unknown_vrr_policy(" automatic ", policy));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KmsResources {
    pub(crate) crtc_count: usize,
    pub(crate) connector_count: usize,
    pub(crate) encoder_count: usize,
    pub(crate) connected_connector_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeModePreference {
    Auto,
    Preferred,
    HighResolution,
    HighRefresh,
    Exact {
        width: u32,
        height: u32,
        refresh_hz: Option<u32>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeModeStartupSource {
    EnvironmentOverride,
    Persisted,
    NormalSelection,
}

pub(crate) fn native_mode_startup_source(
    environment_override: Option<NativeModePreference>,
    persisted_mode_resolved: bool,
) -> NativeModeStartupSource {
    if environment_override.is_some() {
        NativeModeStartupSource::EnvironmentOverride
    } else if persisted_mode_resolved {
        NativeModeStartupSource::Persisted
    } else {
        NativeModeStartupSource::NormalSelection
    }
}

impl NativeModePreference {
    pub(crate) fn from_env() -> Self {
        Self::from_env_override().unwrap_or(Self::Auto)
    }

    pub(crate) fn from_env_override() -> Option<Self> {
        let Some(value) = std::env::var_os("OBLIVION_ONE_MODE") else {
            return None;
        };
        let value = value.to_string_lossy();
        let preference = Self::parse(&value);
        if preference == Self::Auto {
            if !value.eq_ignore_ascii_case("auto") {
                eprintln!("native KMS: unknown OBLIVION_ONE_MODE={value:?}; using auto");
            }
            return None;
        }
        Some(preference)
    }

    pub(crate) fn parse(value: &str) -> Self {
        let value = value.trim();
        if value.eq_ignore_ascii_case("auto") {
            return Self::Auto;
        }
        if value.eq_ignore_ascii_case("highres") {
            return Self::HighResolution;
        }
        if value.eq_ignore_ascii_case("preferred") {
            return Self::Preferred;
        }
        if value.eq_ignore_ascii_case("highrr") || value.eq_ignore_ascii_case("highrefresh") {
            return Self::HighRefresh;
        }
        Self::parse_exact(value).unwrap_or(Self::Auto)
    }

    pub(crate) fn parse_exact(value: &str) -> Option<Self> {
        let (resolution, refresh) = value.split_once('@').unwrap_or((value, ""));
        let (width, height) = resolution
            .split_once(['x', 'X'])
            .and_then(|(width, height)| {
                Some((width.trim().parse().ok()?, height.trim().parse().ok()?))
            })?;
        let refresh_hz = if refresh.trim().is_empty() {
            None
        } else {
            Some(parse_refresh_hz(refresh.trim())?)
        };
        Some(Self::Exact {
            width,
            height,
            refresh_hz,
        })
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Preferred => "preferred",
            Self::HighResolution => "highres",
            Self::HighRefresh => "highrr",
            Self::Exact { .. } => "exact",
        }
    }
}

pub(crate) fn parse_refresh_hz(value: &str) -> Option<u32> {
    if let Ok(refresh) = value.parse::<u32>() {
        return Some(refresh);
    }
    let refresh = value.parse::<f64>().ok()?;
    if refresh.is_finite() && refresh > 0.0 {
        Some(refresh.round() as u32)
    } else {
        None
    }
}

impl NativeOutputBootstrap {
    pub(crate) fn discover() -> Self {
        let kms_device = first_dri_node("card");
        let kms_resources = query_kms_resources(kms_device.as_deref());
        let render_device = kms_device
            .as_deref()
            .and_then(|path| {
                matching_render_node_for_card(
                    path,
                    Path::new("/sys/class/drm"),
                    Path::new("/dev/dri"),
                )
            })
            .or_else(|| first_dri_node("renderD"));
        Self {
            runtime_dir: std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
            kms_device,
            render_device,
            kms_resources,
        }
    }
}

pub(crate) fn matching_render_node_for_card(
    kms_device: &Path,
    drm_sysfs_root: &Path,
    dri_device_root: &Path,
) -> Option<PathBuf> {
    let card_name = kms_device.file_name()?.to_str()?;
    let drm_dir = drm_sysfs_root.join(card_name).join("device").join("drm");
    let mut render_nodes = fs::read_dir(drm_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            name.starts_with("renderD")
                .then(|| dri_device_root.join(name))
        })
        .collect::<Vec<_>>();
    render_nodes.sort();
    render_nodes.into_iter().next()
}
