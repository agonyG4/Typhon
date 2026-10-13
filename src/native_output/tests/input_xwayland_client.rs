use super::input::{NativeInputClientState, attach_native_input_test_buffer};
use super::input_protocol::{ClientCommand, ClientEvent};
use std::{os::unix::net::UnixStream, sync::mpsc, thread};
use wayland_client::protocol::{
    wl_compositor as client_wl_compositor, wl_seat as client_wl_seat, wl_shm as client_wl_shm,
};
use wayland_client::{Connection, globals::registry_queue_init};
use wayland_protocols::wp::pointer_constraints::zv1::client::{
    zwp_locked_pointer_v1 as client_zwp_locked_pointer_v1,
    zwp_pointer_constraints_v1 as client_zwp_pointer_constraints_v1,
};
use wayland_protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_manager_v1 as client_zwp_relative_pointer_manager_v1;
use wayland_protocols::xwayland::shell::v1::client::xwayland_shell_v1 as client_xwayland_shell_v1;

pub(super) fn spawn_native_input_xwayland_client(
    stream: UnixStream,
) -> (mpsc::Sender<ClientCommand>, mpsc::Receiver<ClientEvent>) {
    let (commands_sender, commands_receiver) = mpsc::channel();
    let (events_sender, events_receiver) = mpsc::channel();
    thread::spawn(move || {
        let connection = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) =
            registry_queue_init::<NativeInputClientState>(&connection).unwrap();
        let qh = queue.handle();
        let shell: client_xwayland_shell_v1::XwaylandShellV1 =
            globals.bind(&qh, 1..=1, ()).unwrap();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
        let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let constraints: client_zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
            globals.bind(&qh, 1..=1, ()).unwrap();
        let relative_manager: client_zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
            globals.bind(&qh, 1..=1, ()).unwrap();
        let pointer = seat.get_pointer(&qh, ());
        let _relative_pointer = relative_manager.get_relative_pointer(&pointer, &qh, ());
        let surface = compositor.create_surface(&qh, ());
        let xwayland_surface = shell.get_xwayland_surface(&surface, &qh, ());
        xwayland_surface.set_serial(0x1111_2222, 0x3333_4444);
        let mut state = NativeInputClientState::default();
        surface.commit();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        attach_native_input_test_buffer(&surface, &shm, &qh, 160, 120);
        surface.commit();
        connection.flush().unwrap();
        queue.roundtrip(&mut state).unwrap();
        events_sender.send(ClientEvent::ReadyForPointer).unwrap();
        assert_eq!(commands_receiver.recv().unwrap(), ClientCommand::SetCursor);
        queue.roundtrip(&mut state).unwrap();
        let cursor = compositor.create_surface(&qh, ());
        pointer.set_cursor(state.pointer_enter_serial.unwrap(), Some(&cursor), 3, 4);
        attach_native_input_test_buffer(&cursor, &shm, &qh, 24, 24);
        cursor.commit();
        let mut cursor_surfaces = vec![cursor];
        let mut locked_pointer: Option<client_zwp_locked_pointer_v1::ZwpLockedPointerV1> = None;
        connection.flush().unwrap();
        events_sender
            .send(ClientEvent::CursorReady {
                pointer_motion_count: state.pointer_motion_count,
                pointer_enter_count: state.pointer_enter_count,
                pointer_leave_count: state.pointer_leave_count,
            })
            .unwrap();
        loop {
            match commands_receiver.recv().unwrap() {
                ClientCommand::CaptureButtons => {
                    queue.roundtrip(&mut state).unwrap();
                    events_sender
                        .send(ClientEvent::Buttons {
                            pressed_count: state.pointer_button_press_count,
                            released_count: state.pointer_button_release_count,
                        })
                        .unwrap();
                }
                ClientCommand::LockPointer => {
                    let lock = constraints.lock_pointer(
                        &surface,
                        &pointer,
                        None,
                        client_zwp_pointer_constraints_v1::Lifetime::Persistent,
                        &qh,
                        (),
                    );
                    surface.commit();
                    locked_pointer = Some(lock);
                    connection.flush().unwrap();
                    queue.roundtrip(&mut state).unwrap();
                    events_sender.send(ClientEvent::LockRequested).unwrap();
                }
                ClientCommand::HideCursor => {
                    pointer.set_cursor(state.pointer_enter_serial.unwrap(), None, 0, 0);
                    connection.flush().unwrap();
                    queue.roundtrip(&mut state).unwrap();
                    events_sender
                        .send(ClientEvent::CursorState {
                            pointer_motion_count: state.pointer_motion_count,
                            relative_motion_count: state.relative_motion_count,
                            locked_count: state.locked_count,
                        })
                        .unwrap();
                }
                ClientCommand::ReplaceCursor => {
                    let cursor = compositor.create_surface(&qh, ());
                    pointer.set_cursor(state.pointer_enter_serial.unwrap(), Some(&cursor), 1, 2);
                    attach_native_input_test_buffer(&cursor, &shm, &qh, 32, 20);
                    cursor.commit();
                    cursor_surfaces.push(cursor);
                    connection.flush().unwrap();
                    queue.roundtrip(&mut state).unwrap();
                    events_sender
                        .send(ClientEvent::CursorState {
                            pointer_motion_count: state.pointer_motion_count,
                            relative_motion_count: state.relative_motion_count,
                            locked_count: state.locked_count,
                        })
                        .unwrap();
                }
                ClientCommand::CaptureCursorState => {
                    queue.roundtrip(&mut state).unwrap();
                    events_sender
                        .send(ClientEvent::CursorState {
                            pointer_motion_count: state.pointer_motion_count,
                            relative_motion_count: state.relative_motion_count,
                            locked_count: state.locked_count,
                        })
                        .unwrap();
                }
                ClientCommand::Finish => {
                    queue.roundtrip(&mut state).unwrap();
                    events_sender
                        .send(ClientEvent::Finished {
                            pointer_motion_count: state.pointer_motion_count,
                            pointer_surface_x: state.pointer_surface_x,
                            pointer_surface_y: state.pointer_surface_y,
                            pointer_enter_count: state.pointer_enter_count,
                            pointer_leave_count: state.pointer_leave_count,
                        })
                        .unwrap();
                    break;
                }
                ClientCommand::SetCursor
                | ClientCommand::CaptureActive
                | ClientCommand::BeginXdgMove
                | ClientCommand::BeginXdgResize
                | ClientCommand::CaptureKeyboard
                | ClientCommand::CaptureKeyboardState => panic!("unexpected XWayland test command"),
            }
        }
        drop(locked_pointer);
        drop(cursor_surfaces);
    });
    (commands_sender, events_receiver)
}
