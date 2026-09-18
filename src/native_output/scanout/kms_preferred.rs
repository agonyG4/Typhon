use std::collections::{BTreeMap, BTreeSet};

use oblivion_one::compositor::DmabufKmsPreferredState;
use oblivion_one::native::kms::DrmFormatModifierPair;
use oblivion_one::render_backend::egl_gles::EglGlesDmabufFormat;

const KMS_PREFERRED_ENV: &str = "OBLIVION_ONE_DMABUF_KMS_PREFERRED";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsPreferredDmabufPolicy {
    Auto,
    Off,
    Force,
}

impl KmsPreferredDmabufPolicy {
    pub(crate) fn from_env() -> Self {
        let value = std::env::var(KMS_PREFERRED_ENV).ok();
        let policy = value.as_deref().map(Self::parse).unwrap_or(Self::Auto);
        if value
            .as_deref()
            .is_some_and(|value| !Self::is_known_value(value))
        {
            eprintln!("native GPU protocol: unknown {KMS_PREFERRED_ENV}={value:?}; using off");
        }
        policy
    }

    pub(crate) fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Self::Auto,
            "off" => Self::Off,
            "force" => Self::Force,
            _ => Self::Off,
        }
    }

    pub(crate) fn is_known_value(value: &str) -> bool {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "auto" | "off" | "force"
        )
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
            Self::Force => "force",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KmsPreferredDmabufSelection {
    pub(crate) advertised_renderer_formats: Vec<EglGlesDmabufFormat>,
    pub(crate) requested: KmsPreferredDmabufPolicy,
    pub(crate) effective: bool,
    pub(crate) reason: &'static str,
    pub(crate) renderer_pairs_raw: usize,
    pub(crate) kms_presentable_pairs: usize,
    pub(crate) renderer_pairs_advertised: usize,
    pub(crate) renderer_pairs_removed: usize,
}

impl KmsPreferredDmabufSelection {
    pub(crate) fn summary(&self) -> DmabufKmsPreferredState {
        DmabufKmsPreferredState {
            requested: self.requested.as_str(),
            effective: self.effective,
            renderer_pairs_raw: self.renderer_pairs_raw,
            kms_presentable_pairs: self.kms_presentable_pairs,
            renderer_pairs_advertised: self.renderer_pairs_advertised,
            renderer_pairs_removed: self.renderer_pairs_removed,
            reason: self.reason,
        }
    }
}

pub(crate) fn discover_kms_presentable_client_formats(
    plane_formats: &[DrmFormatModifierPair],
    renderer_formats: &[EglGlesDmabufFormat],
) -> Vec<DrmFormatModifierPair> {
    let renderer_pairs = renderer_formats
        .iter()
        .map(|format| (format.format.as_fourcc(), format.modifier.0))
        .collect::<BTreeSet<_>>();
    let mut pairs = plane_formats
        .iter()
        .map(|format| (format.fourcc, format.modifier))
        .filter(|pair| renderer_pairs.contains(pair))
        .collect::<Vec<_>>();
    pairs.sort_unstable();
    pairs.dedup();
    pairs
        .into_iter()
        .map(|(fourcc, modifier)| DrmFormatModifierPair { fourcc, modifier })
        .collect()
}

