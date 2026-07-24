use std::{cell::Cell, io};

use super::complete_confirmed_pageflip_with_timing;

#[test]
fn timing_failure_after_confirmed_pageflip_still_completes_frame_ownership() {
    let scene_damage_committed = Cell::new(false);
    let surface_damage_committed = Cell::new(false);
    let callbacks_completed = Cell::new(false);
    let presentation_feedback_completed = Cell::new(false);
    let slot_ownership_completed = Cell::new(false);
    let (timing, timing_error): (Option<u64>, Option<io::Error>) =
        complete_confirmed_pageflip_with_timing::<u64>(
            Err(io::Error::from_raw_os_error(libc::EIO)),
            || {
                scene_damage_committed.set(true);
                surface_damage_committed.set(true);
                callbacks_completed.set(true);
                presentation_feedback_completed.set(true);
                slot_ownership_completed.set(true);
            },
        );

    assert!(scene_damage_committed.get());
    assert!(surface_damage_committed.get());
    assert!(callbacks_completed.get());
    assert!(presentation_feedback_completed.get());
    assert!(slot_ownership_completed.get());
    assert_eq!(timing, None);
    assert_eq!(timing_error.unwrap().raw_os_error(), Some(libc::EIO));
}
