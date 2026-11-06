//! Multi-head bootstrap plan: which connectors get a CRTC and where each
//! output sits in the logical layout.
//!
//! The primary head is chosen exactly as before (persisted configuration,
//! environment override or first usable connector) and keeps its CRTC. Every
//! other connected connector with a usable mode is matched to a distinct CRTC
//! from the rest by maximum bipartite matching over the encoders'
//! `possible_crtcs`, so one connector taking a shared CRTC cannot strand
//! another that has no alternative.

use super::*;

/// One connected connector that could drive a head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KmsHeadCandidate {
    pub(crate) connector_id: u32,
    /// CRTC currently bound through the connector's encoder, tried first so
    /// a head keeps the firmware/previous binding when it can.
    pub(crate) current_crtc: Option<u32>,
    /// CRTCs any of the connector's encoders can drive, in resource order.
    pub(crate) possible_crtcs: Vec<u32>,
}

/// Assigns a distinct CRTC to as many candidates as possible.
///
/// `reserved` CRTCs (the primary head's) are never handed out. Candidates are
/// matched in order; an earlier candidate keeps priority for its preferred
/// CRTC unless moving it lets a later one be driven too (augmenting path).
pub(crate) fn assign_head_crtcs(
    candidates: &[KmsHeadCandidate],
    reserved: &[u32],
) -> Vec<Option<u32>> {
    let preferences: Vec<Vec<u32>> = candidates
        .iter()
        .map(|candidate| {
            let mut order = Vec::with_capacity(candidate.possible_crtcs.len() + 1);
            if let Some(current) = candidate.current_crtc
                && candidate.possible_crtcs.contains(&current)
            {
                order.push(current);
            }
            for &crtc in &candidate.possible_crtcs {
                if !order.contains(&crtc) {
                    order.push(crtc);
                }
            }
            order.retain(|crtc| !reserved.contains(crtc));
            order
        })
        .collect();

    // owner[crtc] = candidate index currently holding it.
    let mut owner: HashMap<u32, usize> = HashMap::new();
    let mut assignment: Vec<Option<u32>> = vec![None; candidates.len()];
    for index in 0..candidates.len() {
        let mut visited = Vec::new();
        augment(
            index,
            &preferences,
            &mut owner,
            &mut assignment,
            &mut visited,
        );
    }
    assignment
}

fn augment(
    index: usize,
    preferences: &[Vec<u32>],
    owner: &mut HashMap<u32, usize>,
    assignment: &mut [Option<u32>],
    visited: &mut Vec<u32>,
) -> bool {
    for &crtc in &preferences[index] {
        if visited.contains(&crtc) {
            continue;
        }
        visited.push(crtc);
        let free = match owner.get(&crtc).copied() {
            None => true,
            Some(holder) => augment(holder, preferences, owner, assignment, visited),
        };
        if free {
            owner.insert(crtc, index);
            assignment[index] = Some(crtc);
            return true;
        }
    }
    false
}

/// Logical extent of one head for layout packing: mode pixels and scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HeadLayoutExtent {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) scale: f64,
}

impl HeadLayoutExtent {
    fn logical_width(self) -> i32 {
        let scale = if self.scale.is_finite() && self.scale > 0.0 {
            self.scale
        } else {
            1.0
        };
        (f64::from(self.width) / scale).round() as i32
    }
}

/// Packs heads left to right in order, top-aligned, starting at the origin.
pub(crate) fn pack_head_layout(extents: &[HeadLayoutExtent]) -> Vec<(i32, i32)> {
    let mut x = 0i32;
    extents
        .iter()
        .map(|extent| {
            let position = (x, 0);
            x = x.saturating_add(extent.logical_width());
            position
        })
        .collect()
}

const DRM_MODE_TYPE_PREFERRED: u32 = 1 << 3;

