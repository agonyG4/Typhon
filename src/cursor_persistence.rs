//! Secure, descriptor-relative persistence for the compositor-owned cursor configuration.

use crate::cursor_theme::CursorConfiguration;
use serde::{Deserialize, Serialize};
use std::{
    ffi::{CString, OsStr, OsString},
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
};

const ASTREA_DIRECTORY: &str = "AstreaOS";
const INPUT_DIRECTORY: &str = "input";
const CONFIGURATION_FILE: &str = "cursor.json";
const LOCK_FILE: &str = "cursor.lock";
const TEMP_PREFIX: &str = ".cursor.json.tmp-";
const QUARANTINE_PREFIX: &str = ".cursor.json.quarantine-";
const TEMP_ATTEMPTS: usize = 8;
const MAX_TRANSACTION_DEBRIS: usize = 32;
const MAX_DOCUMENT_BYTES: usize = 4096;
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
const RENAME_NOREPLACE: libc::c_uint = 1;
const RENAME_EXCHANGE: libc::c_uint = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorWriteOutcome {
    pub committed: bool,
    pub cleanup_degraded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorPersistenceFault {
    TemporaryCreation,
    TemporaryWrite,
    TemporaryFsync,
    NoReplacePublication,
    Exchange,
    PostExchangeIdentityVerification,
    NewFileFsync,
    FirstDirectoryFsync,
    OldInodeCleanup,
    CleanupDirectoryFsync,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorPersistenceError {
    Missing,
    Invalid,
    Insecure,
    WriteFailed,
    Busy,
}

impl std::fmt::Display for CursorPersistenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Missing => "cursor configuration is missing",
            Self::Invalid => "cursor configuration is invalid",
            Self::Insecure => "cursor configuration is insecure",
            Self::WriteFailed => "cursor configuration could not be saved",
            Self::Busy => "cursor configuration is being changed by another instance",
        })
    }
}

impl std::error::Error for CursorPersistenceError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorConfigurationStore {
    config_home: PathBuf,
    astrea_directory: PathBuf,
    input_directory: PathBuf,
    configuration_file: PathBuf,
    create_missing_config_home: bool,
    unavailable: Option<CursorPersistenceError>,
    fault: Option<CursorPersistenceFault>,
}

