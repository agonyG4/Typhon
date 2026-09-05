//! Secure, bounded persistence for the desired keyboard configuration.

use crate::compositor::keyboard::KeyboardConfig;
pub use crate::control_snapshots::KeyboardConfigurationValue;
use serde::{Deserialize, Serialize};
use std::{
    ffi::{CString, OsStr},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{
            ffi::OsStrExt,
            fs::{OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
};

const ASTREA_DIRECTORY: &str = "AstreaOS";
const INPUT_DIRECTORY: &str = "input";
const CONFIGURATION_FILE: &str = "keyboard.json";
const LOCK_FILE: &str = "keyboard.lock";
const TEMP_PREFIX: &str = ".keyboard.json.tmp-";
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
pub const MAX_DOCUMENT_BYTES: usize = 16 * 1024;
const MAX_STALE_TEMPORARIES: usize = 32;

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardPersistenceError {
    Missing,
    Invalid,
    Insecure,
    Unavailable,
    WriteFailed,
    Busy,
}

impl std::fmt::Display for KeyboardPersistenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Missing => "keyboard configuration is missing",
            Self::Invalid => "keyboard configuration is invalid",
            Self::Insecure => "keyboard configuration is insecure",
            Self::Unavailable => "keyboard configuration persistence is unavailable",
            Self::WriteFailed => "keyboard configuration could not be saved",
            Self::Busy => "keyboard configuration is being changed by another instance",
        })
    }
}

impl std::error::Error for KeyboardPersistenceError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeyboardConfigurationDocument {
    pub version: u32,
    pub rules: Option<String>,
    pub model: Option<String>,
    pub layout: String,
    pub variant: Option<String>,
    pub options: Option<String>,
    pub repeat_rate: i32,
    pub repeat_delay: i32,
    pub default_layout_index: u32,
}

impl KeyboardConfigurationDocument {
    pub(crate) fn from_config(config: &KeyboardConfig) -> Self {
        Self {
            version: 1,
            rules: config.rules.clone(),
            model: config.model.clone(),
            layout: config.layout.clone(),
            variant: config.variant.clone(),
            options: config.options.clone(),
            repeat_rate: config.repeat_rate,
            repeat_delay: config.repeat_delay,
            default_layout_index: config.default_layout_index,
        }
    }

    pub(crate) fn into_config(self) -> Result<KeyboardConfig, KeyboardPersistenceError> {
        if self.version != 1 {
            return Err(KeyboardPersistenceError::Invalid);
        }
        let config = KeyboardConfig {
            rules: self.rules,
            model: self.model,
            layout: self.layout,
            variant: self.variant,
            options: self.options,
            repeat_rate: self.repeat_rate,
            repeat_delay: self.repeat_delay,
            default_layout_index: self.default_layout_index,
        };
        config
            .validate_structure()
            .map_err(|_| KeyboardPersistenceError::Invalid)?;
        Ok(config)
    }
}

impl From<&KeyboardConfig> for KeyboardConfigurationValue {
    fn from(config: &KeyboardConfig) -> Self {
        Self {
            rules: config.rules.clone(),
            model: config.model.clone(),
            layout: config.layout.clone(),
            variant: config.variant.clone(),
            options: config.options.clone(),
            repeat_rate: config.repeat_rate,
            repeat_delay: config.repeat_delay,
            default_layout_index: config.default_layout_index,
        }
    }
}

impl TryFrom<KeyboardConfigurationValue> for KeyboardConfig {
    type Error = KeyboardPersistenceError;

