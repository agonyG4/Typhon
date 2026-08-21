use super::workspace_protocol::{WorkspaceProtocolSnapshot, WorkspaceRequestTransaction};
use crate::wm::WorkspaceId;

#[test]
fn snapshot_uses_stable_ids_zero_based_coordinates_and_one_active_workspace() {
    let snapshot = WorkspaceProtocolSnapshot::from_workspace_ids(
        [WorkspaceId::new(1).unwrap(), WorkspaceId::new(4).unwrap()],
        WorkspaceId::new(4).unwrap(),
    );

    assert_eq!(snapshot.workspaces[0].id, "typhon.workspace.1");
    assert_eq!(snapshot.workspaces[0].name, "1");
    assert_eq!(snapshot.workspaces[0].coordinates, vec![0]);
    assert!(!snapshot.workspaces[0].active);
    assert!(snapshot.workspaces[1].active);
    assert_eq!(snapshot.workspaces[1].coordinates, vec![1]);
    assert_eq!(
        snapshot
            .workspaces
            .iter()
            .filter(|workspace| workspace.active)
            .count(),
        1
    );
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
