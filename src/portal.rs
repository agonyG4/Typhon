use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use zbus::{
    fdo, interface,
    zvariant::{ObjectPath, OwnedValue, Value},
};

pub const BACKEND_BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.oblivion";
pub const BACKEND_OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";
pub const PORTAL_DESKTOP: &str = "OblivionOne";

#[derive(Debug, Clone, PartialEq)]
pub enum PortalSettingValue {
    U32(u32),
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

pub fn settings_for_namespaces(namespaces: &[String]) -> PortalSettings {
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
    let _connection = zbus::connection::Builder::session()?
        .name(BACKEND_BUS_NAME)?
        .serve_at(BACKEND_OBJECT_PATH, SettingsBackend)?
        .serve_at(BACKEND_OBJECT_PATH, notifications)?
        .serve_at(BACKEND_OBJECT_PATH, AccessBackend)?
        .build()
        .await?;
    futures_lite::future::pending::<()>().await;
    Ok(())
}

struct SettingsBackend;

#[interface(name = "org.freedesktop.impl.portal.Settings")]
impl SettingsBackend {
    fn read_all(&self, namespaces: Vec<String>) -> BTreeMap<String, BTreeMap<String, OwnedValue>> {
        settings_for_namespaces(&namespaces)
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
        setting_value(namespace, key)
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
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, contents)?;
    fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