    fn try_from(value: KeyboardConfigurationValue) -> Result<Self, Self::Error> {
        let config = KeyboardConfig {
            rules: value.rules,
            model: value.model,
            layout: value.layout,
            variant: value.variant,
            options: value.options,
            repeat_rate: value.repeat_rate,
            repeat_delay: value.repeat_delay,
            default_layout_index: value.default_layout_index,
        };
        config
            .validate_structure()
            .map_err(|_| KeyboardPersistenceError::Invalid)?;
        Ok(config)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardConfigurationStore {
    config_home: PathBuf,
    astrea_directory: PathBuf,
    input_directory: PathBuf,
    configuration_file: PathBuf,
    create_missing_config_home: bool,
    unavailable: Option<KeyboardPersistenceError>,
}

impl KeyboardConfigurationStore {
    pub fn from_environment() -> Result<Self, KeyboardPersistenceError> {
        let (config_home, create_missing_config_home) = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(value) if !value.is_empty() => (PathBuf::from(value), false),
            _ => {
                let home = std::env::var_os("HOME")
                    .ok_or(KeyboardPersistenceError::Insecure)
                    .map(PathBuf::from)?;
                (home.join(".config"), true)
            }
        };
        Self::new_with_policy(config_home, create_missing_config_home)
    }

    pub fn unavailable(error: KeyboardPersistenceError) -> Self {
        Self {
            config_home: PathBuf::from("/.invalid"),
            astrea_directory: PathBuf::from("/.invalid/AstreaOS"),
            input_directory: PathBuf::from("/.invalid/AstreaOS/input"),
            configuration_file: PathBuf::from("/.invalid/AstreaOS/input/keyboard.json"),
            create_missing_config_home: false,
            unavailable: Some(error),
        }
    }

    pub fn new(config_home: PathBuf) -> Result<Self, KeyboardPersistenceError> {
        Self::new_with_policy(config_home, false)
    }

    fn new_with_policy(
        config_home: PathBuf,
        create_missing_config_home: bool,
    ) -> Result<Self, KeyboardPersistenceError> {
        if !config_home.is_absolute() {
            return Err(KeyboardPersistenceError::Insecure);
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
        })
    }

    #[cfg(test)]
    pub(crate) fn configuration_file(&self) -> &Path {
        &self.configuration_file
    }

    pub fn read(&self) -> Result<KeyboardConfig, KeyboardPersistenceError> {
        self.check_available()?;
        let directories = self.open_directories(false)?;
        let Some(file) = open_file_at(directories.input.as_raw_fd(), CONFIGURATION_FILE, false)?
        else {
            return Err(KeyboardPersistenceError::Missing);
        };
        validate_file_identity(NodeIdentity::from_fd(file.as_raw_fd())?)?;
        let document = read_bounded(file)?;
        let document: KeyboardConfigurationDocument =
            serde_json::from_slice(&document).map_err(|_| KeyboardPersistenceError::Invalid)?;
        document.into_config()
    }

