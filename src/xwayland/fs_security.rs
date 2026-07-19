//! Filesystem security primitives for X sockets, locks, and authority files.

use std::{
    fs, io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt},
    },
    path::Path,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

impl Identity {
    pub(crate) fn from_path(path: &Path) -> io::Result<Self> {
        let metadata = fs::symlink_metadata(path)?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

pub(crate) fn validate_runtime_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let uid = unsafe { libc::geteuid() } as u32;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != uid
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "XDG_RUNTIME_DIR is not a private directory: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

pub(crate) fn ensure_socket_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            let uid = unsafe { libc::geteuid() } as u32;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || (metadata.uid() != uid && metadata.uid() != 0)
                || (metadata.mode() & 0o002 != 0 && metadata.mode() & 0o1000 == 0)
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("unsafe X11 socket directory: {}", path.display()),
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o1777))?;
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

pub(crate) fn ensure_private_directory(path: &Path) -> io::Result<()> {
    ensure_private_directory_inner(path, true)
}

fn ensure_private_directory_inner(path: &Path, require_private: bool) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            let uid = unsafe { libc::geteuid() } as u32;
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || metadata.uid() != uid
                || (require_private && metadata.mode() & 0o777 != 0o700)
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("unsafe private directory: {}", path.display()),
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "directory has no parent")
            })?;
            ensure_private_directory_inner(parent, false)?;
            fs::create_dir(path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

pub(crate) fn open_directory(path: &Path) -> io::Result<OwnedFd> {
    let fd = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "directory contains NUL"))?;
    let fd = unsafe {
        libc::open(
            fd.as_ptr(),
            libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_RDONLY,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: open returned a new owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub(crate) fn set_socket_mode(fd: RawFd) -> io::Result<()> {
    if unsafe { libc::fchmod(fd, 0o666) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn unlink_owned(path: &Path, expected: Identity) -> bool {
    let Ok(actual) = Identity::from_path(path) else {
        return false;
    };
    if actual != expected {
        return false;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    let Some(name) = path.file_name() else {
        return false;
    };
    let Ok(parent_fd) = open_directory(parent) else {
        return false;
    };
    let Ok(name) = std::ffi::CString::new(name.as_bytes()) else {
        return false;
    };
    let result = unsafe { libc::unlinkat(parent_fd.as_raw_fd(), name.as_ptr(), 0) };
    result == 0
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn symlink_socket_directory_is_rejected() {
        let root = tempfile_dir();
        let target = root.join("target");
        fs::create_dir(&target).expect("target");
        symlink(&target, root.join("socket")).expect("symlink");
        assert!(ensure_socket_directory(&root.join("socket")).is_err());
        let _ = fs::remove_dir_all(root);
    }

    fn tempfile_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!("typhon-security-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).expect("temporary directory");
        path
    }
}
