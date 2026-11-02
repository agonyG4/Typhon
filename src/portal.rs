use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env,
    ffi::OsString,
    fs,
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use zbus::{
    fdo, interface,
    zvariant::{ObjectPath, OwnedValue, Value},
};

use crate::{
    compositor::OUTPUT_NAME,
    cursor_persistence::CursorConfigurationStore,
    cursor_theme::{CursorConfiguration, default_cursor_configuration},
};

pub const BACKEND_BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.oblivion";
pub const BACKEND_OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";
pub const PORTAL_DESKTOP: &str = "OblivionOne";

static NEXT_PORTAL_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq)]
pub enum PortalSettingValue {
    U32(u32),
    I32(i32),
    String(String),
    Rgb(f64, f64, f64),
}

pub type PortalSettings = BTreeMap<String, BTreeMap<String, PortalSettingValue>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalRuntime {
    state_dir: PathBuf,
    executable: PathBuf,
}

impl PortalRuntime {
    pub fn new(state_dir: PathBuf, executable: PathBuf) -> Self {
        Self {
            state_dir,
            executable,
        }
    }

    pub fn for_current_process(state_dir: PathBuf) -> io::Result<Self> {
        Ok(Self::new(state_dir, env::current_exe()?))
    }

    pub fn data_dir(&self) -> PathBuf {
        self.state_dir.join("portal-share")
    }

    pub fn portal_dir(&self) -> PathBuf {
        self.data_dir().join("xdg-desktop-portal").join("portals")
    }

    pub fn service_dir(&self) -> PathBuf {
        self.data_dir().join("dbus-1").join("services")
    }

    pub fn portal_path(&self) -> PathBuf {
        self.portal_dir().join("oblivion.portal")
    }

    pub fn config_path(&self) -> PathBuf {
        self.portal_dir().join("oblivionone-portals.conf")
    }

    pub fn data_config_path(&self) -> PathBuf {
        self.data_dir()
            .join("xdg-desktop-portal")
            .join("oblivionone-portals.conf")
    }

    pub fn service_path(&self) -> PathBuf {
        self.service_dir()
            .join("org.freedesktop.impl.portal.desktop.oblivion.service")
    }

    pub fn portal_contents(&self) -> String {
        [
            "[portal]",
            &format!("DBusName={BACKEND_BUS_NAME}"),
            "Interfaces=org.freedesktop.impl.portal.Settings;org.freedesktop.impl.portal.Notification;org.freedesktop.impl.portal.Access;",
            &format!("UseIn={PORTAL_DESKTOP}"),
            "",
        ]
        .join("\n")
    }

    pub fn service_contents(&self) -> String {
        [
            "[D-BUS Service]",
            &format!("Name={BACKEND_BUS_NAME}"),
            &format!("Exec={} portal", self.executable.display()),
            "",
        ]
        .join("\n")
    }

    pub fn config_contents(&self) -> String {
        [
            "[preferred]",
            "default=none",
            "org.freedesktop.impl.portal.Settings=oblivion",
            "org.freedesktop.impl.portal.Notification=oblivion",
            "org.freedesktop.impl.portal.Access=oblivion",
            "",
        ]
        .join("\n")
    }

    pub fn install(&self) -> io::Result<()> {
        write_if_changed(&self.portal_path(), &self.portal_contents())?;
        write_if_changed(&self.config_path(), &self.config_contents())?;
        write_if_changed(&self.data_config_path(), &self.config_contents())?;
        write_if_changed(&self.service_path(), &self.service_contents())
    }
}

pub const DESKTOP_PORTAL_NAMES: [&str; 2] = ["oblivionone", "astrea"];
const SHARE_PICKER_NAME: &str = "typhon-share-picker";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopIntegration {
    config_home: PathBuf,
    state_dir: PathBuf,
}

impl DesktopIntegration {
    pub fn new(config_home: PathBuf, state_dir: PathBuf) -> Self {
        Self {
            config_home,
            state_dir,
        }
    }

