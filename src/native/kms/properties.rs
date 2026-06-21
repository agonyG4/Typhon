use std::{
    collections::HashMap,
    num::{NonZeroU32, NonZeroU64},
};

use super::{AtomicKmsError, AtomicKmsErrorKind};

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(NonZeroU32);

        impl $name {
            pub const fn new(value: u32) -> Option<Self> {
                match NonZeroU32::new(value) {
                    Some(value) => Some(Self(value)),
                    None => None,
                }
            }

            pub const fn get(self) -> u32 {
                self.0.get()
            }
        }
    };
}

id_type!(ConnectorId);
id_type!(CrtcId);
id_type!(PlaneId);
id_type!(FramebufferId);
id_type!(BlobId);
id_type!(PropertyId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageFlipToken(NonZeroU64);

impl PageFlipToken {
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DrmObjectKind {
    Connector,
    Crtc,
    PrimaryPlane,
    CursorPlane,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmProperty {
    id: PropertyId,
    name: String,
    pub value: u64,
}

impl DrmProperty {
    pub fn new(id: PropertyId, name: impl Into<String>, value: u64) -> Self {
        Self {
            id,
            name: name.into(),
            value,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Debug, Clone)]
pub struct PropertySet {
    kind: DrmObjectKind,
    by_name: HashMap<String, DrmProperty>,
}

impl PropertySet {
    pub fn new(kind: DrmObjectKind, properties: Vec<DrmProperty>) -> Result<Self, AtomicKmsError> {
        let mut by_name = HashMap::new();
        let mut by_id = HashMap::<PropertyId, String>::new();
        for property in properties {
            if let Some(existing) = by_id.insert(property.id, property.name.clone()) {
                return Err(AtomicKmsError::new(
                    AtomicKmsErrorKind::DuplicateProperty,
                    format!(
                        "{kind:?} property id {} is shared by {existing:?} and {:?}",
                        property.id.get(),
                        property.name
                    ),
                ));
            }
            if by_name.insert(property.name.clone(), property).is_some() {
                return Err(AtomicKmsError::new(
                    AtomicKmsErrorKind::DuplicateProperty,
                    format!("{kind:?} has a duplicate property name"),
                ));
            }
        }
        Ok(Self { kind, by_name })
    }

    fn required(&self, name: &str) -> Result<PropertyId, AtomicKmsError> {
        self.by_name
            .get(name)
            .map(|property| property.id)
            .ok_or_else(|| {
                AtomicKmsError::new(
                    AtomicKmsErrorKind::MissingProperty,
                    format!("{:?} is missing required property {name}", self.kind),
                )
            })
    }

    fn optional(&self, name: &str) -> Option<PropertyId> {
        self.by_name.get(name).map(|property| property.id)
    }

    pub fn value(&self, name: &str) -> Option<u64> {
        self.by_name.get(name).map(|property| property.value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnectorPropertyId(pub PropertyId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CrtcPropertyId(pub PropertyId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlanePropertyId(pub PropertyId);

#[derive(Debug, Clone)]
pub struct AtomicConnectorProperties {
    pub crtc_id: ConnectorPropertyId,
}

impl AtomicConnectorProperties {
    pub fn discover(properties: &[DrmProperty]) -> Result<Self, AtomicKmsError> {
        let set = PropertySet::new(DrmObjectKind::Connector, properties.to_vec())?;
        Ok(Self {
            crtc_id: ConnectorPropertyId(set.required("CRTC_ID")?),
        })
    }
}

#[derive(Debug, Clone)]
pub struct AtomicCrtcProperties {
    pub active: CrtcPropertyId,
    pub mode_id: CrtcPropertyId,
    pub out_fence_ptr: Option<CrtcPropertyId>,
    pub vrr_enabled: Option<CrtcPropertyId>,
    pub gamma_lut: Option<CrtcPropertyId>,
    pub degamma_lut: Option<CrtcPropertyId>,
    pub ctm: Option<CrtcPropertyId>,
}

impl AtomicCrtcProperties {
    pub fn discover(properties: &[DrmProperty]) -> Result<Self, AtomicKmsError> {
        let set = PropertySet::new(DrmObjectKind::Crtc, properties.to_vec())?;
        Ok(Self {
            active: CrtcPropertyId(set.required("ACTIVE")?),
            mode_id: CrtcPropertyId(set.required("MODE_ID")?),
            out_fence_ptr: set.optional("OUT_FENCE_PTR").map(CrtcPropertyId),
            vrr_enabled: set.optional("VRR_ENABLED").map(CrtcPropertyId),
            gamma_lut: set.optional("GAMMA_LUT").map(CrtcPropertyId),
            degamma_lut: set.optional("DEGAMMA_LUT").map(CrtcPropertyId),
            ctm: set.optional("CTM").map(CrtcPropertyId),
        })
    }
}

#[derive(Debug, Clone)]
pub struct AtomicPlaneProperties {
    pub fb_id: PlanePropertyId,
    pub crtc_id: PlanePropertyId,
    pub src_x: PlanePropertyId,
    pub src_y: PlanePropertyId,
    pub src_w: PlanePropertyId,
    pub src_h: PlanePropertyId,
    pub crtc_x: PlanePropertyId,
    pub crtc_y: PlanePropertyId,
    pub crtc_w: PlanePropertyId,
    pub crtc_h: PlanePropertyId,
    pub plane_type: PlanePropertyId,
    pub in_fence_fd: Option<PlanePropertyId>,
    pub damage_clips: Option<PlanePropertyId>,
    pub rotation: Option<PlanePropertyId>,
    pub alpha: Option<PlanePropertyId>,
    pub pixel_blend_mode: Option<PlanePropertyId>,
    pub color_encoding: Option<PlanePropertyId>,
    pub color_range: Option<PlanePropertyId>,
}

impl AtomicPlaneProperties {
    pub fn discover(properties: &[DrmProperty]) -> Result<Self, AtomicKmsError> {
        let set = PropertySet::new(DrmObjectKind::PrimaryPlane, properties.to_vec())?;
        Ok(Self {
            fb_id: PlanePropertyId(set.required("FB_ID")?),
            crtc_id: PlanePropertyId(set.required("CRTC_ID")?),
            src_x: PlanePropertyId(set.required("SRC_X")?),
            src_y: PlanePropertyId(set.required("SRC_Y")?),
            src_w: PlanePropertyId(set.required("SRC_W")?),
            src_h: PlanePropertyId(set.required("SRC_H")?),
            crtc_x: PlanePropertyId(set.required("CRTC_X")?),
            crtc_y: PlanePropertyId(set.required("CRTC_Y")?),
            crtc_w: PlanePropertyId(set.required("CRTC_W")?),
            crtc_h: PlanePropertyId(set.required("CRTC_H")?),
            plane_type: PlanePropertyId(set.required("type")?),
            in_fence_fd: set.optional("IN_FENCE_FD").map(PlanePropertyId),
            damage_clips: set.optional("FB_DAMAGE_CLIPS").map(PlanePropertyId),
            rotation: set.optional("rotation").map(PlanePropertyId),
            alpha: set.optional("alpha").map(PlanePropertyId),
            pixel_blend_mode: set.optional("pixel blend mode").map(PlanePropertyId),
            color_encoding: set.optional("COLOR_ENCODING").map(PlanePropertyId),
            color_range: set.optional("COLOR_RANGE").map(PlanePropertyId),
        })
    }
}