impl CursorConfigurationStore {
    pub fn from_environment() -> Result<Self, CursorPersistenceError> {
        let (config_home, create_missing_config_home) = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(value) if !value.is_empty() => (PathBuf::from(value), false),
            _ => {
                let home = std::env::var_os("HOME")
                    .ok_or(CursorPersistenceError::Insecure)
                    .map(PathBuf::from)?;
                (home.join(".config"), true)
            }
        };
        Self::new_with_policy(config_home, create_missing_config_home)
    }

    pub fn unavailable(error: CursorPersistenceError) -> Self {
        Self {
            config_home: PathBuf::from("/"),
            astrea_directory: PathBuf::from("/.invalid/AstreaOS"),
            input_directory: PathBuf::from("/.invalid/AstreaOS/input"),
            configuration_file: PathBuf::from("/.invalid/AstreaOS/input/cursor.json"),
            create_missing_config_home: false,
            unavailable: Some(error),
            fault: None,
        }
    }

    pub fn new(config_home: PathBuf) -> Result<Self, CursorPersistenceError> {
        Self::new_with_policy(config_home, false)
    }

    fn new_with_policy(
        config_home: PathBuf,
        create_missing_config_home: bool,
    ) -> Result<Self, CursorPersistenceError> {
        if !config_home.is_absolute() {
            return Err(CursorPersistenceError::Insecure);
        }
        let astrea_directory = config_home.join(ASTREA_DIRECTORY);
        let input_directory = astrea_directory.join(INPUT_DIRECTORY);
        let configuration_file = input_directory.join(CONFIGURATION_FILE);
        Ok(Self {
            config_home,
            astrea_directory,
            input_directory,
            configuration_file,
            create_missing_config_home,
            unavailable: None,
            fault: None,
        })
    }

    pub fn with_fault_injection(mut self, fault: CursorPersistenceFault) -> Self {
        self.fault = Some(fault);
        self
    }

    pub fn configuration_file(&self) -> &Path {
        &self.configuration_file
    }

    pub fn read(&self) -> Result<CursorConfiguration, CursorPersistenceError> {
        if let Some(error) = self.unavailable {
            return Err(error);
        }
        let directories = self.open_directories(false)?;
        let file = open_file_at(directories.input.as_raw_fd(), CONFIGURATION_FILE, false)?
            .ok_or(CursorPersistenceError::Missing)?;
        let identity = NodeIdentity::from_fd(file.as_raw_fd())?;
        validate_configuration_identity(identity)?;
        let document = read_bounded(file)?;
        let persisted: PersistedCursorConfiguration =
            serde_json::from_slice(&document).map_err(|_| CursorPersistenceError::Invalid)?;
        if persisted.version != 1 {
            return Err(CursorPersistenceError::Invalid);
        }
        CursorConfiguration::new(persisted.theme, persisted.size_px)
            .map_err(|_| CursorPersistenceError::Invalid)
    }

    pub fn write(
        &self,
        configuration: &CursorConfiguration,
    ) -> Result<CursorWriteOutcome, CursorPersistenceError> {
        if let Some(error) = self.unavailable {
            return Err(error);
        }
        CursorConfiguration::new(&configuration.theme, configuration.size_px)
            .map_err(|_| CursorPersistenceError::Invalid)?;
        let document = serde_json::to_vec(&PersistedCursorConfiguration {
            version: 1,
            theme: configuration.theme.clone(),
            size_px: configuration.size_px,
        })
        .map_err(|_| CursorPersistenceError::WriteFailed)?;
        let directories = self.open_directories(true)?;
        let _lock = open_cursor_lock(directories.input.as_raw_fd())?;
        cleanup_stale_transaction_files(directories.input.as_raw_fd())?;
        let existing = open_file_at(directories.input.as_raw_fd(), CONFIGURATION_FILE, true)?;
        let existing_identity = existing
            .as_ref()
            .map(|file| {
                let identity = NodeIdentity::from_fd(file.as_raw_fd())?;
                validate_configuration_identity(identity)?;
                Ok(identity)
            })
            .transpose()?;
        self.fail_if_fault(CursorPersistenceFault::TemporaryCreation)?;
        let (temporary_name, mut temporary) = create_temporary_file(directories.input.as_raw_fd())?;
        let temporary_identity = NodeIdentity::from_fd(temporary.as_raw_fd())?;
        let write_result = (|| {
            self.fail_if_fault(CursorPersistenceFault::TemporaryWrite)?;
            temporary
                .write_all(&document)
                .map_err(|_| CursorPersistenceError::WriteFailed)?;
            temporary
                .flush()
                .map_err(|_| CursorPersistenceError::WriteFailed)?;
            self.fail_if_fault(CursorPersistenceFault::TemporaryFsync)?;
            temporary
                .sync_all()
                .map_err(|_| CursorPersistenceError::WriteFailed)?;
            validate_configuration_identity(NodeIdentity::from_fd(temporary.as_raw_fd())?)?;
            publish_configuration(
                directories.input.as_raw_fd(),
                &temporary_name,
                temporary_identity,
                existing_identity,
                self.fault,
            )
        })();
        drop(temporary);
        if write_result.is_err() {
            let _ = remove_owned_at(
                directories.input.as_raw_fd(),
                &temporary_name,
                temporary_identity,
            );
        }
        write_result
    }

    fn fail_if_fault(&self, fault: CursorPersistenceFault) -> Result<(), CursorPersistenceError> {
        if self.fault == Some(fault) {
            Err(CursorPersistenceError::WriteFailed)
        } else {
            Ok(())
        }
    }

    fn open_directories(
        &self,
        for_write: bool,
    ) -> Result<OpenedDirectories, CursorPersistenceError> {
        let config_home = self.open_config_home(for_write)?;
        let astrea =
            open_or_create_directory_at(config_home.as_raw_fd(), ASTREA_DIRECTORY, for_write)?;
        let input = open_or_create_directory_at(astrea.as_raw_fd(), INPUT_DIRECTORY, for_write)?;
        Ok(OpenedDirectories {
            _config_home: config_home,
            _astrea: astrea,
            input,
        })
    }

    fn open_config_home(&self, for_write: bool) -> Result<OwnedFd, CursorPersistenceError> {
        match open_directory_path(&self.config_home) {
            Ok(directory) => {
                validate_config_home_identity(NodeIdentity::from_fd(directory.as_raw_fd())?)?;
                Ok(directory)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !for_write {
                    return Err(CursorPersistenceError::Missing);
                }
                if !self.create_missing_config_home {
                    return Err(CursorPersistenceError::WriteFailed);
                }
                create_standard_config_home(&self.config_home)
            }
            Err(error) if is_symlink_error(&error) => Err(CursorPersistenceError::Insecure),
            Err(_) => Err(if for_write {
                CursorPersistenceError::WriteFailed
            } else {
                CursorPersistenceError::Insecure
            }),
        }
    }
}