/// Secondary heads start at the sink's preferred resolution with the highest
/// refresh offered there. Largest-area Auto would put a TV advertising DCI
/// 4096x2160 into a mode it letterboxes or stretches.
pub(crate) fn select_secondary_head_mode(
    modes: &[drm_sys::drm_mode_modeinfo],
) -> Option<drm_sys::drm_mode_modeinfo> {
    let preferred = modes
        .iter()
        .find(|mode| mode.type_ & DRM_MODE_TYPE_PREFERRED != 0)
        .or_else(|| modes.first())?;
    select_exact_kms_mode(
        modes,
        u32::from(preferred.hdisplay),
        u32::from(preferred.vdisplay),
        None,
    )
    .or(Some(*preferred))
}

/// `TYPHON_SECONDARY_MODE=WxH` or `WxH@HZ` picks the secondary heads' mode
/// when the sink offers it, e.g. to stay inside a link the sink only accepts
/// at lower bandwidth. Unset, unparsable or unoffered falls back to the
/// preferred mode.
fn secondary_head_mode_override() -> Option<(u32, u32, Option<u32>)> {
    parse_secondary_head_mode(&std::env::var("TYPHON_SECONDARY_MODE").ok()?)
}

fn parse_secondary_head_mode(value: &str) -> Option<(u32, u32, Option<u32>)> {
    let (size, refresh) = match value.trim().split_once('@') {
        Some((size, refresh)) => (size, Some(refresh.trim().parse().ok()?)),
        None => (value.trim(), None),
    };
    let (width, height) = size.split_once('x')?;
    Some((
        width.trim().parse().ok()?,
        height.trim().parse().ok()?,
        refresh,
    ))
}

/// A secondary head ready to be brought up: target, capabilities and layout.
#[derive(Debug, Clone)]
pub(crate) struct NativeSecondaryHeadPlan {
    pub(crate) selection: NativeKmsTargetSelection,
    pub(crate) position: (i32, i32),
}

/// Connected connectors other than the primary that could not be given a head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeUnplannedConnector {
    pub(crate) connector_name: String,
    pub(crate) reason: &'static str,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct NativeHeadPlan {
    pub(crate) secondaries: Vec<NativeSecondaryHeadPlan>,
    pub(crate) unplanned: Vec<NativeUnplannedConnector>,
}

