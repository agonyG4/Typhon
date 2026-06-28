use super::*;

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
    let mut first_connected_connector_id = None;
    let mut first_connected_mode = None;
    for connector_id in &connector_ids {
        let mut modes = Vec::new();
        let connector = drm_ffi::mode::get_connector(
            file.as_fd(),
            *connector_id,
            None,
            None,
            Some(&mut modes),
            None,
            true,
        )
        .map_err(|error| {
            format!("DRM_IOCTL_MODE_GETCONNECTOR failed for connector {connector_id}: {error}")
        })?;
        if connector.connection == 1 {
            connected_connector_count += 1;
            first_connected_connector_id.get_or_insert(*connector_id);
            if first_connected_mode.is_none() {
                first_connected_mode = modes.first().map(drm_mode_name);
            }
        }
    }

    Ok(Some(KmsResources {
        crtc_count: crtcs.len(),
        connector_count: connector_ids.len(),
        encoder_count: encoders.len(),
        connected_connector_count,
        first_connected_connector_id,
        first_connected_mode,
    }))
}

pub(crate) fn select_kms_target(
    file: &fs::File,
    mode_preference: NativeModePreference,
) -> io::Result<KmsTarget> {
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
        let Some(mode) = select_kms_mode(&modes, mode_preference) else {
            continue;
        };

        let current_encoder = (connector.encoder_id != 0).then_some(connector.encoder_id);
        for encoder_id in current_encoder.into_iter().chain(encoder_ids.into_iter()) {
            let encoder = drm_ffi::mode::get_encoder(file.as_fd(), encoder_id)?;
            if let Some(crtc_id) = select_crtc_id(&crtcs, &encoder) {
                return Ok(KmsTarget {
                    connector_id,
                    crtc_id,
                    mode,
                    width: u32::from(mode.hdisplay),
                    height: u32::from(mode.vdisplay),
                });
            }
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no connected KMS connector with a usable CRTC was found",
    ))
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
