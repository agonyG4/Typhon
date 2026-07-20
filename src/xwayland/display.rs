use std::{
    env,
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsRawFd, FromRawFd, RawFd},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
};

use super::fs_security;

type FileIdentity = fs_security::Identity;

#[derive(Debug)]
pub(crate) struct DisplayLease {
    display_number: u32,
    display: String,
    lock_path: PathBuf,
    lock_identity: FileIdentity,
    filesystem_socket_path: PathBuf,
    filesystem_listener: UnixListener,
    filesystem_socket_identity: FileIdentity,
    abstract_listener: UnixListener,
    xauthority_path: PathBuf,
    xauthority_identity: FileIdentity,
    _lock_directory_fd: std::os::fd::OwnedFd,
    _socket_directory_fd: std::os::fd::OwnedFd,
    _auth_directory_fd: std::os::fd::OwnedFd,
}

impl DisplayLease {
    pub(crate) fn allocate(min: u32, max: u32) -> io::Result<Self> {
        let runtime_dir = env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "XDG_RUNTIME_DIR is required for the private XWayland lease",
            )
        })?;
        let runtime_dir = PathBuf::from(runtime_dir);
        fs_security::validate_runtime_directory(&runtime_dir)?;
        Self::allocate_at(
            Path::new("/tmp"),
            Path::new("/tmp/.X11-unix"),
            Path::new("/tmp/.X11-unix"),
            &runtime_dir.join("typhon/xwayland"),
            min,
            max,
        )
    }

    #[cfg(test)]
    pub(crate) fn allocate_for_tests(root: &Path, min: u32, max: u32) -> io::Result<Self> {
        Self::allocate_at(
            root,
            &root.join(".X11-unix"),
            &root.join(".X11-unix"),
            &root.join("typhon/xwayland"),
            min,
            max,
        )
    }

    fn allocate_at(
        lock_directory: &Path,
        socket_directory: &Path,
        abstract_socket_directory: &Path,
        auth_directory: &Path,
        min: u32,
        max: u32,
    ) -> io::Result<Self> {
        if min > max {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "XWayland display range is inverted",
            ));
        }
        ensure_socket_directory(socket_directory)?;
        for display_number in min..=max {
            match Self::try_allocate(
                lock_directory,
                socket_directory,
                abstract_socket_directory,
                auth_directory,
                display_number,
            ) {
                Ok(Some(lease)) => return Ok(lease),
                Ok(None) => continue,
                Err(error) if is_slot_local_error(&error) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "no private X display is available",
        ))
    }

    fn try_allocate(
        lock_directory: &Path,
        socket_directory: &Path,
        abstract_socket_directory: &Path,
        auth_directory: &Path,
        display_number: u32,
    ) -> io::Result<Option<Self>> {
        fs_security::ensure_private_directory(auth_directory)?;
        let lock_directory_fd = fs_security::open_directory(lock_directory)?;
        let socket_directory_fd = fs_security::open_directory(socket_directory)?;
        let auth_directory_fd = fs_security::open_directory(auth_directory)?;
        let lock_path = lock_directory.join(format!(".X{display_number}-lock"));
        let socket_path = socket_directory.join(format!("X{display_number}"));
        let lock_name = lock_path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "lock has no name"))?;
        let socket_name = socket_path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "socket has no name"))?;
        if let Some((should_skip, existing_identity)) =
            inspect_existing_lock(lock_directory_fd.as_raw_fd(), lock_name, &socket_path)?
        {
            if should_skip {
                return Ok(None);
            }
            match fs_security::unlink_owned_at(
                lock_directory_fd.as_raw_fd(),
                lock_name,
                existing_identity,
            ) {
                fs_security::OwnershipCleanup::Removed | fs_security::OwnershipCleanup::Missing => {
                }
                fs_security::OwnershipCleanup::OwnershipMismatch => return Ok(None),
                fs_security::OwnershipCleanup::Failed => {
                    return Err(io::Error::other("could not retire stale X11 display lock"));
                }
            }
        }

        let lock_file = match fs_security::create_lock_at(lock_directory_fd.as_raw_fd(), lock_name)?
        {
            Some(file) => file,
            None => return Ok(None),
        };
        let lock_identity = FileIdentity::from_at(lock_directory_fd.as_raw_fd(), lock_name)?;
        if let Err(error) = write_lock_pid(&lock_file) {
            cleanup_artifact_at(&lock_directory_fd, lock_name, lock_identity);
            return Err(error);
        }

        if let Err(error) =
            reject_or_remove_socket(socket_directory_fd.as_raw_fd(), socket_name, &socket_path)
        {
            cleanup_artifact_at(&lock_directory_fd, lock_name, lock_identity);
            if error.kind() == io::ErrorKind::AddrInUse {
                return Ok(None);
            }
            return Err(error);
        }
        let filesystem_listener = match UnixListener::bind(&socket_path) {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                cleanup_artifact_at(&lock_directory_fd, lock_name, lock_identity);
                return Ok(None);
            }
            Err(error) => {
                cleanup_artifact_at(&lock_directory_fd, lock_name, lock_identity);
                return Err(error);
            }
        };
        let filesystem_socket_identity =
            match FileIdentity::from_at(socket_directory_fd.as_raw_fd(), socket_name) {
                Ok(identity) => identity,
                Err(error) => {
                    cleanup_artifact_at(&lock_directory_fd, lock_name, lock_identity);
                    return Err(error);
                }
            };
        if let Err(error) = fs_security::set_socket_mode_at(
            socket_directory_fd.as_raw_fd(),
            socket_name,
            filesystem_socket_identity,
        ) {
            cleanup_artifact_at(&lock_directory_fd, lock_name, lock_identity);
            cleanup_artifact_at(
                &socket_directory_fd,
                socket_name,
                filesystem_socket_identity,
            );
            return Err(error);
        }
        if let Err(error) = filesystem_listener.set_nonblocking(true) {
            cleanup_artifact_at(&lock_directory_fd, lock_name, lock_identity);
            cleanup_artifact_at(
                &socket_directory_fd,
                socket_name,
                filesystem_socket_identity,
            );
            return Err(error);
        }
        let abstract_listener =
            match bind_abstract_listener(abstract_socket_directory, display_number) {
                Ok(listener) => listener,
                Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                    cleanup_artifact_at(&lock_directory_fd, lock_name, lock_identity);
                    cleanup_artifact_at(
                        &socket_directory_fd,
                        socket_name,
                        filesystem_socket_identity,
                    );
                    return Ok(None);
                }
                Err(error) => {
                    cleanup_artifact_at(&lock_directory_fd, lock_name, lock_identity);
                    cleanup_artifact_at(
                        &socket_directory_fd,
                        socket_name,
                        filesystem_socket_identity,
                    );
                    return Err(error);
                }
            };
        let auth_file = match super::auth::create_auth_file_at(
            auth_directory_fd.as_raw_fd(),
            auth_directory,
            display_number,
        ) {
            Ok(auth_file) => auth_file,
            Err(error) => {
                cleanup_artifact_at(&lock_directory_fd, lock_name, lock_identity);
                cleanup_artifact_at(
                    &socket_directory_fd,
                    socket_name,
                    filesystem_socket_identity,
                );
                return Err(error);
            }
        };
        let auth_name = auth_file
            .path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "authority has no name"))?;
        let xauthority_identity =
            match FileIdentity::from_at(auth_directory_fd.as_raw_fd(), auth_name) {
                Ok(identity) => identity,
                Err(error) => {
                    cleanup_artifact_at(&lock_directory_fd, lock_name, lock_identity);
                    cleanup_artifact_at(
                        &socket_directory_fd,
                        socket_name,
                        filesystem_socket_identity,
                    );
                    return Err(error);
                }
            };

        Ok(Some(Self {
            display_number,
            display: format!(":{display_number}"),
            lock_path,
            lock_identity,
            filesystem_socket_path: socket_path,
            filesystem_listener,
            filesystem_socket_identity,
            abstract_listener,
            xauthority_path: auth_file.path,
            xauthority_identity,
            _lock_directory_fd: lock_directory_fd,
            _socket_directory_fd: socket_directory_fd,
            _auth_directory_fd: auth_directory_fd,
        }))
    }

    pub(crate) fn display_number(&self) -> u32 {
        self.display_number
    }

    pub(crate) fn display(&self) -> &str {
        &self.display
    }

    #[cfg(test)]
    pub(crate) fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    #[cfg(test)]
    pub(crate) fn filesystem_socket_path(&self) -> &Path {
        &self.filesystem_socket_path
    }

    pub(crate) fn xauthority_path(&self) -> &Path {
        &self.xauthority_path
    }

    pub(crate) fn listener_fds(&self) -> (RawFd, RawFd) {
        (
            self.filesystem_listener.as_raw_fd(),
            self.abstract_listener.as_raw_fd(),
        )
    }
}