/// Plans every connected connector besides `primary` as a secondary head.
pub(crate) fn plan_secondary_heads(
    file: &fs::File,
    primary: &NativeAppliedOutputConfiguration,
) -> io::Result<NativeHeadPlan> {
    let mut crtcs = Vec::new();
    let mut connector_ids = Vec::new();
    drm_ffi::mode::get_resources(
        file.as_fd(),
        None,
        Some(&mut crtcs),
        Some(&mut connector_ids),
        None,
    )?;

    struct Probed {
        candidate: KmsHeadCandidate,
        connector: drm_sys::drm_mode_get_connector,
        modes: Vec<drm_sys::drm_mode_modeinfo>,
        mode: drm_sys::drm_mode_modeinfo,
    }

    let mut plan = NativeHeadPlan::default();
    let mut probed = Vec::new();
    for connector_id in connector_ids {
        if connector_id == primary.connector_id {
            continue;
        }
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
        let mode_override = secondary_head_mode_override();
        let Some(mode) = mode_override
            .and_then(|(width, height, refresh_hz)| {
                select_exact_kms_mode(&modes, width, height, refresh_hz)
            })
            .or_else(|| select_secondary_head_mode(&modes))
        else {
            plan.unplanned.push(NativeUnplannedConnector {
                connector_name: drm_connector_presentation_name(&connector),
                reason: "no_usable_mode",
            });
            continue;
        };
        let mut current_crtc = None;
        let mut possible_crtcs = Vec::new();
        let current_encoder = (connector.encoder_id != 0).then_some(connector.encoder_id);
        for encoder_id in current_encoder.into_iter().chain(encoder_ids) {
            let encoder = drm_ffi::mode::get_encoder(file.as_fd(), encoder_id)?;
            if Some(encoder_id) == current_encoder && encoder.crtc_id != 0 {
                current_crtc = Some(encoder.crtc_id);
            }
            for (index, &crtc) in crtcs.iter().enumerate() {
                if index < 32
                    && encoder.possible_crtcs & (1 << index) != 0
                    && !possible_crtcs.contains(&crtc)
                {
                    possible_crtcs.push(crtc);
                }
            }
        }
        probed.push(Probed {
            candidate: KmsHeadCandidate {
                connector_id,
                current_crtc,
                possible_crtcs,
            },
            connector,
            modes,
            mode,
        });
    }

    let candidates: Vec<KmsHeadCandidate> =
        probed.iter().map(|entry| entry.candidate.clone()).collect();
    let assignment = assign_head_crtcs(&candidates, &[primary.crtc_id]);

    let mut extents = vec![HeadLayoutExtent {
        width: primary.width,
        height: primary.height,
        scale: f64::from(primary.scale_milli) / 1000.0,
    }];
    let mut selections = Vec::new();
    for (entry, crtc) in probed.into_iter().zip(assignment) {
        let Some(crtc_id) = crtc else {
            plan.unplanned.push(NativeUnplannedConnector {
                connector_name: drm_connector_presentation_name(&entry.connector),
                reason: "no_free_crtc",
            });
            continue;
        };
        let mode_id = entry
            .modes
            .iter()
            .position(|candidate| native_mode_identity_eq(candidate, &entry.mode))
            .and_then(|index| u32::try_from(index + 1).ok())
            .ok_or_else(|| {
                io::Error::other("selected native mode is absent from its connector inventory")
            })?;
        let target = NativeAppliedOutputConfiguration {
            connector_id: entry.candidate.connector_id,
            crtc_id,
            mode_id,
            mode: entry.mode,
            width: u32::from(entry.mode.hdisplay),
            height: u32::from(entry.mode.vdisplay),
            scale_milli: 1000,
            transform: oblivion_one::control_snapshots::OutputTransformSnapshot::Normal,
            color_depth: OutputColorDepth::Bits8,
        };
        extents.push(HeadLayoutExtent {
            width: target.width,
            height: target.height,
            scale: 1.0,
        });
        selections.push(NativeKmsTargetSelection {
            target,
            capabilities: native_output_capabilities(&entry.connector, &entry.modes),
        });
    }

    let positions = pack_head_layout(&extents);
    plan.secondaries = selections
        .into_iter()
        .zip(positions.into_iter().skip(1))
        .map(|(selection, position)| NativeSecondaryHeadPlan {
            selection,
            position,
        })
        .collect();
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(connector_id: u32, current: Option<u32>, possible: &[u32]) -> KmsHeadCandidate {
        KmsHeadCandidate {
            connector_id,
            current_crtc: current,
            possible_crtcs: possible.to_vec(),
        }
    }

    #[test]
    fn single_candidate_keeps_its_current_crtc() {
        let assignment = assign_head_crtcs(&[candidate(1, Some(52), &[51, 52, 53])], &[]);
        assert_eq!(assignment, vec![Some(52)]);
    }

    #[test]
    fn without_a_current_binding_the_first_possible_crtc_is_used() {
        let assignment = assign_head_crtcs(&[candidate(1, None, &[51, 52])], &[]);
        assert_eq!(assignment, vec![Some(51)]);
    }

    #[test]
    fn reserved_primary_crtc_is_never_reused() {
        let assignment = assign_head_crtcs(&[candidate(2, Some(51), &[51, 52])], &[51]);
        assert_eq!(assignment, vec![Some(52)]);
    }

    #[test]
    fn augmenting_path_moves_a_flexible_head_to_free_a_constrained_one() {
        // Connector 1 could use either CRTC but grabs 51 first; connector 2
        // can only use 51. Maximum matching moves 1 to 52.
        let assignment = assign_head_crtcs(
            &[candidate(1, None, &[51, 52]), candidate(2, None, &[51])],
            &[],
        );
        assert_eq!(assignment, vec![Some(52), Some(51)]);
    }

    #[test]
    fn surplus_connectors_stay_unassigned() {
        let assignment = assign_head_crtcs(
            &[
                candidate(1, None, &[51]),
                candidate(2, None, &[51]),
                candidate(3, None, &[52]),
            ],
            &[],
        );
        assert_eq!(assignment, vec![Some(51), None, Some(52)]);
    }

    #[test]
    fn stale_current_crtc_outside_possible_set_is_ignored() {
        let assignment = assign_head_crtcs(&[candidate(1, Some(99), &[51])], &[]);
        assert_eq!(assignment, vec![Some(51)]);
    }

    fn mode(width: u16, height: u16, refresh: u32, preferred: bool) -> drm_sys::drm_mode_modeinfo {
        // SAFETY: drm_mode_modeinfo is a plain C struct; all-zero is valid.
        let mut mode: drm_sys::drm_mode_modeinfo = unsafe { std::mem::zeroed() };
        mode.hdisplay = width;
        mode.vdisplay = height;
        mode.vrefresh = refresh;
        if preferred {
            mode.type_ = DRM_MODE_TYPE_PREFERRED;
        }
        mode
    }

    #[test]
    fn secondary_mode_uses_preferred_resolution_at_its_highest_refresh() {
        let modes = [
            mode(3840, 2160, 30, true),
            mode(4096, 2160, 60, false),
            mode(3840, 2160, 60, false),
            mode(1920, 1080, 120, false),
        ];
        let selected = select_secondary_head_mode(&modes).expect("mode");
        assert_eq!(
            (selected.hdisplay, selected.vdisplay, selected.vrefresh),
            (3840, 2160, 60)
        );
    }

    #[test]
    fn secondary_mode_without_preferred_flag_follows_first_mode() {
        let modes = [mode(2560, 1440, 144, false), mode(3840, 2160, 60, false)];
        let selected = select_secondary_head_mode(&modes).expect("mode");
        assert_eq!((selected.hdisplay, selected.vrefresh), (2560, 144));
        assert!(select_secondary_head_mode(&[]).is_none());
        assert_eq!(
            parse_secondary_head_mode("1920x1080@60"),
            Some((1920, 1080, Some(60)))
        );
        assert_eq!(
            parse_secondary_head_mode("3840x2160"),
            Some((3840, 2160, None))
        );
        assert_eq!(parse_secondary_head_mode("4k"), None);
    }

    #[test]
    fn layout_packs_left_to_right_in_logical_pixels() {
        let positions = pack_head_layout(&[
            HeadLayoutExtent {
                width: 1920,
                height: 1080,
                scale: 1.0,
            },
            HeadLayoutExtent {
                width: 3840,
                height: 2160,
                scale: 2.0,
            },
            HeadLayoutExtent {
                width: 1280,
                height: 1024,
                scale: 1.0,
            },
        ]);
        assert_eq!(positions, vec![(0, 0), (1920, 0), (3840, 0)]);
    }

    #[test]
    fn invalid_scale_packs_as_unscaled() {
        let positions = pack_head_layout(&[
            HeadLayoutExtent {
                width: 100,
                height: 100,
                scale: 0.0,
            },
            HeadLayoutExtent {
                width: 50,
                height: 50,
                scale: f64::NAN,
            },
        ]);
        assert_eq!(positions, vec![(0, 0), (100, 0)]);
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Reads the real connector inventory without becoming DRM master.
    /// Run: cargo test --bin oblivion-one live_head_plan -- --ignored --nocapture
    #[test]
    #[ignore = "reads the host's DRM device"]
    fn live_head_plan() {
        let path = std::env::var("TYPHON_LIVE_CARD").unwrap_or_else(|_| "/dev/dri/card1".into());
        let file = fs::File::open(&path).expect("open DRM card");
        let primary = select_kms_target_with_capabilities(&file, NativeModePreference::Auto)
            .expect("primary target");
        println!(
            "primary {} connector {} crtc {} {}x{}",
            primary.capabilities.connector_name,
            primary.target.connector_id,
            primary.target.crtc_id,
            primary.target.width,
            primary.target.height,
        );
        let plan = plan_secondary_heads(&file, &primary.target).expect("plan");
        for head in &plan.secondaries {
            println!(
                "secondary {} connector {} crtc {} {}x{}@{} at {:?}",
                head.selection.capabilities.connector_name,
                head.selection.target.connector_id,
                head.selection.target.crtc_id,
                head.selection.target.width,
                head.selection.target.height,
                head.selection.target.mode.vrefresh,
                head.position,
            );
        }
        println!("unplanned {:?}", plan.unplanned);
    }
}
