use super::*;

#[test]
fn window_id_is_nonzero_monotonic_and_not_reused() {
    let mut state = CompositorState::new(None);
    let first = state.allocate_window_id().expect("first id");
    let second = state.allocate_window_id().expect("second id");
    assert!(first.get() != 0);
    assert!(second > first);
    assert_ne!(first, second);
}

#[test]
fn xdg_toplevel_creation_builds_one_role_and_one_desktop_window() {
    let mut state = CompositorState::new(None);
    let id = state.allocate_window_id().expect("window id");
    let window = DesktopWindow::new_xdg(id, 41);
    state.insert_desktop_window(window).expect("insert window");
    assert_eq!(state.desktop_windows.len(), 1);
    assert_eq!(state.window_by_root_surface.get(&41), Some(&id));
}

#[test]
fn surface_lookup_resolves_stable_window_identity() {
    let mut state = CompositorState::new(None);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(id, 7))
        .expect("insert window");
    assert_eq!(state.window_id_for_surface(7), Some(id));
}

#[test]
fn metadata_updates_do_not_touch_backend_protocol_state() {
    let mut state = CompositorState::new(None);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(id, 7))
        .expect("insert window");
    let backend = state.window(id).expect("window").backend;
    state.window_mut(id).expect("window").metadata.title = Some("Typhon".into());
    assert_eq!(state.window(id).expect("window").backend, backend);
}

#[test]
fn destroying_xdg_role_removes_window_and_reverse_index_atomically() {
    let mut state = CompositorState::new(None);
    let id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(id, 7))
        .expect("insert window");
    assert!(state.remove_desktop_window(id).is_some());
    assert!(state.window(id).is_none());
    assert!(state.window_id_for_surface(7).is_none());
}

#[test]
fn parent_relationship_uses_window_id_not_surface_id() {
    let mut state = CompositorState::new(None);
    let parent = state.allocate_window_id().expect("parent id");
    let child = state.allocate_window_id().expect("child id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(parent, 100))
        .expect("insert parent");
    let mut child_window = DesktopWindow::new_xdg(child, 200);
    child_window.relationships.parent = Some(parent);
    state
        .insert_desktop_window(child_window)
        .expect("insert child");
    assert_eq!(
        state.window(child).expect("child").relationships.parent,
        Some(parent)
    );
}

#[test]
fn failed_role_creation_leaves_no_partial_desktop_window() {
    let mut state = CompositorState::new(None);
    let first = state.allocate_window_id().expect("first id");
    let second = state.allocate_window_id().expect("second id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(first, 9))
        .expect("insert first");
    let result = state.insert_desktop_window(DesktopWindow::new_xdg(second, 9));
    assert_eq!(result, Err(DesktopWindowError::DuplicateRootSurface));
    assert!(state.window(second).is_none());
    assert_eq!(state.window_id_for_surface(9), Some(first));
}
