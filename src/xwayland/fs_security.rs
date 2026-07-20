//! Filesystem security primitives for X sockets, locks, and authority files.

use std::{
    fs, io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt},
    },
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) device: u64,
    pub(crate) inode: u64,
}

impl Identity {
    pub(crate) fn from_fd(fd: RawFd) -> io::Result<Self> {
        let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
        if unsafe { libc::fstat(fd, &mut stat) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            device: stat.st_dev,
            inode: stat.st_ino,
        })
    }

    pub(crate) fn from_at(parent_fd: i32, name: &std::ffi::OsStr) -> io::Result<Self> {
        let stat = stat_at(parent_fd, name)?;
        Ok(Self {
            device: stat.st_dev,
            inode: stat.st_ino,
        })
    }
}

pub(crate) fn stat_at(parent_fd: RawFd, name: &std::ffi::OsStr) -> io::Result<libc::stat> {
    let name = std::ffi::CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "entry contains NUL"))?;
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    let result = unsafe {
        libc::fstatat(
            parent_fd,
            name.as_ptr(),
            &mut stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(stat)
}

pub(crate) fn open_read_at(parent_fd: RawFd, name: &std::ffi::OsStr) -> io::Result<fs::File> {
    let name = std::ffi::CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "entry contains NUL"))?;
    let fd = unsafe {
        libc::openat(
            parent_fd,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a new owned descriptor.
    Ok(unsafe { fs::File::from_raw_fd(fd) })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnershipCleanup {
    Removed,
    Missing,
    OwnershipMismatch,
    Failed,
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
            if metadata.file_type().is_symlink() || !metadata.is_dir() || metadata.uid() != uid {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("unsafe private directory: {}", path.display()),
                ));
            }
            if require_private && metadata.mode() & 0o777 != 0o700 {
                let fd = open_directory(path)?;
                if unsafe { libc::fchmod(fd.as_raw_fd(), 0o700) } < 0 {
                    return Err(io::Error::last_os_error());
                }
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

pub(crate) fn set_socket_mode_at(
    parent_fd: RawFd,
    name: &std::ffi::OsStr,
    expected: Identity,
) -> io::Result<()> {
    let name = std::ffi::CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "socket name contains NUL"))?;
    let fd = unsafe {
        libc::openat(
            parent_fd,
            name.as_ptr(),
            libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openat returned a new owned descriptor.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    if Identity::from_fd(fd.as_raw_fd())? != expected {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "X11 socket changed before mode update",
        ));
    }
    let proc_path = std::ffi::CString::new(format!("/proc/self/fd/{}", fd.as_raw_fd()))
        .expect("proc fd path contains no NUL");
    if unsafe { libc::chmod(proc_path.as_ptr(), 0o666) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn create_lock_at(
    parent_fd: i32,
    name: &std::ffi::OsStr,
) -> io::Result<Option<std::fs::File>> {
    let name = std::ffi::CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "lock name contains NUL"))?;
    let fd = unsafe {
        libc::openat(
            parent_fd,
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o644,
        )
    };
    if fd < 0 {
        return if io::Error::last_os_error().kind() == io::ErrorKind::AlreadyExists {
            Ok(None)
        } else {
            Err(io::Error::last_os_error())
        };
    }
    // SAFETY: openat returned a new owned descriptor.
    Ok(Some(unsafe { std::fs::File::from_raw_fd(fd) }))
}

pub(crate) fn unlink_owned_at(
    parent_fd: i32,
    name: &std::ffi::OsStr,
    expected: Identity,
) -> OwnershipCleanup {
    let Ok(current) = Identity::from_at(parent_fd, name) else {
        return OwnershipCleanup::Missing;
    };
    if current != expected {
        return OwnershipCleanup::OwnershipMismatch;
    }

    static CLEANUP_SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let name_bytes = name.as_bytes();
    for _ in 0..16 {
        let suffix = CLEANUP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut quarantine = std::ffi::OsString::from(".typhon-cleanup-");
        quarantine.push(format!("{}-{suffix}", std::process::id()));
        let Ok(quarantine_c) = std::ffi::CString::new(quarantine.as_bytes()) else {
            return OwnershipCleanup::Failed;
        };
        let name_c = match std::ffi::CString::new(name_bytes) {
            Ok(name) => name,
            Err(_) => return OwnershipCleanup::Failed,
        };
        let rename = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                parent_fd,
                name_c.as_ptr(),
                parent_fd,
                quarantine_c.as_ptr(),
                1u32,
            )
        };
        if rename < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::AlreadyExists {
                continue;
            }
            return if error.kind() == io::ErrorKind::NotFound {
                OwnershipCleanup::Missing
            } else {
                OwnershipCleanup::Failed
            };
        }

        let quarantine_identity = Identity::from_at(parent_fd, &quarantine);
        if matches!(quarantine_identity, Ok(identity) if identity == expected) {
            let result = unsafe { libc::unlinkat(parent_fd, quarantine_c.as_ptr(), 0) };
            return if result == 0 {
                OwnershipCleanup::Removed
            } else {
                OwnershipCleanup::Failed
            };
        }

        // The entry changed between validation and the atomic move.  Restore
        // the foreign entry without replacing anything another process may
        // have installed at the original name.
        let _ = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                parent_fd,
                quarantine_c.as_ptr(),
                parent_fd,
                name_c.as_ptr(),
                1u32,
            )
        };
        return OwnershipCleanup::OwnershipMismatch;
    }
    OwnershipCleanup::Failed
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

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

    #[test]
    fn lock_replacement_before_cleanup_preserves_foreign_entry() {
        replacement_before_cleanup_preserves_foreign_entry(".X0-lock");
    }

    #[test]
    fn socket_replacement_before_cleanup_preserves_foreign_entry() {
        replacement_before_cleanup_preserves_foreign_entry("X0");
    }

    #[test]
    fn authority_replacement_before_cleanup_preserves_foreign_entry() {
        replacement_before_cleanup_preserves_foreign_entry(".Xauthority-0-cookie");
    }

    fn replacement_before_cleanup_preserves_foreign_entry(name: &str) {
        let root = tempfile_dir();
        let path = root.join(name);
        fs::write(&path, b"owned").expect("owned artifact");
        let directory = open_directory(&root).expect("open directory");
        let identity = Identity::from_at(
            directory.as_raw_fd(),
            path.file_name().expect("artifact name"),
        )
        .expect("owned identity");
        let replacement = root.join(format!("{name}.replacement"));
        fs::write(&replacement, b"foreign").expect("foreign artifact");
        fs::rename(&replacement, &path).expect("replace artifact");

        assert_eq!(
            unlink_owned_at(
                directory.as_raw_fd(),
                path.file_name().expect("artifact name"),
                identity,
            ),
            OwnershipCleanup::OwnershipMismatch
        );
        assert_eq!(
            fs::read(&path).expect("foreign artifact survives"),
            b"foreign"
        );
        let _ = fs::remove_dir_all(root);
    }

    fn tempfile_dir() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("typhon-security-{}-{nonce}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).expect("temporary directory");
        path
    }
}
