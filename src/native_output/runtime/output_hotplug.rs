use super::*;

/// DRM connector hotplug observation for the runtime's KMS card.
///
/// This only observes: it keeps the connector inventory current and reports
/// changes. Adding and retiring output heads builds on it (see
/// `docs/superpowers/specs/2026-11-02-multi-output-hotplug-design.md`).
#[derive(Default)]
pub(crate) struct NativeOutputHotplug {
    monitor: Option<DrmHotplugMonitor>,
    reactor_token: Option<ReactorToken>,
    inventory: ConnectorInventory,
    // A hotplug arrived while the session could not touch KMS. Session
    // recovery rescans unconditionally, so this only feeds telemetry.
    rescan_deferred: bool,
    uevents: u64,
    rescans: u64,
    failures: u64,
}

impl NativeOutputHotplug {
    pub(crate) fn open(kms: &fs::File) -> Self {
        let mut hotplug = Self::default();
        match kms_card_path(kms).and_then(|path| DrmHotplugMonitor::open(&path)) {
            Ok(monitor) => {
                println!(
                    "native output hotplug: watching {} for connector changes",
                    monitor.card_name()
                );
                hotplug.monitor = Some(monitor);
            }
            Err(error) => {
                eprintln!("native output hotplug: uevent monitor unavailable: {error}");
                hotplug.failures += 1;
            }
        }
        match ConnectorInventory::query(kms) {
            Ok(inventory) => hotplug.inventory = inventory,
            Err(error) => {
                eprintln!("native output hotplug: initial connector inventory failed: {error}");
                hotplug.failures += 1;
            }
        }
        hotplug
    }
}

fn kms_card_path(kms: &fs::File) -> io::Result<PathBuf> {
    fs::read_link(format!("/proc/self/fd/{}", kms.as_raw_fd()))
}

impl NativeRuntime {
    pub(super) fn register_drm_hotplug_source(&mut self) -> NativeResult<()> {
        if self.output_hotplug.reactor_token.is_some() {
            return Ok(());
        }
        let Some(fd) = self
            .output_hotplug
            .monitor
            .as_ref()
            .map(DrmHotplugMonitor::raw_fd)
        else {
            return Ok(());
        };
        match self.event_loop.register(fd, NativeEventSource::DrmHotplug) {
            Ok(token) => self.output_hotplug.reactor_token = Some(token),
            Err(error) => {
                eprintln!("native output hotplug: reactor registration failed: {error}");
                self.output_hotplug.monitor = None;
                self.output_hotplug.failures += 1;
            }
        }
        Ok(())
    }

    pub(super) fn service_drm_hotplug(&mut self) -> NativeResult<()> {
        let Some(monitor) = self.output_hotplug.monitor.as_ref() else {
            return Ok(());
        };
        let drain = match monitor.drain() {
            Ok(drain) => drain,
            Err(error) => {
                eprintln!("native output hotplug: uevent socket failed, disabling: {error}");
                self.output_hotplug.failures += 1;
                if let Some(token) = self.output_hotplug.reactor_token.take() {
                    self.event_loop.unregister(token)?;
                }
                self.output_hotplug.monitor = None;
                return Ok(());
            }
        };
        if !drain.hotplug {
            return Ok(());
        }
        self.output_hotplug.uevents += 1;
        if !self.session.permits_output() {
            self.output_hotplug.rescan_deferred = true;
            return Ok(());
        }
        self.rescan_connector_inventory("uevent");
        Ok(())
    }

