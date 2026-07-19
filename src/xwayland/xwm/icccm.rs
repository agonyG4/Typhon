//! Pure ICCCM normalization used at the XWM boundary.

use super::{X11ConfigureFlags, X11Geometry};
use crate::compositor::WindowConstraints;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransientParentError {
    SelfReference,
    Cycle,
    Missing,
}

pub(crate) fn apply_configure_request(
    current: X11Geometry,
    requested: X11Geometry,
    flags: X11ConfigureFlags,
    constraints: WindowConstraints,
) -> X11Geometry {
    let mut result = current;
    if flags.x {
        result.x = requested.x;
    }
    if flags.y {
        result.y = requested.y;
    }
    if flags.width {
        result.width = requested.width.max(1);
    }
    if flags.height {
        result.height = requested.height.max(1);
    }
    result.width = constrain_dimension(result.width, constraints.min_width, constraints.max_width);
    result.height = constrain_dimension(
        result.height,
        constraints.min_height,
        constraints.max_height,
    );
    result.width = apply_increment(
        result.width,
        constraints.base_width,
        constraints.width_increment,
    );
    result.height = apply_increment(
        result.height,
        constraints.base_height,
        constraints.height_increment,
    );
    if let Some(min_aspect) = constraints.min_aspect
        && min_aspect > 0.0
        && f64::from(result.width) / f64::from(result.height) < min_aspect
    {
        result.width = (f64::from(result.height) * min_aspect).ceil() as u32;
    }
    if let Some(max_aspect) = constraints.max_aspect
        && max_aspect > 0.0
        && f64::from(result.width) / f64::from(result.height) > max_aspect
    {
        result.height = (f64::from(result.width) / max_aspect).ceil() as u32;
    }
    result
}

fn constrain_dimension(value: u32, min: Option<u32>, max: Option<u32>) -> u32 {
    value.max(min.unwrap_or(1)).min(max.unwrap_or(u32::MAX))
}

fn apply_increment(value: u32, base: Option<u32>, increment: Option<u32>) -> u32 {
    let Some(increment) = increment.filter(|increment| *increment > 0) else {
        return value;
    };
    let base = base.unwrap_or(0);
    base.saturating_add(value.saturating_sub(base) / increment * increment)
}

pub(crate) fn validate_transient_parent(
    child: u32,
    parent: Option<u32>,
    mut parent_of: impl FnMut(u32) -> Option<u32>,
) -> Result<Option<u32>, TransientParentError> {
    let Some(parent) = parent else {
        return Ok(None);
    };
    if parent == child {
        return Err(TransientParentError::SelfReference);
    }
    let mut current = Some(parent);
    for _ in 0..256 {
        let Some(candidate) = current else {
            return Ok(Some(parent));
        };
        if candidate == child {
            return Err(TransientParentError::Cycle);
        }
        current = parent_of(candidate);
    }
    Err(TransientParentError::Missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configure_applies_only_requested_fields_and_constraints() {
        let result = apply_configure_request(
            X11Geometry {
                x: 4,
                y: 5,
                width: 100,
                height: 100,
            },
            X11Geometry {
                x: 10,
                y: 20,
                width: 801,
                height: 601,
            },
            X11ConfigureFlags {
                width: true,
                height: true,
                ..Default::default()
            },
            WindowConstraints {
                min_width: Some(200),
                width_increment: Some(8),
                ..Default::default()
            },
        );
        assert_eq!(result.x, 4);
        assert_eq!(result.y, 5);
        assert_eq!(result.width, 800);
        assert_eq!(result.height, 601);
    }

    #[test]
    fn transient_cycle_is_rejected() {
        let result = validate_transient_parent(1, Some(2), |window| match window {
            2 => Some(3),
            3 => Some(1),
            _ => None,
        });
        assert_eq!(result, Err(TransientParentError::Cycle));
    }
}
