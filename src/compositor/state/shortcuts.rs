use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AstreaShortcutPhase {
    Pressed,
    Repeated,
    Released,
}

impl AstreaShortcutPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pressed => "pressed",
            Self::Repeated => "repeated",
            Self::Released => "released",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AstreaShortcutKey {
    namespace: String,
    name: String,
}

#[derive(Debug, Default)]
pub(crate) struct AstreaShortcutRegistry {
    shell_owners: HashMap<AstreaShortcutKey, AstreaShortcutRegistration>,
    other_registrations: Vec<AstreaShortcutRegistration>,
}

impl AstreaShortcutRegistry {
    fn register(
        &mut self,
        resource: astrea_shortcut_v1::AstreaShortcutV1,
        namespace: String,
        name: String,
        cancellation_serial: u32,
    ) {
        self.remove_dead();
        self.unregister(&resource);
        let registration = AstreaShortcutRegistration {
            resource,
            namespace: namespace.clone(),
            name: name.clone(),
        };
        if namespace == "astrea-shell" {
            let key = AstreaShortcutKey { namespace, name };
            if let Some(previous) = self.shell_owners.remove(&key)
                && !same_shortcut_resource(&previous.resource, &registration.resource)
            {
                previous.resource.cancelled(cancellation_serial);
            }
            self.shell_owners.insert(key, registration);
        } else {
            self.other_registrations.push(registration);
        }
    }

    fn unregister(&mut self, resource: &astrea_shortcut_v1::AstreaShortcutV1) {
        let resource_id = resource.id().protocol_id();
        self.shell_owners.retain(|_, registration| {
            registration.resource.id().protocol_id() != resource_id
                || !registration.resource.id().same_client_as(&resource.id())
        });
        self.other_registrations.retain(|registration| {
            registration.resource.id().protocol_id() != resource_id
                || !registration.resource.id().same_client_as(&resource.id())
        });
    }

    fn emit(
        &mut self,
        namespace: &str,
        name: &str,
        phase: AstreaShortcutPhase,
        serial: u32,
        timestamp: u32,
    ) -> usize {
        self.remove_dead();
        if namespace == "astrea-shell" {
            let key = AstreaShortcutKey {
                namespace: namespace.to_string(),
                name: name.to_string(),
            };
            let Some(registration) = self.shell_owners.get(&key) else {
                return 0;
            };
            if !registration.resource.is_alive() {
                self.shell_owners.remove(&key);
                return 0;
            }
            send_astrea_shortcut_event(&registration.resource, phase, serial, timestamp);
            return 1;
        }

        let mut dispatched = 0usize;
        self.other_registrations.retain(|registration| {
            if !registration.resource.is_alive() {
                return false;
            }
            if registration.namespace == namespace && registration.name == name {
                send_astrea_shortcut_event(&registration.resource, phase, serial, timestamp);
                dispatched = dispatched.saturating_add(1);
            }
            true
        });
        dispatched
    }

    fn remove_dead(&mut self) {
        self.shell_owners
            .retain(|_, registration| registration.resource.is_alive());
        self.other_registrations
            .retain(|registration| registration.resource.is_alive());
    }
}

fn same_shortcut_resource(
    left: &astrea_shortcut_v1::AstreaShortcutV1,
    right: &astrea_shortcut_v1::AstreaShortcutV1,
) -> bool {
    left.id().protocol_id() == right.id().protocol_id() && left.id().same_client_as(&right.id())
}

fn send_astrea_shortcut_event(
    resource: &astrea_shortcut_v1::AstreaShortcutV1,
    phase: AstreaShortcutPhase,
    serial: u32,
    timestamp: u32,
) {
    match phase {
        AstreaShortcutPhase::Pressed => resource.pressed(serial, timestamp),
        AstreaShortcutPhase::Repeated => resource.repeated(serial, timestamp),
        AstreaShortcutPhase::Released => resource.released(serial, timestamp),
    }
}

const ASTREA_SHELL_PID_ANCESTOR_LIMIT: usize = 32;

impl CompositorState {
    pub(in crate::compositor) fn authorize_astrea_shell_pid(&mut self, pid: u32) {
        self.astrea_shell_client_pids.insert(pid);
        if let Some(uid) = proc_uid(pid) {
            self.astrea_shell_client_uids.insert(uid);
        }
    }

    #[cfg(test)]
    pub(in crate::compositor) fn clear_astrea_shell_authorization(&mut self) {
        self.astrea_shell_client_pids.clear();
        self.astrea_shell_client_uids.clear();
    }

