use std::{
    fs::{self, OpenOptions},
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::Path,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GpuGlobal {
    LinuxDmabuf,
    LinuxDrmSyncobj,
    WlDrm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmabufProtocolVersion {
    None,
    V1,
    V3,
    V4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderNodeKind {
    Missing,
    Empty,
    Nonexistent,
    RegularFile,
    Inaccessible,
    CardNode,
    RenderNode,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderNodeEvidence {
    pub path: Option<String>,
    pub kind: RenderNodeKind,
    pub device_identity: Option<u64>,
    pub canonical: bool,
    pub openable: bool,
}

impl RenderNodeEvidence {
    pub fn new(
        path: Option<impl Into<String>>,
        kind: RenderNodeKind,
        device_identity: Option<u64>,
        openable: bool,
    ) -> Self {
        let path = path.map(Into::into);
        Self {
            canonical: path.as_deref().is_some_and(|path| !path.is_empty()),
            path,
            kind,
            device_identity,
            openable,
        }
    }

    pub fn missing() -> Self {
        Self::new(None::<String>, RenderNodeKind::Missing, None, false)
    }

    pub fn empty() -> Self {
        Self::new(Some(String::new()), RenderNodeKind::Empty, None, false)
    }
}

pub fn inspect_render_node(path: Option<&Path>) -> RenderNodeEvidence {
    let Some(path) = path else {
        return RenderNodeEvidence::missing();
    };
    if path.as_os_str().is_empty() {
        return RenderNodeEvidence::empty();
    }

    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return RenderNodeEvidence::new(
                Some(path.to_string_lossy().into_owned()),
                RenderNodeKind::Nonexistent,
                None,
                false,
            );
        }
        Err(_) => {
            return RenderNodeEvidence::new(
                Some(path.to_string_lossy().into_owned()),
                RenderNodeKind::Inaccessible,
                None,
                false,
            );
        }
    };

    let kind = if metadata.file_type().is_char_device() {
        match path.file_name().and_then(|name| name.to_str()) {
            Some(name) if name.starts_with("renderD") => RenderNodeKind::RenderNode,
            Some(name) if name.starts_with("card") => RenderNodeKind::CardNode,
            _ => RenderNodeKind::Other,
        }
    } else if metadata.is_file() {
        RenderNodeKind::RegularFile
    } else {
        RenderNodeKind::Other
    };
    let openable = OpenOptions::new().read(true).write(true).open(path).is_ok();
    let canonical = fs::canonicalize(path)
        .ok()
        .is_some_and(|canonical| canonical == path);

    RenderNodeEvidence {
        path: Some(path.to_string_lossy().into_owned()),
        kind,
        device_identity: Some(metadata.rdev()),
        canonical,
        openable,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GpuFormat {
    pub fourcc: u32,
    pub modifier: u64,
}

impl GpuFormat {
    pub const fn new(fourcc: u32, modifier: u64) -> Self {
        Self { fourcc, modifier }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuProtocolProbe {
    pub kms_device: Option<String>,
    pub dmabuf_device: Option<u64>,
    pub render_node: RenderNodeEvidence,
    pub importer_formats: Vec<GpuFormat>,
    pub feedback_format_table: Vec<GpuFormat>,
    pub feedback_main_device: Option<u64>,
    pub feedback_tranches_valid: bool,
    pub feedback_has_modifiers: bool,
    pub basic_import_valid: bool,
    pub syncobj_device: Option<u64>,
    pub syncobj_timeline_create: bool,
    pub syncobj_timeline_import: bool,
    pub wl_drm_prime: bool,
    pub wl_drm_magic_authentication: bool,
}

impl GpuProtocolProbe {
    pub fn valid_for_tests() -> Self {
        let formats = vec![
            GpuFormat::new(u32::from_le_bytes(*b"AR24"), 0),
            GpuFormat::new(u32::from_le_bytes(*b"XR24"), 0),
        ];
        Self {
            kms_device: Some("/dev/dri/card0".to_owned()),
            dmabuf_device: Some(7),
            render_node: RenderNodeEvidence::new(
                Some("/dev/dri/renderD128"),
                RenderNodeKind::RenderNode,
                Some(7),
                true,
            ),
            importer_formats: formats.clone(),
            feedback_format_table: formats,
            feedback_main_device: Some(7),
            feedback_tranches_valid: true,
            feedback_has_modifiers: true,
            basic_import_valid: true,
            syncobj_device: Some(7),
            syncobj_timeline_create: true,
            syncobj_timeline_import: true,
            wl_drm_prime: true,
            wl_drm_magic_authentication: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuProtocolDiagnostic {
    pub global: GpuGlobal,
    pub enabled: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuDeviceDiagnostics {
    pub selected_kms_device: Option<String>,
    pub dmabuf_device: Option<u64>,
    pub render_node_path: Option<String>,
    pub render_node_kind: RenderNodeKind,
    pub render_node_canonical: bool,
    pub render_node_openable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuProtocolCapabilities {
    selected_kms_device: Option<String>,
    dmabuf_device: Option<u64>,
    render_node_path: Option<String>,
    dmabuf_version: DmabufProtocolVersion,
    dmabuf_formats: Vec<GpuFormat>,
    syncobj_enabled: bool,
    wl_drm_enabled: bool,
    wl_drm_device: Option<String>,
    wl_drm_formats: Vec<u32>,
    wl_drm_prime: bool,
    wl_drm_authentication: bool,
    device_diagnostics: GpuDeviceDiagnostics,
    diagnostics: Vec<GpuProtocolDiagnostic>,
}

impl GpuProtocolCapabilities {
    pub fn from_probe(probe: GpuProtocolProbe) -> Self {
        let common_formats = probe
            .feedback_format_table
            .iter()
            .copied()
            .filter(|format| probe.importer_formats.contains(format))
            .collect::<Vec<_>>();
        let basic_dmabuf = probe.basic_import_valid && !probe.importer_formats.is_empty();
        let valid_v4 = probe.dmabuf_device.is_some_and(|device| {
            probe.basic_import_valid
                && probe.feedback_main_device == Some(device)
                && device != 0
                && !probe.feedback_format_table.is_empty()
                && probe.feedback_tranches_valid
                && !common_formats.is_empty()
        });
        let valid_v3 = probe.basic_import_valid
            && !probe.feedback_format_table.is_empty()
            && probe.feedback_has_modifiers
            && !common_formats.is_empty();
        let wl_drm_formats = common_formats
            .iter()
            .filter(|format| format.modifier == 0)
            .map(|format| format.fourcc)
            .fold(Vec::new(), |mut formats, fourcc| {
                if !formats.contains(&fourcc) {
                    formats.push(fourcc);
                }
                formats
            });
        let has_common_formats = !common_formats.is_empty();

        let (dmabuf_version, dmabuf_formats, dmabuf_reason) = if valid_v4 {
            (
                DmabufProtocolVersion::V4,
                common_formats,
                "valid main-device feedback, format table, tranches, and importer intersection"
                    .to_owned(),
            )
        } else if valid_v3 {
            (
                DmabufProtocolVersion::V3,
                common_formats,
                "valid modifier events and importer intersection".to_owned(),
            )
        } else if basic_dmabuf {
            (
                DmabufProtocolVersion::V1,
                probe.importer_formats.clone(),
                "validated basic dmabuf import".to_owned(),
            )
        } else {
            (
                DmabufProtocolVersion::None,
                Vec::new(),
                "no complete dmabuf import contract".to_owned(),
            )
        };

        let syncobj_enabled = probe.dmabuf_device.is_some_and(|device| {
            probe.syncobj_device == Some(device)
                && probe.syncobj_timeline_create
                && probe.syncobj_timeline_import
        });
        let syncobj_reason = if syncobj_enabled {
            "selected dmabuf device supports timeline creation and import".to_owned()
        } else {
            "selected dmabuf device lacks a complete timeline create/import contract".to_owned()
        };

        let wl_drm_reason = if probe.render_node.kind != RenderNodeKind::RenderNode {
            match probe.render_node.kind {
                RenderNodeKind::Missing => "missing render-node path".to_owned(),
                RenderNodeKind::Empty => "empty render-node path".to_owned(),
                RenderNodeKind::Nonexistent => "render-node path does not exist".to_owned(),
                RenderNodeKind::RegularFile | RenderNodeKind::Other => {
                    "path is not a character render node".to_owned()
                }
                RenderNodeKind::Inaccessible => {
                    "render node cannot be opened with O_RDWR".to_owned()
                }
                RenderNodeKind::CardNode => {
                    "card node is not a canonical render-node contract".to_owned()
                }
                RenderNodeKind::RenderNode => unreachable!(),
            }
        } else if !probe.render_node.canonical {
            "render-node path is not canonical".to_owned()
        } else if !probe.render_node.openable {
            "render node cannot be opened with O_RDWR".to_owned()
        } else if probe.render_node.device_identity != probe.dmabuf_device {
            "render node device identity does not match the dmabuf importer".to_owned()
        } else if !probe.wl_drm_prime {
            "selected render node does not support PRIME".to_owned()
        } else if probe.feedback_format_table.is_empty() || !has_common_formats {
            "wl_drm formats do not match the importer".to_owned()
        } else if wl_drm_formats.is_empty() {
            "wl_drm has no common linear importer formats".to_owned()
        } else {
            "canonical matching render node, PRIME, openability, and formats are valid".to_owned()
        };
        let wl_drm_enabled = wl_drm_reason
            == "canonical matching render node, PRIME, openability, and formats are valid";
        let wl_drm_authentication = wl_drm_enabled && probe.wl_drm_magic_authentication;
        let device_diagnostics = GpuDeviceDiagnostics {
            selected_kms_device: probe.kms_device.clone(),
            dmabuf_device: probe.dmabuf_device,
            render_node_path: probe.render_node.path.clone(),
            render_node_kind: probe.render_node.kind,
            render_node_canonical: probe.render_node.canonical,
            render_node_openable: probe.render_node.openable,
        };

        let diagnostics = vec![
            GpuProtocolDiagnostic {
                global: GpuGlobal::LinuxDmabuf,
                enabled: dmabuf_version != DmabufProtocolVersion::None,
                reason: dmabuf_reason,
            },
            GpuProtocolDiagnostic {
                global: GpuGlobal::LinuxDrmSyncobj,
                enabled: syncobj_enabled,
                reason: syncobj_reason,
            },
            GpuProtocolDiagnostic {
                global: GpuGlobal::WlDrm,
                enabled: wl_drm_enabled,
                reason: wl_drm_reason,
            },
        ];

        Self {
            selected_kms_device: probe.kms_device,
            dmabuf_device: probe.dmabuf_device,
            render_node_path: probe.render_node.path.clone(),
            dmabuf_version,
            dmabuf_formats,
            syncobj_enabled,
            wl_drm_enabled,
            wl_drm_device: wl_drm_enabled.then_some(probe.render_node.path).flatten(),
            wl_drm_formats: if wl_drm_enabled {
                wl_drm_formats
            } else {
                Vec::new()
            },
            wl_drm_prime: wl_drm_enabled && probe.wl_drm_prime,
            wl_drm_authentication,
            device_diagnostics,
            diagnostics,
        }
    }

    pub fn global_enabled(&self, global: GpuGlobal) -> bool {
        match global {
            GpuGlobal::LinuxDmabuf => self.dmabuf_version != DmabufProtocolVersion::None,
            GpuGlobal::LinuxDrmSyncobj => self.syncobj_enabled,
            GpuGlobal::WlDrm => self.wl_drm_enabled,
        }
    }

    pub fn any_global_enabled(&self) -> bool {
        self.global_enabled(GpuGlobal::LinuxDmabuf)
            || self.global_enabled(GpuGlobal::LinuxDrmSyncobj)
            || self.global_enabled(GpuGlobal::WlDrm)
    }

    pub fn dmabuf_version(&self) -> DmabufProtocolVersion {
        self.dmabuf_version
    }

    pub fn dmabuf_formats(&self) -> &[GpuFormat] {
        &self.dmabuf_formats
    }

    pub fn selected_kms_device(&self) -> Option<&str> {
        self.selected_kms_device.as_deref()
    }

    pub fn dmabuf_device(&self) -> Option<u64> {
        self.dmabuf_device
    }

    pub fn render_node_path(&self) -> Option<&str> {
        self.render_node_path.as_deref()
    }

    pub fn syncobj_enabled(&self) -> bool {
        self.syncobj_enabled
    }

    pub fn wl_drm_enabled(&self) -> bool {
        self.wl_drm_enabled
    }

    pub fn wl_drm_device(&self) -> Option<&str> {
        self.wl_drm_device.as_deref()
    }

    pub fn wl_drm_formats(&self) -> &[u32] {
        &self.wl_drm_formats
    }

    pub fn wl_drm_prime(&self) -> bool {
        self.wl_drm_prime
    }

    pub fn wl_drm_authentication(&self) -> bool {
        self.wl_drm_authentication
    }

    pub fn device_diagnostics(&self) -> &GpuDeviceDiagnostics {
        &self.device_diagnostics
    }

    pub fn wl_drm_disable_reason(&self) -> &str {
        self.diagnostics
            .iter()
            .find(|entry| entry.global == GpuGlobal::WlDrm)
            .map_or("wl_drm diagnostic unavailable", |entry| &entry.reason)
    }

    pub fn diagnostics(&self) -> &[GpuProtocolDiagnostic] {
        &self.diagnostics
    }
}

impl DmabufProtocolVersion {
    pub const fn wayland_version(self) -> u32 {
        match self {
            Self::None => 0,
            Self::V1 => 1,
            Self::V3 => 3,
            Self::V4 => 4,
        }
    }
}

impl Default for GpuProtocolCapabilities {
    fn default() -> Self {
        Self::from_probe(GpuProtocolProbe {
            kms_device: None,
            dmabuf_device: None,
            render_node: RenderNodeEvidence::missing(),
            importer_formats: Vec::new(),
            feedback_format_table: Vec::new(),
            feedback_main_device: None,
            feedback_tranches_valid: false,
            feedback_has_modifiers: false,
            basic_import_valid: false,
            syncobj_device: None,
            syncobj_timeline_create: false,
            syncobj_timeline_import: false,
            wl_drm_prime: false,
            wl_drm_magic_authentication: false,
        })
    }
}

#[cfg(test)]
impl GpuProtocolCapabilities {
    pub(crate) fn test_contract(syncobj_available: bool) -> Self {
        let mut probe = GpuProtocolProbe::valid_for_tests();
        probe.syncobj_device = syncobj_available.then_some(7);
        probe.syncobj_timeline_create = syncobj_available;
        probe.syncobj_timeline_import = syncobj_available;
        Self::from_probe(probe)
    }
}
