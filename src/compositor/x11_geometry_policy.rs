use super::{OutputRect, ToplevelMode, X11DesktopRole, X11PlacementPolicy};
use crate::xwayland::xwm::{X11ConfigureFlags, X11Geometry};

pub(super) struct ManagedX11GeometryPolicy {
    pub role: Option<X11DesktopRole>,
    pub placement: Option<X11PlacementPolicy>,
    pub mode: ToplevelMode,
    pub tiled: bool,
    pub usable_area: OutputRect,
    pub frame_extents: [u32; 4],
}

pub(super) struct ManagedX11GeometryResult {
    pub accepted: X11Geometry,
    pub reasons: Vec<&'static str>,
}

pub(super) fn apply_managed_x11_geometry_policy(
    normalized: X11Geometry,
    current: X11Geometry,
    fields: X11ConfigureFlags,
    policy: &ManagedX11GeometryPolicy,
) -> ManagedX11GeometryResult {
    if policy.role != Some(X11DesktopRole::Toplevel)
        || policy.placement != Some(X11PlacementPolicy::CompositorManaged)
    {
        return ManagedX11GeometryResult {
            accepted: normalized,
            reasons: Vec::new(),
        };
    }
    if policy.mode != ToplevelMode::Normal {
        return ManagedX11GeometryResult {
            accepted: current,
            reasons: vec!["mode_authority"],
        };
    }
    if policy.tiled {
        return ManagedX11GeometryResult {
            accepted: current,
            reasons: vec!["layout_authority"],
        };
    }

    let (accepted, reasons) =
        constrain_managed_x11_request(normalized, fields, policy.usable_area, policy.frame_extents);
    ManagedX11GeometryResult { accepted, reasons }
}

fn constrain_managed_x11_request(
    mut geometry: X11Geometry,
    fields: X11ConfigureFlags,
    usable_area: OutputRect,
    extents: [u32; 4],
) -> (X11Geometry, Vec<&'static str>) {
    let usable_left = usable_area.x.ceil() as i64;
    let usable_top = usable_area.y.ceil() as i64;
    let usable_right = (usable_area.x + usable_area.width).floor() as i64;
    let usable_bottom = (usable_area.y + usable_area.height).floor() as i64;
    let [left, right, top, bottom] = extents.map(i64::from);
    let maximum_width = usable_right
        .saturating_sub(usable_left)
        .saturating_sub(left)
        .saturating_sub(right)
        .max(1);
    let maximum_height = usable_bottom
        .saturating_sub(usable_top)
        .saturating_sub(top)
        .saturating_sub(bottom)
        .max(1);
    let mut reasons = Vec::new();

    let original_width = geometry.width;
    geometry.width =
        u32::try_from(i64::from(geometry.width).min(maximum_width)).unwrap_or(u32::MAX);
    if geometry.width != original_width {
        reasons.push("usable_width");
    }
    let original_height = geometry.height;
    geometry.height =
        u32::try_from(i64::from(geometry.height).min(maximum_height)).unwrap_or(u32::MAX);
    if geometry.height != original_height {
        reasons.push("usable_height");
    }

    let original_x = geometry.x;
    if fields.x {
        let minimum_x = usable_left.saturating_add(left);
        let maximum_x = usable_right
            .saturating_sub(right)
            .saturating_sub(i64::from(geometry.width))
            .max(minimum_x);
        geometry.x = clamp_i64_to_i32(i64::from(geometry.x).clamp(minimum_x, maximum_x));
    } else {
        let available_width = usable_right
            .saturating_sub(right)
            .saturating_sub(i64::from(geometry.x))
            .max(1);
        let accepted_width =
            u32::try_from(i64::from(geometry.width).min(available_width)).unwrap_or(u32::MAX);
        if accepted_width != geometry.width {
            geometry.width = accepted_width;
            if !reasons.contains(&"usable_width") {
                reasons.push("usable_width");
            }
        }
    }
    if geometry.x != original_x {
        reasons.push("usable_position");
    }

    let original_y = geometry.y;
    if fields.y {
        let minimum_y = usable_top.saturating_add(top);
        let maximum_y = usable_bottom
            .saturating_sub(bottom)
            .saturating_sub(i64::from(geometry.height))
            .max(minimum_y);
        geometry.y = clamp_i64_to_i32(i64::from(geometry.y).clamp(minimum_y, maximum_y));
    } else {
        let available_height = usable_bottom
            .saturating_sub(bottom)
            .saturating_sub(i64::from(geometry.y))
            .max(1);
        let accepted_height =
            u32::try_from(i64::from(geometry.height).min(available_height)).unwrap_or(u32::MAX);
        if accepted_height != geometry.height {
            geometry.height = accepted_height;
            if !reasons.contains(&"usable_height") {
                reasons.push("usable_height");
            }
        }
    }
    if geometry.y != original_y {
        reasons.push("usable_position");
    }

    (geometry, reasons)
}

fn clamp_i64_to_i32(value: i64) -> i32 {
    i32::try_from(value).unwrap_or_else(|_| {
        if value.is_negative() {
            i32::MIN
        } else {
            i32::MAX
        }
    })
}