    fn refresh_active_vrr_capability(&mut self, reason: &'static str) {
        let before = (
            self.kms_backend.atomic_connector_vrr_capable(),
            self.kms_backend.atomic_crtc_vrr_property_available(),
        );
        if let Err(error) = self
            .kms_backend
            .rediscover_atomic_pipeline_for_recovery(self.kms.file().as_fd())
        {
            // Legacy KMS or a pipeline that changed under us: keep the old
            // capability; head reconfiguration owns the latter.
            if self.kms_backend.effective_kind()
                == oblivion_one::native::kms::KmsBackendKind::Atomic
            {
                eprintln!(
                    "native output hotplug: VRR capability refresh ({reason}) failed: {error}"
                );
            }
            return;
        }
        let after = (
            self.kms_backend.atomic_connector_vrr_capable(),
            self.kms_backend.atomic_crtc_vrr_property_available(),
        );
        let selected_connector = kms_card_path(self.kms.file()).ok().and_then(|kms_path| {
            crate::native_output::output::selected_connector_for_kms_target(
                Some(&kms_path),
                Path::new("/sys/class/drm"),
                self.head.target.connector_id,
                &self.head.output_capabilities.connector_name,
            )
        });
        let edid_vrr_range = selected_connector.and_then(|connector| connector.edid_vrr_range);
        if after == before && edid_vrr_range == self.head.output_capabilities.edid_vrr_range {
            return;
        }
        self.head.scanout.refresh_vrr_capability(after.0, after.1);
        self.head.output_capabilities.edid_vrr_range = edid_vrr_range;
        let range = super::vrr_cadence::VrrRange::discover(
            self.head.output_refresh_rate.refresh_millihz(),
            edid_vrr_range,
            super::vrr_cadence::VrrRange::override_min_hz_from_environment(),
        );
        self.head.vrr_cadence = super::vrr_cadence::VrrCadenceRecorder::new(range);
        eprintln!(
            "native output hotplug ({reason}): VRR capability connector={} crtc={} (was {} {}), range {}..{} mHz from {}",
            after.0,
            after.1,
            before.0,
            before.1,
            range.min_hz.map_or(0, |hz| hz * 1_000),
            range.max_millihz,
            range.source.as_str()
        );
        self.perf.log("native.vrr_capability_changed", || {
            vec![
                NativePerfField::str("reason", reason),
                NativePerfField::bool("drm_connector_capable", after.0),
                NativePerfField::bool("crtc_vrr_property_available", after.1),
                NativePerfField::bool("previous_connector_capable", before.0),
                NativePerfField::u64("vrr_min_hz", range.min_hz.map_or(0, u64::from)),
                NativePerfField::u64("vrr_max_millihz", u64::from(range.max_millihz)),
                NativePerfField::str("vrr_range_source", range.source.as_str()),
            ]
        });
    }

    pub(super) fn rescan_connector_inventory(&mut self, reason: &'static str) {
        let next = match ConnectorInventory::query(self.kms.file()) {
            Ok(inventory) => inventory,
            Err(error) => {
                eprintln!("native output hotplug: connector rescan ({reason}) failed: {error}");
                self.output_hotplug.failures += 1;
                return;
            }
        };
        self.output_hotplug.rescans += 1;
        self.output_hotplug.rescan_deferred = false;
        let delta = self.output_hotplug.inventory.diff(&next);
        self.output_hotplug.inventory = next;
        // A sink can change its VRR capability without changing identity (an
        // OSD Adaptive-Sync toggle renegotiates the DP link), so the live
        // capability is re-read on every uevent, not only on inventory deltas.
        self.refresh_active_vrr_capability(reason);
        if delta.is_empty() {
            return;
        }
        for entry in &delta.connected {
            eprintln!("native output hotplug ({reason}): connected {entry}");
        }
        for entry in &delta.disconnected {
            eprintln!("native output hotplug ({reason}): disconnected {entry}");
        }
        for entry in &delta.changed {
            eprintln!("native output hotplug ({reason}): sink changed {entry}");
        }
        let active_connector = self.head.target.connector_id;
        if delta.touches(active_connector) {
            let still_connected = self
                .output_hotplug
                .inventory
                .entry(active_connector)
                .is_some_and(|entry| entry.connected);
            eprintln!(
                "native output hotplug: active connector #{active_connector} affected (connected={still_connected}); head reconfiguration is not implemented yet"
            );
        }
        self.perf.log("native.output_hotplug", || {
            vec![
                NativePerfField::str("reason", reason),
                NativePerfField::u64("connected", delta.connected.len() as u64),
                NativePerfField::u64("disconnected", delta.disconnected.len() as u64),
                NativePerfField::u64("changed", delta.changed.len() as u64),
                NativePerfField::u64(
                    "connected_total",
                    self.output_hotplug.inventory.connected().count() as u64,
                ),
                NativePerfField::u64("uevents", self.output_hotplug.uevents),
                NativePerfField::u64("rescans", self.output_hotplug.rescans),
                NativePerfField::u64("failures", self.output_hotplug.failures),
            ]
        });
    }
}
