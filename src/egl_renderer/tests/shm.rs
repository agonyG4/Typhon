use super::*;

#[test]
fn shm_resource_update_requires_full_resync_when_baseline_is_behind() {
    let surface = test_shm_surface(RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    }]));
    let resource = test_shm_resource(Some(SurfaceCommitCounter(1)));
    let state = SurfaceResourceSyncState {
        surface_id: surface.surface_id,
        complete_since: Some(SurfaceCommitCounter(2)),
        current_commit: SurfaceCommitCounter(3),
        authoritative: true,
    };

    assert_eq!(
        resource.update_for(&surface, state),
        EglSurfaceResourceUpdate::FullShmResync
    );
}

#[test]
fn shm_resource_update_keeps_partial_upload_when_damage_is_complete() {
    let surface = test_shm_surface(RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    }]));
    let resource = test_shm_resource(Some(SurfaceCommitCounter(2)));
    let state = SurfaceResourceSyncState {
        surface_id: surface.surface_id,
        complete_since: Some(SurfaceCommitCounter(2)),
        current_commit: SurfaceCommitCounter(3),
        authoritative: true,
    };

    assert_eq!(
        resource.update_for(&surface, state),
        EglSurfaceResourceUpdate::UploadDamage
    );
}

#[test]
fn shm_resource_update_reuses_exact_current_commit_without_upload() {
    let surface = test_shm_surface(RenderableSurfaceDamage::Empty);
    let resource = test_shm_resource(Some(SurfaceCommitCounter(3)));
    let state = SurfaceResourceSyncState {
        surface_id: surface.surface_id,
        complete_since: Some(SurfaceCommitCounter(2)),
        current_commit: SurfaceCommitCounter(3),
        authoritative: true,
    };

    assert_eq!(
        resource.update_for(&surface, state),
        EglSurfaceResourceUpdate::Reuse
    );
}

#[test]
fn shm_resource_update_history_loss_requires_full_resync() {
    let surface = test_shm_surface(RenderableSurfaceDamage::HistoryLost);
    let resource = test_shm_resource(Some(SurfaceCommitCounter(2)));
    let state = SurfaceResourceSyncState {
        surface_id: surface.surface_id,
        complete_since: Some(SurfaceCommitCounter(2)),
        current_commit: SurfaceCommitCounter(3),
        authoritative: true,
    };

    assert_eq!(
        resource.update_for(&surface, state),
        EglSurfaceResourceUpdate::FullShmResync
    );
}

#[test]
fn shm_resource_update_requires_full_resync_when_presentation_settlement_outruns_baseline() {
    let surface = test_shm_surface(RenderableSurfaceDamage::Empty);
    let resource = test_shm_resource(Some(SurfaceCommitCounter(1)));
    let state = SurfaceResourceSyncState {
        surface_id: surface.surface_id,
        complete_since: Some(SurfaceCommitCounter(2)),
        current_commit: SurfaceCommitCounter(2),
        authoritative: true,
    };

    assert_eq!(
        resource.update_for(&surface, state),
        EglSurfaceResourceUpdate::FullShmResync
    );
}

#[test]
fn shm_resource_update_keeps_accumulated_hidden_damage_partial() {
    let surface = test_shm_surface(RenderableSurfaceDamage::Partial(vec![
        SurfaceDamageRect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        },
        SurfaceDamageRect {
            x: 1,
            y: 1,
            width: 1,
            height: 1,
        },
    ]));
    let resource = test_shm_resource(Some(SurfaceCommitCounter(1)));
    let state = SurfaceResourceSyncState {
        surface_id: surface.surface_id,
        complete_since: Some(SurfaceCommitCounter(1)),
        current_commit: SurfaceCommitCounter(3),
        authoritative: true,
    };

    assert_eq!(
        resource.update_for(&surface, state),
        EglSurfaceResourceUpdate::UploadDamage
    );
}

#[test]
fn shm_resource_update_advances_empty_damage_baseline_without_upload() {
    let surface = test_shm_surface(RenderableSurfaceDamage::Empty);
    let mut resource = test_shm_resource(Some(SurfaceCommitCounter(1)));
    let current_commit = SurfaceCommitCounter(2);
    let state = SurfaceResourceSyncState {
        surface_id: surface.surface_id,
        complete_since: Some(SurfaceCommitCounter(1)),
        current_commit,
        authoritative: true,
    };
    assert_eq!(
        resource.update_for(&surface, state),
        EglSurfaceResourceUpdate::ReuseShm
    );

    resource.advance_shm_sync_baseline(current_commit);
    assert_eq!(resource.shm_synced_commit, Some(current_commit));
}