struct OpenedDirectories {
    _config_home: OwnedFd,
    _astrea: OwnedFd,
    input: OwnedFd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NodeIdentity {
    device: u64,
    inode: u64,
    owner: u32,
    file_type: libc::mode_t,
    mode: u32,
}

impl NodeIdentity {
    fn from_fd(fd: RawFd) -> Result<Self, CursorPersistenceError> {
        let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
        // SAFETY: `stat` is initialized writable storage and `fd` is owned by
        // the caller for the duration of this fstat call.
        if unsafe { libc::fstat(fd, &mut stat) } < 0 {
            return Err(CursorPersistenceError::Insecure);
        }
        Ok(Self::from_stat(&stat))
    }

    fn from_stat(stat: &libc::stat) -> Self {
        Self {
            device: stat.st_dev,
            inode: stat.st_ino,
            owner: stat.st_uid,
            file_type: stat.st_mode & libc::S_IFMT,
            mode: stat.st_mode & 0o777,
        }
    }
}

fn validate_config_home_identity(identity: NodeIdentity) -> Result<(), CursorPersistenceError> {
    if identity.file_type != libc::S_IFDIR || identity.owner != effective_uid() {
        return Err(CursorPersistenceError::Insecure);
    }
    Ok(())
}

fn validate_directory_identity(identity: NodeIdentity) -> Result<(), CursorPersistenceError> {
    if identity.file_type != libc::S_IFDIR
        || identity.owner != effective_uid()
        || identity.mode != PRIVATE_DIRECTORY_MODE
    {
        return Err(CursorPersistenceError::Insecure);
    }
    Ok(())
}

fn validate_configuration_identity(identity: NodeIdentity) -> Result<(), CursorPersistenceError> {
    if identity.file_type != libc::S_IFREG
        || identity.owner != effective_uid()
        || identity.mode != PRIVATE_FILE_MODE
    {
        return Err(CursorPersistenceError::Insecure);
    }
    Ok(())
}

fn open_directory_path(path: &Path) -> io::Result<OwnedFd> {
    let path = c_string(path.as_os_str(), "directory path")?;
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `open` returned a new owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn create_standard_config_home(path: &Path) -> Result<OwnedFd, CursorPersistenceError> {
    let parent = path.parent().ok_or(CursorPersistenceError::WriteFailed)?;
    let name = path
        .file_name()
        .ok_or(CursorPersistenceError::WriteFailed)?;
    let parent_fd = open_directory_path(parent).map_err(|_| CursorPersistenceError::WriteFailed)?;
    validate_config_home_identity(NodeIdentity::from_fd(parent_fd.as_raw_fd())?)?;
    let name =
        c_string(name, "config directory name").map_err(|_| CursorPersistenceError::WriteFailed)?;
    let result =
        unsafe { libc::mkdirat(parent_fd.as_raw_fd(), name.as_ptr(), PRIVATE_DIRECTORY_MODE) };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(CursorPersistenceError::WriteFailed);
        }
    }
    let directory = open_directory_at(parent_fd.as_raw_fd(), OsStr::from_bytes(name.as_bytes()))
        .map_err(|_| CursorPersistenceError::WriteFailed)?;
    validate_config_home_identity(NodeIdentity::from_fd(directory.as_raw_fd())?)?;
    Ok(directory)
}

