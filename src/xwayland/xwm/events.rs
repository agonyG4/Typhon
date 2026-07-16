use x11rb::connection::Connection;

use super::{Xwm, XwmDrain, XwmError};

pub(crate) fn drain(xwm: &mut Xwm, budget: usize) -> Result<XwmDrain, XwmError> {
    let mut processed = 0;
    while processed < budget {
        let Some(_event) = xwm
            .connection
            .poll_for_event()
            .map_err(XwmError::Connection)?
        else {
            break;
        };
        processed += 1;
    }
    Ok(XwmDrain {
        processed,
        budget_exhausted: processed == budget && budget != 0,
    })
}
