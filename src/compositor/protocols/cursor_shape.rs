use super::super::*;
use wayland_protocols::wp::cursor_shape::v1::server::{
    wp_cursor_shape_device_v1, wp_cursor_shape_manager_v1,
};
use wayland_server::{GlobalDispatch, New, Resource};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum ProtocolCursorShape {
    Default = 1,
    ContextMenu = 2,
    Help = 3,
    Pointer = 4,
    Progress = 5,
    Wait = 6,
    Cell = 7,
    Crosshair = 8,
    Text = 9,
    VerticalText = 10,
    Alias = 11,
    Copy = 12,
    Move = 13,
    NoDrop = 14,
    NotAllowed = 15,
    Grab = 16,
    Grabbing = 17,
    EResize = 18,
    NResize = 19,
    NeResize = 20,
    NwResize = 21,
    SResize = 22,
    SeResize = 23,
    SwResize = 24,
    WResize = 25,
    EwResize = 26,
    NsResize = 27,
    NeswResize = 28,
    NwseResize = 29,
    ColResize = 30,
    RowResize = 31,
    AllScroll = 32,
    ZoomIn = 33,
    ZoomOut = 34,
    DndAsk = 35,
    AllResize = 36,
}

impl TryFrom<u32> for ProtocolCursorShape {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Ok(match value {
            1 => Self::Default,
            2 => Self::ContextMenu,
            3 => Self::Help,
            4 => Self::Pointer,
            5 => Self::Progress,
            6 => Self::Wait,
            7 => Self::Cell,
            8 => Self::Crosshair,
            9 => Self::Text,
            10 => Self::VerticalText,
            11 => Self::Alias,
            12 => Self::Copy,
            13 => Self::Move,
            14 => Self::NoDrop,
            15 => Self::NotAllowed,
            16 => Self::Grab,
            17 => Self::Grabbing,
            18 => Self::EResize,
            19 => Self::NResize,
            20 => Self::NeResize,
            21 => Self::NwResize,
            22 => Self::SResize,
            23 => Self::SeResize,
            24 => Self::SwResize,
            25 => Self::WResize,
            26 => Self::EwResize,
            27 => Self::NsResize,
            28 => Self::NeswResize,
            29 => Self::NwseResize,
            30 => Self::ColResize,
            31 => Self::RowResize,
            32 => Self::AllScroll,
            33 => Self::ZoomIn,
            34 => Self::ZoomOut,
            35 => Self::DndAsk,
            36 => Self::AllResize,
            _ => return Err(()),
        })
    }
}