fn open_or_create_directory_at(
    parent_fd: RawFd,
    name: &str,
    for_write: bool,
) -> Result<OwnedFd, CursorPersistenceError> {
    match open_directory_at(parent_fd, OsStr::new(name)) {
        Ok(directory) => {
            validate_directory_identity(NodeIdentity::from_fd(directory.as_raw_fd())?)?;
            Ok(directory)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && for_write => {
            let c_name = c_string(OsStr::new(name), "directory name")
                .map_err(|_| CursorPersistenceError::WriteFailed)?;
            let result =
                unsafe { libc::mkdirat(parent_fd, c_name.as_ptr(), PRIVATE_DIRECTORY_MODE) };
            if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
                return Err(CursorPersistenceError::WriteFailed);
            }
            let directory = open_directory_at(parent_fd, OsStr::new(name)).map_err(|error| {
                if is_symlink_error(&error) {
                    CursorPersistenceError::Insecure
                } else {
                    CursorPersistenceError::WriteFailed
                }
            })?;
            validate_directory_identity(NodeIdentity::from_fd(directory.as_raw_fd())?)?;
            Ok(directory)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(CursorPersistenceError::Missing)
        }
        Err(error) if is_symlink_error(&error) => Err(CursorPersistenceError::Insecure),
        Err(_) => Err(if for_write {
            CursorPersistenceError::WriteFailed
        } else {
            CursorPersistenceError::Insecure
        }),
    }
}

