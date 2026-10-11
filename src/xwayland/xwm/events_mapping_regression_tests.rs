use super::*;

#[test]
fn managed_map_command_preserves_nonzero_frame_extents() {
    let generation = generation(34);
    let (mut xwm, mut peer) = test_fixture(generation);
    let handle = prepare_managed_window(&mut xwm, 134, true, true, true);
    let extents_atom = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::NetFrameExtents);
    let wm_state_atom = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::WmState);
    let net_wm_state_atom = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::NetWmState);
    let hidden_atom = xwm
        .atoms
        .get(super::super::super::atoms::XwmAtomName::NetWmStateHidden);

    super::super::super::commands::execute(
        &mut xwm,
        XwmCommand::SetFrameExtents(handle, [3, 5, 21, 7]),
    )
    .expect("publish decorated frame extents");
    xwm.connection.flush().expect("flush frame extent property");
    let published = read_fixture_requests(&mut peer);
    assert_eq!(
        change_property32_values(&published, extents_atom),
        vec![vec![3, 5, 21, 7]],
        "the managed window must start with its nonzero decoration extents"
    );

    super::super::super::commands::execute(
        &mut xwm,
        XwmCommand::SetState {
            window: handle,
            state: super::super::super::X11PublishedState {
                hidden: true,
                ..Default::default()
            },
        },
    )
    .expect("publish Iconic and HIDDEN state");
    xwm.connection.flush().expect("flush Iconic state");
    let hidden_state = read_fixture_requests(&mut peer);
    assert!(change_property32_values(&hidden_state, extents_atom).is_empty());
    assert_eq!(
        change_property32_values(&hidden_state, wm_state_atom),
        vec![vec![3, 0]]
    );
    assert_eq!(
        change_property32_values(&hidden_state, net_wm_state_atom),
        vec![vec![hidden_atom]]
    );

    super::super::super::commands::execute(&mut xwm, XwmCommand::Unmap(handle))
        .expect("WM unmap command");
    xwm.connection.flush().expect("flush WM unmap");
    let unmap_requests = read_fixture_requests(&mut peer);
    assert_eq!(
        change_property32_values(&unmap_requests, wm_state_atom),
        vec![vec![3, 0]]
    );
    normalize(&mut xwm, unmap_event(handle.xid())).expect("WM UnmapNotify");

    super::super::super::commands::execute(&mut xwm, XwmCommand::Map(handle))
        .expect("managed remap command");
    xwm.connection.flush().expect("flush managed remap");
    let remap_requests = read_fixture_requests(&mut peer);
    assert!(
        change_property32_values(&remap_requests, extents_atom).is_empty(),
        "managed remapping must leave _NET_FRAME_EXTENTS untouched"
    );
    assert_eq!(
        change_property32_values(&remap_requests, wm_state_atom),
        vec![vec![1, 0]]
    );
}

fn change_property32_values(requests: &[u8], property_atom: u32) -> Vec<Vec<u32>> {
    let mut values = Vec::new();
    let mut cursor = 0;
    while cursor + 4 <= requests.len() {
        let request_units = usize::from(u16::from_ne_bytes([
            requests[cursor + 2],
            requests[cursor + 3],
        ]));
        let request_bytes = request_units.saturating_mul(4);
        if request_bytes < 4 || cursor + request_bytes > requests.len() {
            break;
        }
        if requests[cursor] == 18 && request_bytes >= 24 {
            let request_property = u32::from_ne_bytes(
                requests[cursor + 8..cursor + 12]
                    .try_into()
                    .expect("X11 property atom bytes"),
            );
            let format = requests[cursor + 16];
            let item_count = usize::from(u16::from_ne_bytes([
                requests[cursor + 20],
                requests[cursor + 21],
            ]));
            if request_property == property_atom && format == 32 {
                let data_start = cursor + 24;
                let data_end = data_start.saturating_add(item_count.saturating_mul(4));
                if data_end <= cursor + request_bytes {
                    values.push(
                        requests[data_start..data_end]
                            .chunks_exact(4)
                            .map(|chunk| {
                                u32::from_ne_bytes(
                                    chunk.try_into().expect("X11 property value bytes"),
                                )
                            })
                            .collect(),
                    );
                }
            }
        }
        cursor += request_bytes;
    }
    values
}