    pub fn from_environment(state_dir: PathBuf) -> io::Result<Self> {
        let config_home = default_config_home().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "neither XDG_CONFIG_HOME nor HOME is set",
            )
        })?;
        Ok(Self::new(config_home, state_dir))
    }

    pub fn portal_config_paths(&self) -> Vec<PathBuf> {
        DESKTOP_PORTAL_NAMES
            .iter()
            .map(|name| {
                self.config_home
                    .join("xdg-desktop-portal")
                    .join(format!("{name}-portals.conf"))
            })
            .collect()
    }

    pub fn picker_path(&self) -> PathBuf {
        self.state_dir.join("bin").join(SHARE_PICKER_NAME)
    }

    pub fn xdph_config_path(&self) -> PathBuf {
        self.config_home.join("hypr").join("xdph.conf")
    }

    pub fn install(&self) -> io::Result<()> {
        for path in self.portal_config_paths() {
            write_if_absent(&path, &desktop_portal_config_contents())?;
            if fs::read_to_string(&path).is_ok_and(|current| !has_screencast_line(&current)) {
                eprintln!(
                    "oblivion-one: {} has no ScreenCast= line; add `org.freedesktop.impl.portal.ScreenCast=hyprland` for screen sharing",
                    path.display()
                );
            }
        }

        let picker = self.picker_path();
        write_if_changed(&picker, &share_picker_script())?;
        fs::set_permissions(&picker, fs::Permissions::from_mode(0o755))?;

        let xdph = self.xdph_config_path();
        if !write_if_absent(&xdph, &xdph_config_contents(&picker))? {
            let current = fs::read_to_string(&xdph).unwrap_or_default();
            if legacy_xdph_config_contents(&picker).contains(&current) {
                write_if_changed(&xdph, &xdph_config_contents(&picker))?;
            } else if !current.contains("custom_picker_binary") {
                eprintln!(
                    "oblivion-one: set `custom_picker_binary = {}` in the screencopy block of {}",
                    picker.display(),
                    xdph.display()
                );
            }
        }
        Ok(())
    }
}

pub fn default_config_home() -> Option<PathBuf> {
    config_home_from_env(env::var_os("XDG_CONFIG_HOME"), env::var_os("HOME"))
}

pub fn config_home_from_env(
    xdg_config_home: Option<OsString>,
    home: Option<OsString>,
) -> Option<PathBuf> {
    xdg_config_home
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
}

pub fn desktop_portal_config_contents() -> String {
    [
        "[preferred]",
        "default=gtk",
        "org.freedesktop.impl.portal.ScreenCast=hyprland",
        "org.freedesktop.impl.portal.Screenshot=hyprland",
        "org.freedesktop.impl.portal.Secret=gnome-keyring",
        "",
    ]
    .join("\n")
}

pub fn share_picker_script() -> String {
    format!(
        r#"#!/bin/sh
case "$XDG_CURRENT_DESKTOP" in
  OblivionOne|Astrea) printf '[SELECTION]r/screen:%s\n' '{OUTPUT_NAME}' ;;
  *) exec hyprland-share-picker "$@" ;;
esac
"#
    )
}

/// Typhon's screencopy fills DMA-BUFs on the GPU, so XDPH keeps its default
/// DMA-BUF buffers.
pub fn xdph_config_contents(picker: &Path) -> String {
    [
        "screencopy {",
        "    allow_token_by_default = true",
        &format!("    custom_picker_binary = {}", picker.display()),
        "}",
        "",
    ]
    .join("\n")
}

/// Every xdph.conf an earlier Typhon generated, upgraded in place because
/// Typhon owns it; any other contents are the user's and stay untouched.
fn legacy_xdph_config_contents(picker: &Path) -> [String; 2] {
    let picker_line = format!("    custom_picker_binary = {}", picker.display());
    [
        [
            "screencopy {",
            "    allow_token_by_default = true",
            &picker_line,
            "}",
            "",
        ]
        .join("\n"),
        [
            "screencopy {",
            "    allow_token_by_default = true",
            "    force_shm = true",
            &picker_line,
            "}",
            "",
        ]
        .join("\n"),
    ]
}

fn has_screencast_line(config: &str) -> bool {
    config.lines().any(|line| {
        line.trim_start()
            .starts_with("org.freedesktop.impl.portal.ScreenCast=")
    })
}

pub fn settings_for_namespaces(namespaces: &[String]) -> PortalSettings {
    settings_for_configuration(namespaces, &default_cursor_configuration())
}

