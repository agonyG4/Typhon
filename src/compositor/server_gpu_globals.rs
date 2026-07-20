use wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_v1;
use wayland_protocols::wp::linux_drm_syncobj::v1::server::wp_linux_drm_syncobj_manager_v1;
use wayland_server::DisplayHandle;

use crate::wayland_drm::server::wl_drm;

use super::super::gpu_protocol_capabilities::{GpuGlobal, GpuProtocolCapabilities};
use super::super::{CompositorState, protocols::versions};

pub(super) fn register_gpu_buffer_globals(
    display: &DisplayHandle,
    capabilities: &GpuProtocolCapabilities,
) {
    if capabilities.global_enabled(GpuGlobal::LinuxDmabuf) {
        display.create_global::<CompositorState, zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, _>(
            capabilities.dmabuf_version().wayland_version(),
            (),
        );
    }
    if capabilities.global_enabled(GpuGlobal::LinuxDrmSyncobj) {
        display.create_global::<
            CompositorState,
            wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1,
            _,
        >(versions::WP_LINUX_DRM_SYNCOBJ_MANAGER_V1, ());
    }
    if capabilities.global_enabled(GpuGlobal::WlDrm) {
        display.create_global::<CompositorState, wl_drm::WlDrm, _>(versions::WL_DRM, ());
    }
}
