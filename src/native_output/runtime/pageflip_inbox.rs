//! Routes DRM pageflip events from the one shared DRM fd to the head that
//! owns their CRTC. The active head drains the fd into the inbox and takes
//! its own completion; events for other heads wait there for their turn in
//! the same cycle.

use super::*;

/// Takes `own_crtc`'s completion out of `inbox`.
///
/// Mirrors the single-head drain: only the first event for a CRTC completes
/// in one cycle and later ones for it are dropped. Events for CRTCs no head
/// owns are dropped too; when the head has no completion, the first of them
/// is returned so the caller counts it as a wrong-CRTC event as before.
pub(super) fn take_head_pageflip(
    inbox: &mut Vec<DrmPresentationEvent>,
    own_crtc: u32,
    other_head_crtcs: &[u32],
) -> NativePageFlipDrain {
    let own = inbox
        .iter()
        .copied()
        .find(|event| event.crtc_id == own_crtc);
    let unowned = inbox
        .iter()
        .copied()
        .find(|event| event.crtc_id != own_crtc && !other_head_crtcs.contains(&event.crtc_id));
    inbox.retain(|event| other_head_crtcs.contains(&event.crtc_id) && event.crtc_id != own_crtc);
    NativePageFlipDrain {
        completion: own.or(unowned),
        ..NativePageFlipDrain::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(crtc_id: u32, sequence: u32) -> DrmPresentationEvent {
        DrmPresentationEvent {
            user_data: u64::from(sequence),
            sequence,
            timestamp: oblivion_one::native::drm::DrmPresentationTimestamp {
                seconds: 0,
                microseconds: 0,
            },
            crtc_id,
        }
    }

    #[test]
    fn single_head_takes_its_first_event_and_drops_later_ones() {
        let mut inbox = vec![event(206, 1), event(206, 2)];
        let drain = take_head_pageflip(&mut inbox, 206, &[]);
        assert_eq!(drain.completion.map(|event| event.sequence), Some(1));
        assert!(inbox.is_empty());
    }

    #[test]
    fn other_heads_events_wait_in_the_inbox() {
        let mut inbox = vec![event(398, 7), event(206, 1)];
        let drain = take_head_pageflip(&mut inbox, 206, &[398]);
        assert_eq!(drain.completion.map(|event| event.crtc_id), Some(206));
        assert_eq!(inbox, vec![event(398, 7)]);
        let drain = take_head_pageflip(&mut inbox, 398, &[206]);
        assert_eq!(drain.completion.map(|event| event.sequence), Some(7));
        assert!(inbox.is_empty());
    }

    #[test]
    fn unowned_crtc_is_reported_when_the_head_has_no_completion() {
        let mut inbox = vec![event(99, 3)];
        let drain = take_head_pageflip(&mut inbox, 206, &[398]);
        assert_eq!(drain.completion.map(|event| event.crtc_id), Some(99));
        assert!(inbox.is_empty());
    }

    #[test]
    fn own_completion_wins_over_unowned_events() {
        let mut inbox = vec![event(99, 3), event(206, 4)];
        let drain = take_head_pageflip(&mut inbox, 206, &[]);
        assert_eq!(drain.completion.map(|event| event.sequence), Some(4));
        assert!(inbox.is_empty());
    }
}