pub(crate) fn resolve_kms_preferred_dmabuf_policy(
    requested: KmsPreferredDmabufPolicy,
    egl_vendor: &str,
    raw_renderer_formats: &[EglGlesDmabufFormat],
    kms_presentable_formats: &[DrmFormatModifierPair],
) -> KmsPreferredDmabufSelection {
    let raw_renderer_formats_input = raw_renderer_formats.to_vec();
    let raw_renderer_formats = normalize_renderer_formats(raw_renderer_formats);
    let kms_presentable_formats = normalize_kms_presentable_formats(kms_presentable_formats);
    let candidate = select_renderer_formats(&raw_renderer_formats, &kms_presentable_formats);
    let candidate_changed = candidate != raw_renderer_formats;
    let nvidia_vendor = egl_vendor.to_ascii_lowercase().contains("nvidia");
    let policy_allows_activation = match requested {
        KmsPreferredDmabufPolicy::Off => false,
        KmsPreferredDmabufPolicy::Auto => nvidia_vendor,
        KmsPreferredDmabufPolicy::Force => true,
    };
    let effective = candidate_changed && policy_allows_activation;
    let advertised_renderer_formats = if effective {
        candidate
    } else {
        raw_renderer_formats_input
    };
    let reason = if requested == KmsPreferredDmabufPolicy::Off {
        "policy off"
    } else if requested == KmsPreferredDmabufPolicy::Auto && !nvidia_vendor {
        "active EGL vendor is not NVIDIA"
    } else if !candidate_changed {
        "no same-FOURCC KMS-compatible modifier would be replaced"
    } else {
        "same-FOURCC KMS-compatible modifiers selected"
    };

    KmsPreferredDmabufSelection {
        renderer_pairs_raw: raw_renderer_formats.len(),
        kms_presentable_pairs: kms_presentable_formats.len(),
        renderer_pairs_advertised: advertised_renderer_formats.len(),
        renderer_pairs_removed: raw_renderer_formats
            .len()
            .saturating_sub(advertised_renderer_formats.len()),
        advertised_renderer_formats,
        requested,
        effective,
        reason,
    }
}

fn normalize_renderer_formats(formats: &[EglGlesDmabufFormat]) -> Vec<EglGlesDmabufFormat> {
    let mut formats = formats.to_vec();
    formats.sort_unstable_by_key(|format| (format.format.as_fourcc(), format.modifier.0));
    formats.dedup_by_key(|format| (format.format.as_fourcc(), format.modifier.0));
    formats
}

fn normalize_kms_presentable_formats(formats: &[DrmFormatModifierPair]) -> Vec<(u32, u64)> {
    let mut formats = formats
        .iter()
        .map(|format| (format.fourcc, format.modifier))
        .collect::<Vec<_>>();
    formats.sort_unstable();
    formats.dedup();
    formats
}