impl Drop for DisplayLease {
    fn drop(&mut self) {
        if let Some(name) = self.xauthority_path.file_name() {
            cleanup_artifact_at(&self._auth_directory_fd, name, self.xauthority_identity);
        }
        if let Some(name) = self.filesystem_socket_path.file_name() {
            cleanup_artifact_at(
                &self._socket_directory_fd,
                name,
                self.filesystem_socket_identity,
            );
        }
        if let Some(name) = self.lock_path.file_name() {
            cleanup_artifact_at(&self._lock_directory_fd, name, self.lock_identity);
        }
    }
}

fn ensure_socket_directory(path: &Path) -> io::Result<()> {
    fs_security::ensure_socket_directory(path)
}

fn inspect_existing_lock(
    lock_directory_fd: RawFd,
    lock_name: &std::ffi::OsStr,
    socket_path: &Path,
) -> io::Result<Option<(bool, FileIdentity)>> {
    let stat = match fs_security::stat_at(lock_directory_fd, lock_name) {
        Ok(stat) => stat,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let file_type = stat.st_mode & libc::S_IFMT as libc::mode_t;
    if file_type != libc::S_IFREG as libc::mode_t {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "X11 display lock is not a safe regular file",
        ));
    }
    let mut file = fs_security::open_read_at(lock_directory_fd, lock_name)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file).take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "X11 display lock is too large",
        ));
    }
    let pid_bytes = bytes.split(|byte| *byte == 0).next().unwrap_or_default();
    let pid = std::str::from_utf8(pid_bytes)
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "X11 display lock has no PID"))?;
    if process_is_alive(pid) || socket_is_active(socket_path)? {
        Ok(Some((
            true,
            FileIdentity {
                device: stat.st_dev,
                inode: stat.st_ino,
            },
        )))
    } else {
        Ok(Some((
            false,
            FileIdentity {
                device: stat.st_dev,
                inode: stat.st_ino,
            },
        )))
    }
}

