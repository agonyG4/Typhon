use std::{
    os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
    time::Instant,
};

use super::{
    AtomicCommitFlags, AtomicConnectorProperties, AtomicCrtcProperties, AtomicFailureAction,
    AtomicFailurePhase, AtomicKmsError, AtomicKmsErrorKind, AtomicPipelineProperties,
    AtomicPipelineSnapshot, AtomicPlaneGeometry, AtomicPlaneProperties, AtomicRequest,
    AtomicSubmission, BlobId, ConnectorId, CrtcId, DrmFormatModifierPair, DrmModeBlobIo,
    DrmObjectKind, DrmProperty, FramebufferId, KmsBackendKind, KmsPolicy, LegacyKmsBackend,
    ModeBlob, PageFlipToken, PlaneCandidate, PlaneId, PlaneType, RestorationOutcome,
    disable_atomic_client_capability, enable_atomic_client_capability, object_properties,
    parse_in_formats_blob, property_blob, select_primary_plane, submit_atomic,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtomicOptionalCapabilities {
    pub vrr_enabled: bool,
    pub in_fence_fd: bool,
    pub out_fence_ptr: bool,
    pub framebuffer_damage_clips: bool,
}

#[derive(Debug)]
pub struct AtomicDiscovery {
    pub pipeline: AtomicPipelineProperties,
    pub snapshot: AtomicPipelineSnapshot,
    pub optional: AtomicOptionalCapabilities,
    pub plane_possible_crtcs: u32,
    pub plane_formats: Vec<u32>,
    pub plane_scanout_formats: Vec<DrmFormatModifierPair>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtomicDiscoveryRequest {
    connector: ConnectorId,
    crtc: CrtcId,
    framebuffer_format: u32,
}

#[derive(Debug)]
pub struct AtomicFlipRequest {
    pub framebuffer: FramebufferId,
    pub token: PageFlipToken,
    pub in_fence: OwnedFd,
}

#[derive(Debug)]
pub struct AtomicFlipSubmission {
    pub out_fence: Option<OwnedFd>,
}

impl AtomicDiscoveryRequest {
    pub const fn new(connector: ConnectorId, crtc: CrtcId, framebuffer_format: u32) -> Self {
        Self {
            connector,
            crtc,
            framebuffer_format,
        }
    }

    pub const fn connector(self) -> ConnectorId {
        self.connector
    }

    pub const fn crtc(self) -> CrtcId {
        self.crtc
    }

    pub const fn framebuffer_format(self) -> u32 {
        self.framebuffer_format
    }
}

pub fn discover_atomic_pipeline(
    fd: BorrowedFd<'_>,
    request: AtomicDiscoveryRequest,
) -> Result<AtomicDiscovery, AtomicKmsError> {
    enable_atomic_client_capability(fd)?;
    AtomicDiscovery::discover(
        fd,
        request.connector,
        request.crtc,
        request.framebuffer_format,
    )
}

fn mode_matches(
    candidate: &drm_sys::drm_mode_modeinfo,
    expected: &drm_sys::drm_mode_modeinfo,
) -> bool {
    candidate.hdisplay == expected.hdisplay
        && candidate.vdisplay == expected.vdisplay
        && candidate.vrefresh == expected.vrefresh
}

impl AtomicDiscovery {
    pub fn discover(
        fd: BorrowedFd<'_>,
        connector: ConnectorId,
        crtc: CrtcId,
        framebuffer_format: u32,
    ) -> Result<Self, AtomicKmsError> {
        let connector_entries =
            object_properties(fd, connector.get(), drm_sys::DRM_MODE_OBJECT_CONNECTOR)?;
        let crtc_entries = object_properties(fd, crtc.get(), drm_sys::DRM_MODE_OBJECT_CRTC)?;
        let connector_props = AtomicConnectorProperties::discover(&connector_entries)?;
        let crtc_props = AtomicCrtcProperties::discover(&crtc_entries)?;

        let mut crtcs = Vec::new();
        drm_ffi::mode::get_resources(fd, None, Some(&mut crtcs), None, None).map_err(|error| {
            AtomicKmsError::new(
                AtomicKmsErrorKind::MissingObject,
                format!("enumerate CRTCs for primary-plane selection failed: {error}"),
            )
        })?;
        let crtc_index = crtcs
            .iter()
            .position(|id| *id == crtc.get())
            .ok_or_else(|| {
                AtomicKmsError::new(
                    AtomicKmsErrorKind::MissingObject,
                    format!("selected CRTC {} is absent from DRM resources", crtc.get()),
                )
            })?;

        let mut plane_ids = Vec::new();
        drm_ffi::mode::get_plane_resources(fd, Some(&mut plane_ids)).map_err(|error| {
            AtomicKmsError::new(
                AtomicKmsErrorKind::MissingObject,
                format!("enumerate DRM planes failed: {error}"),
            )
        })?;
        let mut candidates = Vec::new();
        let mut plane_property_entries = Vec::<(PlaneId, Vec<DrmProperty>)>::new();
        for raw_id in plane_ids {
            let Some(id) = PlaneId::new(raw_id) else {
                continue;
            };
            let mut formats = Vec::new();
            let plane =
                drm_ffi::mode::get_plane(fd, raw_id, Some(&mut formats)).map_err(|error| {
                    AtomicKmsError::new(
                        AtomicKmsErrorKind::MissingObject,
                        format!("query DRM plane {raw_id} failed: {error}"),
                    )
                })?;
            let entries = object_properties(fd, raw_id, drm_sys::DRM_MODE_OBJECT_PLANE)?;
            let plane_type = property_value(&entries, "type")
                .map(plane_type_from_value)
                .unwrap_or(PlaneType::Unknown(u64::MAX));
            candidates.push(PlaneCandidate {
                id,
                plane_type,
                possible_crtcs: plane.possible_crtcs,
                formats,
                current_crtc: CrtcId::new(plane.crtc_id),
            });
            plane_property_entries.push((id, entries));
        }
        let selected = select_primary_plane(&candidates, crtc_index, crtc, framebuffer_format)?;
        let selected_id = selected.id;
        let selected_possible_crtcs = selected.possible_crtcs;
        let selected_formats = selected.formats.clone();
        let plane_entries = plane_property_entries
            .into_iter()
            .find(|(id, _)| *id == selected_id)
            .map(|(_, entries)| entries)
            .ok_or_else(|| {
                AtomicKmsError::new(
                    AtomicKmsErrorKind::MissingObject,
                    "selected primary plane properties disappeared",
                )
            })?;
        let plane_props = AtomicPlaneProperties::discover(&plane_entries)?;
        let plane_scanout_formats = match property_value(&plane_entries, "IN_FORMATS") {
            Some(blob_id) if blob_id != 0 => {
                let blob_id = u32::try_from(blob_id).map_err(|_| {
                    AtomicKmsError::new(
                        AtomicKmsErrorKind::MalformedPropertyBlob,
                        "primary-plane IN_FORMATS blob ID exceeds u32",
                    )
                })?;
                parse_in_formats_blob(&property_blob(fd, blob_id)?)?
            }
            _ => Vec::new(),
        };
        let optional = AtomicOptionalCapabilities {
            vrr_enabled: crtc_props.vrr_enabled.is_some(),
            in_fence_fd: plane_props.in_fence_fd.is_some(),
            out_fence_ptr: crtc_props.out_fence_ptr.is_some(),
            framebuffer_damage_clips: plane_props.damage_clips.is_some(),
        };
        let snapshot = AtomicPipelineSnapshot {
            connector_crtc_id: required_value(
                &connector_entries,
                "CRTC_ID",
                DrmObjectKind::Connector,
            )?,
            crtc_active: required_value(&crtc_entries, "ACTIVE", DrmObjectKind::Crtc)?,
            crtc_mode_id: required_value(&crtc_entries, "MODE_ID", DrmObjectKind::Crtc)?,
            plane_fb_id: required_value(&plane_entries, "FB_ID", DrmObjectKind::PrimaryPlane)?,
            plane_crtc_id: required_value(&plane_entries, "CRTC_ID", DrmObjectKind::PrimaryPlane)?,
            src_x: required_value(&plane_entries, "SRC_X", DrmObjectKind::PrimaryPlane)?,
            src_y: required_value(&plane_entries, "SRC_Y", DrmObjectKind::PrimaryPlane)?,
            src_w: required_value(&plane_entries, "SRC_W", DrmObjectKind::PrimaryPlane)?,
            src_h: required_value(&plane_entries, "SRC_H", DrmObjectKind::PrimaryPlane)?,
            crtc_x: required_value(&plane_entries, "CRTC_X", DrmObjectKind::PrimaryPlane)?,
            crtc_y: required_value(&plane_entries, "CRTC_Y", DrmObjectKind::PrimaryPlane)?,
            crtc_w: required_value(&plane_entries, "CRTC_W", DrmObjectKind::PrimaryPlane)?,
            crtc_h: required_value(&plane_entries, "CRTC_H", DrmObjectKind::PrimaryPlane)?,
        };
        Ok(Self {
            pipeline: AtomicPipelineProperties {
                connector,
                crtc,
                plane: selected_id,
                connector_props,
                crtc_props,
                plane_props,
            },
            snapshot,
            optional,
            plane_possible_crtcs: selected_possible_crtcs,
            plane_formats: selected_formats,
            plane_scanout_formats,
        })
    }

    fn validate_live_pipeline(
        &self,
        fd: BorrowedFd<'_>,
        expected_mode: &drm_sys::drm_mode_modeinfo,
    ) -> Result<(), AtomicKmsError> {
        let mut crtcs = Vec::new();
        let mut connectors = Vec::new();
        drm_ffi::mode::get_resources(fd, None, Some(&mut crtcs), Some(&mut connectors), None)
            .map_err(|error| {
                AtomicKmsError::new(
                    AtomicKmsErrorKind::DeviceLost,
                    format!("revalidate DRM resources before session recovery failed: {error}"),
                )
            })?;
        if !crtcs.contains(&self.pipeline.crtc.get()) {
            return Err(AtomicKmsError::new(
                AtomicKmsErrorKind::DeviceLost,
                format!(
                    "recovery CRTC {} is no longer present",
                    self.pipeline.crtc.get()
                ),
            ));
        }
        if !connectors.contains(&self.pipeline.connector.get()) {
            return Err(AtomicKmsError::new(
                AtomicKmsErrorKind::DeviceLost,
                format!(
                    "recovery connector {} is no longer present",
                    self.pipeline.connector.get()
                ),
            ));
        }

        let mut modes = Vec::new();
        let connector = drm_ffi::mode::get_connector(
            fd,
            self.pipeline.connector.get(),
            None,
            None,
            Some(&mut modes),
            None,
            true,
        )
        .map_err(|error| {
            AtomicKmsError::new(
                AtomicKmsErrorKind::DeviceLost,
                format!("revalidate connector mode before session recovery failed: {error}"),
            )
        })?;
        if connector.connection != 1 || !modes.iter().any(|mode| mode_matches(mode, expected_mode))
        {
            return Err(AtomicKmsError::new(
                AtomicKmsErrorKind::DeviceLost,
                format!(
                    "recovery connector {} no longer exposes the selected mode",
                    self.pipeline.connector.get()
                ),
            ));
        }

        let mut planes = Vec::new();
        drm_ffi::mode::get_plane_resources(fd, Some(&mut planes)).map_err(|error| {
            AtomicKmsError::new(
                AtomicKmsErrorKind::DeviceLost,
                format!("revalidate DRM planes before session recovery failed: {error}"),
            )
        })?;
        if !planes.contains(&self.pipeline.plane.get()) {
            return Err(AtomicKmsError::new(
                AtomicKmsErrorKind::DeviceLost,
                format!(
                    "recovery primary plane {} is no longer present",
                    self.pipeline.plane.get()
                ),
            ));
        }

        let connector_properties = object_properties(
            fd,
            self.pipeline.connector.get(),
            drm_sys::DRM_MODE_OBJECT_CONNECTOR,
        )?;
        AtomicConnectorProperties::discover(&connector_properties)?;
        let crtc_properties =
            object_properties(fd, self.pipeline.crtc.get(), drm_sys::DRM_MODE_OBJECT_CRTC)?;
        AtomicCrtcProperties::discover(&crtc_properties)?;
        let plane_properties = object_properties(
            fd,
            self.pipeline.plane.get(),
            drm_sys::DRM_MODE_OBJECT_PLANE,
        )?;
        AtomicPlaneProperties::discover(&plane_properties)?;
        Ok(())
    }
}

fn property_value(properties: &[DrmProperty], name: &str) -> Option<u64> {
    properties
        .iter()
        .find(|property| property.name() == name)
        .map(|property| property.value)
}

fn required_value(
    properties: &[DrmProperty],
    name: &str,
    kind: DrmObjectKind,
) -> Result<u64, AtomicKmsError> {
    property_value(properties, name).ok_or_else(|| {
        AtomicKmsError::new(
            AtomicKmsErrorKind::MissingProperty,
            format!("{kind:?} is missing property value {name}"),
        )
    })
}

fn plane_type_from_value(value: u64) -> PlaneType {
    match u32::try_from(value).ok() {
        Some(drm_sys::DRM_PLANE_TYPE_OVERLAY) => PlaneType::Overlay,
        Some(drm_sys::DRM_PLANE_TYPE_PRIMARY) => PlaneType::Primary,
        Some(drm_sys::DRM_PLANE_TYPE_CURSOR) => PlaneType::Cursor,
        _ => PlaneType::Unknown(value),
    }
}

#[derive(Debug)]
pub struct DrmAtomicBackend {
    fd: RawFd,
    discovery: AtomicDiscovery,
    mode_blob: ModeBlob<DrmModeBlobIo>,
    mode: Box<drm_sys::drm_mode_modeinfo>,
    geometry: AtomicPlaneGeometry,
    initial_property_count: usize,
    test_only_us: u64,
    initial_commit_us: u64,
    restored: bool,
    restore_on_drop: bool,
}

impl DrmAtomicBackend {
    pub fn initialize_from_discovery(
        fd: BorrowedFd<'_>,
        discovery: AtomicDiscovery,
        mode: drm_sys::drm_mode_modeinfo,
        width: u32,
        height: u32,
        framebuffer: FramebufferId,
    ) -> Result<Self, AtomicKmsError> {
        let mode_blob = ModeBlob::create(DrmModeBlobIo::new(fd.as_raw_fd()), &mode)?;
        let geometry = AtomicPlaneGeometry::fullscreen(width, height)?;
        let request = initial_modeset_request_from_discovery(
            &discovery,
            mode_blob.id(),
            framebuffer,
            geometry,
        )?;
        let test = AtomicSubmission {
            request: request.clone(),
            flags: AtomicCommitFlags::initial_test(),
            user_data: 0,
        };
        let test_started = Instant::now();
        submit_atomic(
            fd,
            &test,
            AtomicKmsErrorKind::TestOnlyRejected,
            "initial atomic TEST_ONLY commit",
        )?;
        let test_only_us = elapsed_micros(test_started);
        let initial_property_count = request.assignment_count();
        let real = AtomicSubmission {
            request,
            flags: AtomicCommitFlags::initial_real(),
            user_data: 0,
        };
        let initial_commit_started = Instant::now();
        if let Err(error) = submit_atomic(
            fd,
            &real,
            AtomicKmsErrorKind::InitialCommitRejected,
            "initial atomic modeset commit",
        ) {
            rollback_pipeline(fd, &discovery);
            return Err(error);
        }
        let initial_commit_us = elapsed_micros(initial_commit_started);
        Ok(Self {
            fd: fd.as_raw_fd(),
            discovery,
            mode_blob,
            mode: Box::new(mode),
            geometry,
            initial_property_count,
            test_only_us,
            initial_commit_us,
            restored: false,
            restore_on_drop: true,
        })
    }

    pub fn submit_flip(
        &self,
        framebuffer: FramebufferId,
        token: PageFlipToken,
    ) -> Result<(), AtomicKmsError> {
        let request = AtomicRequest::primary_flip(
            self.discovery.pipeline.plane,
            self.discovery.pipeline.plane_props.fb_id,
            framebuffer,
        )?;
        let submission = AtomicSubmission::page_flip(request, token);
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        submit_atomic(
            fd,
            &submission,
            AtomicKmsErrorKind::FlipRejected,
            "atomic primary-plane flip",
        )
    }

    pub fn submit_atomic_flip(
        &self,
        request: AtomicFlipRequest,
    ) -> Result<AtomicFlipSubmission, AtomicKmsError> {
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        submit_atomic_flip_with(&self.discovery.pipeline, request, |submission| {
            submit_atomic(
                fd,
                submission,
                AtomicKmsErrorKind::FlipRejected,
                "atomic primary-plane flip with explicit fence",
            )
        })
    }

    pub fn recover(&self, framebuffer: FramebufferId) -> Result<(), AtomicKmsError> {
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        self.discovery.validate_live_pipeline(fd, &self.mode)?;
        let pipeline = &self.discovery.pipeline;
        let request = AtomicRequest::resume_modeset(
            pipeline.connector,
            pipeline.crtc,
            pipeline.plane,
            &pipeline.connector_props,
            &pipeline.crtc_props,
            &pipeline.plane_props,
            self.mode_blob.id(),
            framebuffer,
            self.geometry,
        )?;
        let submission = AtomicSubmission::resume_modeset(request);
        submit_atomic(
            fd,
            &submission,
            AtomicKmsErrorKind::InitialCommitRejected,
            "atomic native-session recovery modeset",
        )
    }

    pub fn disarm_drm_io(&mut self) {
        self.restore_on_drop = false;
        self.restored = true;
        self.mode_blob.disarm();
    }

    pub const fn discovery(&self) -> &AtomicDiscovery {
        &self.discovery
    }

    pub fn restore(&mut self) -> Result<RestorationOutcome, AtomicKmsError> {
        if self.restored {
            return Ok(RestorationOutcome::AlreadyRestored);
        }
        let fd = unsafe { BorrowedFd::borrow_raw(self.fd) };
        let outcome = restore_pipeline(fd, &self.discovery)?;
        self.restored = true;
        Ok(outcome)
    }

    pub const fn mode_blob_id(&self) -> BlobId {
        self.mode_blob.id()
    }

    pub const fn initial_property_count(&self) -> usize {
        self.initial_property_count
    }

    pub const fn test_only_us(&self) -> u64 {
        self.test_only_us
    }

    pub const fn initial_commit_us(&self) -> u64 {
        self.initial_commit_us
    }
}

pub(crate) fn submit_atomic_flip_with(
    pipeline: &AtomicPipelineProperties,
    request: AtomicFlipRequest,
    submit: impl FnOnce(&AtomicSubmission) -> Result<(), AtomicKmsError>,
) -> Result<AtomicFlipSubmission, AtomicKmsError> {
    let mut out_fence_storage = -1i32;
    let out_fence_ptr = pipeline
        .crtc_props
        .out_fence_ptr
        .map(|_| std::ptr::addr_of_mut!(out_fence_storage));
    let atomic_request = AtomicRequest::primary_flip_with_fences(
        pipeline,
        request.framebuffer,
        request.in_fence.as_raw_fd(),
        out_fence_ptr,
    )?;
    let submission = AtomicSubmission::page_flip(atomic_request, request.token);
    let result = submit(&submission);
    match result {
        Ok(()) => Ok(AtomicFlipSubmission {
            out_fence: adopt_out_fence(out_fence_storage),
        }),
        Err(error) => {
            drop(adopt_out_fence(out_fence_storage));
            Err(error)
        }
    }
}

fn adopt_out_fence(raw_fd: i32) -> Option<OwnedFd> {
    (raw_fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(raw_fd) })
}

pub fn initial_modeset_request_from_discovery(
    discovery: &AtomicDiscovery,
    mode_blob: BlobId,
    framebuffer: FramebufferId,
    geometry: AtomicPlaneGeometry,
) -> Result<AtomicRequest, AtomicKmsError> {
    let pipeline = &discovery.pipeline;
    AtomicRequest::initial_modeset(
        pipeline.connector,
        pipeline.crtc,
        pipeline.plane,
        &pipeline.connector_props,
        &pipeline.crtc_props,
        &pipeline.plane_props,
        mode_blob,
        framebuffer,
        geometry,
    )
}

fn elapsed_micros(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

impl Drop for DrmAtomicBackend {
    fn drop(&mut self) {
        if self.restore_on_drop
            && !self.restored
            && let Err(error) = self.restore()
        {
            eprintln!("atomic KMS restore failed: {error}");
        }
    }
}

fn rollback_pipeline(fd: BorrowedFd<'_>, discovery: &AtomicDiscovery) {
    if let Err(error) = restore_pipeline(fd, discovery) {
        eprintln!("atomic KMS startup rollback failed: {error}");
    }
}

fn restore_pipeline(
    fd: BorrowedFd<'_>,
    discovery: &AtomicDiscovery,
) -> Result<RestorationOutcome, AtomicKmsError> {
    let restore = discovery.snapshot.restore_request(&discovery.pipeline)?;
    let restore_test = AtomicSubmission {
        request: restore.clone(),
        flags: AtomicCommitFlags::initial_test(),
        user_data: 0,
    };
    if submit_atomic(
        fd,
        &restore_test,
        AtomicKmsErrorKind::RestoreFailed,
        "atomic restore TEST_ONLY commit",
    )
    .is_ok()
    {
        let restore_real = AtomicSubmission {
            request: restore,
            flags: AtomicCommitFlags::initial_real(),
            user_data: 0,
        };
        if submit_atomic(
            fd,
            &restore_real,
            AtomicKmsErrorKind::RestoreFailed,
            "atomic exact restore commit",
        )
        .is_ok()
        {
            return Ok(RestorationOutcome::Exact);
        }
    }
    let disable = AtomicRequest::safe_disable(&discovery.pipeline)?;
    let disable_test = AtomicSubmission {
        request: disable.clone(),
        flags: AtomicCommitFlags::initial_test(),
        user_data: 0,
    };
    submit_atomic(
        fd,
        &disable_test,
        AtomicKmsErrorKind::RestoreFailed,
        "atomic safe-disable TEST_ONLY commit",
    )?;
    let disable_real = AtomicSubmission {
        request: disable,
        flags: AtomicCommitFlags::initial_real(),
        user_data: 0,
    };
    submit_atomic(
        fd,
        &disable_real,
        AtomicKmsErrorKind::RestoreFailed,
        "atomic safe-disable commit",
    )?;
    Ok(RestorationOutcome::SafeDisable)
}

#[derive(Debug)]
pub enum KmsDisplayBackend {
    Atomic(DrmAtomicBackend),
    Legacy(LegacyKmsBackend),
}

#[derive(Debug)]
pub struct KmsBackendSelection {
    pub requested: KmsPolicy,
    pub fallback_reason: Option<AtomicKmsError>,
    pub backend: KmsDisplayBackend,
}

impl KmsBackendSelection {
    pub fn discover_atomic_pipeline(
        fd: BorrowedFd<'_>,
        connector: ConnectorId,
        crtc: CrtcId,
        framebuffer_format: u32,
    ) -> Result<AtomicDiscovery, AtomicKmsError> {
        discover_atomic_pipeline(
            fd,
            AtomicDiscoveryRequest::new(connector, crtc, framebuffer_format),
        )
    }

    pub fn initialize_atomic_from_discovery(
        fd: BorrowedFd<'_>,
        policy: KmsPolicy,
        discovery: AtomicDiscovery,
        mode: drm_sys::drm_mode_modeinfo,
        width: u32,
        height: u32,
        framebuffer: FramebufferId,
    ) -> Result<Self, AtomicKmsError> {
        let backend = DrmAtomicBackend::initialize_from_discovery(
            fd,
            discovery,
            mode,
            width,
            height,
            framebuffer,
        )?;
        Ok(Self {
            requested: policy,
            fallback_reason: None,
            backend: KmsDisplayBackend::Atomic(backend),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn initialize(
        fd: BorrowedFd<'_>,
        policy: KmsPolicy,
        connector: ConnectorId,
        crtc: CrtcId,
        mode: drm_sys::drm_mode_modeinfo,
        width: u32,
        height: u32,
        framebuffer_format: u32,
        framebuffer: FramebufferId,
    ) -> Result<Self, AtomicKmsError> {
        if policy == KmsPolicy::Legacy {
            return Ok(Self {
                requested: policy,
                fallback_reason: None,
                backend: KmsDisplayBackend::Legacy(LegacyKmsBackend::initialize(
                    fd,
                    connector,
                    crtc,
                    mode,
                    framebuffer,
                )?),
            });
        }
        match Self::discover_atomic_pipeline(fd, connector, crtc, framebuffer_format) {
            Ok(discovery) => {
                match Self::initialize_atomic_from_discovery(
                    fd,
                    policy,
                    discovery,
                    mode,
                    width,
                    height,
                    framebuffer,
                ) {
                    Ok(selection) => Ok(selection),
                    Err(error) => match atomic_failure_action_after_discovery(policy) {
                        AtomicFailureAction::Fail => Err(error),
                        AtomicFailureAction::UseLegacy => unreachable!(
                            "successful Atomic discovery cannot fall back during initialization"
                        ),
                    },
                }
            }
            Err(error) => {
                let phase = error_phase(error.kind);
                if policy.on_atomic_failure(phase) == AtomicFailureAction::UseLegacy {
                    disable_atomic_client_capability(fd);
                    Ok(Self {
                        requested: policy,
                        fallback_reason: Some(error),
                        backend: KmsDisplayBackend::Legacy(LegacyKmsBackend::initialize(
                            fd,
                            connector,
                            crtc,
                            mode,
                            framebuffer,
                        )?),
                    })
                } else {
                    Err(error)
                }
            }
        }
    }

    pub const fn effective_kind(&self) -> KmsBackendKind {
        match self.backend {
            KmsDisplayBackend::Atomic(_) => KmsBackendKind::Atomic,
            KmsDisplayBackend::Legacy(_) => KmsBackendKind::Legacy,
        }
    }

    pub fn submit_flip(
        &self,
        framebuffer: FramebufferId,
        token: PageFlipToken,
    ) -> Result<(), AtomicKmsError> {
        match &self.backend {
            KmsDisplayBackend::Atomic(backend) => backend.submit_flip(framebuffer, token),
            KmsDisplayBackend::Legacy(backend) => backend.submit_flip(framebuffer, token),
        }
    }

    pub fn submit_atomic_flip(
        &self,
        request: AtomicFlipRequest,
    ) -> Result<AtomicFlipSubmission, AtomicKmsError> {
        match &self.backend {
            KmsDisplayBackend::Atomic(backend) => backend.submit_atomic_flip(request),
            KmsDisplayBackend::Legacy(_) => Err(AtomicKmsError::new(
                AtomicKmsErrorKind::Unsupported,
                "legacy KMS cannot submit an explicit-fence Atomic flip",
            )),
        }
    }

    pub fn restore(&mut self) -> Result<RestorationOutcome, AtomicKmsError> {
        match &mut self.backend {
            KmsDisplayBackend::Atomic(backend) => backend.restore(),
            KmsDisplayBackend::Legacy(backend) => backend.restore(),
        }
    }

    pub fn recover(&self, framebuffer: FramebufferId) -> Result<(), AtomicKmsError> {
        match &self.backend {
            KmsDisplayBackend::Atomic(backend) => backend.recover(framebuffer),
            KmsDisplayBackend::Legacy(backend) => backend.recover(framebuffer),
        }
    }

    pub fn disarm_drm_io(&mut self) {
        match &mut self.backend {
            KmsDisplayBackend::Atomic(backend) => backend.disarm_drm_io(),
            KmsDisplayBackend::Legacy(backend) => backend.disarm_drm_io(),
        }
    }

    pub const fn atomic(&self) -> Option<&DrmAtomicBackend> {
        match &self.backend {
            KmsDisplayBackend::Atomic(backend) => Some(backend),
            KmsDisplayBackend::Legacy(_) => None,
        }
    }
}

pub const fn atomic_failure_action_after_discovery(_policy: KmsPolicy) -> AtomicFailureAction {
    AtomicFailureAction::Fail
}

fn error_phase(kind: AtomicKmsErrorKind) -> AtomicFailurePhase {
    match kind {
        AtomicKmsErrorKind::Unsupported => AtomicFailurePhase::Capability,
        AtomicKmsErrorKind::TestOnlyRejected => AtomicFailurePhase::TestOnly,
        AtomicKmsErrorKind::InitialCommitRejected => AtomicFailurePhase::InitialCommit,
        AtomicKmsErrorKind::MissingObject
        | AtomicKmsErrorKind::MissingProperty
        | AtomicKmsErrorKind::DuplicateProperty
        | AtomicKmsErrorKind::MalformedPropertyBlob
        | AtomicKmsErrorKind::NoCompatiblePrimaryPlane
        | AtomicKmsErrorKind::InvalidGeometry
        | AtomicKmsErrorKind::BlobCreation => AtomicFailurePhase::Discovery,
        AtomicKmsErrorKind::InvalidPolicy
        | AtomicKmsErrorKind::DuplicateAssignment
        | AtomicKmsErrorKind::AlreadyPending
        | AtomicKmsErrorKind::FlipRejected
        | AtomicKmsErrorKind::Busy
        | AtomicKmsErrorKind::PermissionOrSession
        | AtomicKmsErrorKind::DeviceLost
        | AtomicKmsErrorKind::RestoreFailed
        | AtomicKmsErrorKind::Io => AtomicFailurePhase::Runtime,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_device_and_internal_errors_never_trigger_auto_fallback() {
        for kind in [
            AtomicKmsErrorKind::PermissionOrSession,
            AtomicKmsErrorKind::DeviceLost,
            AtomicKmsErrorKind::DuplicateAssignment,
            AtomicKmsErrorKind::AlreadyPending,
            AtomicKmsErrorKind::Io,
        ] {
            assert_eq!(
                KmsPolicy::Auto.on_atomic_failure(error_phase(kind)),
                AtomicFailureAction::Fail
            );
        }
    }
}