    pub(crate) fn write(&self, config: &KeyboardConfig) -> Result<(), KeyboardPersistenceError> {
        self.check_available()?;
        config
            .validate_structure()
            .map_err(|_| KeyboardPersistenceError::Invalid)?;
        let document = serde_json::to_vec(&KeyboardConfigurationDocument::from_config(config))
            .map_err(|_| KeyboardPersistenceError::WriteFailed)?;
        if document.len() > MAX_DOCUMENT_BYTES {
            return Err(KeyboardPersistenceError::Invalid);
        }

        let directories = self.open_directories(true)?;
        let lock = open_lock(directories.input.as_raw_fd())?;
        acquire_lock(&lock)?;
        cleanup_stale_temporaries(&self.input_directory)?;

        let existing = match open_file_at(directories.input.as_raw_fd(), CONFIGURATION_FILE, true)?
        {
            Some(file) => {
                validate_file_identity(NodeIdentity::from_fd(file.as_raw_fd())?)?;
                Some(file)
            }
            None => None,
        };
        let temporary_name = format!(
            "{TEMP_PREFIX}{}-{}",
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        );
        let temporary_path = self.input_directory.join(&temporary_name);
        let write_result = (|| {
            let mut temporary = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .mode(PRIVATE_FILE_MODE)
                .open(&temporary_path)
                .map_err(|_| KeyboardPersistenceError::WriteFailed)?;
            temporary
                .write_all(&document)
                .map_err(|_| KeyboardPersistenceError::WriteFailed)?;
            temporary
                .flush()
                .map_err(|_| KeyboardPersistenceError::WriteFailed)?;
            temporary
                .sync_all()
                .map_err(|_| KeyboardPersistenceError::WriteFailed)?;
            drop(temporary);

            match fs::symlink_metadata(&self.configuration_file) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                        return Err(KeyboardPersistenceError::Insecure);
                    }
                    if metadata.permissions().mode() & 0o777 != PRIVATE_FILE_MODE {
                        return Err(KeyboardPersistenceError::Insecure);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => return Err(KeyboardPersistenceError::Insecure),
            }

            fs::rename(&temporary_path, &self.configuration_file)
                .map_err(|_| KeyboardPersistenceError::WriteFailed)?;
            let published = open_file_at(directories.input.as_raw_fd(), CONFIGURATION_FILE, true)?
                .ok_or(KeyboardPersistenceError::WriteFailed)?;
            validate_file_identity(NodeIdentity::from_fd(published.as_raw_fd())?)?;
            published
                .sync_all()
                .map_err(|_| KeyboardPersistenceError::WriteFailed)?;
            sync_directory(directories.input.as_raw_fd())?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        drop(existing);
        drop(lock);
        write_result
    }

    fn check_available(&self) -> Result<(), KeyboardPersistenceError> {
        if let Some(error) = self.unavailable {
            Err(error)
        } else {
            Ok(())
        }
    }

    fn open_directories(
        &self,
        for_write: bool,
    ) -> Result<OpenedDirectories, KeyboardPersistenceError> {
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

    fn open_config_home(&self, for_write: bool) -> Result<OwnedFd, KeyboardPersistenceError> {
        match open_directory_path(&self.config_home) {
            Ok(directory) => {
                validate_config_home_identity(NodeIdentity::from_fd(directory.as_raw_fd())?)?;
                Ok(directory)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && for_write => {
                if !self.create_missing_config_home {
                    return Err(KeyboardPersistenceError::WriteFailed);
                }
                create_config_home(&self.config_home)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Err(KeyboardPersistenceError::Missing)
            }
            Err(error) if is_symlink_error(&error) => Err(KeyboardPersistenceError::Insecure),
            Err(_) => Err(if for_write {
                KeyboardPersistenceError::WriteFailed
            } else {
                KeyboardPersistenceError::Insecure
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardJobId(pub u64);

#[derive(Debug)]
pub enum KeyboardPersistenceOperation {
    Write {
        job_id: KeyboardJobId,
        configuration: KeyboardConfig,
    },
}

impl KeyboardPersistenceOperation {
    pub(crate) const fn job_id(&self) -> KeyboardJobId {
        match self {
            Self::Write { job_id, .. } => *job_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardPersistenceSubmitError {
    Busy,
    Unavailable,
}

#[derive(Debug)]
pub struct KeyboardPersistenceCompletion {
    pub job_id: KeyboardJobId,
    pub result: Result<(), KeyboardPersistenceError>,
}

pub struct KeyboardPersistenceWorker {
    jobs: SyncSender<KeyboardPersistenceOperation>,
    completions: Receiver<KeyboardPersistenceCompletion>,
    notification: Arc<OwnedFd>,
    busy: Arc<std::sync::atomic::AtomicBool>,
    available: Arc<std::sync::atomic::AtomicBool>,
    _thread: JoinHandle<()>,
}

impl KeyboardPersistenceWorker {
    pub fn new(store: KeyboardConfigurationStore) -> io::Result<Self> {
        let notification = Arc::new(create_event_fd()?);
        let worker_notification = Arc::clone(&notification);
        let (jobs, job_receiver) = mpsc::sync_channel::<KeyboardPersistenceOperation>(1);
        let (completion_sender, completions) =
            mpsc::sync_channel::<KeyboardPersistenceCompletion>(1);
        let busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_busy = Arc::clone(&busy);
        let available = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let worker_available = Arc::clone(&available);
        let thread = thread::Builder::new()
            .name("typhon-keyboard-persistence".to_string())
            .spawn(move || {
                while let Ok(operation) = job_receiver.recv() {
                    let job_id = operation.job_id();
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                            || match operation {
                                KeyboardPersistenceOperation::Write { configuration, .. } => {
                                    store.write(&configuration)
                                }
                            },
                        ))
                        .unwrap_or(Err(KeyboardPersistenceError::Unavailable));
                    let completion = KeyboardPersistenceCompletion { job_id, result };
                    let delivered = completion_sender.send(completion).is_ok();
                    worker_busy.store(false, Ordering::Release);
                    if !delivered || notify_eventfd(&worker_notification).is_err() {
                        worker_available.store(false, Ordering::Release);
                        break;
                    }
                }
                worker_available.store(false, Ordering::Release);
            })?;
        Ok(Self {
            jobs,
            completions,
            notification,
            busy,
            available,
            _thread: thread,
        })
    }

    pub fn event_fd(&self) -> i32 {
        self.notification.as_raw_fd()
    }

    pub fn submit(
        &self,
        operation: KeyboardPersistenceOperation,
    ) -> Result<(), KeyboardPersistenceSubmitError> {
        if !self.available.load(Ordering::Acquire) {
            return Err(KeyboardPersistenceSubmitError::Unavailable);
        }
        if self.busy.swap(true, Ordering::AcqRel) {
            return Err(KeyboardPersistenceSubmitError::Busy);
        }
        match self.jobs.try_send(operation) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.busy.store(false, Ordering::Release);
                Err(KeyboardPersistenceSubmitError::Busy)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.busy.store(false, Ordering::Release);
                self.available.store(false, Ordering::Release);
                Err(KeyboardPersistenceSubmitError::Unavailable)
            }
        }
    }

    pub fn drain_notification(&self) -> io::Result<()> {
        let mut value = 0u64;
        loop {
            // SAFETY: the eventfd is open and `value` is writable storage.
            let result = unsafe {
                libc::read(
                    self.notification.as_raw_fd(),
                    (&mut value as *mut u64).cast(),
                    std::mem::size_of::<u64>(),
                )
            };
            if result == std::mem::size_of::<u64>() as isize {
                continue;
            }
            if result < 0 && io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
                return Ok(());
            }
            if result < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return if result < 0 {
                Err(io::Error::last_os_error())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "short eventfd read",
                ))
            };
        }
    }

    pub fn receive_completion(&self) -> Option<KeyboardPersistenceCompletion> {
        let completion = self.completions.recv().ok();
        if completion.is_some() {
            while self.busy.load(Ordering::Acquire) {
                thread::yield_now();
            }
        }
        completion
    }

    pub fn try_completion(&self) -> Option<KeyboardPersistenceCompletion> {
        self.completions.try_recv().ok()
    }

    pub fn is_available(&self) -> bool {
        self.available.load(Ordering::Acquire)
    }
}

fn create_event_fd() -> io::Result<OwnedFd> {
    // SAFETY: `eventfd` has no borrowed pointers and returns a new descriptor.
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `eventfd` returned a new owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn notify_eventfd(fd: &OwnedFd) -> io::Result<()> {
    let value = 1u64;
    // SAFETY: `fd` is live and `value` is a valid readable u64 buffer.
    let written = unsafe {
        libc::write(
            fd.as_raw_fd(),
            (&value as *const u64).cast(),
            std::mem::size_of::<u64>(),
        )
    };
    if written == std::mem::size_of::<u64>() as isize {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

struct OpenedDirectories {
    _config_home: OwnedFd,
    _astrea: OwnedFd,
    input: OwnedFd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NodeIdentity {
    owner: u32,
    file_type: libc::mode_t,
    mode: u32,
}

impl NodeIdentity {
    fn from_fd(fd: i32) -> Result<Self, KeyboardPersistenceError> {
        let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
        // SAFETY: `fd` is an open descriptor and `stat` is writable storage.
        if unsafe { libc::fstat(fd, &mut stat) } < 0 {
            return Err(KeyboardPersistenceError::Insecure);
        }
        Ok(Self {
            owner: stat.st_uid,
            file_type: stat.st_mode & libc::S_IFMT,
            mode: stat.st_mode & 0o777,
        })
    }
}

fn validate_config_home_identity(identity: NodeIdentity) -> Result<(), KeyboardPersistenceError> {
    if identity.file_type != libc::S_IFDIR
        || identity.owner != effective_uid()
        || identity.mode & 0o022 != 0
    {
        return Err(KeyboardPersistenceError::Insecure);
    }
    Ok(())
}

fn validate_directory_identity(identity: NodeIdentity) -> Result<(), KeyboardPersistenceError> {
    if identity.file_type != libc::S_IFDIR
        || identity.owner != effective_uid()
        || identity.mode != PRIVATE_DIRECTORY_MODE
    {
        return Err(KeyboardPersistenceError::Insecure);
    }
    Ok(())
}

fn validate_file_identity(identity: NodeIdentity) -> Result<(), KeyboardPersistenceError> {
    if identity.file_type != libc::S_IFREG
        || identity.owner != effective_uid()
        || identity.mode != PRIVATE_FILE_MODE
    {
        return Err(KeyboardPersistenceError::Insecure);
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

fn create_config_home(path: &Path) -> Result<OwnedFd, KeyboardPersistenceError> {
    let parent = path.parent().ok_or(KeyboardPersistenceError::WriteFailed)?;
    let name = path
        .file_name()
        .ok_or(KeyboardPersistenceError::WriteFailed)?;
    let parent_fd =
        open_directory_path(parent).map_err(|_| KeyboardPersistenceError::WriteFailed)?;
    validate_config_home_identity(NodeIdentity::from_fd(parent_fd.as_raw_fd())?)?;
    let name = c_string(name, "config directory name")
        .map_err(|_| KeyboardPersistenceError::WriteFailed)?;
    let result =
        unsafe { libc::mkdirat(parent_fd.as_raw_fd(), name.as_ptr(), PRIVATE_DIRECTORY_MODE) };
    if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
        return Err(KeyboardPersistenceError::WriteFailed);
    }
    let directory = open_directory_at(parent_fd.as_raw_fd(), OsStr::from_bytes(name.as_bytes()))
        .map_err(|_| KeyboardPersistenceError::WriteFailed)?;
    validate_config_home_identity(NodeIdentity::from_fd(directory.as_raw_fd())?)?;
    Ok(directory)
}

fn open_or_create_directory_at(
    parent_fd: i32,
    name: &str,
    for_write: bool,
) -> Result<OwnedFd, KeyboardPersistenceError> {
    match open_directory_at(parent_fd, OsStr::new(name)) {
        Ok(directory) => {
            validate_directory_identity(NodeIdentity::from_fd(directory.as_raw_fd())?)?;
            Ok(directory)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && for_write => {
            let name = c_string(OsStr::new(name), "directory name")
                .map_err(|_| KeyboardPersistenceError::WriteFailed)?;
            let result = unsafe { libc::mkdirat(parent_fd, name.as_ptr(), PRIVATE_DIRECTORY_MODE) };
            if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
                return Err(KeyboardPersistenceError::WriteFailed);
            }
            let directory = open_directory_at(parent_fd, OsStr::from_bytes(name.as_bytes()))
                .map_err(|error| {
                    if is_symlink_error(&error) {
                        KeyboardPersistenceError::Insecure
                    } else {
                        KeyboardPersistenceError::WriteFailed
                    }
                })?;
            validate_directory_identity(NodeIdentity::from_fd(directory.as_raw_fd())?)?;
            Ok(directory)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(KeyboardPersistenceError::Missing)
        }
        Err(error) if is_symlink_error(&error) => Err(KeyboardPersistenceError::Insecure),
        Err(_) => Err(if for_write {
            KeyboardPersistenceError::WriteFailed
        } else {
            KeyboardPersistenceError::Insecure
        }),
    }
}

fn open_directory_at(parent_fd: i32, name: &OsStr) -> io::Result<OwnedFd> {
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
    parent_fd: i32,
    name: &str,
    for_write: bool,
) -> Result<Option<File>, KeyboardPersistenceError> {
    let name =
        c_string(OsStr::new(name), "file name").map_err(|_| KeyboardPersistenceError::Insecure)?;
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
        } else if is_symlink_error(&error) {
            Err(KeyboardPersistenceError::Insecure)
        } else if for_write {
            Err(KeyboardPersistenceError::WriteFailed)
        } else {
            Err(KeyboardPersistenceError::Invalid)
        };
    }
    // SAFETY: `openat` returned a new owned descriptor.
    Ok(Some(unsafe { File::from_raw_fd(fd) }))
}

fn open_lock(parent_fd: i32) -> Result<File, KeyboardPersistenceError> {
    let name = c_string(OsStr::new(LOCK_FILE), "lock file")
        .map_err(|_| KeyboardPersistenceError::Insecure)?;
    let fd = unsafe {
        libc::openat(
            parent_fd,
            name.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            PRIVATE_FILE_MODE,
        )
    };
    if fd < 0 {
        let error = io::Error::last_os_error();
        return if is_symlink_error(&error) {
            Err(KeyboardPersistenceError::Insecure)
        } else {
            Err(KeyboardPersistenceError::WriteFailed)
        };
    }
    // SAFETY: `openat` returned a new owned descriptor.
    let file = unsafe { File::from_raw_fd(fd) };
    validate_file_identity(NodeIdentity::from_fd(file.as_raw_fd())?)?;
    Ok(file)
}

fn acquire_lock(lock: &File) -> Result<(), KeyboardPersistenceError> {
    // SAFETY: `lock` is a live regular lock descriptor owned by this call.
    let result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        Ok(())
    } else if io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK) {
        Err(KeyboardPersistenceError::Busy)
    } else {
        Err(KeyboardPersistenceError::WriteFailed)
    }
}

fn cleanup_stale_temporaries(input_directory: &Path) -> Result<(), KeyboardPersistenceError> {
    let entries =
        fs::read_dir(input_directory).map_err(|_| KeyboardPersistenceError::WriteFailed)?;
    let mut cleaned = 0usize;
    for entry in entries {
        let entry = entry.map_err(|_| KeyboardPersistenceError::WriteFailed)?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(TEMP_PREFIX) {
            if cleaned == MAX_STALE_TEMPORARIES {
                break;
            }
            fs::remove_file(entry.path()).map_err(|_| KeyboardPersistenceError::WriteFailed)?;
            cleaned += 1;
        }
    }
    Ok(())
}

fn read_bounded(mut file: File) -> Result<Vec<u8>, KeyboardPersistenceError> {
    let mut document = Vec::with_capacity(MAX_DOCUMENT_BYTES + 1);
    let mut buffer = [0_u8; 512];
    loop {
        let read_len = (MAX_DOCUMENT_BYTES + 1 - document.len()).min(buffer.len());
        if read_len == 0 {
            return Err(KeyboardPersistenceError::Invalid);
        }
        let count = file
            .read(&mut buffer[..read_len])
            .map_err(|_| KeyboardPersistenceError::Invalid)?;
        if count == 0 {
            return Ok(document);
        }
        document.extend_from_slice(&buffer[..count]);
        if document.len() > MAX_DOCUMENT_BYTES {
            return Err(KeyboardPersistenceError::Invalid);
        }
    }
}

fn sync_directory(fd: i32) -> Result<(), KeyboardPersistenceError> {
    // SAFETY: `fd` is the validated input directory descriptor retained by
    // the write transaction.
    let result = unsafe { libc::fsync(fd) };
    if result == 0
        || matches!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::EINVAL | libc::ENOTSUP)
        )
    {
        Ok(())
    } else {
        Err(KeyboardPersistenceError::WriteFailed)
    }
}

fn c_string(value: &OsStr, label: &str) -> io::Result<CString> {
    CString::new(value.as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, label))
}

fn is_symlink_error(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ELOOP)
}

fn effective_uid() -> u32 {
    // SAFETY: `geteuid` has no pointer arguments and only reads process state.
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("typhon-keyboard-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).expect("temporary keyboard config root");
        root
    }

    #[test]
    fn document_round_trip_is_versioned_and_bounded() {
        let config = KeyboardConfig::default();
        let document = KeyboardConfigurationDocument::from_config(&config);
        let bytes = serde_json::to_vec(&document).expect("keyboard document");
        assert!(bytes.len() <= MAX_DOCUMENT_BYTES);
        assert_eq!(document.into_config().expect("valid document"), config);
    }

    #[test]
    fn store_writes_private_atomic_document_and_reads_it_back() {
        let root = root("round-trip");
        let store = KeyboardConfigurationStore::new(root.clone()).expect("keyboard store");
        let config = KeyboardConfig {
            layout: "br,us".to_string(),
            variant: Some("abnt2,".to_string()),
            default_layout_index: 1,
            ..KeyboardConfig::default()
        };
        store
            .write(&config)
            .expect("persist keyboard configuration");
        assert_eq!(store.read().expect("read keyboard configuration"), config);
        assert_eq!(
            fs::metadata(store.configuration_file())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            PRIVATE_FILE_MODE
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn store_rejects_unknown_fields_and_wrong_version() {
        let unknown = serde_json::json!({
            "version": 1,
            "rules": null,
            "model": null,
            "layout": "us",
            "variant": null,
            "options": null,
            "repeatRate": 25,
            "repeatDelay": 600,
            "defaultLayoutIndex": 0,
            "compiledKeymap": "no"
        });
        assert!(serde_json::from_value::<KeyboardConfigurationDocument>(unknown).is_err());
        let wrong_version = serde_json::json!({
            "version": 2,
            "rules": null,
            "model": null,
            "layout": "us",
            "variant": null,
            "options": null,
            "repeatRate": 25,
            "repeatDelay": 600,
            "defaultLayoutIndex": 0
        });
        assert!(serde_json::from_value::<KeyboardConfigurationDocument>(wrong_version).is_ok());
        assert_eq!(
            KeyboardConfigurationDocument::from_config(&KeyboardConfig::default())
                .into_config()
                .unwrap()
                .default_layout_index,
            0
        );
    }

    #[test]
    fn store_reports_missing_truncated_and_oversized_documents() {
        let root = root("bounds");
        let store = KeyboardConfigurationStore::new(root.clone()).expect("keyboard store");
        assert_eq!(store.read(), Err(KeyboardPersistenceError::Missing));

        let input = root.join("AstreaOS/input");
        fs::create_dir_all(&input).unwrap();
        fs::set_permissions(root.join("AstreaOS"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&input, fs::Permissions::from_mode(0o700)).unwrap();
        let path = input.join(CONFIGURATION_FILE);
        fs::write(&path, b"{\"version\":1").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(store.read(), Err(KeyboardPersistenceError::Invalid));

        fs::write(&path, vec![b'x'; MAX_DOCUMENT_BYTES + 1]).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(store.read(), Err(KeyboardPersistenceError::Invalid));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn store_rejects_configuration_symlinks() {
        let root = root("symlink");
        let store = KeyboardConfigurationStore::new(root.clone()).expect("keyboard store");
        let input = root.join("AstreaOS/input");
        fs::create_dir_all(&input).unwrap();
        fs::set_permissions(root.join("AstreaOS"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&input, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(root.join("target.json"), b"outside").unwrap();
        std::os::unix::fs::symlink(root.join("target.json"), input.join(CONFIGURATION_FILE))
            .unwrap();
        assert_eq!(store.read(), Err(KeyboardPersistenceError::Insecure));
        assert_eq!(
            store.write(&KeyboardConfig::default()),
            Err(KeyboardPersistenceError::Insecure)
        );
        assert_eq!(fs::read(root.join("target.json")).unwrap(), b"outside");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn persistence_worker_writes_only_sendable_configuration_data() {
        let root = root("worker");
        let store = KeyboardConfigurationStore::new(root.clone()).expect("keyboard store");
        let worker = KeyboardPersistenceWorker::new(store).expect("keyboard worker");
        let config = KeyboardConfig {
            layout: "us".to_string(),
            variant: None,
            ..KeyboardConfig::default()
        };
        worker
            .submit(KeyboardPersistenceOperation::Write {
                job_id: KeyboardJobId(7),
                configuration: config.clone(),
            })
            .expect("submit keyboard persistence");
        let completion = worker.receive_completion().expect("worker completion");
        assert_eq!(completion.job_id, KeyboardJobId(7));
        assert_eq!(completion.result, Ok(()));
        assert_eq!(
            KeyboardConfigurationStore::new(root.clone())
                .unwrap()
                .read()
                .unwrap(),
            config
        );
        let _ = fs::remove_dir_all(root);
    }
}