fn open_directory_at(parent_fd: RawFd, name: &OsStr) -> io::Result<OwnedFd> {
    let name = c_string(name, "directory name")?;
    let fd = unsafe {
        libc::openat(
            parent_fd,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `openat` returned a new owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn open_file_at(
    parent_fd: RawFd,
    name: &str,
    for_write: bool,
) -> Result<Option<File>, CursorPersistenceError> {
    let name =
        c_string(OsStr::new(name), "file name").map_err(|_| CursorPersistenceError::Insecure)?;
    let fd = unsafe {
        libc::openat(
            parent_fd,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::NotFound {
            Ok(None)
        } else if for_write || is_symlink_error(&error) {
            Err(CursorPersistenceError::Insecure)
        } else {
            Err(CursorPersistenceError::Invalid)
        };
    }
    // SAFETY: `openat` returned a new owned descriptor.
    Ok(Some(unsafe { File::from_raw_fd(fd) }))
}

fn open_cursor_lock(parent_fd: RawFd) -> Result<OwnedFd, CursorPersistenceError> {
    let name = c_string(OsStr::new(LOCK_FILE), "lock file name")
        .map_err(|_| CursorPersistenceError::Insecure)?;
    let fd = unsafe {
        // SAFETY: `parent_fd` is the validated input directory and `name` is
        // a bounded NUL-free entry name.  The flags prevent symlink following
        // and make the returned descriptor close-on-exec.
        libc::openat(
            parent_fd,
            name.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            PRIVATE_FILE_MODE,
        )
    };
    if fd < 0 {
        let error = io::Error::last_os_error();
        return Err(
            if is_symlink_error(&error)
                || error.raw_os_error() == Some(libc::EISDIR)
                || error.raw_os_error() == Some(libc::ENXIO)
            {
                CursorPersistenceError::Insecure
            } else {
                CursorPersistenceError::WriteFailed
            },
        );
    }
    // SAFETY: `openat` returned a new owned descriptor.
    let lock = unsafe { OwnedFd::from_raw_fd(fd) };
    validate_configuration_identity(NodeIdentity::from_fd(lock.as_raw_fd())?)?;
    let result = unsafe {
        // SAFETY: `lock` is a valid descriptor for the validated lock file;
        // `flock` only changes this process's advisory lock state.
        libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)
    };
    if result == 0 {
        Ok(lock)
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EAGAIN) {
            Err(CursorPersistenceError::Busy)
        } else {
            Err(CursorPersistenceError::WriteFailed)
        }
    }
}

fn create_temporary_file(parent_fd: RawFd) -> Result<(OsString, File), CursorPersistenceError> {
    for _ in 0..TEMP_ATTEMPTS {
        let name = OsString::from(format!(
            "{TEMP_PREFIX}{}",
            random_suffix().map_err(|_| { CursorPersistenceError::WriteFailed })?
        ));
        let c_name = c_string(&name, "temporary file name")
            .map_err(|_| CursorPersistenceError::WriteFailed)?;
        let fd = unsafe {
            libc::openat(
                parent_fd,
                c_name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                PRIVATE_FILE_MODE,
            )
        };
        if fd >= 0 {
            // SAFETY: `openat` returned a new owned descriptor.
            return Ok((name, unsafe { File::from_raw_fd(fd) }));
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
            return Err(CursorPersistenceError::WriteFailed);
        }
    }
    Err(CursorPersistenceError::WriteFailed)
}

fn cleanup_stale_transaction_files(parent_fd: RawFd) -> Result<(), CursorPersistenceError> {
    let directory = format!("/proc/self/fd/{parent_fd}");
    let entries = std::fs::read_dir(directory).map_err(|_| CursorPersistenceError::WriteFailed)?;
    let mut transaction_names = Vec::with_capacity(MAX_TRANSACTION_DEBRIS + 1);
    for entry in entries {
        let entry = entry.map_err(|_| CursorPersistenceError::WriteFailed)?;
        let name = entry.file_name();
        let Some(name_string) = name.to_str() else {
            continue;
        };
        if !(name_string.starts_with(TEMP_PREFIX) || name_string.starts_with(QUARANTINE_PREFIX)) {
            continue;
        }
        if transaction_names.len() >= MAX_TRANSACTION_DEBRIS {
            return Err(CursorPersistenceError::WriteFailed);
        };
        transaction_names.push(name);
    }
    transaction_names.sort_unstable();
    let mut remaining = 0_usize;
    for name in transaction_names {
        let Ok(identity) = open_identity_at(parent_fd, &name) else {
            remaining = remaining.saturating_add(1);
            continue;
        };
        if validate_configuration_identity(identity).is_err()
            || remove_owned_at(parent_fd, &name, identity).is_err()
        {
            remaining = remaining.saturating_add(1);
        }
    }
    if remaining >= MAX_TRANSACTION_DEBRIS {
        Err(CursorPersistenceError::WriteFailed)
    } else {
        Ok(())
    }
}

fn publish_configuration(
    parent_fd: RawFd,
    temporary_name: &OsStr,
    temporary_identity: NodeIdentity,
    existing_identity: Option<NodeIdentity>,
    fault: Option<CursorPersistenceFault>,
) -> Result<CursorWriteOutcome, CursorPersistenceError> {
    let destination = OsStr::new(CONFIGURATION_FILE);
    match existing_identity {
        None => {
            fail_if_fault(fault, CursorPersistenceFault::NoReplacePublication)?;
            rename_at2(
                parent_fd,
                temporary_name,
                parent_fd,
                destination,
                RENAME_NOREPLACE,
            )
            .map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    CursorPersistenceError::Insecure
                } else {
                    CursorPersistenceError::WriteFailed
                }
            })?;
            if let Err(error) =
                verify_published_file(parent_fd, destination, temporary_identity, fault)
            {
                let _ = remove_owned_at(parent_fd, destination, temporary_identity);
                return Err(error);
            }
            fail_if_fault(fault, CursorPersistenceFault::FirstDirectoryFsync).inspect_err(
                |_| {
                    let _ = remove_owned_at(parent_fd, destination, temporary_identity);
                },
            )?;
            if let Err(error) = sync_directory(parent_fd) {
                let _ = remove_owned_at(parent_fd, destination, temporary_identity);
                return Err(error);
            }
            Ok(CursorWriteOutcome {
                committed: true,
                cleanup_degraded: false,
            })
        }
        Some(expected_old) => {
            let current = open_file_at(parent_fd, CONFIGURATION_FILE, true)?
                .ok_or(CursorPersistenceError::Insecure)?;
            let current_identity = NodeIdentity::from_fd(current.as_raw_fd())?;
            if current_identity != expected_old {
                return Err(CursorPersistenceError::Insecure);
            }
            drop(current);
            fail_if_fault(fault, CursorPersistenceFault::Exchange)?;
            if let Err(error) = rename_at2(
                parent_fd,
                temporary_name,
                parent_fd,
                destination,
                RENAME_EXCHANGE,
            ) {
                return Err(if error.kind() == io::ErrorKind::AlreadyExists {
                    CursorPersistenceError::Insecure
                } else {
                    CursorPersistenceError::WriteFailed
                });
            }

            if let Err(error) = fail_if_fault(
                fault,
                CursorPersistenceFault::PostExchangeIdentityVerification,
            ) {
                if rollback_exchange_if_exact(
                    parent_fd,
                    temporary_name,
                    destination,
                    temporary_identity,
                    expected_old,
                ) {
                    return Err(error);
                }
                return Err(CursorPersistenceError::WriteFailed);
            }
            let destination_identity = entry_identity_at(parent_fd, destination)?;
            let temporary_after_exchange = entry_identity_at(parent_fd, temporary_name)?;
            if destination_identity != temporary_identity
                || temporary_after_exchange != expected_old
            {
                let rolled_back = rollback_exchange_if_exact(
                    parent_fd,
                    temporary_name,
                    destination,
                    temporary_identity,
                    temporary_after_exchange,
                );
                if !rolled_back {
                    return Err(CursorPersistenceError::WriteFailed);
                }
                return Err(CursorPersistenceError::Insecure);
            }
            if let Err(error) =
                verify_published_file(parent_fd, destination, temporary_identity, fault)
            {
                if rollback_exchange_if_exact(
                    parent_fd,
                    temporary_name,
                    destination,
                    temporary_identity,
                    expected_old,
                ) {
                    return Err(error);
                }
                return Err(CursorPersistenceError::WriteFailed);
            }
            if let Err(error) = fail_if_fault(fault, CursorPersistenceFault::FirstDirectoryFsync) {
                if rollback_exchange_if_exact(
                    parent_fd,
                    temporary_name,
                    destination,
                    temporary_identity,
                    expected_old,
                ) {
                    return Err(error);
                }
                return Err(CursorPersistenceError::WriteFailed);
            }
            if let Err(error) = sync_directory(parent_fd) {
                if rollback_exchange_if_exact(
                    parent_fd,
                    temporary_name,
                    destination,
                    temporary_identity,
                    expected_old,
                ) {
                    return Err(error);
                }
                return Err(CursorPersistenceError::WriteFailed);
            }
            let cleanup_degraded = fail_if_fault(fault, CursorPersistenceFault::OldInodeCleanup)
                .is_err()
                || remove_owned_at(parent_fd, temporary_name, expected_old).is_err()
                || fail_if_fault(fault, CursorPersistenceFault::CleanupDirectoryFsync).is_err()
                || sync_directory(parent_fd).is_err();
            Ok(CursorWriteOutcome {
                committed: true,
                cleanup_degraded,
            })
        }
    }
}