fn select_renderer_formats(
    renderer_formats: &[EglGlesDmabufFormat],
    kms_presentable_formats: &[(u32, u64)],
) -> Vec<EglGlesDmabufFormat> {
    let mut kms_by_fourcc = BTreeMap::<u32, BTreeSet<u64>>::new();
    for &(fourcc, modifier) in kms_presentable_formats {
        kms_by_fourcc.entry(fourcc).or_default().insert(modifier);
    }

    let mut renderer_by_fourcc = BTreeMap::<u32, BTreeSet<u64>>::new();
    for format in renderer_formats {
        renderer_by_fourcc
            .entry(format.format.as_fourcc())
            .or_default()
            .insert(format.modifier.0);
    }

    let mut selected = Vec::new();
    for format in renderer_formats {
        let fourcc = format.format.as_fourcc();
        let renderer_modifiers = renderer_by_fourcc
            .get(&fourcc)
            .expect("normalized renderer format must have a FOURCC entry");
        let Some(kms_modifiers) = kms_by_fourcc.get(&fourcc) else {
            selected.push(*format);
            continue;
        };
        let has_intersection = renderer_modifiers
            .iter()
            .any(|modifier| kms_modifiers.contains(modifier));
        if !has_intersection || kms_modifiers.contains(&format.modifier.0) {
            selected.push(*format);
        }
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;
    use oblivion_one::render_backend::buffer::{DrmFormat, DrmModifier};

    fn renderer(format: DrmFormat, modifier: u64) -> EglGlesDmabufFormat {
        EglGlesDmabufFormat::new(format, DrmModifier(modifier))
    }

    fn plane(format: DrmFormat, modifier: u64) -> DrmFormatModifierPair {
        DrmFormatModifierPair {
            fourcc: format.as_fourcc(),
            modifier,
        }
    }

    #[test]
    fn kms_presentable_discovery_keeps_exact_renderer_plane_intersection() {
        let result = discover_kms_presentable_client_formats(
            &[
                plane(DrmFormat::Xbgr8888, 2),
                plane(DrmFormat::Argb8888, 4),
                plane(DrmFormat::Xbgr8888, 2),
                plane(DrmFormat::Xrgb8888, 8),
            ],
            &[
                renderer(DrmFormat::Xbgr8888, 2),
                renderer(DrmFormat::Xbgr8888, 3),
                renderer(DrmFormat::Argb8888, 4),
                renderer(DrmFormat::Argb8888, 5),
            ],
        );

        assert_eq!(
            result,
            vec![plane(DrmFormat::Xbgr8888, 2), plane(DrmFormat::Argb8888, 4)]
        );
    }

    #[test]
    fn kms_preferred_selector_replaces_unsafe_modifier_per_fourcc() {
        let raw = [
            renderer(DrmFormat::Xbgr8888, 2),
            renderer(DrmFormat::Xbgr8888, 3),
            renderer(DrmFormat::Argb8888, 7),
        ];
        let kms = [plane(DrmFormat::Xbgr8888, 2)];

        let selection = resolve_kms_preferred_dmabuf_policy(
            KmsPreferredDmabufPolicy::Force,
            "Mesa Project",
            &raw,
            &kms,
        );

        assert_eq!(
            selection.advertised_renderer_formats,
            vec![
                renderer(DrmFormat::Xbgr8888, 2),
                renderer(DrmFormat::Argb8888, 7),
            ]
        );
        assert!(selection.effective);
        assert_eq!(selection.renderer_pairs_raw, 3);
        assert_eq!(selection.kms_presentable_pairs, 1);
        assert_eq!(selection.renderer_pairs_advertised, 2);
        assert_eq!(selection.renderer_pairs_removed, 1);
    }

    #[test]
    fn kms_preferred_selector_preserves_multiple_safe_modifiers_and_isolates_fourccs() {
        let raw = [
            renderer(DrmFormat::Xbgr8888, 2),
            renderer(DrmFormat::Xbgr8888, 3),
            renderer(DrmFormat::Xbgr8888, 4),
            renderer(DrmFormat::Argb8888, 7),
            renderer(DrmFormat::Argb8888, 8),
        ];
        let kms = [
            plane(DrmFormat::Xbgr8888, 2),
            plane(DrmFormat::Xbgr8888, 4),
            plane(DrmFormat::Argb8888, 9),
        ];

        let selection = resolve_kms_preferred_dmabuf_policy(
            KmsPreferredDmabufPolicy::Force,
            "Mesa Project",
            &raw,
            &kms,
        );

        assert_eq!(
            selection.advertised_renderer_formats,
            vec![
                renderer(DrmFormat::Xbgr8888, 2),
                renderer(DrmFormat::Xbgr8888, 4),
                renderer(DrmFormat::Argb8888, 7),
                renderer(DrmFormat::Argb8888, 8),
            ]
        );
    }

    #[test]
    fn kms_preferred_selector_preserves_renderer_fourcc_without_kms_intersection() {
        let raw = [
            renderer(DrmFormat::Xbgr8888, 2),
            renderer(DrmFormat::Argb8888, 7),
        ];
        let kms = [plane(DrmFormat::Xbgr8888, 9)];

        let selection = resolve_kms_preferred_dmabuf_policy(
            KmsPreferredDmabufPolicy::Force,
            "Mesa Project",
            &raw,
            &kms,
        );

        assert_eq!(selection.advertised_renderer_formats, raw.to_vec());
        assert!(!selection.effective);
        assert_eq!(selection.renderer_pairs_removed, 0);
    }

    #[test]
    fn kms_preferred_policy_auto_requires_nvidia_when_filtering_changes_pairs() {
        let raw = [
            renderer(DrmFormat::Xbgr8888, 2),
            renderer(DrmFormat::Xbgr8888, 3),
        ];
        let kms = [plane(DrmFormat::Xbgr8888, 2)];

        let selection = resolve_kms_preferred_dmabuf_policy(
            KmsPreferredDmabufPolicy::Auto,
            "Mesa Project",
            &raw,
            &kms,
        );

        assert_eq!(selection.advertised_renderer_formats, raw.to_vec());
        assert!(!selection.effective);
        assert_eq!(selection.renderer_pairs_removed, 0);
    }

    #[test]
    fn kms_preferred_policy_auto_activates_for_nvidia_when_filtering_changes_pairs() {
        let raw = [
            renderer(DrmFormat::Xbgr8888, 2),
            renderer(DrmFormat::Xbgr8888, 3),
        ];
        let kms = [plane(DrmFormat::Xbgr8888, 2)];

        let selection = resolve_kms_preferred_dmabuf_policy(
            KmsPreferredDmabufPolicy::Auto,
            "NVIDIA",
            &raw,
            &kms,
        );

        assert!(selection.effective);
        assert_eq!(selection.renderer_pairs_removed, 1);
    }

    #[test]
    fn kms_preferred_policy_auto_is_noop_when_no_pair_changes() {
        let raw = [renderer(DrmFormat::Xbgr8888, 2)];
        let kms = [plane(DrmFormat::Xbgr8888, 2)];

        let selection = resolve_kms_preferred_dmabuf_policy(
            KmsPreferredDmabufPolicy::Auto,
            "NVIDIA",
            &raw,
            &kms,
        );

        assert_eq!(selection.advertised_renderer_formats, raw.to_vec());
        assert!(!selection.effective);
        assert_eq!(selection.renderer_pairs_removed, 0);
    }

    #[test]
    fn kms_preferred_policy_off_preserves_raw_renderer_formats() {
        let raw = [
            renderer(DrmFormat::Xbgr8888, 3),
            renderer(DrmFormat::Xbgr8888, 2),
        ];
        let kms = [plane(DrmFormat::Xbgr8888, 2)];

        let selection = resolve_kms_preferred_dmabuf_policy(
            KmsPreferredDmabufPolicy::Off,
            "NVIDIA",
            &raw,
            &kms,
        );

        assert_eq!(selection.advertised_renderer_formats, raw.to_vec());
        assert!(!selection.effective);
        assert_eq!(selection.renderer_pairs_removed, 0);
    }

    #[test]
    fn kms_preferred_policy_modes_parse_safely() {
        assert_eq!(
            KmsPreferredDmabufPolicy::parse("auto"),
            KmsPreferredDmabufPolicy::Auto
        );
        assert_eq!(
            KmsPreferredDmabufPolicy::parse("OFF"),
            KmsPreferredDmabufPolicy::Off
        );
        assert_eq!(
            KmsPreferredDmabufPolicy::parse(" force "),
            KmsPreferredDmabufPolicy::Force
        );
        assert_eq!(
            KmsPreferredDmabufPolicy::parse("unexpected"),
            KmsPreferredDmabufPolicy::Off
        );
    }

    #[test]
    fn kms_preferred_policy_unknown_environment_value_falls_back_to_off() {
        let _guard = crate::native_output::ASTREA_ENV_LOCK.lock().unwrap();
        let previous = std::env::var_os(super::KMS_PREFERRED_ENV);
        // SAFETY: ASTREA_ENV_LOCK serializes environment mutation in native-output tests.
        unsafe { std::env::set_var(super::KMS_PREFERRED_ENV, "unknown") };

        assert_eq!(
            KmsPreferredDmabufPolicy::from_env(),
            KmsPreferredDmabufPolicy::Off
        );

        // SAFETY: ASTREA_ENV_LOCK still guards restoration of the process environment.
        unsafe {
            match previous {
                Some(value) => std::env::set_var(super::KMS_PREFERRED_ENV, value),
                None => std::env::remove_var(super::KMS_PREFERRED_ENV),
            }
        }
    }
}
