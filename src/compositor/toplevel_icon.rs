//! Immutable, bounded window icons. Pixel snapshots never retain client memory.
use super::{CompositorState, shm::ShmBufferData};
use crate::render_backend::buffer::{BufferSize, ShmBufferSnapshot};
use std::{
    collections::HashMap,
    fs::File,
    io::{self, Write},
    os::fd::{AsFd, FromRawFd},
    sync::{Arc, Mutex},
};
use wayland_protocols::xdg::toplevel_icon::v1::server::xdg_toplevel_icon_v1;
use wayland_server::{
    Resource,
    backend::{ClientId, ObjectId},
};

pub(super) const MAX_ICON_EDGE: u32 = 256;
pub(super) const MAX_SOURCE_EDGE: i32 = 1024;
pub(super) const MAX_ICON_BUFFERS: usize = 8;
pub(super) const MAX_ICON_OBJECTS_PER_CLIENT: usize = 16;
pub(super) const MAX_ICON_OBJECTS: usize = 128;
pub(super) const MAX_ICON_SURFACES: usize = 256;

#[derive(Debug, Clone)]
pub(super) struct ToplevelIcon {
    pub name: String,
    pub image: Option<ShmBufferSnapshot>,
    pub file: Option<Arc<File>>,
}

impl PartialEq for ToplevelIcon {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.image == other.image
    }
}
impl Eq for ToplevelIcon {}

impl ToplevelIcon {
    pub(super) fn new(name: String, image: Option<ShmBufferSnapshot>) -> io::Result<Self> {
        let file = if let Some(image) = &image {
            let fd = unsafe {
                libc::memfd_create(
                    c"typhon-window-icon".as_ptr(),
                    libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let mut file = unsafe { File::from_raw_fd(fd) };
            file.write_all(bytemuck::cast_slice(image.pixels()))?;
            let seals =
                libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;
            if unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, seals) } < 0 {
                return Err(io::Error::last_os_error());
            }
            Some(Arc::new(file))
        } else {
            None
        };
        Ok(Self { name, image, file })
    }
}

#[derive(Debug)]
pub(super) struct IconBuffer {
    pub resource: wayland_server::protocol::wl_buffer::WlBuffer,
    pub scale: i32,
    pub source_edge: i32,
    pub image: ShmBufferSnapshot,
}

#[derive(Debug, Default)]
pub(super) struct IconBuilder {
    pub immutable: bool,
    pub name: String,
    pub buffers: Vec<IconBuffer>,
}

#[derive(Debug, Default)]
pub(super) struct ToplevelIcons {
    pub resources: HashMap<ObjectId, (ClientId, xdg_toplevel_icon_v1::XdgToplevelIconV1)>,
    pub pending: HashMap<u32, Option<Arc<ToplevelIcon>>>,
    pub current: HashMap<u32, Arc<ToplevelIcon>>,
}

impl ToplevelIcons {
    pub fn remove_surface(&mut self, surface_id: u32) {
        self.pending.remove(&surface_id);
        self.current.remove(&surface_id);
    }

    pub fn buffer_destroyed(&self, buffer_id: ObjectId) {
        for (_, icon) in self.resources.values() {
            if let Some(data) = icon.data::<Mutex<IconBuilder>>()
                && let Ok(data) = data.lock()
                && data
                    .buffers
                    .iter()
                    .any(|buffer| buffer.resource.id() == buffer_id)
            {
                icon.post_error(
                    xdg_toplevel_icon_v1::Error::NoBuffer,
                    "icon buffer destroyed before its icon",
                );
            }
        }
    }
}

impl CompositorState {
    pub(super) fn commit_toplevel_icon(&mut self, surface_id: u32) {
        let Some(pending) = self.toplevel_icons.pending.remove(&surface_id) else {
            return;
        };
        let changed = self.toplevel_icons.current.get(&surface_id) != pending.as_ref();
        if let Some(icon) = pending {
            self.toplevel_icons.current.insert(surface_id, icon);
        } else {
            self.toplevel_icons.current.remove(&surface_id);
        }
        if changed && let Some(window_id) = self.window_id_for_surface(surface_id) {
            self.mark_astrea_toplevel_dirty(window_id);
        }
    }
}

pub(super) fn snapshot_icon_buffer(
    data: &ShmBufferData,
    scale: i32,
) -> io::Result<ShmBufferSnapshot> {
    if scale <= 0 || data.width != data.height || data.width <= 0 || data.width > MAX_SOURCE_EDGE {
        return Err(io::Error::other(
            "icon must be square wl_shm, at most 1024 pixels, with positive scale",
        ));
    }
    let source = data.read_pixels()?;
    let source_edge = data.width as u32;
    let edge = source_edge.min(MAX_ICON_EDGE);
    let pixels = if edge == source_edge {
        source
    } else {
        (0..edge)
            .flat_map(|y| (0..edge).map(move |x| (x, y)))
            .map(|(x, y)| {
                source[((y * source_edge / edge) * source_edge + x * source_edge / edge) as usize]
            })
            .collect()
    };
    ShmBufferSnapshot::new(
        BufferSize::new(edge, edge).ok_or_else(|| io::Error::other("invalid icon size"))?,
        pixels,
    )
    .map_err(|error| io::Error::other(format!("{error:?}")))
}

pub(super) fn send_icon(
    resource: &crate::astrea_toplevel_management::server::astrea_toplevel_v1::AstreaToplevelV1,
    icon: Option<&ToplevelIcon>,
) -> Result<(), wayland_server::backend::InvalidId> {
    use crate::astrea_toplevel_management::server::astrea_toplevel_v1::Event;
    if resource.version() < 4 {
        return Ok(());
    }
    resource.send_event(Event::IconName {
        name: icon.map_or_else(String::new, |icon| icon.name.clone()),
    })?;
    if let Some(icon) = icon
        && let (Some(image), Some(file)) = (&icon.image, &icon.file)
    {
        resource.send_event(Event::IconPixels {
            pixels: file.as_fd(),
            size: image.size().width,
        })?;
    }
    Ok(())
}