pub fn settings_for_configuration(
    namespaces: &[String],
    configuration: &CursorConfiguration,
) -> PortalSettings {
    let mut values = PortalSettings::new();
    if namespace_requested(namespaces, "org.freedesktop.appearance") {
        values.insert(
            "org.freedesktop.appearance".to_string(),
            BTreeMap::from([
                ("color-scheme".to_string(), PortalSettingValue::U32(1)),
                ("contrast".to_string(), PortalSettingValue::U32(0)),
                ("reduced-motion".to_string(), PortalSettingValue::U32(0)),
                (
                    "accent-color".to_string(),
                    PortalSettingValue::Rgb(0.42, 0.64, 1.0),
                ),
            ]),
        );
    }
    if namespace_requested(namespaces, "org.gnome.desktop.interface") {
        values.insert(
            "org.gnome.desktop.interface".to_string(),
            BTreeMap::from([
                (
                    "cursor-theme".to_string(),
                    PortalSettingValue::String(configuration.theme.clone()),
                ),
                (
                    "cursor-size".to_string(),
                    PortalSettingValue::I32(configuration.size_px as i32),
                ),
            ]),
        );
    }
    values
}

pub fn setting_value(namespace: &str, key: &str) -> Option<PortalSettingValue> {
    settings_for_namespaces(&[namespace.to_string()])
        .remove(namespace)
        .and_then(|mut values| values.remove(key))
}

pub fn prepend_data_dir(data_dir: &Path, current: Option<&str>) -> String {
    let data_dir = data_dir.to_string_lossy();
    match current.filter(|value| !value.is_empty()) {
        Some(value) if value.split(':').any(|part| part == data_dir) => value.to_string(),
        Some(value) => format!("{data_dir}:{value}"),
        None => format!("{data_dir}:/usr/local/share:/usr/share"),
    }
}

pub fn run_backend() -> zbus::Result<()> {
    futures_lite::future::block_on(run_backend_async())
}

async fn run_backend_async() -> zbus::Result<()> {
    let notifications = NotificationBackend::default();
    let settings = SettingsBackend::from_environment();
    let _connection = zbus::connection::Builder::session()?
        .name(BACKEND_BUS_NAME)?
        .serve_at(BACKEND_OBJECT_PATH, settings)?
        .serve_at(BACKEND_OBJECT_PATH, notifications)?
        .serve_at(BACKEND_OBJECT_PATH, AccessBackend)?
        .build()
        .await?;
    futures_lite::future::pending::<()>().await;
    Ok(())
}

struct SettingsBackend {
    store: CursorConfigurationStore,
}

impl SettingsBackend {
    fn from_environment() -> Self {
        let store = CursorConfigurationStore::from_environment()
            .unwrap_or_else(CursorConfigurationStore::unavailable);
        Self { store }
    }

    fn configuration(&self) -> CursorConfiguration {
        self.store
            .read()
            .unwrap_or_else(|_| default_cursor_configuration())
    }
}

#[interface(name = "org.freedesktop.impl.portal.Settings")]
impl SettingsBackend {
    fn read_all(&self, namespaces: Vec<String>) -> BTreeMap<String, BTreeMap<String, OwnedValue>> {
        settings_for_configuration(&namespaces, &self.configuration())
            .into_iter()
            .map(|(namespace, values)| {
                (
                    namespace,
                    values
                        .into_iter()
                        .map(|(key, value)| (key, setting_to_owned_value(value)))
                        .collect(),
                )
            })
            .collect()
    }

    fn read(&self, namespace: &str, key: &str) -> fdo::Result<OwnedValue> {
        settings_for_configuration(&[namespace.to_string()], &self.configuration())
            .remove(namespace)
            .and_then(|mut values| values.remove(key))
            .map(setting_to_owned_value)
            .ok_or_else(|| fdo::Error::Failed(format!("unknown setting {namespace}.{key}")))
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        2
    }
}

#[derive(Debug, Clone, Default)]
struct NotificationBackend {
    notifications: Arc<Mutex<HashSet<(String, String)>>>,
}

#[interface(name = "org.freedesktop.impl.portal.Notification")]
impl NotificationBackend {
    fn add_notification(
        &self,
        app_id: &str,
        id: &str,
        _notification: HashMap<String, OwnedValue>,
    ) -> fdo::Result<()> {
        self.notifications
            .lock()
            .map_err(|_| fdo::Error::Failed("notification store poisoned".to_string()))?
            .insert((app_id.to_string(), id.to_string()));
        Ok(())
    }

    fn remove_notification(&self, app_id: &str, id: &str) -> fdo::Result<()> {
        self.notifications
            .lock()
            .map_err(|_| fdo::Error::Failed("notification store poisoned".to_string()))?
            .remove(&(app_id.to_string(), id.to_string()));
        Ok(())
    }

    #[zbus(property)]
    fn supported_options(&self) -> HashMap<String, OwnedValue> {
        HashMap::new()
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        2
    }
}

