use super::*;

const ASTREA_SHELL_PID_ANCESTOR_LIMIT: usize = 32;

impl CompositorState {
    pub(in crate::compositor) fn authorize_astrea_shell_pid(&mut self, pid: u32) {
        self.astrea_shell_client_pids.insert(pid);
        if let Some(uid) = proc_uid(pid) {
            self.astrea_shell_client_uids.insert(uid);
        }
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

    pub(in crate::compositor) fn register_astrea_shortcut(
        &mut self,
        resource: astrea_shortcut_v1::AstreaShortcutV1,
        namespace: String,
        name: String,
    ) {
        self.unregister_astrea_shortcut(&resource);
        self.astrea_shortcuts.push(AstreaShortcutRegistration {
            resource,
            namespace,
            name,
        });
    }

    pub(in crate::compositor) fn unregister_astrea_shortcut(
        &mut self,
        resource: &astrea_shortcut_v1::AstreaShortcutV1,
    ) {
        let resource_id = resource.id().protocol_id();
        self.astrea_shortcuts
            .retain(|registration| registration.resource.id().protocol_id() != resource_id);
    }

    pub(in crate::compositor) fn emit_astrea_shortcut_pressed(
        &mut self,
        namespace: &str,
        name: &str,
        timestamp: u32,
    ) -> usize {
        let serial = self.next_configure_serial();
        let mut dispatched = 0usize;
        self.astrea_shortcuts.retain(|registration| {
            if !registration.resource.is_alive() {
                return false;
            }
            if registration.namespace == namespace && registration.name == name {
                registration.resource.pressed(serial, timestamp);
                dispatched = dispatched.saturating_add(1);
            }
            true
        });
        dispatched
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
