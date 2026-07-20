use std::os::unix::net::UnixStream;

use crate::xwayland::XwaylandGeneration;

use super::{Xwm, XwmStartupError};

impl Xwm {
    pub fn connect(
        generation: XwaylandGeneration,
        stream: UnixStream,
    ) -> Result<Self, XwmStartupError> {
        let _ = generation;
        drop(stream);
        Err(XwmStartupError::Protocol(
            "synchronous XWM connection is unavailable; use the incremental startup driver"
                .to_owned(),
        ))
    }
}

impl Drop for Xwm {
    fn drop(&mut self) {
        use x11rb::protocol::sync::ConnectionExt as _;
        use x11rb::{
            connection::Connection as _, protocol::xproto::ConnectionExt as XprotoConnectionExt,
        };

        for alarm in self.sync_alarms.values().copied() {
            let _ = self.connection.sync_destroy_alarm(alarm);
        }
        let _ = self.connection.destroy_window(self.supporting_wm_check);
        let _ = self.connection.flush();
    }
}