struct AccessBackend;

#[interface(name = "org.freedesktop.impl.portal.Access")]
impl AccessBackend {
    #[expect(
        clippy::too_many_arguments,
        reason = "DBus AccessDialog signature is fixed by xdg-desktop-portal"
    )]
    fn access_dialog(
        &self,
        _handle: ObjectPath<'_>,
        _app_id: &str,
        _parent_window: &str,
        _title: &str,
        _subtitle: &str,
        _body: &str,
        _options: HashMap<String, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        (1, HashMap::new())
    }
}

fn setting_to_owned_value(value: PortalSettingValue) -> OwnedValue {
    match value {
        PortalSettingValue::U32(value) => OwnedValue::from(value),
        PortalSettingValue::I32(value) => OwnedValue::from(value),
        PortalSettingValue::String(value) => Value::from(value)
            .try_into()
            .expect("static string portal setting must be representable as DBus value"),
        PortalSettingValue::Rgb(red, green, blue) => Value::from((red, green, blue))
            .try_into()
            .expect("static RGB portal setting must be representable as DBus value"),
    }
}

fn namespace_requested(namespaces: &[String], namespace: &str) -> bool {
    namespaces.is_empty()
        || namespaces.iter().any(|candidate| {
            candidate.is_empty()
                || candidate == namespace
                || candidate
                    .strip_suffix(".*")
                    .is_some_and(|prefix| namespace.starts_with(prefix))
        })
}

