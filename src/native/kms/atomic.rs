use std::collections::BTreeMap;

use super::{
    AtomicConnectorProperties, AtomicCrtcProperties, AtomicKmsError, AtomicKmsErrorKind,
    AtomicPlaneProperties, BlobId, ConnectorId, ConnectorPropertyId, CrtcId, CrtcPropertyId,
    DrmObjectKind, FramebufferId, PageFlipToken, PlaneId, PlanePropertyId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneType {
    Overlay,
    Primary,
    Cursor,
    Unknown(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneCandidate {
    pub id: PlaneId,
    pub plane_type: PlaneType,
    pub possible_crtcs: u32,
    pub formats: Vec<u32>,
    pub current_crtc: Option<CrtcId>,
}

pub fn select_primary_plane(
    candidates: &[PlaneCandidate],
    crtc_index: usize,
    crtc: CrtcId,
    framebuffer_format: u32,
) -> Result<&PlaneCandidate, AtomicKmsError> {
    let mask = 1u32.checked_shl(u32::try_from(crtc_index).unwrap_or(u32::MAX));
    let Some(mask) = mask else {
        return Err(AtomicKmsError::new(
            AtomicKmsErrorKind::NoCompatiblePrimaryPlane,
            "selected CRTC index cannot be represented in possible_crtcs",
        ));
    };
    candidates
        .iter()
        .filter(|candidate| candidate.plane_type == PlaneType::Primary)
        .filter(|candidate| candidate.possible_crtcs & mask != 0)
        .filter(|candidate| candidate.formats.contains(&framebuffer_format))
        .filter(|candidate| candidate.current_crtc.is_none_or(|current| current == crtc))
        .min_by_key(|candidate| (candidate.current_crtc != Some(crtc), candidate.id))
        .ok_or_else(|| {
            AtomicKmsError::new(
                AtomicKmsErrorKind::NoCompatiblePrimaryPlane,
                format!(
                    "no primary plane supports CRTC index {crtc_index} and format {framebuffer_format:#010x}"
                ),
            )
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtomicPlaneGeometry {
    pub src_x: u64,
    pub src_y: u64,
    pub src_w: u64,
    pub src_h: u64,
    pub crtc_x: u64,
    pub crtc_y: u64,
    pub crtc_w: u64,
    pub crtc_h: u64,
}

pub trait ModeBlobIo {
    fn create_mode_blob(&self, mode: &drm_sys::drm_mode_modeinfo)
    -> Result<BlobId, AtomicKmsError>;

    fn destroy_mode_blob(&self, blob: BlobId) -> Result<(), AtomicKmsError>;
}

#[derive(Debug)]
pub struct ModeBlob<I: ModeBlobIo> {
    io: Option<I>,
    id: BlobId,
}

impl<I: ModeBlobIo> ModeBlob<I> {
    pub fn create(io: I, mode: &drm_sys::drm_mode_modeinfo) -> Result<Self, AtomicKmsError> {
        let id = io.create_mode_blob(mode)?;
        Ok(Self { io: Some(io), id })
    }

    pub const fn id(&self) -> BlobId {
        self.id
    }

    pub fn disarm(&mut self) {
        self.io = None;
    }
}

#[cfg(test)]
mod mode_blob_session_tests {
    use std::{cell::Cell, rc::Rc};

    use super::*;

    #[derive(Debug)]
    struct FakeBlobIo(Rc<Cell<usize>>);

    impl ModeBlobIo for FakeBlobIo {
        fn create_mode_blob(
            &self,
            _mode: &drm_sys::drm_mode_modeinfo,
        ) -> Result<BlobId, AtomicKmsError> {
            Ok(BlobId::new(7).unwrap())
        }

        fn destroy_mode_blob(&self, _blob: BlobId) -> Result<(), AtomicKmsError> {
            self.0.set(self.0.get() + 1);
            Ok(())
        }
    }

    #[test]
    fn disarmed_mode_blob_drop_performs_no_drm_io() {
        let destroys = Rc::new(Cell::new(0));
        let mut blob = ModeBlob::create(
            FakeBlobIo(Rc::clone(&destroys)),
            &drm_sys::drm_mode_modeinfo::default(),
        )
        .unwrap();
        blob.disarm();
        drop(blob);

        assert_eq!(destroys.get(), 0);
    }
}

impl<I: ModeBlobIo> Drop for ModeBlob<I> {
    fn drop(&mut self) {
        let Some(io) = self.io.as_ref() else {
            return;
        };
        if let Err(error) = io.destroy_mode_blob(self.id) {
            eprintln!(
                "atomic KMS: failed to destroy mode blob {}: {error}",
                self.id.get()
            );
        }
    }
}

impl AtomicPlaneGeometry {
    pub fn fullscreen(width: u32, height: u32) -> Result<Self, AtomicKmsError> {
        if width == 0 || height == 0 {
            return Err(AtomicKmsError::new(
                AtomicKmsErrorKind::InvalidGeometry,
                "atomic plane geometry must be nonzero",
            ));
        }
        let src_w = u64::from(width)
            .checked_shl(16)
            .filter(|value| *value <= u64::from(u32::MAX))
            .ok_or_else(|| {
                AtomicKmsError::new(
                    AtomicKmsErrorKind::InvalidGeometry,
                    "atomic source width overflows unsigned 16.16",
                )
            })?;
        let src_h = u64::from(height)
            .checked_shl(16)
            .filter(|value| *value <= u64::from(u32::MAX))
            .ok_or_else(|| {
                AtomicKmsError::new(
                    AtomicKmsErrorKind::InvalidGeometry,
                    "atomic source height overflows unsigned 16.16",
                )
            })?;
        Ok(Self {
            src_x: 0,
            src_y: 0,
            src_w,
            src_h,
            crtc_x: 0,
            crtc_y: 0,
            crtc_w: u64::from(width),
            crtc_h: u64::from(height),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AtomicObjectId {
    Connector(ConnectorId),
    Crtc(CrtcId),
    Plane(PlaneId),
}

impl AtomicObjectId {
    const fn raw(self) -> u32 {
        match self {
            Self::Connector(id) => id.get(),
            Self::Crtc(id) => id.get(),
            Self::Plane(id) => id.get(),
        }
    }

    const fn kind(self) -> DrmObjectKind {
        match self {
            Self::Connector(_) => DrmObjectKind::Connector,
            Self::Crtc(_) => DrmObjectKind::Crtc,
            Self::Plane(_) => DrmObjectKind::PrimaryPlane,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicRequest {
    assignments: BTreeMap<(AtomicObjectId, u32), u64>,
}

impl AtomicRequest {
    pub fn new() -> Self {
        Self {
            assignments: BTreeMap::new(),
        }
    }

    pub fn set_connector(
        &mut self,
        object: ConnectorId,
        property: ConnectorPropertyId,
        value: u64,
    ) -> Result<(), AtomicKmsError> {
        self.set(AtomicObjectId::Connector(object), property.0.get(), value)
    }

    pub fn set_crtc(
        &mut self,
        object: CrtcId,
        property: CrtcPropertyId,
        value: u64,
    ) -> Result<(), AtomicKmsError> {
        self.set(AtomicObjectId::Crtc(object), property.0.get(), value)
    }

    pub fn set_plane(
        &mut self,
        object: PlaneId,
        property: PlanePropertyId,
        value: u64,
    ) -> Result<(), AtomicKmsError> {
        self.set(AtomicObjectId::Plane(object), property.0.get(), value)
    }

    fn set(
        &mut self,
        object: AtomicObjectId,
        property: u32,
        value: u64,
    ) -> Result<(), AtomicKmsError> {
        if self.assignments.contains_key(&(object, property)) {
            return Err(AtomicKmsError::new(
                AtomicKmsErrorKind::DuplicateAssignment,
                format!(
                    "duplicate atomic assignment object={} property={property}",
                    object.raw()
                ),
            ));
        }
        self.assignments.insert((object, property), value);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn initial_modeset(
        connector: ConnectorId,
        crtc: CrtcId,
        plane: PlaneId,
        connector_props: &AtomicConnectorProperties,
        crtc_props: &AtomicCrtcProperties,
        plane_props: &AtomicPlaneProperties,
        mode_blob: BlobId,
        framebuffer: FramebufferId,
        geometry: AtomicPlaneGeometry,
    ) -> Result<Self, AtomicKmsError> {
        let mut request = Self::new();
        request.set_connector(connector, connector_props.crtc_id, u64::from(crtc.get()))?;
        request.set_crtc(crtc, crtc_props.mode_id, u64::from(mode_blob.get()))?;
        request.set_crtc(crtc, crtc_props.active, 1)?;
        request.set_plane(plane, plane_props.fb_id, u64::from(framebuffer.get()))?;
        request.set_plane(plane, plane_props.crtc_id, u64::from(crtc.get()))?;
        request.set_plane(plane, plane_props.src_x, geometry.src_x)?;
        request.set_plane(plane, plane_props.src_y, geometry.src_y)?;
        request.set_plane(plane, plane_props.src_w, geometry.src_w)?;
        request.set_plane(plane, plane_props.src_h, geometry.src_h)?;
        request.set_plane(plane, plane_props.crtc_x, geometry.crtc_x)?;
        request.set_plane(plane, plane_props.crtc_y, geometry.crtc_y)?;
        request.set_plane(plane, plane_props.crtc_w, geometry.crtc_w)?;
        request.set_plane(plane, plane_props.crtc_h, geometry.crtc_h)?;
        Ok(request)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn resume_modeset(
        connector: ConnectorId,
        crtc: CrtcId,
        plane: PlaneId,
        connector_props: &AtomicConnectorProperties,
        crtc_props: &AtomicCrtcProperties,
        plane_props: &AtomicPlaneProperties,
        mode_blob: BlobId,
        framebuffer: FramebufferId,
        geometry: AtomicPlaneGeometry,
    ) -> Result<Self, AtomicKmsError> {
        Self::initial_modeset(
            connector,
            crtc,
            plane,
            connector_props,
            crtc_props,
            plane_props,
            mode_blob,
            framebuffer,
            geometry,
        )
    }

    pub fn primary_flip(
        plane: PlaneId,
        fb_property: PlanePropertyId,
        framebuffer: FramebufferId,
    ) -> Result<Self, AtomicKmsError> {
        let mut request = Self::new();
        request.set_plane(plane, fb_property, u64::from(framebuffer.get()))?;
        Ok(request)
    }

    pub fn primary_flip_with_fences(
        pipeline: &AtomicPipelineProperties,
        framebuffer: FramebufferId,
        in_fence_fd: i32,
        out_fence_ptr: Option<*mut i32>,
    ) -> Result<Self, AtomicKmsError> {
        let in_fence_property = pipeline.plane_props.in_fence_fd.ok_or_else(|| {
            AtomicKmsError::new(
                AtomicKmsErrorKind::MissingProperty,
                "primary plane is missing required IN_FENCE_FD",
            )
        })?;
        let mut request =
            Self::primary_flip(pipeline.plane, pipeline.plane_props.fb_id, framebuffer)?;
        request.set_plane(
            pipeline.plane,
            in_fence_property,
            u64::try_from(in_fence_fd).map_err(|_| {
                AtomicKmsError::new(
                    AtomicKmsErrorKind::MissingProperty,
                    "Atomic input fence FD is negative",
                )
            })?,
        )?;
        if let (Some(property), Some(pointer)) = (pipeline.crtc_props.out_fence_ptr, out_fence_ptr)
        {
            request.set_crtc(pipeline.crtc, property, pointer as u64)?;
        }
        Ok(request)
    }

    pub fn set_initial_input_fence(
        &mut self,
        pipeline: &AtomicPipelineProperties,
        in_fence_fd: i32,
    ) -> Result<(), AtomicKmsError> {
        let property = pipeline.plane_props.in_fence_fd.ok_or_else(|| {
            AtomicKmsError::new(
                AtomicKmsErrorKind::MissingProperty,
                "primary plane is missing required IN_FENCE_FD",
            )
        })?;
        let value = u64::try_from(in_fence_fd).map_err(|_| {
            AtomicKmsError::new(
                AtomicKmsErrorKind::MissingProperty,
                "initial Atomic input fence FD is negative",
            )
        })?;
        self.set_plane(pipeline.plane, property, value)
    }

    pub fn set_test_input_fence_none(
        &mut self,
        pipeline: &AtomicPipelineProperties,
    ) -> Result<(), AtomicKmsError> {
        if let Some(property) = pipeline.plane_props.in_fence_fd {
            self.set_plane(pipeline.plane, property, u64::MAX)?;
        }
        Ok(())
    }

    pub fn safe_disable(pipeline: &AtomicPipelineProperties) -> Result<Self, AtomicKmsError> {
        let mut request = Self::new();
        request.set_connector(pipeline.connector, pipeline.connector_props.crtc_id, 0)?;
        request.set_crtc(pipeline.crtc, pipeline.crtc_props.active, 0)?;
        request.set_crtc(pipeline.crtc, pipeline.crtc_props.mode_id, 0)?;
        request.set_plane(pipeline.plane, pipeline.plane_props.fb_id, 0)?;
        request.set_plane(pipeline.plane, pipeline.plane_props.crtc_id, 0)?;
        Ok(request)
    }

    pub fn assignment_count(&self) -> usize {
        self.assignments.len()
    }

    pub fn touches_object_kind(&self, kind: DrmObjectKind) -> bool {
        self.assignments
            .keys()
            .any(|(object, _)| object.kind() == kind)
    }

    pub fn serialize(&self) -> SerializedAtomicRequest {
        let mut objects = Vec::new();
        let mut property_counts = Vec::new();
        let mut properties = Vec::new();
        let mut values = Vec::new();
        let mut current_object = None;
        for ((object, property), value) in &self.assignments {
            if current_object != Some(*object) {
                current_object = Some(*object);
                objects.push(object.raw());
                property_counts.push(0);
            }
            *property_counts
                .last_mut()
                .expect("object was just inserted") += 1;
            properties.push(*property);
            values.push(*value);
        }
        SerializedAtomicRequest {
            objects,
            property_counts,
            properties,
            values,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AtomicPipelineProperties {
    pub connector: ConnectorId,
    pub crtc: CrtcId,
    pub plane: PlaneId,
    pub connector_props: AtomicConnectorProperties,
    pub crtc_props: AtomicCrtcProperties,
    pub plane_props: AtomicPlaneProperties,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtomicPipelineSnapshot {
    pub connector_crtc_id: u64,
    pub crtc_active: u64,
    pub crtc_mode_id: u64,
    pub plane_fb_id: u64,
    pub plane_crtc_id: u64,
    pub src_x: u64,
    pub src_y: u64,
    pub src_w: u64,
    pub src_h: u64,
    pub crtc_x: u64,
    pub crtc_y: u64,
    pub crtc_w: u64,
    pub crtc_h: u64,
}

impl AtomicPipelineSnapshot {
    pub fn restore_request(
        self,
        pipeline: &AtomicPipelineProperties,
    ) -> Result<AtomicRequest, AtomicKmsError> {
        let mut request = AtomicRequest::new();
        request.set_connector(
            pipeline.connector,
            pipeline.connector_props.crtc_id,
            self.connector_crtc_id,
        )?;
        request.set_crtc(pipeline.crtc, pipeline.crtc_props.active, self.crtc_active)?;
        request.set_crtc(
            pipeline.crtc,
            pipeline.crtc_props.mode_id,
            self.crtc_mode_id,
        )?;
        request.set_plane(pipeline.plane, pipeline.plane_props.fb_id, self.plane_fb_id)?;
        request.set_plane(
            pipeline.plane,
            pipeline.plane_props.crtc_id,
            self.plane_crtc_id,
        )?;
        request.set_plane(pipeline.plane, pipeline.plane_props.src_x, self.src_x)?;
        request.set_plane(pipeline.plane, pipeline.plane_props.src_y, self.src_y)?;
        request.set_plane(pipeline.plane, pipeline.plane_props.src_w, self.src_w)?;
        request.set_plane(pipeline.plane, pipeline.plane_props.src_h, self.src_h)?;
        request.set_plane(pipeline.plane, pipeline.plane_props.crtc_x, self.crtc_x)?;
        request.set_plane(pipeline.plane, pipeline.plane_props.crtc_y, self.crtc_y)?;
        request.set_plane(pipeline.plane, pipeline.plane_props.crtc_w, self.crtc_w)?;
        request.set_plane(pipeline.plane, pipeline.plane_props.crtc_h, self.crtc_h)?;
        Ok(request)
    }
}

impl Default for AtomicRequest {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerializedAtomicRequest {
    pub objects: Vec<u32>,
    pub property_counts: Vec<u32>,
    pub properties: Vec<u32>,
    pub values: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtomicCommitFlags(u32);

impl AtomicCommitFlags {
    pub const fn initial_test() -> Self {
        Self::test_only_allow_modeset()
    }

    pub const fn initial_real() -> Self {
        Self::allow_modeset()
    }

    pub const fn test_only_allow_modeset() -> Self {
        Self(drm_sys::DRM_MODE_ATOMIC_TEST_ONLY | drm_sys::DRM_MODE_ATOMIC_ALLOW_MODESET)
    }

    pub const fn allow_modeset() -> Self {
        Self(drm_sys::DRM_MODE_ATOMIC_ALLOW_MODESET)
    }

    pub const fn page_flip() -> Self {
        Self(drm_sys::DRM_MODE_ATOMIC_NONBLOCK | drm_sys::DRM_MODE_PAGE_FLIP_EVENT)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn contains_allow_modeset(self) -> bool {
        self.0 & drm_sys::DRM_MODE_ATOMIC_ALLOW_MODESET != 0
    }

    pub const fn contains_nonblock(self) -> bool {
        self.0 & drm_sys::DRM_MODE_ATOMIC_NONBLOCK != 0
    }

    pub const fn contains_pageflip_event(self) -> bool {
        self.0 & drm_sys::DRM_MODE_PAGE_FLIP_EVENT != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicSubmission {
    pub request: AtomicRequest,
    pub flags: AtomicCommitFlags,
    pub user_data: u64,
}

impl AtomicSubmission {
    pub fn page_flip(request: AtomicRequest, token: PageFlipToken) -> Self {
        Self {
            request,
            flags: AtomicCommitFlags::page_flip(),
            user_data: token.get(),
        }
    }

    pub fn resume_modeset(request: AtomicRequest) -> Self {
        Self {
            request,
            flags: AtomicCommitFlags::allow_modeset(),
            user_data: 0,
        }
    }
}
