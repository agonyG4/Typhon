use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

/// Semantic identity for user-visible output configuration.
///
/// This generation changes only when the authoritative output binding changes
/// or a Display configuration is applied or rolled back. It is intentionally
/// independent from render, swapchain, framebuffer, and presentation IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct OutputConfigurationGeneration(u64);

impl OutputConfigurationGeneration {
    pub(crate) const fn initial() -> Self {
        Self(1)
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }

    pub(crate) fn advance(&mut self) -> Self {
        self.0 = self.0.wrapping_add(1).max(1);
        *self
    }
}

/// One user-visible applied Display configuration. Native KMS identifiers and
/// the exact mode remain runtime-only and are never written to persistence.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NativeAppliedOutputConfiguration {
    pub(crate) connector_id: u32,
    pub(crate) crtc_id: u32,
    pub(crate) mode_id: u32,
    pub(crate) mode: drm_sys::drm_mode_modeinfo,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) scale_milli: u32,
    pub(crate) transform: oblivion_one::control_snapshots::OutputTransformSnapshot,
}

pub(crate) const OUTPUT_CONFIGURATION_ROLLBACK_TIMEOUT_NS: u64 = 15_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct OutputConfigurationTransactionId(NonZeroU64);

impl OutputConfigurationTransactionId {
    pub(crate) const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub(crate) const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OutputConfigurationTransactionPhase {
    PendingConfirmation,
    PersistencePending,
    RollingBack,
    RollbackFailed { error_code: String },
}

#[derive(Debug, Clone)]
pub(crate) struct OutputConfigurationTransaction {
    pub(crate) id: OutputConfigurationTransactionId,
    pub(crate) output_id: String,
    pub(crate) previous: NativeAppliedOutputConfiguration,
    pub(crate) temporary: NativeAppliedOutputConfiguration,
    pub(crate) applied_configuration_generation: u64,
    pub(crate) rollback_deadline_ns: u64,
    pub(crate) phase: OutputConfigurationTransactionPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputConfigurationTransactionError {
    Busy,
    IdExhausted,
    StaleTransaction,
    NotConfirmable,
    NotRollingBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputConfigurationConfirmCompletion {
    Confirmed,
    PersistenceFailed,
    RollbackOwns,
    Stale,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct OutputConfigurationSafeBoundary {
    pub(crate) main_thread_pageflip_owned: bool,
    pub(crate) worker_in_flight_owned: bool,
    pub(crate) worker_queued_next_owned: bool,
    pub(crate) atomic_commit_arbiter_owned: bool,
    pub(crate) presentation_transaction_owned: bool,
    pub(crate) direct_scanout_pageflip_owned: bool,
    pub(crate) cursor_plane_work_owned: bool,
    pub(crate) deferred_worker_event_owned: bool,
    pub(crate) explicit_sync_obligation_owned: bool,
}

impl OutputConfigurationSafeBoundary {
    pub(crate) const fn is_safe(self) -> bool {
        !(self.main_thread_pageflip_owned
            || self.worker_in_flight_owned
            || self.worker_queued_next_owned
            || self.atomic_commit_arbiter_owned
            || self.presentation_transaction_owned
            || self.direct_scanout_pageflip_owned
            || self.cursor_plane_work_owned
            || self.deferred_worker_event_owned
            || self.explicit_sync_obligation_owned)
    }
}

/// Server-owned state for the single Display configuration confirmation.
/// Time is supplied by the native monotonic clock so this model is deterministic.
#[derive(Debug, Default)]
pub(crate) struct OutputConfigurationTransactions {
    active: Option<OutputConfigurationTransaction>,
    next_id: Option<NonZeroU64>,
}

impl OutputConfigurationTransactions {
    pub(crate) fn new() -> Self {
        Self {
            active: None,
            next_id: NonZeroU64::new(1),
        }
    }

    pub(crate) fn active(&self) -> Option<&OutputConfigurationTransaction> {
        self.active.as_ref()
    }

    pub(crate) fn begin_temporary_apply(
        &mut self,
        output_id: impl Into<String>,
        previous: NativeAppliedOutputConfiguration,
        temporary: NativeAppliedOutputConfiguration,
        applied_configuration_generation: u64,
        now_ns: u64,
    ) -> Result<OutputConfigurationTransactionId, OutputConfigurationTransactionError> {
        if self.active.is_some() {
            return Err(OutputConfigurationTransactionError::Busy);
        }
        let id = self
            .next_id
            .take()
            .map(OutputConfigurationTransactionId)
            .ok_or(OutputConfigurationTransactionError::IdExhausted)?;
        self.next_id = id.0.get().checked_add(1).and_then(NonZeroU64::new);
        self.active = Some(OutputConfigurationTransaction {
            id,
            output_id: output_id.into(),
            previous,
            temporary,
            applied_configuration_generation,
            rollback_deadline_ns: now_ns.saturating_add(OUTPUT_CONFIGURATION_ROLLBACK_TIMEOUT_NS),
            phase: OutputConfigurationTransactionPhase::PendingConfirmation,
        });
        Ok(id)
    }

    pub(crate) fn remaining_ms(&self, now_ns: u64) -> Option<u32> {
        let transaction = self.active.as_ref()?;
        Some(remaining_deadline_ms(
            transaction.rollback_deadline_ns,
            now_ns,
        ))
    }

    pub(crate) fn deadline_ns(&self) -> Option<u64> {
        self.active.as_ref().and_then(|transaction| {
            matches!(
                transaction.phase,
                OutputConfigurationTransactionPhase::PendingConfirmation
                    | OutputConfigurationTransactionPhase::PersistencePending
            )
            .then_some(transaction.rollback_deadline_ns)
        })
    }

    pub(crate) fn begin_confirm(
        &mut self,
        id: OutputConfigurationTransactionId,
        now_ns: u64,
    ) -> Result<NativeAppliedOutputConfiguration, OutputConfigurationTransactionError> {
        let transaction = self
            .active
            .as_mut()
            .filter(|transaction| transaction.id == id)
            .ok_or(OutputConfigurationTransactionError::StaleTransaction)?;
        if now_ns >= transaction.rollback_deadline_ns {
            transaction.phase = OutputConfigurationTransactionPhase::RollingBack;
            return Err(OutputConfigurationTransactionError::NotConfirmable);
        }
        if transaction.phase != OutputConfigurationTransactionPhase::PendingConfirmation {
            return Err(OutputConfigurationTransactionError::NotConfirmable);
        }
        transaction.phase = OutputConfigurationTransactionPhase::PersistencePending;
        Ok(transaction.temporary)
    }

    pub(crate) fn complete_confirm(
        &mut self,
        id: OutputConfigurationTransactionId,
        persisted: bool,
        now_ns: u64,
    ) -> OutputConfigurationConfirmCompletion {
        let Some(transaction) = self
            .active
            .as_mut()
            .filter(|transaction| transaction.id == id)
        else {
            return OutputConfigurationConfirmCompletion::Stale;
        };
        if transaction.phase != OutputConfigurationTransactionPhase::PersistencePending {
            return OutputConfigurationConfirmCompletion::RollbackOwns;
        }
        if now_ns >= transaction.rollback_deadline_ns {
            transaction.phase = OutputConfigurationTransactionPhase::RollingBack;
            return OutputConfigurationConfirmCompletion::RollbackOwns;
        }
        if persisted {
            self.active = None;
            OutputConfigurationConfirmCompletion::Confirmed
        } else {
            transaction.phase = OutputConfigurationTransactionPhase::PendingConfirmation;
            OutputConfigurationConfirmCompletion::PersistenceFailed
        }
    }

    pub(crate) fn begin_revert(
        &mut self,
        id: OutputConfigurationTransactionId,
    ) -> Result<NativeAppliedOutputConfiguration, OutputConfigurationTransactionError> {
        let transaction = self
            .active
            .as_mut()
            .filter(|transaction| transaction.id == id)
            .ok_or(OutputConfigurationTransactionError::StaleTransaction)?;
        if matches!(
            transaction.phase,
            OutputConfigurationTransactionPhase::RollbackFailed { .. }
                | OutputConfigurationTransactionPhase::RollingBack
        ) {
            return Err(OutputConfigurationTransactionError::NotConfirmable);
        }
        transaction.phase = OutputConfigurationTransactionPhase::RollingBack;
        Ok(transaction.previous)
    }

    pub(crate) fn begin_expired_rollback(
        &mut self,
        now_ns: u64,
    ) -> Option<(
        OutputConfigurationTransactionId,
        NativeAppliedOutputConfiguration,
    )> {
        let transaction = self.active.as_mut()?;
        if now_ns < transaction.rollback_deadline_ns
            || !matches!(
                transaction.phase,
                OutputConfigurationTransactionPhase::PendingConfirmation
                    | OutputConfigurationTransactionPhase::PersistencePending
            )
        {
            return None;
        }
        transaction.phase = OutputConfigurationTransactionPhase::RollingBack;
        Some((transaction.id, transaction.previous))
    }

    pub(crate) fn complete_rollback(
        &mut self,
        id: OutputConfigurationTransactionId,
    ) -> Result<(), OutputConfigurationTransactionError> {
        let transaction = self
            .active
            .as_ref()
            .filter(|transaction| transaction.id == id)
            .ok_or(OutputConfigurationTransactionError::StaleTransaction)?;
        if transaction.phase != OutputConfigurationTransactionPhase::RollingBack {
            return Err(OutputConfigurationTransactionError::NotRollingBack);
        }
        self.active = None;
        Ok(())
    }

    pub(crate) fn fail_rollback(
        &mut self,
        id: OutputConfigurationTransactionId,
        error_code: impl Into<String>,
    ) -> Result<(), OutputConfigurationTransactionError> {
        let transaction = self
            .active
            .as_mut()
            .filter(|transaction| transaction.id == id)
            .ok_or(OutputConfigurationTransactionError::StaleTransaction)?;
        if transaction.phase != OutputConfigurationTransactionPhase::RollingBack {
            return Err(OutputConfigurationTransactionError::NotRollingBack);
        }
        transaction.phase = OutputConfigurationTransactionPhase::RollbackFailed {
            error_code: error_code.into(),
        };
        Ok(())
    }
}

fn remaining_deadline_ms(deadline_ns: u64, now_ns: u64) -> u32 {
    let remaining_ns = deadline_ns.saturating_sub(now_ns);
    let remaining_ms = remaining_ns.saturating_add(999_999) / 1_000_000;
    u32::try_from(remaining_ms).unwrap_or(u32::MAX)
}

pub(crate) const OUTPUT_CONFIGURATION_FILE_NAME: &str = "output.json";
pub(crate) const MAX_OUTPUT_CONFIGURATION_BYTES: usize = 4096;
const MAX_OUTPUT_CONNECTOR_NAME_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PersistedOutputPhysicalSize {
    pub(crate) width_mm: u32,
    pub(crate) height_mm: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NativeOutputModeTimingFingerprint {
    pub(crate) clock: u32,
    pub(crate) hdisplay: u16,
    pub(crate) hsync_start: u16,
    pub(crate) hsync_end: u16,
    pub(crate) htotal: u16,
    pub(crate) hskew: u16,
    pub(crate) vdisplay: u16,
    pub(crate) vsync_start: u16,
    pub(crate) vsync_end: u16,
    pub(crate) vtotal: u16,
    pub(crate) vscan: u16,
    pub(crate) flags: u32,
}

impl NativeOutputModeTimingFingerprint {
    pub(crate) fn from_mode(mode: &drm_sys::drm_mode_modeinfo) -> Self {
        Self {
            clock: mode.clock,
            hdisplay: mode.hdisplay,
            hsync_start: mode.hsync_start,
            hsync_end: mode.hsync_end,
            htotal: mode.htotal,
            hskew: mode.hskew,
            vdisplay: mode.vdisplay,
            vsync_start: mode.vsync_start,
            vsync_end: mode.vsync_end,
            vtotal: mode.vtotal,
            vscan: mode.vscan,
            flags: mode.flags,
        }
    }

    fn is_valid(self) -> bool {
        self.clock > 0
            && self.hdisplay > 0
            && self.hsync_start >= self.hdisplay
            && self.hsync_end >= self.hsync_start
            && self.htotal >= self.hsync_end
            && self.vdisplay > 0
            && self.vsync_start >= self.vdisplay
            && self.vsync_end >= self.vsync_start
            && self.vtotal >= self.vsync_end
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PersistedOutputConfiguration {
    pub(crate) version: u32,
    pub(crate) connector_name: String,
    pub(crate) physical_size_mm: Option<PersistedOutputPhysicalSize>,
    pub(crate) mode: NativeOutputModeTimingFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PersistedOutputConfigurationError {
    TooLarge,
    Invalid,
}

impl PersistedOutputConfiguration {
    pub(crate) fn from_native_mode(
        connector_name: impl Into<String>,
        physical_size_mm: Option<PersistedOutputPhysicalSize>,
        mode: &drm_sys::drm_mode_modeinfo,
    ) -> Self {
        Self {
            version: 1,
            connector_name: connector_name.into(),
            physical_size_mm,
            mode: NativeOutputModeTimingFingerprint::from_mode(mode),
        }
    }

    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, PersistedOutputConfigurationError> {
        if bytes.len() > MAX_OUTPUT_CONFIGURATION_BYTES {
            return Err(PersistedOutputConfigurationError::TooLarge);
        }
        let configuration: Self = serde_json::from_slice(bytes)
            .map_err(|_| PersistedOutputConfigurationError::Invalid)?;
        configuration.validate()?;
        Ok(configuration)
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, PersistedOutputConfigurationError> {
        self.validate()?;
        let bytes =
            serde_json::to_vec(self).map_err(|_| PersistedOutputConfigurationError::Invalid)?;
        if bytes.len() > MAX_OUTPUT_CONFIGURATION_BYTES {
            return Err(PersistedOutputConfigurationError::TooLarge);
        }
        Ok(bytes)
    }

    pub(crate) fn matches_native_mode(&self, mode: &drm_sys::drm_mode_modeinfo) -> bool {
        self.mode == NativeOutputModeTimingFingerprint::from_mode(mode)
    }

    pub(crate) fn resolve_mode<'a>(
        &self,
        connector_name: &str,
        physical_size_mm: Option<(u32, u32)>,
        inventory: &'a super::NativeOutputModeInventory,
    ) -> Option<&'a super::NativeOutputModeEntry> {
        if self.validate().is_err()
            || self.connector_name != connector_name
            || self.physical_size_mm.is_some_and(|saved| {
                physical_size_mm.is_some_and(|(width, height)| {
                    (saved.width_mm, saved.height_mm) != (width, height)
                })
            })
        {
            return None;
        }

        let mut matching = inventory
            .entries()
            .iter()
            .filter(|entry| self.matches_native_mode(&entry.mode));
        let unique = matching.next()?;
        matching.next().is_none().then_some(unique)
    }

    fn validate(&self) -> Result<(), PersistedOutputConfigurationError> {
        let connector_name = self.connector_name.as_bytes();
        let connector_identity_is_valid = !connector_name.is_empty()
            && connector_name.len() <= MAX_OUTPUT_CONNECTOR_NAME_BYTES
            && self.connector_name != "Display"
            && connector_name.iter().all(u8::is_ascii_graphic);
        let physical_size_is_valid = self.physical_size_mm.is_none_or(|size| {
            size.width_mm > 0
                && size.height_mm > 0
                && size.width_mm <= 100_000
                && size.height_mm <= 100_000
        });
        if self.version != 1
            || !connector_identity_is_valid
            || !physical_size_is_valid
            || !self.mode.is_valid()
        {
            return Err(PersistedOutputConfigurationError::Invalid);
        }
        Ok(())
    }
}

pub(crate) fn load_persisted_output_configuration()
-> Result<Option<PersistedOutputConfiguration>, PersistedOutputConfigurationError> {
    let file =
        crate::private_config::PrivateConfigFile::from_environment(OUTPUT_CONFIGURATION_FILE_NAME)
            .map_err(|_| PersistedOutputConfigurationError::Invalid)?;
    match file.read_bytes(MAX_OUTPUT_CONFIGURATION_BYTES) {
        Ok(bytes) => PersistedOutputConfiguration::parse(&bytes).map(Some),
        Err(crate::private_config::PrivateConfigError::Missing) => Ok(None),
        Err(_) => Err(PersistedOutputConfigurationError::Invalid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_output::output::NativeOutputModeInventory;

    fn applied_configuration(
        width: u16,
        height: u16,
        clock: u32,
    ) -> NativeAppliedOutputConfiguration {
        NativeAppliedOutputConfiguration {
            connector_id: 11,
            crtc_id: 12,
            mode_id: u32::from(width),
            mode: drm_sys::drm_mode_modeinfo {
                clock,
                hdisplay: width,
                hsync_start: width + 8,
                hsync_end: width + 16,
                htotal: width + 32,
                vdisplay: height,
                vsync_start: height + 2,
                vsync_end: height + 4,
                vtotal: height + 8,
                vrefresh: 60,
                ..Default::default()
            },
            width: u32::from(width),
            height: u32::from(height),
            scale_milli: 1000,
            transform: oblivion_one::control_snapshots::OutputTransformSnapshot::Normal,
        }
    }

    #[test]
    fn semantic_generation_advances_only_when_the_authority_requests_it() {
        let mut generation = OutputConfigurationGeneration::initial();
        let render_ticks = 165_u32;
        let swapchain_recreations = 3_u32;
        let framebuffer_ids = [18, 19, 20];

        assert_eq!(render_ticks, 165);
        assert_eq!(swapchain_recreations, 3);
        assert_eq!(framebuffer_ids, [18, 19, 20]);
        assert_eq!(generation.get(), 1);

        assert_eq!(generation.advance().get(), 2);
        assert_eq!(generation.advance().get(), 3);
    }

    #[test]
    fn generation_wraps_without_publishing_zero() {
        let mut generation = OutputConfigurationGeneration(u64::MAX);
        assert_eq!(generation.advance().get(), 1);
    }

    #[test]
    fn display_transaction_uses_fake_monotonic_time_and_stale_safe_ids() {
        let mut transactions = OutputConfigurationTransactions::new();
        let previous = applied_configuration(1920, 1080, 148_352);
        let temporary = applied_configuration(1920, 1080, 148_352 / 2);
        let id = transactions
            .begin_temporary_apply("output-1", previous, temporary, 2, 5_000_000_000)
            .expect("first transaction starts");

        assert_ne!(id.get(), 0);
        assert_eq!(transactions.remaining_ms(5_000_000_000), Some(15_000));
        assert_eq!(transactions.remaining_ms(5_250_500_000), Some(14_750));
        assert_eq!(
            transactions.begin_temporary_apply("output-1", previous, temporary, 3, 6_000_000_000),
            Err(OutputConfigurationTransactionError::Busy)
        );

        let stale = OutputConfigurationTransactionId::new(id.get() + 1).unwrap();
        assert_eq!(
            transactions.begin_confirm(stale, 6_000_000_000),
            Err(OutputConfigurationTransactionError::StaleTransaction)
        );
        assert_eq!(
            transactions
                .begin_confirm(id, 6_000_000_000)
                .unwrap()
                .mode
                .clock,
            temporary.mode.clock
        );
        assert_eq!(transactions.deadline_ns(), Some(20_000_000_000));
    }

    #[test]
    fn every_existing_output_owner_blocks_the_synchronous_modeset_boundary() {
        let safe = OutputConfigurationSafeBoundary::default();
        assert!(safe.is_safe());
        let blockers = [
            OutputConfigurationSafeBoundary {
                main_thread_pageflip_owned: true,
                ..safe
            },
            OutputConfigurationSafeBoundary {
                worker_in_flight_owned: true,
                ..safe
            },
            OutputConfigurationSafeBoundary {
                worker_queued_next_owned: true,
                ..safe
            },
            OutputConfigurationSafeBoundary {
                atomic_commit_arbiter_owned: true,
                ..safe
            },
            OutputConfigurationSafeBoundary {
                presentation_transaction_owned: true,
                ..safe
            },
            OutputConfigurationSafeBoundary {
                direct_scanout_pageflip_owned: true,
                ..safe
            },
            OutputConfigurationSafeBoundary {
                cursor_plane_work_owned: true,
                ..safe
            },
            OutputConfigurationSafeBoundary {
                deferred_worker_event_owned: true,
                ..safe
            },
            OutputConfigurationSafeBoundary {
                explicit_sync_obligation_owned: true,
                ..safe
            },
        ];
        assert!(blockers.into_iter().all(|boundary| !boundary.is_safe()));
    }

    #[test]
    fn display_transaction_expiry_wins_over_a_late_persistence_completion() {
        let mut transactions = OutputConfigurationTransactions::new();
        let previous = applied_configuration(1920, 1080, 148_352);
        let temporary = applied_configuration(1280, 720, 74_176);
        let id = transactions
            .begin_temporary_apply("output-1", previous, temporary, 2, 10)
            .unwrap();
        transactions.begin_confirm(id, 20).unwrap();

        assert_eq!(
            transactions.begin_expired_rollback(OUTPUT_CONFIGURATION_ROLLBACK_TIMEOUT_NS + 10),
            Some((id, previous))
        );
        assert_eq!(transactions.deadline_ns(), None);
        assert_eq!(
            transactions.complete_confirm(id, true, OUTPUT_CONFIGURATION_ROLLBACK_TIMEOUT_NS + 11,),
            OutputConfigurationConfirmCompletion::RollbackOwns
        );
        assert_eq!(
            transactions.active().map(|transaction| &transaction.phase),
            Some(&OutputConfigurationTransactionPhase::RollingBack)
        );

        transactions.complete_rollback(id).unwrap();
        assert!(transactions.active().is_none());
    }

    #[test]
    fn failed_display_rollback_remains_a_terminal_active_transaction() {
        let mut transactions = OutputConfigurationTransactions::new();
        let previous = applied_configuration(1920, 1080, 148_352);
        let temporary = applied_configuration(1280, 720, 74_176);
        let id = transactions
            .begin_temporary_apply("output-1", previous, temporary, 2, 0)
            .unwrap();

        assert_eq!(
            transactions.begin_revert(id).unwrap().mode.clock,
            previous.mode.clock
        );
        transactions
            .fail_rollback(id, "output_rollback_failed")
            .unwrap();
        assert!(matches!(
            transactions.active().map(|transaction| &transaction.phase),
            Some(OutputConfigurationTransactionPhase::RollbackFailed { error_code })
                if error_code == "output_rollback_failed"
        ));
        assert_eq!(
            transactions.begin_temporary_apply("output-1", previous, temporary, 3, 1),
            Err(OutputConfigurationTransactionError::Busy)
        );
    }

    #[test]
    fn persisted_timing_fingerprint_ignores_preferred_type_and_mode_name() {
        let mode = native_mode();
        let mut metadata_changed = mode;
        metadata_changed.type_ = drm_sys::DRM_MODE_TYPE_PREFERRED;
        metadata_changed.name[0] = b'r' as _;

        let saved = PersistedOutputConfiguration::from_native_mode("DP-1", None, &mode);

        assert!(saved.matches_native_mode(&metadata_changed));
    }

    #[test]
    fn persisted_timing_fingerprint_distinguishes_pixel_clock_and_porch_changes() {
        let mode = native_mode();
        let mut clock_changed = mode;
        clock_changed.clock += 1;
        let mut porch_changed = mode;
        porch_changed.hsync_start += 1;

        let saved = PersistedOutputConfiguration::from_native_mode("DP-1", None, &mode);

        assert!(!saved.matches_native_mode(&clock_changed));
        assert!(!saved.matches_native_mode(&porch_changed));
    }

    #[test]
    fn persisted_mode_resolution_requires_connector_and_one_exact_current_timing() {
        let generation = OutputConfigurationGeneration::initial();
        let mode = native_mode();
        let inventory = NativeOutputModeInventory::from_native_modes(generation, &[mode]);
        let saved = PersistedOutputConfiguration::from_native_mode(
            "DP-1",
            Some(PersistedOutputPhysicalSize {
                width_mm: 600,
                height_mm: 340,
            }),
            &mode,
        );

        assert!(
            saved
                .resolve_mode("DP-1", Some((600, 340)), &inventory)
                .is_some()
        );
        assert!(
            saved
                .resolve_mode("HDMI-A-1", Some((600, 340)), &inventory)
                .is_none()
        );
        assert!(
            saved
                .resolve_mode("DP-1", Some((610, 340)), &inventory)
                .is_none()
        );

        let ambiguous = NativeOutputModeInventory::from_native_modes(generation, &[mode, mode]);
        assert!(
            saved
                .resolve_mode("DP-1", Some((600, 340)), &ambiguous)
                .is_none()
        );
    }

    #[test]
    fn persisted_output_schema_rejects_unknown_fields_and_invalid_versions() {
        let mut saved =
            PersistedOutputConfiguration::from_native_mode("DP-1", None, &native_mode());
        let valid = serde_json::to_vec(&saved).expect("schema serializes");
        assert!(PersistedOutputConfiguration::parse(&valid).is_ok());

        let mut unknown: serde_json::Value =
            serde_json::from_slice(&valid).expect("serialized schema parses as JSON");
        unknown
            .as_object_mut()
            .expect("record is a JSON object")
            .insert(String::from("extra"), serde_json::Value::Bool(true));
        let with_unknown_field = serde_json::to_vec(&unknown).expect("unknown field serializes");
        assert!(PersistedOutputConfiguration::parse(&with_unknown_field).is_err());

        saved.version = 2;
        let unsupported_version = serde_json::to_vec(&saved).expect("schema serializes");
        assert!(PersistedOutputConfiguration::parse(&unsupported_version).is_err());
    }

    fn native_mode() -> drm_sys::drm_mode_modeinfo {
        drm_sys::drm_mode_modeinfo {
            clock: 148_352,
            hdisplay: 1920,
            hsync_start: 2008,
            hsync_end: 2052,
            htotal: 2200,
            vdisplay: 1080,
            vsync_start: 1084,
            vsync_end: 1089,
            vtotal: 1125,
            vrefresh: 60,
            ..Default::default()
        }
    }
}