fn write_if_changed(path: &Path, contents: &str) -> io::Result<()> {
    if fs::read_to_string(path).is_ok_and(|current| current == contents) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        NEXT_PORTAL_TEMP_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(error) = fs::write(&temporary, contents) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn write_if_absent(path: &Path, contents: &str) -> io::Result<bool> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(error),
    };
    if let Err(error) = file.write_all(contents.as_bytes()) {
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cursor_persistence::CursorPersistenceError;
    use crate::cursor_theme::CursorConfiguration;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_SETTINGS_ROOT: AtomicU64 = AtomicU64::new(0);
    static NEXT_DESKTOP_ROOT: AtomicU64 = AtomicU64::new(0);

    fn temporary_desktop_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "typhon-desktop-portal-{}-{}",
            std::process::id(),
            NEXT_DESKTOP_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("temporary desktop root");
        root
    }

    #[test]
    fn desktop_portal_config_routes_screencast_and_screenshot_to_hyprland() {
        assert_eq!(
            desktop_portal_config_contents(),
            "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.ScreenCast=hyprland\norg.freedesktop.impl.portal.Screenshot=hyprland\norg.freedesktop.impl.portal.Secret=gnome-keyring\n"
        );
    }

    #[test]
    fn config_home_follows_xdg_rules() {
        assert_eq!(
            config_home_from_env(Some("/xdg".into()), Some("/home/u".into())),
            Some(PathBuf::from("/xdg"))
        );
        assert_eq!(
            config_home_from_env(Some("relative".into()), Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.config"))
        );
        assert_eq!(
            config_home_from_env(None, Some("/home/u".into())),
            Some(PathBuf::from("/home/u/.config"))
        );
        assert_eq!(config_home_from_env(Some("".into()), None), None);
    }

    #[test]
    fn desktop_portal_configs_are_installed_without_overwriting() {
        let root = temporary_desktop_root();
        let integration = DesktopIntegration::new(root.join("config"), root.join("state"));
        integration.install().expect("install desktop integration");

        for path in integration.portal_config_paths() {
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                desktop_portal_config_contents()
            );
        }

        let oblivion = integration.portal_config_paths().remove(0);
        fs::write(&oblivion, "[preferred]\ndefault=none\n").unwrap();
        integration
            .install()
            .expect("reinstall desktop integration");
        assert_eq!(
            fs::read_to_string(&oblivion).unwrap(),
            "[preferred]\ndefault=none\n"
        );
        assert!(!has_screencast_line("[preferred]\ndefault=none\n"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn share_picker_script_is_executable_and_names_the_output() {
        let root = temporary_desktop_root();
        let integration = DesktopIntegration::new(root.join("config"), root.join("state"));
        integration.install().expect("install desktop integration");

        let picker = integration.picker_path();
        assert_eq!(fs::read_to_string(&picker).unwrap(), share_picker_script());
        let mode = fs::metadata(&picker).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
        assert!(share_picker_script().contains(&format!("'{OUTPUT_NAME}'")));
        assert!(share_picker_script().starts_with("#!/bin/sh\n"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn xdph_config_is_written_once_and_preserved() {
        let root = temporary_desktop_root();
        let integration = DesktopIntegration::new(root.join("config"), root.join("state"));
        integration.install().expect("install desktop integration");

        let xdph = integration.xdph_config_path();
        assert_eq!(
            fs::read_to_string(&xdph).unwrap(),
            xdph_config_contents(&integration.picker_path())
        );
        assert!(
            xdph_config_contents(&integration.picker_path())
                .contains("allow_token_by_default = true")
        );

        assert!(!xdph_config_contents(&integration.picker_path()).contains("force_shm"));
        for legacy in legacy_xdph_config_contents(&integration.picker_path()) {
            fs::write(&xdph, legacy).unwrap();
            integration
                .install()
                .expect("upgrade generated xdph config");
            assert_eq!(
                fs::read_to_string(&xdph).unwrap(),
                xdph_config_contents(&integration.picker_path())
            );
        }

        fs::write(&xdph, "user config\n").unwrap();
        integration
            .install()
            .expect("reinstall desktop integration");
        assert_eq!(fs::read_to_string(&xdph).unwrap(), "user config\n");

        let edited = format!(
            "{}    max_fps = 30\n",
            legacy_xdph_config_contents(&integration.picker_path())[1]
        );
        fs::write(&xdph, &edited).unwrap();
        integration
            .install()
            .expect("reinstall desktop integration");
        assert_eq!(fs::read_to_string(&xdph).unwrap(), edited);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn notification_backend_tracks_add_and_remove() {
        let backend = NotificationBackend::default();

        backend
            .add_notification("org.example.App", "hello", HashMap::new())
            .unwrap();
        backend
            .remove_notification("org.example.App", "hello")
            .unwrap();

        assert!(backend.notifications.lock().unwrap().is_empty());
    }

    #[test]
    fn canonical_cursor_settings_are_filtered_to_the_gnome_interface_namespace() {
        let configuration = CursorConfiguration::new("Bibata", 32).unwrap();
        let values = settings_for_configuration(
            &["org.gnome.desktop.interface".to_string()],
            &configuration,
        );

        assert_eq!(
            values["org.gnome.desktop.interface"]["cursor-theme"],
            PortalSettingValue::String("Bibata".to_string())
        );
        assert_eq!(
            values["org.gnome.desktop.interface"]["cursor-size"],
            PortalSettingValue::I32(32)
        );
        assert_eq!(values["org.gnome.desktop.interface"].len(), 2);
    }

    #[test]
    fn canonical_cursor_settings_follow_namespace_matching_rules() {
        let configuration = CursorConfiguration::new("default", 24).unwrap();

        for namespaces in [
            Vec::new(),
            vec![String::new()],
            vec!["org.gnome.desktop.*".to_string()],
        ] {
            let values = settings_for_configuration(&namespaces, &configuration);
            assert!(values.contains_key("org.gnome.desktop.interface"));
        }

        assert!(
            settings_for_configuration(
                &["org.gnome.desktop.background".to_string()],
                &configuration
            )
            .is_empty()
        );
        assert!(
            settings_for_configuration(&["org.example.unknown".to_string()], &configuration)
                .is_empty()
        );
    }

    #[test]
    fn settings_backend_reads_the_latest_persisted_configuration_per_request() {
        let root = std::env::temp_dir().join(format!(
            "typhon-portal-settings-{}-{}",
            std::process::id(),
            NEXT_SETTINGS_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).expect("temporary configuration directory");
        let store = crate::cursor_persistence::CursorConfigurationStore::new(root.clone())
            .expect("temporary configuration store");
        let first = CursorConfiguration::new("default", 24).unwrap();
        let second = CursorConfiguration::new("Bibata", 32).unwrap();
        store.write(&first).expect("first cursor configuration");
        let backend = SettingsBackend { store };
        assert_eq!(backend.configuration(), first);
        let mut second_committed = false;
        for attempt in 0..100 {
            match backend.store.write(&second) {
                Ok(_) => {
                    second_committed = true;
                    break;
                }
                Err(CursorPersistenceError::Busy) if attempt < 99 => {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(error) => panic!("second cursor configuration: {error:?}"),
            }
        }
        assert!(
            second_committed,
            "second cursor configuration remained busy"
        );
        assert_eq!(backend.configuration(), second);
        let _ = std::fs::remove_dir_all(root);
    }
}
