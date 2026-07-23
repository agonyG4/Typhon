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
}
