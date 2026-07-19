//! Coherent single-policy RandR model for the X11 view of Typhon outputs.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RandrOutput {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub mm_width: u32,
    pub mm_height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RandrSnapshot {
    pub root_width: u32,
    pub root_height: u32,
    pub primary: Option<String>,
    pub outputs: Vec<RandrOutput>,
    pub workarea: (i32, i32, u32, u32),
    pub dpi: u32,
    pub mixed_dpi: bool,
}

impl RandrSnapshot {
    pub fn from_outputs(outputs: Vec<RandrOutput>, dpi: u32) -> Option<Self> {
        if outputs.is_empty()
            || dpi == 0
            || outputs
                .iter()
                .any(|output| output.width == 0 || output.height == 0)
        {
            return None;
        }
        let root_width = outputs
            .iter()
            .map(|output| output.x.max(0) as u32 + output.width)
            .max()?;
        let root_height = outputs
            .iter()
            .map(|output| output.y.max(0) as u32 + output.height)
            .max()?;
        let primary = outputs.first().map(|output| output.name.clone());
        Some(Self {
            root_width,
            root_height,
            primary,
            outputs,
            workarea: (0, 0, root_width, root_height),
            dpi,
            mixed_dpi: false,
        })
    }

    pub fn validate(&self) -> bool {
        self.root_width > 0
            && self.root_height > 0
            && self.dpi > 0
            && !self.mixed_dpi
            && self.outputs.iter().all(|output| {
                output.width > 0
                    && output.height > 0
                    && output.x >= 0
                    && output.y >= 0
                    && output.x as u32 + output.width <= self.root_width
                    && output.y as u32 + output.height <= self.root_height
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(name: &str, x: i32, y: i32, width: u32, height: u32) -> RandrOutput {
        RandrOutput {
            name: name.to_owned(),
            x,
            y,
            width,
            height,
            mm_width: 530,
            mm_height: 300,
        }
    }

    #[test]
    fn output_geometry_and_single_dpi_policy_are_coherent() {
        let snapshot = RandrSnapshot::from_outputs(
            vec![
                output("HDMI-A-1", 0, 0, 1920, 1080),
                output("DP-1", 1920, 0, 1920, 1080),
            ],
            96,
        )
        .expect("RandR snapshot");
        assert_eq!(snapshot.root_width, 3840);
        assert_eq!(snapshot.primary.as_deref(), Some("HDMI-A-1"));
        assert!(snapshot.validate());
    }

    #[test]
    fn empty_or_zero_dpi_output_set_is_rejected() {
        assert!(RandrSnapshot::from_outputs(Vec::new(), 96).is_none());
        assert!(RandrSnapshot::from_outputs(vec![output("DP-1", 0, 0, 1, 1)], 0).is_none());
    }
}
