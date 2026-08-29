use super::RegistryTestState;
use wayland_client::protocol::wl_pointer as client_wl_pointer;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};

impl Dispatch<client_wl_pointer::WlPointer, ()> for RegistryTestState {
    fn event(
        state: &mut Self,
        proxy: &client_wl_pointer::WlPointer,
        event: client_wl_pointer::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            client_wl_pointer::Event::Enter {
                serial, surface, ..
            } => {
                state.pointer_enter = true;
                state.pointer_enter_count += 1;
                state.pointer_enter_serial = Some(serial);
                state
                    .pointer_enter_serials
                    .push((proxy.id().protocol_id(), serial));
                state.pointer_enter_surface_id = Some(surface.id().protocol_id());
                state.pointer_event_log.push("enter");
            }
            client_wl_pointer::Event::Leave { .. } => {
                state.pointer_leave_count += 1;
                state.pointer_enter_surface_id = None;
                state.pointer_event_log.push("leave");
            }
            client_wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                state.pointer_motion = true;
                state.pointer_surface_x = Some(surface_x);
                state.pointer_surface_y = Some(surface_y);
                state.pointer_event_log.push("motion");
            }
            client_wl_pointer::Event::Warp {
                surface_x,
                surface_y,
            } => {
                state.pointer_surface_x = Some(surface_x);
                state.pointer_surface_y = Some(surface_y);
                state.pointer_event_log.push("warp");
            }
            client_wl_pointer::Event::Button {
                serial,
                state: button_state,
                ..
            } => {
                state.pointer_button = true;
                state.pointer_button_serial = Some(serial);
                state.pointer_button_surface_id = state.pointer_enter_surface_id;
                if let Some(surface_id) = state.pointer_enter_surface_id {
                    state.pointer_button_surface_ids.push(surface_id);
                }
                match button_state {
                    WEnum::Value(client_wl_pointer::ButtonState::Pressed) => {
                        state.pointer_event_log.push("button_pressed");
                    }
                    WEnum::Value(client_wl_pointer::ButtonState::Released) => {
                        state.pointer_event_log.push("button_released");
                    }
                    _ => state.pointer_event_log.push("button"),
                }
            }
            client_wl_pointer::Event::Axis {
                axis: WEnum::Value(axis),
                time,
                value,
            } => {
                state.pointer_axis = true;
                match axis {
                    client_wl_pointer::Axis::VerticalScroll => {
                        state.pointer_vertical_axis = Some(value);
                    }
                    client_wl_pointer::Axis::HorizontalScroll => {
                        state.pointer_horizontal_axis = Some(value);
                    }
                    _ => {}
                }
                state.pointer_axis_times.push(time);
                state.pointer_event_log.push("axis");
            }
            client_wl_pointer::Event::AxisSource { axis_source } => {
                let source = match axis_source {
                    WEnum::Value(client_wl_pointer::AxisSource::Wheel) => 0,
                    WEnum::Value(client_wl_pointer::AxisSource::Finger) => 1,
                    WEnum::Value(client_wl_pointer::AxisSource::Continuous) => 2,
                    WEnum::Value(client_wl_pointer::AxisSource::WheelTilt) => 3,
                    WEnum::Value(_) => u32::MAX,
                    WEnum::Unknown(source) => source,
                };
                state.pointer_axis_sources.push(source);
                state.pointer_event_log.push("axis_source");
            }
            client_wl_pointer::Event::AxisDiscrete { axis, discrete } => {
                let axis = match axis {
                    WEnum::Value(client_wl_pointer::Axis::VerticalScroll) => 0,
                    WEnum::Value(client_wl_pointer::Axis::HorizontalScroll) => 1,
                    WEnum::Value(_) => u32::MAX,
                    WEnum::Unknown(axis) => axis,
                };
                state.pointer_axis_discrete.push((axis, discrete));
                state.pointer_event_log.push("axis_discrete");
            }
            client_wl_pointer::Event::AxisValue120 { axis, value120 } => {
                let axis = match axis {
                    WEnum::Value(client_wl_pointer::Axis::VerticalScroll) => 0,
                    WEnum::Value(client_wl_pointer::Axis::HorizontalScroll) => 1,
                    WEnum::Value(_) => u32::MAX,
                    WEnum::Unknown(axis) => axis,
                };
                state.pointer_axis_value120.push((axis, value120));
                state.pointer_event_log.push("axis_value120");
            }
            client_wl_pointer::Event::AxisStop { time, axis } => {
                let axis = match axis {
                    WEnum::Value(client_wl_pointer::Axis::VerticalScroll) => 0,
                    WEnum::Value(client_wl_pointer::Axis::HorizontalScroll) => 1,
                    WEnum::Value(_) => u32::MAX,
                    WEnum::Unknown(axis) => axis,
                };
                state.pointer_axis_stops.push((time, axis));
                state.pointer_event_log.push("axis_stop");
            }
            client_wl_pointer::Event::Frame => {
                state.pointer_frame_count += 1;
                state
                    .pointer_frame_resource_ids
                    .push(proxy.id().protocol_id());
                if state.pointer_event_log.last() == Some(&"enter") {
                    state.pointer_enter_frame_count += 1;
                }
                if state.sdl_pending_relative_motion_count > 0 {
                    state.sdl_camera_motion_count += state.sdl_pending_relative_motion_count;
                    state.sdl_pending_relative_motion_count = 0;
                }
                state.pointer_event_log.push("frame");
                if state.pointer_event_log.contains(&"enter")
                    && !state
                        .pointer_event_log
                        .windows(2)
                        .any(|events| events == ["enter", "frame"])
                {
                    state.pointer_enter_without_frame_count += 1;
                }
            }
            _ => {}
        }
    }
}