impl ProtocolCursorShape {
    pub(crate) const fn aliases(self) -> &'static [&'static str] {
        match self {
            Self::Default => &["default", "left_ptr", "arrow"],
            Self::ContextMenu => &["context-menu", "context_menu"],
            Self::Help => &["help"],
            Self::Pointer => &["pointer", "left_ptr", "default", "arrow"],
            Self::Progress => &["progress"],
            Self::Wait => &["wait"],
            Self::Cell => &["cell"],
            Self::Crosshair => &["crosshair"],
            Self::Text => &["text", "ibeam"],
            Self::VerticalText => &["vertical-text", "vertical_text"],
            Self::Alias => &["alias"],
            Self::Copy => &["copy"],
            Self::Move => &["move", "fleur", "all-scroll"],
            Self::NoDrop => &["no-drop", "no_drop"],
            Self::NotAllowed => &["not-allowed", "not_allowed"],
            Self::Grab => &["grab"],
            Self::Grabbing => &["grabbing"],
            Self::EResize => &["e-resize", "right_side"],
            Self::NResize => &["n-resize", "top_side"],
            Self::NeResize => &["ne-resize", "top_right_corner"],
            Self::NwResize => &["nw-resize", "top_left_corner"],
            Self::SResize => &["s-resize", "bottom_side"],
            Self::SeResize => &["se-resize", "bottom_right_corner"],
            Self::SwResize => &["sw-resize", "bottom_left_corner"],
            Self::WResize => &["w-resize", "left_side"],
            Self::EwResize => &["ew-resize", "size_hor", "sb_h_double_arrow"],
            Self::NsResize => &["ns-resize", "size_ver", "sb_v_double_arrow"],
            Self::NeswResize => &["nesw-resize", "size_bdiag"],
            Self::NwseResize => &["nwse-resize", "size_fdiag"],
            Self::ColResize => &["col-resize", "vertical-text"],
            Self::RowResize => &["row-resize", "row-resize"],
            Self::AllScroll => &["all-scroll", "fleur"],
            Self::ZoomIn => &["zoom-in"],
            Self::ZoomOut => &["zoom-out"],
            Self::DndAsk => &["dnd-ask", "dnd_ask"],
            Self::AllResize => &["all-resize", "all_resize"],
        }
    }

    pub(crate) const fn as_u32(self) -> u32 {
        self as u32
    }

    pub(crate) const fn requires_version_two(self) -> bool {
        matches!(self, Self::DndAsk | Self::AllResize)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CursorShapeDeviceData {
    pointer: wl_pointer::WlPointer,
}

impl GlobalDispatch<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1, ()> for CompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
        request: wp_cursor_shape_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_cursor_shape_manager_v1::Request::Destroy => {}
            wp_cursor_shape_manager_v1::Request::GetPointer { id, pointer } => {
                let same_client = pointer
                    .client()
                    .is_some_and(|owner| owner.id() == client.id());
                if same_client && pointer.is_alive() {
                    data_init.init(id, CursorShapeDeviceData { pointer });
                }
            }
            wp_cursor_shape_manager_v1::Request::GetTabletToolV2 { .. } => {
                state.compliance_metrics.note_unhandled_request(
                    "wp_cursor_shape_manager_v1",
                    resource.version(),
                    UnhandledRequestClass::SupportedButUnhandled,
                );
            }
            _ => {
                state.compliance_metrics.note_unhandled_request(
                    "wp_cursor_shape_manager_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<wp_cursor_shape_device_v1::WpCursorShapeDeviceV1, CursorShapeDeviceData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &wp_cursor_shape_device_v1::WpCursorShapeDeviceV1,
        request: wp_cursor_shape_device_v1::Request,
        data: &CursorShapeDeviceData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_cursor_shape_device_v1::Request::Destroy => {}
            wp_cursor_shape_device_v1::Request::SetShape { serial, shape } => {
                let Ok(shape) = ProtocolCursorShape::try_from(shape) else {
                    state.post_protocol_error(
                        client,
                        resource,
                        wp_cursor_shape_device_v1::Error::InvalidShape,
                        "invalid cursor shape value",
                    );
                    return;
                };
                if shape.requires_version_two() && resource.version() < 2 {
                    state.post_protocol_error(
                        client,
                        resource,
                        wp_cursor_shape_device_v1::Error::InvalidShape,
                        "cursor shape requires version 2",
                    );
                    return;
                }
                if data.pointer.is_alive()
                    && data
                        .pointer
                        .client()
                        .is_some_and(|owner| owner.id() == client.id())
                {
                    state.set_pointer_shape(&data.pointer, serial, shape.as_u32());
                }
            }
            _ => {
                state.compliance_metrics.note_unhandled_request(
                    "wp_cursor_shape_device_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProtocolCursorShape;

    #[test]
    fn protocol_cursor_shape_accepts_all_version_two_values() {
        for value in 1..=36 {
            assert!(
                ProtocolCursorShape::try_from(value).is_ok(),
                "shape={value}"
            );
        }
        assert!(ProtocolCursorShape::try_from(0).is_err());
        assert!(ProtocolCursorShape::try_from(37).is_err());
    }

    #[test]
    fn protocol_cursor_shape_has_stable_aliases_for_theme_lookup() {
        assert_eq!(
            ProtocolCursorShape::try_from(4).unwrap().aliases(),
            &["pointer", "left_ptr", "default", "arrow"]
        );
        assert!(
            ProtocolCursorShape::try_from(35)
                .unwrap()
                .aliases()
                .contains(&"dnd-ask")
        );
        assert!(
            ProtocolCursorShape::try_from(36)
                .unwrap()
                .aliases()
                .contains(&"all-resize")
        );
    }
}