fn write_lock_pid(file: &File) -> io::Result<()> {
    let mut file = file;
    writeln!(file, "{}", std::process::id())
}

fn reject_or_remove_socket(
    socket_directory_fd: RawFd,
    socket_name: &std::ffi::OsStr,
    path: &Path,
) -> io::Result<()> {
    let stat = match fs_security::stat_at(socket_directory_fd, socket_name) {
        Ok(stat) => stat,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if stat.st_mode & libc::S_IFMT as libc::mode_t == libc::S_IFLNK as libc::mode_t {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("X11 socket path is a symlink: {}", path.display()),
        ));
    }
    let identity = FileIdentity {
        device: stat.st_dev,
        inode: stat.st_ino,
    };
    if socket_is_active(path)? {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("X11 socket is active: {}", path.display()),
        ));
    }
    match fs_security::unlink_owned_at(socket_directory_fd, socket_name, identity) {
        fs_security::OwnershipCleanup::Removed | fs_security::OwnershipCleanup::Missing => Ok(()),
        fs_security::OwnershipCleanup::OwnershipMismatch => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "X11 socket changed during stale cleanup",
        )),
        fs_security::OwnershipCleanup::Failed => {
            Err(io::Error::other("could not retire stale X11 socket"))
        }
    }
}

fn socket_is_active(path: &Path) -> io::Result<bool> {
    match UnixStream::connect(path) {
        Ok(_) => Ok(true),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::NotFound
                    | io::ErrorKind::ConnectionReset
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

fn process_is_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn is_slot_local_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::AddrInUse
            | io::ErrorKind::AlreadyExists
            | io::ErrorKind::PermissionDenied
            | io::ErrorKind::InvalidData
    )
}

fn cleanup_artifact_at(
    directory_fd: &std::os::fd::OwnedFd,
    name: &std::ffi::OsStr,
    expected: FileIdentity,
) {
    if matches!(
        fs_security::unlink_owned_at(directory_fd.as_raw_fd(), name, expected),
        fs_security::OwnershipCleanup::OwnershipMismatch
    ) {
        eprintln!(
            "oblivion-one xwayland: event=filesystem_ownership_mismatch entry={:?}",
            name
        );
    }
}

fn abstract_socket_name(directory: &Path, display_number: u32) -> Vec<u8> {
    format!("{}/X{display_number}", directory.display()).into_bytes()
}

fn abstract_sockaddr(
    directory: &Path,
    display_number: u32,
) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    let name = abstract_socket_name(directory, display_number);
    let mut address = unsafe { std::mem::zeroed::<libc::sockaddr_un>() };
    if name.contains(&0) || name.len().saturating_add(1) >= address.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "XWayland abstract socket name is too long or contains NUL",
        ));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    address.sun_path[0] = 0;
    for (index, byte) in name.iter().enumerate() {
        address.sun_path[index + 1] = *byte as libc::c_char;
    }
    let length = (std::mem::size_of::<libc::sa_family_t>() + 1 + name.len())
        .try_into()
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "abstract socket length overflow",
            )
        })?;
    Ok((address, length))
}

fn bind_abstract_listener(directory: &Path, display_number: u32) -> io::Result<UnixListener> {
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let (address, length) = abstract_sockaddr(directory, display_number)?;
    let result = unsafe { libc::bind(fd, (&address as *const libc::sockaddr_un).cast(), length) };
    if result < 0 {
        let error = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(error);
    }
    if unsafe { libc::listen(fd, 64) } < 0 {
        let error = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(error);
    }
    Ok(unsafe { UnixListener::from_raw_fd(fd) })
}

#[cfg(test)]
pub(crate) fn connect_abstract_socket_for_tests(
    directory: &Path,
    display_number: u32,
) -> io::Result<()> {
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let (address, length) = abstract_sockaddr(directory, display_number)?;
    let result =
        unsafe { libc::connect(fd, (&address as *const libc::sockaddr_un).cast(), length) };
    let error = if result < 0 {
        Some(io::Error::last_os_error())
    } else {
        None
    };
    unsafe { libc::close(fd) };
    error.map_or(Ok(()), Err)
}
