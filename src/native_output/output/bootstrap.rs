use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeOutputBootstrap {
    pub(crate) runtime_dir: Option<PathBuf>,
    pub(crate) kms_device: Option<PathBuf>,
    pub(crate) render_device: Option<PathBuf>,
    pub(crate) connector: Option<NativeConnector>,
    pub(crate) kms_resources: Result<Option<KmsResources>, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeConnector {
    pub(crate) name: String,
    pub(crate) enabled: Option<String>,
    pub(crate) modes: Vec<String>,
    pub(crate) vrr_capable: Option<bool>,
}

impl NativeConnector {
    pub(crate) fn preferred_mode(&self) -> Option<&str> {
        self.modes.first().map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeVrrPreference {
    Auto,
    On,
    Off,
}

impl NativeVrrPreference {
    pub(crate) fn from_env() -> Self {
        let Some(value) = std::env::var_os("OBLIVION_ONE_VRR") else {
            return Self::Auto;
        };
        let value = value.to_string_lossy();
        let preference = Self::parse(&value);
        if preference == Self::Auto && !value.eq_ignore_ascii_case("auto") {
            eprintln!("native KMS: unknown OBLIVION_ONE_VRR={value:?}; using auto");
        }
        preference
    }

    pub(crate) fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" | "enable" | "enabled" => Self::On,
            "0" | "false" | "no" | "off" | "disable" | "disabled" => Self::Off,
            _ => Self::Auto,
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NativeVrrPlan {
    pub(crate) requested: NativeVrrPreference,
    pub(crate) supported: bool,
    pub(crate) planned_enabled: bool,
}

impl NativeVrrPlan {
    pub(crate) fn choose(
        requested: NativeVrrPreference,
        connector_vrr_capable: Option<bool>,
    ) -> Self {
        let supported = connector_vrr_capable.unwrap_or(false);
        let planned_enabled = match requested {
            NativeVrrPreference::Auto | NativeVrrPreference::On => supported,
            NativeVrrPreference::Off => false,
        };
        Self {
            requested,
            supported,
            planned_enabled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KmsResources {
    pub(crate) crtc_count: usize,
    pub(crate) connector_count: usize,
    pub(crate) encoder_count: usize,
    pub(crate) connected_connector_count: usize,
    pub(crate) first_connected_connector_id: Option<u32>,
    pub(crate) first_connected_mode: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct KmsTarget {
    pub(crate) connector_id: u32,
    pub(crate) crtc_id: u32,
    pub(crate) mode: drm_sys::drm_mode_modeinfo,
    pub(crate) width: u32,
    pub(crate) height: u32,
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

impl NativeModePreference {
    pub(crate) fn from_env() -> Self {
        let Some(value) = std::env::var_os("OBLIVION_ONE_MODE") else {
            return Self::Auto;
        };
        let value = value.to_string_lossy();
        let preference = Self::parse(&value);
        if preference == Self::Auto && !value.eq_ignore_ascii_case("auto") {
            eprintln!("native KMS: unknown OBLIVION_ONE_MODE={value:?}; using auto");
        }
        preference
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
        let connector =
            connected_connector_for_card(kms_device.as_deref(), Path::new("/sys/class/drm"));
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
            connector,
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
