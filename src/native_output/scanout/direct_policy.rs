/// Product policy for attempting exact per-candidate Direct Scanout validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeDirectScanoutPreference {
    Off,
    Auto,
}

fn direct_scanout_deprecation_warning(value: Option<&str>) -> Option<&'static str> {
    match value {
        Some("experimental-auto") => {
            Some("OBLIVION_ONE_DIRECT_SCANOUT=experimental-auto is deprecated; use auto")
        }
        _ => None,
    }
}

impl Default for NativeDirectScanoutPreference {
    fn default() -> Self {
        Self::Auto
    }
}

impl NativeDirectScanoutPreference {
    pub(crate) fn from_env() -> Self {
        let value = std::env::var("OBLIVION_ONE_DIRECT_SCANOUT").ok();
        let preference = Self::from_value(value.as_deref());
        if let Some(warning) = direct_scanout_deprecation_warning(value.as_deref()) {
            eprintln!("native scanout: {warning}");
        } else if let Some(value) = value.as_deref()
            && value != "off"
            && value != "auto"
        {
            eprintln!("native scanout: unknown OBLIVION_ONE_DIRECT_SCANOUT={value:?}; using off");
        }
        eprintln!(
            "native scanout: direct_scanout_policy={}",
            preference.as_str()
        );
        preference
    }

    pub(crate) fn from_value(value: Option<&str>) -> Self {
        match value {
            None => Self::default(),
            Some("off") => Self::Off,
            Some("experimental-auto" | "auto") => Self::Auto,
            Some(_) => Self::Off,
        }
    }

    #[cfg(test)]
    pub(crate) fn parse(value: &str) -> Self {
        Self::from_value(Some(value))
    }

    pub(crate) const fn enabled(self) -> bool {
        matches!(self, Self::Auto)
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
        }
    }
}

pub(crate) fn direct_blocker(reason: &str) -> (&'static str, u64) {
    match reason {
        "acquire_not_ready" => ("acquire_not_ready", 1 << 0),
        "buffer_device_or_modifier_unproven" => ("buffer_device_or_modifier_unproven", 1 << 1),
        "atomic_backend_unavailable" => ("atomic_backend_unavailable", 1 << 2),
        "primary_in_fence_property_missing" => ("primary_in_fence_property_missing", 1 << 3),
        "candidate_plane_missing" => ("candidate_plane_missing", 1 << 4),
        "worker_unavailable" => ("worker_unavailable", 1 << 5),
        "import_failed" => ("import_failed", 1 << 6),
        "candidate_key_invalid" => ("candidate_key_invalid", 1 << 7),
        "test_only_rejected" => ("test_only_rejected", 1 << 8),
        "real_submit_rejected" => ("real_submit_rejected", 1 << 10),
        "primary_plane_format_modifier_unsupported" => {
            ("primary_plane_format_modifier_unsupported", 1 << 11)
        }
        _ => ("candidate_rejected", 1 << 9),
    }
}

pub(crate) fn direct_scanout_doctor_severity(
    enabled: bool,
    state: oblivion_one::control_snapshots::FeatureState,
) -> oblivion_one::control_snapshots::DoctorSeverity {
    use oblivion_one::control_snapshots::{DoctorSeverity, FeatureState};

    if !enabled {
        return DoctorSeverity::Ok;
    }
    match state {
        FeatureState::Configured => DoctorSeverity::Ok,
        FeatureState::Unavailable | FeatureState::Degraded => DoctorSeverity::Warning,
        FeatureState::Available | FeatureState::Active => DoctorSeverity::Ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_scanout_defaults_to_canonical_auto_and_unknown_values_stay_off() {
        assert_eq!(NativeDirectScanoutPreference::default().as_str(), "auto");
        assert_eq!(
            NativeDirectScanoutPreference::from_value(None).as_str(),
            "auto"
        );
        assert_eq!(
            NativeDirectScanoutPreference::from_value(Some("auto")).as_str(),
            "auto"
        );
        assert_eq!(
            NativeDirectScanoutPreference::parse("auto").as_str(),
            "auto"
        );
        assert_eq!(
            NativeDirectScanoutPreference::from_value(Some("experimental-auto")).as_str(),
            "auto"
        );
        assert_eq!(
            NativeDirectScanoutPreference::parse("experimental-auto").as_str(),
            "auto"
        );
        assert_eq!(
            NativeDirectScanoutPreference::from_value(Some("off")).as_str(),
            "off"
        );
        assert_eq!(
            NativeDirectScanoutPreference::from_value(Some("force")).as_str(),
            "off"
        );
        assert_eq!(
            NativeDirectScanoutPreference::from_value(Some("unknown")).as_str(),
            "off"
        );
        assert!(NativeDirectScanoutPreference::from_value(None).enabled());
        assert!(!NativeDirectScanoutPreference::from_value(Some("off")).enabled());
    }

    #[test]
    fn experimental_auto_warns_with_the_canonical_value_but_auto_does_not_warn() {
        assert_eq!(direct_scanout_deprecation_warning(Some("auto")), None);
        assert_eq!(
            direct_scanout_deprecation_warning(Some("experimental-auto")),
            Some("OBLIVION_ONE_DIRECT_SCANOUT=experimental-auto is deprecated; use auto")
        );
    }

    #[test]
    fn doctor_treats_unproven_auto_candidates_as_healthy() {
        use oblivion_one::control_snapshots::{DoctorSeverity, FeatureState};

        assert_eq!(
            direct_scanout_doctor_severity(false, FeatureState::Unavailable),
            DoctorSeverity::Ok
        );
        for state in [
            FeatureState::Configured,
            FeatureState::Available,
            FeatureState::Active,
        ] {
            assert_eq!(
                direct_scanout_doctor_severity(true, state),
                DoctorSeverity::Ok
            );
        }
        for state in [FeatureState::Unavailable, FeatureState::Degraded] {
            assert_eq!(
                direct_scanout_doctor_severity(true, state),
                DoctorSeverity::Warning
            );
        }
    }

    #[test]
    fn direct_blockers_have_stable_names_and_distinct_bits() {
        assert_eq!(
            direct_blocker("worker_unavailable"),
            ("worker_unavailable", 1 << 5)
        );
        assert_ne!(
            direct_blocker("test_only_rejected").1,
            direct_blocker("real_submit_rejected").1
        );
        assert_eq!(
            direct_blocker("primary_plane_format_modifier_unsupported"),
            ("primary_plane_format_modifier_unsupported", 1 << 11)
        );
    }
}