fn rollback_exchange_if_exact(
    parent_fd: RawFd,
    temporary_name: &OsStr,
    destination: &OsStr,
    new_identity: NodeIdentity,
    old_identity: NodeIdentity,
) -> bool {
    let Ok(destination_identity) = entry_identity_at(parent_fd, destination) else {
        return false;
    };
    let Ok(temporary_identity) = entry_identity_at(parent_fd, temporary_name) else {
        return false;
    };
    if destination_identity != new_identity || temporary_identity != old_identity {
        return false;
    }
    rename_at2(
        parent_fd,
        temporary_name,
        parent_fd,
        destination,
        RENAME_EXCHANGE,
    )
    .is_ok()
}

fn fail_if_fault(
    fault: Option<CursorPersistenceFault>,
    expected: CursorPersistenceFault,
) -> Result<(), CursorPersistenceError> {
    if fault == Some(expected) {
        Err(CursorPersistenceError::WriteFailed)
    } else {
        Ok(())
    }
}

fn open_identity_at(
    parent_fd: RawFd,
    name: &OsStr,
) -> Result<NodeIdentity, CursorPersistenceError> {
    let file = open_file_at(
        parent_fd,
        name.to_str().ok_or(CursorPersistenceError::Insecure)?,
        true,
    )?
    .ok_or(CursorPersistenceError::Insecure)?;
    NodeIdentity::from_fd(file.as_raw_fd())
}

