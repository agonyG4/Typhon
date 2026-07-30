#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeDirectScanoutPreference {
    Off,
    ExperimentalAuto,
}

impl NativeDirectScanoutPreference {
    pub(crate) fn from_env() -> Self {
        let value = std::env::var("OBLIVION_ONE_DIRECT_SCANOUT").ok();
        let preference = Self::from_value(value.as_deref());
        if value.as_deref() == Some("auto") {
            eprintln!(
                "native scanout: OBLIVION_ONE_DIRECT_SCANOUT=auto is deprecated; using experimental-auto"
            );
        } else if value.is_some() && preference == Self::Off && value.as_deref() != Some("off") {
            eprintln!("native scanout: unknown OBLIVION_ONE_DIRECT_SCANOUT={value:?}; using off");
        }
        eprintln!(
            "native scanout: direct_scanout_policy={} qualification=not_qualified",
            preference.as_str()
        );
        preference
    }

    pub(crate) fn from_value(value: Option<&str>) -> Self {
        match value {
            None | Some("off") => Self::Off,
            Some("experimental-auto" | "auto") => Self::ExperimentalAuto,
            Some(_) => Self::Off,
        }
    }

    #[cfg(test)]
    pub(crate) fn parse(value: &str) -> Self {
        Self::from_value(Some(value))
    }

    pub(crate) const fn enabled(self) -> bool {
        matches!(self, Self::ExperimentalAuto)
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::ExperimentalAuto => "experimental-auto",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_scanout_defaults_to_off_until_qualified() {
        assert_eq!(
            NativeDirectScanoutPreference::from_value(None),
            NativeDirectScanoutPreference::Off
        );
        assert_eq!(
            NativeDirectScanoutPreference::from_value(Some("experimental-auto")),
            NativeDirectScanoutPreference::ExperimentalAuto
        );
    }

    #[test]
    fn compatibility_auto_alias_enables_only_experimental_mode() {
        assert_eq!(
            NativeDirectScanoutPreference::parse("auto"),
            NativeDirectScanoutPreference::ExperimentalAuto
        );
        assert_eq!(
            NativeDirectScanoutPreference::parse("off"),
            NativeDirectScanoutPreference::Off
        );
        assert_eq!(
            NativeDirectScanoutPreference::parse("force"),
            NativeDirectScanoutPreference::Off
        );
        assert!(NativeDirectScanoutPreference::ExperimentalAuto.enabled());
        assert!(!NativeDirectScanoutPreference::Off.enabled());
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
