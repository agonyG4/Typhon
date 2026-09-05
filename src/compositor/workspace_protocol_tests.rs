use super::workspace_protocol::{WorkspaceProtocolSnapshot, WorkspaceRequestTransaction};
use crate::wm::WorkspaceId;
use std::collections::BTreeSet;

#[test]
fn snapshot_uses_stable_ids_zero_based_coordinates_and_one_active_workspace() {
    let active = WorkspaceId::new(1).unwrap();
    let occupied = BTreeSet::from([WorkspaceId::new(4).unwrap()]);
    let snapshot = WorkspaceProtocolSnapshot::from_workspace_ids(
        [
            active,
            WorkspaceId::new(4).unwrap(),
            WorkspaceId::new(7).unwrap(),
        ],
        active,
        &occupied,
    );

    assert_eq!(snapshot.workspaces[0].id, "typhon.workspace.1");
    assert_eq!(snapshot.workspaces[0].name, "1");
    assert_eq!(snapshot.workspaces[0].coordinates, vec![0]);
    assert!(snapshot.workspaces[0].active);
    assert!(!snapshot.workspaces[0].hidden);
    assert!(!snapshot.workspaces[1].active);
    assert!(!snapshot.workspaces[1].hidden);
    assert_eq!(snapshot.workspaces[1].coordinates, vec![1]);
    assert!(!snapshot.workspaces[2].active);
    assert!(snapshot.workspaces[2].hidden);
    assert_eq!(snapshot.workspaces[2].coordinates, vec![2]);
    assert_eq!(
        snapshot
            .workspaces
            .iter()
            .filter(|workspace| workspace.active)
            .count(),
        1
    );

    let occupied_active = BTreeSet::from([active, WorkspaceId::new(4).unwrap()]);
    let occupied_snapshot = WorkspaceProtocolSnapshot::from_workspace_ids(
        [
            active,
            WorkspaceId::new(4).unwrap(),
            WorkspaceId::new(7).unwrap(),
        ],
        active,
        &occupied_active,
    );
    assert!(!occupied_snapshot.workspaces[0].hidden);
}

#[test]
fn transaction_uses_last_valid_activation_and_ignores_invalid_requests() {
    let mut transaction = WorkspaceRequestTransaction::default();
    let workspace_one = WorkspaceId::new(1).unwrap();
    let workspace_four = WorkspaceId::new(4).unwrap();

    transaction.request_activation(workspace_one, true);
    transaction.request_activation(workspace_four, true);
    transaction.request_activation(WorkspaceId::new(99).unwrap(), false);

    assert_eq!(transaction.take_activation(), Some(workspace_four));
}

#[test]
fn initial_snapshot_hides_empty_inactive_logical_workspaces() {
    let state = super::CompositorState::new(None);
    let snapshot = state.workspace_protocol_snapshot();

    assert_eq!(snapshot.workspaces.len(), 10);
    assert!(!snapshot.workspaces[0].hidden);
    assert!(
        snapshot.workspaces[1..]
            .iter()
            .all(|workspace| workspace.hidden)
    );
}

#[test]
fn structure_changes_schedule_presence_publication_until_published() {
    let mut state = super::CompositorState::new(None);
    assert!(!state.workspace_presence_dirty);

    state.mark_astrea_toplevel_structure_dirty();
    assert!(state.workspace_presence_dirty);

    state.publish_workspace_state();
    assert!(!state.workspace_presence_dirty);
}