fn entry_identity_at(
    parent_fd: RawFd,
    name: &OsStr,
) -> Result<NodeIdentity, CursorPersistenceError> {
    let name = c_string(name, "entry name").map_err(|_| CursorPersistenceError::Insecure)?;
    let mut stat = unsafe {
        // SAFETY: zeroed storage is valid before `fstatat` initializes the
        // complete `libc::stat` value.
        std::mem::zeroed::<libc::stat>()
    };
    let result = unsafe {
        // SAFETY: `parent_fd` is the validated input directory, `name` is a
        // bounded NUL-free entry name, and `stat` is writable storage.
        libc::fstatat(
            parent_fd,
            name.as_ptr(),
            &mut stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result < 0 {
        Err(CursorPersistenceError::Insecure)
    } else {
        Ok(NodeIdentity::from_stat(&stat))
    }
}

fn verify_published_file(
    parent_fd: RawFd,
    destination: &OsStr,
    expected: NodeIdentity,
    fault: Option<CursorPersistenceFault>,
) -> Result<(), CursorPersistenceError> {
    let file = open_file_at(parent_fd, destination.to_str().unwrap(), true)?
        .ok_or(CursorPersistenceError::WriteFailed)?;
    let identity = NodeIdentity::from_fd(file.as_raw_fd())?;
    if identity != expected {
        return Err(CursorPersistenceError::Insecure);
    }
    fail_if_fault(fault, CursorPersistenceFault::NewFileFsync)?;
    validate_configuration_identity(identity)?;
    file.sync_all()
        .map_err(|_| CursorPersistenceError::WriteFailed)
}

fn rename_at2(
    source_dir: RawFd,
    source: &OsStr,
    destination_dir: RawFd,
    destination: &OsStr,
    flags: libc::c_uint,
) -> io::Result<()> {
    let source = c_string(source, "source name")?;
    let destination = c_string(destination, "destination name")?;
    let result = unsafe {
        // SAFETY: the directory descriptors and NUL-free names are owned by
        // this transaction; `renameat2` only changes the named entries.
        libc::syscall(
            libc::SYS_renameat2,
            source_dir,
            source.as_ptr(),
            destination_dir,
            destination.as_ptr(),
            flags,
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn remove_owned_at(
    parent_fd: RawFd,
    name: &OsStr,
    expected: NodeIdentity,
) -> Result<(), CursorPersistenceError> {
    let cleanup_name = transaction_name(QUARANTINE_PREFIX)?;
    match rename_at2(parent_fd, name, parent_fd, &cleanup_name, RENAME_NOREPLACE) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(CursorPersistenceError::WriteFailed),
    }
    let current = match open_file_at(parent_fd, cleanup_name.to_str().unwrap(), true) {
        Ok(Some(file)) => file,
        Ok(None) | Err(_) => {
            let _ = restore_entry(parent_fd, &cleanup_name, name);
            return Err(CursorPersistenceError::Insecure);
        }
    };
    if NodeIdentity::from_fd(current.as_raw_fd())? != expected {
        drop(current);
        restore_entry(parent_fd, &cleanup_name, name)?;
        return Err(CursorPersistenceError::Insecure);
    }
    drop(current);
    let cleanup_name =
        c_string(&cleanup_name, "cleanup name").map_err(|_| CursorPersistenceError::WriteFailed)?;
    let result = unsafe {
        // SAFETY: `parent_fd` is the validated input directory and the
        // quarantined entry was verified against the exact expected identity.
        libc::unlinkat(parent_fd, cleanup_name.as_ptr(), 0)
    };
    if result < 0 {
        Err(CursorPersistenceError::WriteFailed)
    } else {
        Ok(())
    }
}

fn restore_entry(
    parent_fd: RawFd,
    quarantine_name: &OsStr,
    destination: &OsStr,
) -> Result<(), CursorPersistenceError> {
    rename_at2(
        parent_fd,
        quarantine_name,
        parent_fd,
        destination,
        RENAME_NOREPLACE,
    )
    .map_err(|_| CursorPersistenceError::WriteFailed)
}

fn transaction_name(prefix: &str) -> Result<OsString, CursorPersistenceError> {
    Ok(OsString::from(format!(
        "{prefix}{}",
        random_suffix().map_err(|_| CursorPersistenceError::WriteFailed)?
    )))
}

fn read_bounded(mut file: File) -> Result<Vec<u8>, CursorPersistenceError> {
    let mut document = Vec::with_capacity(MAX_DOCUMENT_BYTES + 1);
    let mut buffer = [0_u8; 512];
    loop {
        let remaining = MAX_DOCUMENT_BYTES + 1 - document.len();
        if remaining == 0 {
            return Err(CursorPersistenceError::Invalid);
        }
        let read_len = remaining.min(buffer.len());
        let count = file
            .read(&mut buffer[..read_len])
            .map_err(|_| CursorPersistenceError::Invalid)?;
        if count == 0 {
            return Ok(document);
        }
        if document.len().saturating_add(count) > MAX_DOCUMENT_BYTES {
            return Err(CursorPersistenceError::Invalid);
        }
        document.extend_from_slice(&buffer[..count]);
    }
}

fn sync_directory(fd: RawFd) -> Result<(), CursorPersistenceError> {
    let result = unsafe {
        // SAFETY: `fd` is the validated input directory descriptor retained by
        // the write transaction.
        libc::fsync(fd)
    };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if matches!(error.raw_os_error(), Some(libc::EINVAL | libc::ENOTSUP)) {
        Ok(())
    } else {
        Err(CursorPersistenceError::WriteFailed)
    }
}

fn c_string(value: &OsStr, label: &str) -> io::Result<CString> {
    CString::new(value.as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, label))
}

fn is_symlink_error(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ELOOP)
}

fn random_suffix() -> io::Result<String> {
    let mut bytes = [0_u8; 16];
    loop {
        // SAFETY: the buffer is valid writable storage for exactly its length,
        // and `getrandom` does not retain the pointer after returning.
        let count = unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), 0) };
        if count == bytes.len() as isize {
            break;
        }
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "short random suffix",
        ));
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn effective_uid() -> u32 {
    // SAFETY: `geteuid` has no pointer arguments and only reads process state.
    unsafe { libc::geteuid() }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedCursorConfiguration {
    version: u32,
    theme: String,
    size_px: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistence_document_validation_uses_shared_cursor_validator() {
        assert!(CursorConfiguration::new("default", 8).is_ok());
        assert!(crate::cursor_theme::validate_cursor_theme("bad/theme").is_err());
        assert!(crate::cursor_theme::validate_cursor_size(7).is_err());
    }

    #[test]
    fn metadata_policy_requires_effective_uid_and_expected_mode() {
        assert!(owned_with_mode_for_test(1000, 0o700, 1000, Some(0o700)));
        assert!(!owned_with_mode_for_test(1001, 0o700, 1000, Some(0o700)));
        assert!(!owned_with_mode_for_test(1000, 0o755, 1000, Some(0o700)));
        assert!(owned_with_mode_for_test(1000, 0o755, 1000, None));
        assert!(!owned_with_mode_for_test(1000, 0o775, 1000, None));
    }

    #[test]
    fn lock_identity_requires_a_private_owned_regular_file() {
        let uid = effective_uid();
        let valid = NodeIdentity {
            device: 1,
            inode: 1,
            owner: uid,
            file_type: libc::S_IFREG,
            mode: PRIVATE_FILE_MODE,
        };
        assert!(validate_configuration_identity(valid).is_ok());
        assert!(
            validate_configuration_identity(NodeIdentity {
                owner: uid ^ 1,
                ..valid
            })
            .is_err()
        );
        assert!(
            validate_configuration_identity(NodeIdentity {
                mode: 0o644,
                ..valid
            })
            .is_err()
        );
        assert!(
            validate_configuration_identity(NodeIdentity {
                file_type: libc::S_IFDIR,
                ..valid
            })
            .is_err()
        );
    }

    fn owned_with_mode_for_test(
        uid: u32,
        mode: u32,
        effective_uid: u32,
        exact_mode: Option<u32>,
    ) -> bool {
        uid == effective_uid
            && exact_mode.map_or(mode & 0o022 == 0, |expected| mode & 0o777 == expected)
    }
}