    pub(in crate::compositor) fn astrea_shortcut_registration_allowed(
        &self,
        namespace: &str,
        client: &Client,
        handle: &DisplayHandle,
    ) -> bool {
        if namespace != "astrea-shell" {
            return true;
        }
        if std::env::var_os("OBLIVION_ONE_ASTREA_SHORTCUTS_ALLOW_ANY_CLIENT").is_some() {
            return true;
        }
        let Ok(credentials) = client.get_credentials(handle) else {
            return false;
        };
        let Ok(pid) = u32::try_from(credentials.pid) else {
            return false;
        };
        astrea_shell_identity_is_authorized(
            pid,
            credentials.uid,
            &self.astrea_shell_client_pids,
            &self.astrea_shell_client_uids,
        )
    }

    pub(in crate::compositor) fn astrea_shell_client_allowed(
        &self,
        client: &Client,
        handle: &DisplayHandle,
    ) -> bool {
        if std::env::var_os("OBLIVION_ONE_ASTREA_SHORTCUTS_ALLOW_ANY_CLIENT").is_some() {
            return true;
        }
        let Ok(credentials) = client.get_credentials(handle) else {
            return false;
        };
        let Ok(pid) = u32::try_from(credentials.pid) else {
            return false;
        };
        astrea_shell_identity_is_authorized(
            pid,
            credentials.uid,
            &self.astrea_shell_client_pids,
            &self.astrea_shell_client_uids,
        )
    }

    pub(in crate::compositor) fn set_typhon_socket_name(&mut self, socket_name: String) {
        self.typhon_socket_name = Some(socket_name);
    }

    pub(in crate::compositor) fn queue_pending_process_launch(
        &mut self,
        launch: PendingProcessLaunch,
    ) {
        self.pending_process_launches.push_back(launch);
    }

    pub(in crate::compositor) fn take_pending_process_launches(
        &mut self,
    ) -> Vec<PendingProcessLaunch> {
        self.pending_process_launches.drain(..).collect()
    }

    pub(in crate::compositor) fn register_astrea_shortcut(
        &mut self,
        resource: astrea_shortcut_v1::AstreaShortcutV1,
        namespace: String,
        name: String,
    ) {
        let cancellation_serial = self.next_configure_serial();
        self.astrea_shortcut_registry
            .register(resource, namespace, name, cancellation_serial);
    }

    pub(in crate::compositor) fn unregister_astrea_shortcut(
        &mut self,
        resource: &astrea_shortcut_v1::AstreaShortcutV1,
    ) {
        self.astrea_shortcut_registry.unregister(resource);
    }

    pub(in crate::compositor) fn emit_astrea_shortcut(
        &mut self,
        namespace: &str,
        name: &str,
        phase: AstreaShortcutPhase,
        timestamp: u32,
    ) -> usize {
        let serial = self.next_configure_serial();
        self.astrea_shortcut_registry
            .emit(namespace, name, phase, serial, timestamp)
    }

    pub(in crate::compositor) fn remove_pending_process_launch(
        &mut self,
        request: &astrea_launch_request_v1::AstreaLaunchRequestV1,
    ) {
        let request_id = request.id().protocol_id();
        self.pending_process_launches.retain(|pending| {
            pending.request.id().protocol_id() != request_id
                || !pending.request.id().same_client_as(&request.id())
        });
    }
}

pub(crate) fn astrea_shell_identity_is_authorized(
    pid: u32,
    uid: u32,
    authorized_pids: &HashSet<u32>,
    authorized_uids: &HashSet<u32>,
) -> bool {
    astrea_shell_identity_is_authorized_with_lookup(
        pid,
        uid,
        authorized_pids,
        authorized_uids,
        proc_parent_pid,
    )
}

pub(crate) fn astrea_shell_identity_is_authorized_with_lookup(
    pid: u32,
    uid: u32,
    authorized_pids: &HashSet<u32>,
    authorized_uids: &HashSet<u32>,
    parent_pid: impl FnMut(u32) -> Option<u32>,
) -> bool {
    astrea_shell_pid_is_authorized_with_lookup(pid, authorized_pids, parent_pid)
        || authorized_uids.contains(&uid)
}

pub(crate) fn astrea_shell_pid_is_authorized_with_lookup(
    mut pid: u32,
    authorized_pids: &HashSet<u32>,
    mut parent_pid: impl FnMut(u32) -> Option<u32>,
) -> bool {
    for _ in 0..ASTREA_SHELL_PID_ANCESTOR_LIMIT {
        if authorized_pids.contains(&pid) {
            return true;
        }
        let Some(next_pid) = parent_pid(pid) else {
            return false;
        };
        if next_pid == 0 || next_pid == pid {
            return false;
        }
        pid = next_pid;
    }
    false
}

fn proc_parent_pid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close_paren = stat.rfind(')')?;
    let fields_after_comm = stat.get(close_paren + 2..)?;
    let mut fields = fields_after_comm.split_whitespace();
    fields.next()?;
    fields.next()?.parse().ok()
}

fn proc_uid(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|line| {
        let values = line.strip_prefix("Uid:")?;
        values.split_whitespace().next()?.parse().ok()
    })
}
