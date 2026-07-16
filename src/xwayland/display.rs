use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::fd::{AsRawFd, FromRawFd, RawFd},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
};

use super::auth::create_auth_file;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    fn from_path(path: &Path) -> io::Result<Self> {
        let metadata = fs::symlink_metadata(path)?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

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
    #[allow(dead_code)]
    cookie: Vec<u8>,
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
        fs::create_dir_all(root.join(".X11-unix"))?;
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
        let lock_path = lock_directory.join(format!(".X{display_number}-lock"));
        let socket_path = socket_directory.join(format!("X{display_number}"));
        if let Some(should_skip) = inspect_existing_lock(&lock_path, &socket_path)? {
            if should_skip {
                return Ok(None);
            }
            fs::remove_file(&lock_path)?;
        }

        let lock_file = match create_lock(&lock_path)? {
            Some(file) => file,
            None => return Ok(None),
        };
        let lock_identity = FileIdentity::from_path(&lock_path)?;
        if let Err(error) = write_lock_pid(&lock_file) {
            cleanup_artifact(&lock_path, lock_identity);
            return Err(error);
        }

        if let Err(error) = reject_or_remove_socket(&socket_path) {
            cleanup_artifact(&lock_path, lock_identity);
            if error.kind() == io::ErrorKind::AddrInUse {
                return Ok(None);
            }
            return Err(error);
        }
        let filesystem_listener = match UnixListener::bind(&socket_path) {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                cleanup_artifact(&lock_path, lock_identity);
                return Ok(None);
            }
            Err(error) => {
                cleanup_artifact(&lock_path, lock_identity);
                return Err(error);
            }
        };
        filesystem_listener.set_nonblocking(true)?;
        let filesystem_socket_identity = match FileIdentity::from_path(&socket_path) {
            Ok(identity) => identity,
            Err(error) => {
                cleanup_artifact(&lock_path, lock_identity);
                let _ = fs::remove_file(&socket_path);
                return Err(error);
            }
        };
        let abstract_listener =
            match bind_abstract_listener(abstract_socket_directory, display_number) {
                Ok(listener) => listener,
                Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                    cleanup_artifact(&lock_path, lock_identity);
                    cleanup_artifact(&socket_path, filesystem_socket_identity);
                    return Ok(None);
                }
                Err(error) => {
                    cleanup_artifact(&lock_path, lock_identity);
                    cleanup_artifact(&socket_path, filesystem_socket_identity);
                    return Err(error);
                }
            };
        let auth_file = match create_auth_file(auth_directory, display_number) {
            Ok(auth_file) => auth_file,
            Err(error) => {
                cleanup_artifact(&lock_path, lock_identity);
                cleanup_artifact(&socket_path, filesystem_socket_identity);
                return Err(error);
            }
        };
        let xauthority_identity = match FileIdentity::from_path(&auth_file.path) {
            Ok(identity) => identity,
            Err(error) => {
                cleanup_artifact(&lock_path, lock_identity);
                cleanup_artifact(&socket_path, filesystem_socket_identity);
                cleanup_artifact(
                    &auth_file.path,
                    FileIdentity {
                        device: 0,
                        inode: 0,
                    },
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
            cookie: auth_file.cookie,
        }))
    }

    pub(crate) fn display_number(&self) -> u32 {
        self.display_number
    }

    pub(crate) fn display(&self) -> &str {
        &self.display
    }

    pub(crate) fn lock_path(&self) -> &Path {
        &self.lock_path
    }

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
        cleanup_artifact(&self.xauthority_path, self.xauthority_identity);
        cleanup_artifact(
            &self.filesystem_socket_path,
            self.filesystem_socket_identity,
        );
        cleanup_artifact(&self.lock_path, self.lock_identity);
    }
}

fn ensure_socket_directory(path: &Path) -> io::Result<()> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "X11 socket directory is not a safe directory: {}",
                    path.display()
                ),
            ));
        }
        return Ok(());
    }
    fs::create_dir_all(path)?;
    Ok(())
}

fn inspect_existing_lock(lock_path: &Path, socket_path: &Path) -> io::Result<Option<bool>> {
    let metadata = match fs::symlink_metadata(lock_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "X11 display lock is not a safe regular file: {}",
                lock_path.display()
            ),
        ));
    }
    let pid = fs::read_to_string(lock_path)?
        .trim()
        .parse::<u32>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "X11 display lock has no PID"))?;
    if process_is_alive(pid) || socket_is_active(socket_path)? {
        Ok(Some(true))
    } else {
        Ok(Some(false))
    }
}

fn create_lock(path: &Path) -> io::Result<Option<File>> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .mode(0o644);
    match options.open(path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_lock_pid(file: &File) -> io::Result<()> {
    let mut file = file;
    writeln!(file, "{}", std::process::id())
}

fn reject_or_remove_socket(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("X11 socket path is a symlink: {}", path.display()),
        ));
    }
    if socket_is_active(path)? {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("X11 socket is active: {}", path.display()),
        ));
    }
    fs::remove_file(path)
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

fn cleanup_artifact(path: &Path, expected: FileIdentity) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink()
        || (expected.device != 0
            && (metadata.dev() != expected.device || metadata.ino() != expected.inode))
    {
        return;
    }
    let _ = fs::remove_file(path);
}

fn abstract_socket_name(directory: &Path, display_number: u32) -> Vec<u8> {
    format!("{}/X{display_number}", directory.display()).into_bytes()
}

fn abstract_sockaddr(
    directory: &Path,
    display_number: u32,
) -> (libc::sockaddr_un, libc::socklen_t) {
    let name = abstract_socket_name(directory, display_number);
    let mut address = unsafe { std::mem::zeroed::<libc::sockaddr_un>() };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    address.sun_path[0] = 0;
    for (index, byte) in name.iter().enumerate() {
        address.sun_path[index + 1] = *byte as libc::c_char;
    }
    let length = (std::mem::size_of::<libc::sa_family_t>() + 1 + name.len())
        .try_into()
        .expect("abstract socket address fits socklen_t");
    (address, length)
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
    let (address, length) = abstract_sockaddr(directory, display_number);
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
    let (address, length) = abstract_sockaddr(directory, display_number);
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
