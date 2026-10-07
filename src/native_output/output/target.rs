use super::*;
use oblivion_one::compositor::OutputRefreshRate;
use oblivion_one::control_snapshots::{
    FeatureState, MAX_CONTROL_OUTPUT_MODES, OutputModeSnapshot, PhysicalSizeSnapshot,
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct NativeOutputModeEntry {
    pub(crate) id: u32,
    pub(crate) mode: drm_sys::drm_mode_modeinfo,
}

#[derive(Debug, Clone)]
pub(crate) struct NativeOutputModeInventory {
    generation: OutputConfigurationGeneration,
    entries: Vec<NativeOutputModeEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeOutputModeResolveError {
    StaleGeneration,
    UnknownMode,
}

impl NativeOutputModeInventory {
    pub(crate) fn from_native_modes(
        generation: OutputConfigurationGeneration,
        modes: &[drm_sys::drm_mode_modeinfo],
    ) -> Self {
        let entries = modes
            .iter()
            .enumerate()
            .filter_map(|(index, mode)| {
                Some(NativeOutputModeEntry {
                    id: u32::try_from(index.checked_add(1)?).ok()?,
                    mode: *mode,
                })
            })
            .collect();
        Self {
            generation,
            entries,
        }
    }

    pub(crate) fn resolve(
        &self,
        generation: OutputConfigurationGeneration,
        id: u32,
    ) -> Result<&NativeOutputModeEntry, NativeOutputModeResolveError> {
        if generation != self.generation {
            return Err(NativeOutputModeResolveError::StaleGeneration);
        }
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .ok_or(NativeOutputModeResolveError::UnknownMode)
    }

    pub(crate) fn entries(&self) -> &[NativeOutputModeEntry] {
        &self.entries
    }

    pub(crate) fn requalify(&mut self, generation: OutputConfigurationGeneration) {
        self.generation = generation;
    }

    fn project(&self) -> (Vec<OutputModeSnapshot>, bool) {
        let mut projected = self
            .entries
            .iter()
            .filter_map(|entry| {
                let mode = &entry.mode;
                let width = u32::from(mode.hdisplay);
                let height = u32::from(mode.vdisplay);
                let refresh_millihz = drm_mode_refresh_millihz(mode)?;
                (width > 0 && height > 0 && refresh_millihz > 0).then_some((
                    entry.id,
                    OutputModeSnapshot {
                        id: entry.id,
                        width,
                        height,
                        refresh_millihz,
                        preferred: mode.type_ & drm_sys::DRM_MODE_TYPE_PREFERRED != 0,
                        interlaced: mode.flags & drm_sys::DRM_MODE_FLAG_INTERLACE != 0,
                    },
                ))
            })
            .collect::<Vec<_>>();
        projected.sort_by_key(|(_, mode)| {
            (
                mode.width,
                mode.height,
                mode.refresh_millihz,
                mode.interlaced,
            )
        });

        let modes_truncated = projected.len() > MAX_CONTROL_OUTPUT_MODES;
        projected.truncate(MAX_CONTROL_OUTPUT_MODES);
        (
            projected.into_iter().map(|(_, mode)| mode).collect(),
            modes_truncated,
        )
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NativeOutputCapabilities {
    pub(crate) connector_name: String,
    pub(crate) physical_size_mm: Option<PhysicalSizeSnapshot>,
    pub(crate) modes: Vec<OutputModeSnapshot>,
    pub(crate) modes_truncated: bool,
    pub(crate) mode_inventory: NativeOutputModeInventory,
    pub(crate) sysfs_vrr_capable: Option<bool>,
}

impl NativeOutputCapabilities {
    pub(crate) fn qualify_sysfs_connector(&mut self, connector: Option<&NativeConnector>) {
        if let Some(connector) = connector {
            self.connector_name.clone_from(&connector.name);
            self.sysfs_vrr_capable = connector.vrr_capable;
        } else {
            self.sysfs_vrr_capable = None;
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NativeKmsTargetSelection {
    pub(crate) target: NativeAppliedOutputConfiguration,
    pub(crate) capabilities: NativeOutputCapabilities,
}

pub(crate) fn first_dri_node(prefix: &str) -> Option<PathBuf> {
    let mut entries = fs::read_dir("/dev/dri")
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix))
        })
        .collect::<Vec<_>>();
    entries.sort();
    entries.into_iter().next()
}

pub(crate) fn query_kms_resources(
    kms_device: Option<&Path>,
) -> Result<Option<KmsResources>, String> {
    let Some(kms_device) = kms_device else {
        return Ok(None);
    };
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(kms_device)
        .map_err(|error| format!("failed to open {}: {error}", kms_device.display()))?;

    let mut crtcs = Vec::new();
    let mut connector_ids = Vec::new();
    let mut encoders = Vec::new();
    drm_ffi::mode::get_resources(
        file.as_fd(),
        None,
        Some(&mut crtcs),
        Some(&mut connector_ids),
        Some(&mut encoders),
    )
    .map_err(|error| format!("DRM_IOCTL_MODE_GETRESOURCES failed: {error}"))?;

    let mut connected_connector_count = 0usize;
    for connector_id in &connector_ids {
        let connector =
            drm_ffi::mode::get_connector(file.as_fd(), *connector_id, None, None, None, None, true)
                .map_err(|error| {
                    format!(
                        "DRM_IOCTL_MODE_GETCONNECTOR failed for connector {connector_id}: {error}"
                    )
                })?;
        if connector.connection == 1 {
            connected_connector_count += 1;
        }
    }

    Ok(Some(KmsResources {
        crtc_count: crtcs.len(),
        connector_count: connector_ids.len(),
        encoder_count: encoders.len(),
        connected_connector_count,
    }))
}

pub(crate) fn select_kms_target(
    file: &fs::File,
    mode_preference: NativeModePreference,
) -> io::Result<NativeAppliedOutputConfiguration> {
    select_kms_target_with_capabilities(file, mode_preference).map(|selection| selection.target)
}

pub(crate) fn select_kms_target_with_capabilities(
    file: &fs::File,
    mode_preference: NativeModePreference,
) -> io::Result<NativeKmsTargetSelection> {
    select_kms_target_internal(file, mode_preference, None)?.ok_or_else(no_connected_kms_target)
}

pub(crate) fn select_kms_target_with_persisted_configuration(
    file: &fs::File,
    configuration: &PersistedOutputConfiguration,
) -> io::Result<Option<NativeKmsTargetSelection>> {
    select_kms_target_internal(file, NativeModePreference::Auto, Some(configuration))
}

fn select_kms_target_internal(
    file: &fs::File,
    mode_preference: NativeModePreference,
    persisted_configuration: Option<&PersistedOutputConfiguration>,
) -> io::Result<Option<NativeKmsTargetSelection>> {
    let mut crtcs = Vec::new();
    let mut connector_ids = Vec::new();
    drm_ffi::mode::get_resources(
        file.as_fd(),
        None,
        Some(&mut crtcs),
        Some(&mut connector_ids),
        None,
    )?;

    for connector_id in connector_ids {
        let mut modes = Vec::new();
        let mut encoder_ids = Vec::new();
        let connector = drm_ffi::mode::get_connector(
            file.as_fd(),
            connector_id,
            None,
            None,
            Some(&mut modes),
            Some(&mut encoder_ids),
            true,
        )?;
        if connector.connection != 1 {
            continue;
        }
        let connector_name = drm_connector_presentation_name(&connector);
        let physical_size = physical_size_snapshot(connector.mm_width, connector.mm_height);
        let mode = if let Some(configuration) = persisted_configuration {
            let inventory = NativeOutputModeInventory::from_native_modes(
                OutputConfigurationGeneration::initial(),
                &modes,
            );
            configuration
                .resolve_mode(
                    &connector_name,
                    physical_size.map(|size| (size.width_mm, size.height_mm)),
                    &inventory,
                )
                .map(|entry| entry.mode)
        } else {
            select_kms_mode(&modes, mode_preference)
        };
        let Some(mode) = mode else {
            continue;
        };

        let current_encoder = (connector.encoder_id != 0).then_some(connector.encoder_id);
        for encoder_id in current_encoder.into_iter().chain(encoder_ids.into_iter()) {
            let encoder = drm_ffi::mode::get_encoder(file.as_fd(), encoder_id)?;
            if let Some(crtc_id) = select_crtc_id(&crtcs, &encoder) {
                let mode_id = modes
                    .iter()
                    .position(|candidate| native_mode_identity_eq(candidate, &mode))
                    .and_then(|index| u32::try_from(index + 1).ok())
                    .ok_or_else(|| {
                        io::Error::other(
                            "selected native mode is absent from its connector inventory",
                        )
                    })?;
                return Ok(Some(NativeKmsTargetSelection {
                    target: NativeAppliedOutputConfiguration {
                        connector_id,
                        crtc_id,
                        mode_id,
                        mode,
                        width: u32::from(mode.hdisplay),
                        height: u32::from(mode.vdisplay),
                        scale_milli: 1000,
                        transform: oblivion_one::control_snapshots::OutputTransformSnapshot::Normal,
                    },
                    capabilities: native_output_capabilities(&connector, &modes),
                }));
            }
        }
    }

    Ok(None)
}

fn no_connected_kms_target() -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        "no connected KMS connector with a usable CRTC was found",
    )
}

pub(crate) fn native_output_capabilities(
    connector: &drm_sys::drm_mode_get_connector,
    modes: &[drm_sys::drm_mode_modeinfo],
) -> NativeOutputCapabilities {
    let mode_inventory = NativeOutputModeInventory::from_native_modes(
        OutputConfigurationGeneration::initial(),
        modes,
    );
    let (projected_modes, modes_truncated) = mode_inventory.project();
    NativeOutputCapabilities {
        connector_name: drm_connector_presentation_name(connector),
        physical_size_mm: physical_size_snapshot(connector.mm_width, connector.mm_height),
        modes: projected_modes,
        modes_truncated,
        mode_inventory,
        sysfs_vrr_capable: None,
    }
}

pub(crate) fn project_native_output_modes(
    modes: &[drm_sys::drm_mode_modeinfo],
) -> (Vec<OutputModeSnapshot>, bool) {
    NativeOutputModeInventory::from_native_modes(OutputConfigurationGeneration::initial(), modes)
        .project()
}

fn native_mode_identity_eq(
    left: &drm_sys::drm_mode_modeinfo,
    right: &drm_sys::drm_mode_modeinfo,
) -> bool {
    left.clock == right.clock
        && left.hdisplay == right.hdisplay
        && left.hsync_start == right.hsync_start
        && left.hsync_end == right.hsync_end
        && left.htotal == right.htotal
        && left.hskew == right.hskew
        && left.vdisplay == right.vdisplay
        && left.vsync_start == right.vsync_start
        && left.vsync_end == right.vsync_end
        && left.vtotal == right.vtotal
        && left.vscan == right.vscan
        && left.flags == right.flags
        && left.type_ == right.type_
        && left.name == right.name
}

pub(crate) fn drm_mode_refresh_millihz(mode: &drm_sys::drm_mode_modeinfo) -> Option<u32> {
    let calculated = (|| {
        let width = u64::from(mode.hdisplay);
        let height = u64::from(mode.vdisplay);
        let htotal = u64::from(mode.htotal);
        let vtotal = u64::from(mode.vtotal);
        if mode.clock == 0 || htotal < width || vtotal < height || htotal == 0 || vtotal == 0 {
            return None;
        }
        let mut numerator = u64::from(mode.clock).checked_mul(1_000_000)?;
        let mut denominator = htotal.checked_mul(vtotal)?;
        if mode.flags & drm_sys::DRM_MODE_FLAG_INTERLACE != 0 {
            numerator = numerator.checked_mul(2)?;
        }
        if mode.flags & drm_sys::DRM_MODE_FLAG_DBLSCAN != 0 {
            denominator = denominator.checked_mul(2)?;
        }
        let vscan = u64::from(mode.vscan.max(1));
        denominator = denominator.checked_mul(vscan)?;
        let rounded = numerator.checked_add(denominator / 2)? / denominator;
        u32::try_from(rounded).ok().filter(|refresh| *refresh > 0)
    })();
    calculated.or_else(|| {
        (mode.vrefresh > 0)
            .then(|| mode.vrefresh.checked_mul(1000))
            .flatten()
    })
}

pub(crate) fn output_refresh_rate_for_mode(mode: &drm_sys::drm_mode_modeinfo) -> OutputRefreshRate {
    let refresh_millihz =
        drm_mode_refresh_millihz(mode).unwrap_or_else(|| mode.vrefresh.saturating_mul(1_000));
    let refresh_millihz = if refresh_millihz == 0 {
        60_000
    } else {
        refresh_millihz
    };
    let fallback_interval_ns = 1_000_000_000_000u64 / u64::from(refresh_millihz);
    let interval_ns =
        oblivion_one::native_output::presentation::kms_timing::KmsModeTiming::from_mode(
            mode,
            fallback_interval_ns,
        )
        .refresh_interval_ns();
    OutputRefreshRate::from_native_timing(refresh_millihz, interval_ns)
}

pub(crate) fn physical_size_snapshot(
    width_mm: u32,
    height_mm: u32,
) -> Option<PhysicalSizeSnapshot> {
    (width_mm > 0 && height_mm > 0).then_some(PhysicalSizeSnapshot {
        width_mm,
        height_mm,
    })
}

pub(crate) const fn runtime_vrr_feature_state(
    atomic_capable: bool,
    pageflip_confirmed_active: bool,
) -> FeatureState {
    if !atomic_capable {
        FeatureState::Unavailable
    } else if pageflip_confirmed_active {
        FeatureState::Active
    } else {
        FeatureState::Available
    }
}

fn drm_connector_presentation_name(connector: &drm_sys::drm_mode_get_connector) -> String {
    let prefix = match connector.connector_type {
        drm_sys::DRM_MODE_CONNECTOR_VGA => "VGA",
        drm_sys::DRM_MODE_CONNECTOR_DVII => "DVI-I",
        drm_sys::DRM_MODE_CONNECTOR_DVID => "DVI-D",
        drm_sys::DRM_MODE_CONNECTOR_DVIA => "DVI-A",
        drm_sys::DRM_MODE_CONNECTOR_Composite => "Composite",
        drm_sys::DRM_MODE_CONNECTOR_SVIDEO => "SVIDEO",
        drm_sys::DRM_MODE_CONNECTOR_LVDS => "LVDS",
        drm_sys::DRM_MODE_CONNECTOR_Component => "Component",
        drm_sys::DRM_MODE_CONNECTOR_9PinDIN => "DIN",
        drm_sys::DRM_MODE_CONNECTOR_DisplayPort => "DP",
        drm_sys::DRM_MODE_CONNECTOR_HDMIA => "HDMI-A",
        drm_sys::DRM_MODE_CONNECTOR_HDMIB => "HDMI-B",
        drm_sys::DRM_MODE_CONNECTOR_TV => "TV",
        drm_sys::DRM_MODE_CONNECTOR_eDP => "eDP",
        drm_sys::DRM_MODE_CONNECTOR_VIRTUAL => "Virtual",
        drm_sys::DRM_MODE_CONNECTOR_DSI => "DSI",
        drm_sys::DRM_MODE_CONNECTOR_DPI => "DPI",
        drm_sys::DRM_MODE_CONNECTOR_WRITEBACK => "Writeback",
        drm_sys::DRM_MODE_CONNECTOR_SPI => "SPI",
        drm_sys::DRM_MODE_CONNECTOR_USB => "USB",
        _ => return String::from("Display"),
    };
    format!("{prefix}-{}", connector.connector_type_id)
}

pub(crate) fn select_kms_mode(
    modes: &[drm_sys::drm_mode_modeinfo],
    preference: NativeModePreference,
) -> Option<drm_sys::drm_mode_modeinfo> {
    match preference {
        NativeModePreference::Preferred => modes.first().copied(),
        NativeModePreference::Auto | NativeModePreference::HighResolution => modes
            .iter()
            .copied()
            .max_by_key(|mode| (mode_area(mode), mode.vrefresh)),
        NativeModePreference::HighRefresh => modes
            .iter()
            .copied()
            .max_by_key(|mode| (mode.vrefresh, mode_area(mode))),
        NativeModePreference::Exact {
            width,
            height,
            refresh_hz,
        } => select_exact_kms_mode(modes, width, height, refresh_hz)
            .or_else(|| select_kms_mode(modes, NativeModePreference::Auto)),
    }
}

pub(crate) fn select_exact_kms_mode(
    modes: &[drm_sys::drm_mode_modeinfo],
    width: u32,
    height: u32,
    refresh_hz: Option<u32>,
) -> Option<drm_sys::drm_mode_modeinfo> {
    let matching_modes = modes
        .iter()
        .copied()
        .filter(|mode| u32::from(mode.hdisplay) == width && u32::from(mode.vdisplay) == height);
    if let Some(refresh_hz) = refresh_hz {
        return matching_modes.min_by_key(|mode| {
            (
                mode.vrefresh.abs_diff(refresh_hz),
                u32::MAX.saturating_sub(mode.vrefresh),
            )
        });
    }
    matching_modes.max_by_key(|mode| mode.vrefresh)
}

pub(crate) fn mode_area(mode: &drm_sys::drm_mode_modeinfo) -> u64 {
    u64::from(mode.hdisplay) * u64::from(mode.vdisplay)
}

pub(crate) fn select_crtc_id(
    crtcs: &[u32],
    encoder: &drm_sys::drm_mode_get_encoder,
) -> Option<u32> {
    if encoder.crtc_id != 0 && crtcs.contains(&encoder.crtc_id) {
        return Some(encoder.crtc_id);
    }

    crtcs
        .iter()
        .enumerate()
        .find(|(index, _)| encoder.possible_crtcs & (1 << index) != 0)
        .map(|(_, crtc_id)| *crtc_id)
}

pub(crate) fn drm_mode_name(mode: &drm_sys::drm_mode_modeinfo) -> String {
    let bytes = mode
        .name
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect::<Vec<_>>();
    String::from_utf8_lossy(&bytes).into_owned()
}
