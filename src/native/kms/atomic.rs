use std::collections::BTreeMap;

use super::{
    AtomicConnectorProperties, AtomicCrtcProperties, AtomicCursorPlaneProperties, AtomicKmsError,
    AtomicKmsErrorKind, AtomicPlaneProperties, AtomicPlaneRole, BlobId, ConnectorId,
    ConnectorPropertyId, CrtcId, CrtcPropertyId, DrmFormatModifierPair, DrmObjectKind,
    FramebufferId, PageFlipToken, PlaneId, PlanePropertyId,
};

pub const DRM_FORMAT_ARGB8888: u32 = u32::from_le_bytes(*b"AR24");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicCursorVisualState {
    pub visible: bool,
    pub x: i32,
    pub y: i32,
    pub hotspot_x: i32,
    pub hotspot_y: i32,
    pub width: u32,
    pub height: u32,
    pub framebuffer_id: Option<u32>,
    pub image_generation: u64,
}

impl AtomicCursorVisualState {
    pub const fn hidden(width: u32, height: u32) -> Self {
        Self {
            visible: false,
            x: 0,
            y: 0,
            hotspot_x: 0,
            hotspot_y: 0,
            width,
            height,
            framebuffer_id: None,
            image_generation: 0,
        }
    }

    pub fn kms_equivalent(&self, other: &Self) -> bool {
        if !self.visible && !other.visible {
            return true;
        }
        self.visible == other.visible
            && self.x == other.x
            && self.y == other.y
            && self.hotspot_x == other.hotspot_x
            && self.hotspot_y == other.hotspot_y
            && self.width == other.width
            && self.height == other.height
            && self.framebuffer_id == other.framebuffer_id
            && self.image_generation == other.image_generation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneType {
    Overlay,
    Primary,
    Cursor,
    Unknown(u64),
}

pub const fn cursor_dimension_from_capability(value: Option<u64>) -> u32 {
    match value {
        Some(value) if value > 0 && value <= u32::MAX as u64 => value as u32,
        _ => 64,
    }
}

pub(crate) fn plane_type_from_value(value: u64) -> PlaneType {
    match u32::try_from(value).ok() {
        Some(drm_sys::DRM_PLANE_TYPE_OVERLAY) => PlaneType::Overlay,
        Some(drm_sys::DRM_PLANE_TYPE_PRIMARY) => PlaneType::Primary,
        Some(drm_sys::DRM_PLANE_TYPE_CURSOR) => PlaneType::Cursor,
        _ => PlaneType::Unknown(value),
    }
}

pub(crate) fn select_cursor_format_modifier(
    scanout_formats: &[DrmFormatModifierPair],
    legacy_formats: &[u32],
) -> Option<DrmFormatModifierPair> {
    scanout_formats
        .iter()
        .copied()
        .filter(|pair| pair.fourcc == DRM_FORMAT_ARGB8888)
        .min_by_key(|pair| (pair.modifier != 0, pair.modifier))
        .or_else(|| {
            legacy_formats
                .contains(&DRM_FORMAT_ARGB8888)
                .then_some(DrmFormatModifierPair {
                    fourcc: DRM_FORMAT_ARGB8888,
                    modifier: 0,
                })
        })
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

pub fn select_cursor_plane(
    candidates: &[PlaneCandidate],
    crtc_index: usize,
    crtc: CrtcId,
    framebuffer_format: u32,
    primary_plane: PlaneId,
) -> Option<&PlaneCandidate> {
    let mask = 1u32.checked_shl(u32::try_from(crtc_index).ok()?)?;
    candidates
        .iter()
        .filter(|candidate| candidate.plane_type == PlaneType::Cursor)
        .filter(|candidate| candidate.id != primary_plane)
        .filter(|candidate| candidate.possible_crtcs & mask != 0)
        .filter(|candidate| candidate.formats.contains(&framebuffer_format))
        .filter(|candidate| candidate.current_crtc.is_none_or(|current| current == crtc))
        .min_by_key(|candidate| (candidate.current_crtc != Some(crtc), candidate.id))
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
    Plane(PlaneId, AtomicPlaneRole),
}

impl AtomicObjectId {
    const fn raw(self) -> u32 {
        match self {
            Self::Connector(id) => id.get(),
            Self::Crtc(id) => id.get(),
            Self::Plane(id, _) => id.get(),
        }
    }

    const fn kind(self) -> DrmObjectKind {
        match self {
            Self::Connector(_) => DrmObjectKind::Connector,
            Self::Crtc(_) => DrmObjectKind::Crtc,
            Self::Plane(_, AtomicPlaneRole::Primary) => DrmObjectKind::PrimaryPlane,
            Self::Plane(_, AtomicPlaneRole::Cursor) => DrmObjectKind::CursorPlane,
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
        self.set(
            AtomicObjectId::Plane(object, AtomicPlaneRole::Primary),
            property.0.get(),
            value,
        )
    }

    pub fn set_cursor_plane(
        &mut self,
        object: PlaneId,
        property: PlanePropertyId,
        value: u64,
    ) -> Result<(), AtomicKmsError> {
        self.set(
            AtomicObjectId::Plane(object, AtomicPlaneRole::Cursor),
            property.0.get(),
            value,
        )
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

    pub fn initial_modeset_for_pipeline(
        pipeline: &AtomicPipelineProperties,
        mode_blob: BlobId,
        framebuffer: FramebufferId,
        geometry: AtomicPlaneGeometry,
        cursor: Option<&AtomicCursorVisualState>,
    ) -> Result<Self, AtomicKmsError> {
        let mut request = Self::initial_modeset(
            pipeline.connector,
            pipeline.crtc,
            pipeline.plane,
            &pipeline.connector_props,
            &pipeline.crtc_props,
            &pipeline.plane_props,
            mode_blob,
            framebuffer,
            geometry,
        )?;
        append_cursor_plane_state(&mut request, pipeline, cursor)?;
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

    pub fn resume_modeset_for_pipeline(
        pipeline: &AtomicPipelineProperties,
        mode_blob: BlobId,
        framebuffer: FramebufferId,
        geometry: AtomicPlaneGeometry,
        cursor: Option<&AtomicCursorVisualState>,
    ) -> Result<Self, AtomicKmsError> {
        Self::initial_modeset_for_pipeline(pipeline, mode_blob, framebuffer, geometry, cursor)
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

    pub fn primary_flip_with_cursor(
        pipeline: &AtomicPipelineProperties,
        framebuffer: FramebufferId,
        cursor: Option<&AtomicCursorVisualState>,
    ) -> Result<Self, AtomicKmsError> {
        let mut request =
            Self::primary_flip(pipeline.plane, pipeline.plane_props.fb_id, framebuffer)?;
        append_cursor_plane_state(&mut request, pipeline, cursor)?;
        Ok(request)
    }

    pub fn cursor_only(
        pipeline: &AtomicPipelineProperties,
        cursor: Option<&AtomicCursorVisualState>,
    ) -> Result<Self, AtomicKmsError> {
        let mut request = Self::new();
        append_cursor_plane_state(&mut request, pipeline, cursor)?;
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
        append_cursor_plane_state(&mut request, pipeline, None)?;
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
    pub cursor_plane: Option<AtomicCursorPlaneProperties>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtomicCursorPlaneSnapshot {
    pub fb_id: u64,
    pub crtc_id: u64,
    pub src_x: u64,
    pub src_y: u64,
    pub src_w: u64,
    pub src_h: u64,
    pub crtc_x: u64,
    pub crtc_y: u64,
    pub crtc_w: u64,
    pub crtc_h: u64,
    pub alpha: Option<u64>,
    pub pixel_blend_mode: Option<u64>,
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
    pub cursor: Option<AtomicCursorPlaneSnapshot>,
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
        append_cursor_plane_snapshot(&mut request, pipeline, self.cursor)?;
        Ok(request)
    }
}

impl AtomicCursorPlaneProperties {
    const fn plane_id(&self) -> PlaneId {
        // Discovery rejects zero object IDs before constructing this value.
        PlaneId::new(self.plane_id).expect("cursor plane ID is nonzero")
    }
}

fn append_cursor_plane_snapshot(
    request: &mut AtomicRequest,
    pipeline: &AtomicPipelineProperties,
    snapshot: Option<AtomicCursorPlaneSnapshot>,
) -> Result<(), AtomicKmsError> {
    let Some((cursor, snapshot)) = pipeline.cursor_plane.as_ref().zip(snapshot) else {
        return Ok(());
    };
    let plane = cursor.plane_id();
    let props = &cursor.property_ids;
    request.set_cursor_plane(plane, props.fb_id, snapshot.fb_id)?;
    request.set_cursor_plane(plane, props.crtc_id, snapshot.crtc_id)?;
    request.set_cursor_plane(plane, props.src_x, snapshot.src_x)?;
    request.set_cursor_plane(plane, props.src_y, snapshot.src_y)?;
    request.set_cursor_plane(plane, props.src_w, snapshot.src_w)?;
    request.set_cursor_plane(plane, props.src_h, snapshot.src_h)?;
    request.set_cursor_plane(plane, props.crtc_x, snapshot.crtc_x)?;
    request.set_cursor_plane(plane, props.crtc_y, snapshot.crtc_y)?;
    request.set_cursor_plane(plane, props.crtc_w, snapshot.crtc_w)?;
    request.set_cursor_plane(plane, props.crtc_h, snapshot.crtc_h)?;
    if let (Some(property), Some(value)) = (props.alpha, snapshot.alpha) {
        request.set_cursor_plane(plane, property, value)?;
    }
    if let (Some(property), Some(value)) = (props.pixel_blend_mode, snapshot.pixel_blend_mode) {
        request.set_cursor_plane(plane, property, value)?;
    }
    Ok(())
}

pub fn append_cursor_plane_state(
    request: &mut AtomicRequest,
    pipeline: &AtomicPipelineProperties,
    cursor: Option<&AtomicCursorVisualState>,
) -> Result<(), AtomicKmsError> {
    let Some(cursor_plane) = pipeline.cursor_plane.as_ref() else {
        if cursor.is_some_and(|cursor| cursor.visible) {
            return Err(AtomicKmsError::new(
                AtomicKmsErrorKind::Unsupported,
                "visible Atomic cursor requested without a compatible cursor plane",
            ));
        }
        return Ok(());
    };
    let plane = cursor_plane.plane_id();
    let props = &cursor_plane.property_ids;
    let Some(cursor) = cursor.filter(|cursor| cursor.visible) else {
        request.set_cursor_plane(plane, props.fb_id, 0)?;
        request.set_cursor_plane(plane, props.crtc_id, 0)?;
        return Ok(());
    };
    let framebuffer_id = cursor.framebuffer_id.ok_or_else(|| {
        AtomicKmsError::new(
            AtomicKmsErrorKind::MissingProperty,
            "visible Atomic cursor has no framebuffer",
        )
    })?;
    let src_w = u64::from(cursor.width).checked_shl(16).ok_or_else(|| {
        AtomicKmsError::new(
            AtomicKmsErrorKind::InvalidGeometry,
            "cursor source width overflows unsigned 16.16",
        )
    })?;
    let src_h = u64::from(cursor.height).checked_shl(16).ok_or_else(|| {
        AtomicKmsError::new(
            AtomicKmsErrorKind::InvalidGeometry,
            "cursor source height overflows unsigned 16.16",
        )
    })?;
    request.set_cursor_plane(plane, props.fb_id, u64::from(framebuffer_id))?;
    request.set_cursor_plane(plane, props.crtc_id, u64::from(cursor_plane.crtc_id))?;
    request.set_cursor_plane(plane, props.src_x, 0)?;
    request.set_cursor_plane(plane, props.src_y, 0)?;
    request.set_cursor_plane(plane, props.src_w, src_w)?;
    request.set_cursor_plane(plane, props.src_h, src_h)?;
    request.set_cursor_plane(
        plane,
        props.crtc_x,
        i64::from(cursor.x.saturating_sub(cursor.hotspot_x)) as u64,
    )?;
    request.set_cursor_plane(
        plane,
        props.crtc_y,
        i64::from(cursor.y.saturating_sub(cursor.hotspot_y)) as u64,
    )?;
    request.set_cursor_plane(plane, props.crtc_w, u64::from(cursor.width))?;
    request.set_cursor_plane(plane, props.crtc_h, u64::from(cursor.height))?;
    if let Some(rotation) = props.rotation {
        request.set_cursor_plane(plane, rotation, u64::from(drm_sys::DRM_MODE_ROTATE_0))?;
    }
    if let (Some(alpha), Some(maximum)) = (props.alpha, cursor_plane.alpha_maximum) {
        request.set_cursor_plane(plane, alpha, maximum)?;
    }
    if let (Some(blend), Some(premultiplied)) = (
        props.pixel_blend_mode,
        cursor_plane.pixel_blend_mode_premultiplied,
    ) {
        request.set_cursor_plane(plane, blend, premultiplied)?;
    }
    Ok(())
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

    pub const fn test_only_no_modeset() -> Self {
        Self(drm_sys::DRM_MODE_ATOMIC_TEST_ONLY)
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

    pub const fn contains_test_only(self) -> bool {
        self.0 & drm_sys::DRM_MODE_ATOMIC_TEST_ONLY != 0
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

    pub fn test_only(request: AtomicRequest) -> Self {
        Self {
            request,
            flags: AtomicCommitFlags::test_only_no_modeset(),
            user_data: 0,
        }
    }
}
