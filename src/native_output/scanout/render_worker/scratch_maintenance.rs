use std::{
    sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError},
    time::Instant,
};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum WaitOutcome<T> {
    Command(T),
    Maintenance,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MaintenanceTarget {
    Active,
    Candidate,
}

pub(super) fn nearest_deadline(
    active: Option<Instant>,
    candidate: Option<Instant>,
) -> Option<Instant> {
    match (active, candidate) {
        (Some(active), Some(candidate)) => Some(active.min(candidate)),
        (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
        (None, None) => None,
    }
}

pub(super) fn due_target(
    active: Option<Instant>,
    candidate: Option<Instant>,
    now: Instant,
) -> Option<MaintenanceTarget> {
    match (
        active.filter(|deadline| *deadline <= now),
        candidate.filter(|deadline| *deadline <= now),
    ) {
        (Some(active), Some(candidate)) if candidate < active => Some(MaintenanceTarget::Candidate),
        (Some(_), _) => Some(MaintenanceTarget::Active),
        (None, Some(_)) => Some(MaintenanceTarget::Candidate),
        (None, None) => None,
    }
}

pub(super) fn wait_for_command<T>(
    commands: &Receiver<T>,
    deadline: Option<Instant>,
) -> WaitOutcome<T> {
    let Some(deadline) = deadline else {
        return commands
            .recv()
            .map(WaitOutcome::Command)
            .unwrap_or(WaitOutcome::Disconnected);
    };

    let now = Instant::now();
    if deadline <= now {
        return prioritize_queued_command(commands);
    }

    match commands.recv_timeout(deadline.saturating_duration_since(now)) {
        Ok(command) => WaitOutcome::Command(command),
        Err(RecvTimeoutError::Disconnected) => WaitOutcome::Disconnected,
        Err(RecvTimeoutError::Timeout) => prioritize_queued_command(commands),
    }
}

fn prioritize_queued_command<T>(commands: &Receiver<T>) -> WaitOutcome<T> {
    match commands.try_recv() {
        Ok(command) => WaitOutcome::Command(command),
        Err(TryRecvError::Empty) => WaitOutcome::Maintenance,
        Err(TryRecvError::Disconnected) => WaitOutcome::Disconnected,
    }
}

#[cfg(test)]
mod tests {
    use super::{MaintenanceTarget, WaitOutcome, due_target, nearest_deadline, wait_for_command};
    use std::{
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    #[test]
    fn no_deadline_wait_is_blocking_until_a_real_command_arrives() {
        let (sender, receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            sender.send(17).expect("receiver remains connected");
        });
        assert_eq!(wait_for_command(&receiver, None), WaitOutcome::Command(17));
        thread.join().expect("sender thread exits");
    }

    #[test]
    fn command_before_future_deadline_is_processed_normally() {
        let (sender, receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            sender.send(23).expect("receiver remains connected");
        });
        assert_eq!(
            wait_for_command(&receiver, Some(Instant::now() + Duration::from_secs(1))),
            WaitOutcome::Command(23)
        );
        thread.join().expect("sender thread exits");
    }

    #[test]
    fn queued_command_wins_over_an_already_due_deadline() {
        let (sender, receiver) = mpsc::channel();
        sender.send(29).expect("receiver remains connected");
        assert_eq!(
            wait_for_command(&receiver, Some(Instant::now() - Duration::from_millis(1))),
            WaitOutcome::Command(29)
        );
    }

    #[test]
    fn timeout_runs_maintenance_without_using_a_command() {
        let (_sender, receiver) = mpsc::channel::<()>();
        let started = Instant::now();
        assert_eq!(
            wait_for_command(&receiver, Some(Instant::now() + Duration::from_millis(5))),
            WaitOutcome::Maintenance
        );
        assert!(started.elapsed() >= Duration::from_millis(4));
    }

    #[test]
    fn disconnected_command_channel_exits_cleanly() {
        let (sender, receiver) = mpsc::channel::<()>();
        drop(sender);
        assert_eq!(wait_for_command(&receiver, None), WaitOutcome::Disconnected);
        assert_eq!(
            wait_for_command(&receiver, Some(Instant::now() + Duration::from_secs(1))),
            WaitOutcome::Disconnected
        );
        assert_eq!(
            wait_for_command(&receiver, Some(Instant::now() - Duration::from_secs(1))),
            WaitOutcome::Disconnected
        );
    }

    #[test]
    fn nearest_deadline_and_due_candidate_are_selected() {
        let now = Instant::now();
        let active = now + Duration::from_secs(2);
        let candidate = now + Duration::from_secs(1);
        assert_eq!(
            nearest_deadline(Some(active), Some(candidate)),
            Some(candidate)
        );
        assert_eq!(
            due_target(Some(active), Some(candidate), now + Duration::from_secs(1)),
            Some(MaintenanceTarget::Candidate)
        );
        assert_eq!(nearest_deadline(None, None), None);
        assert_eq!(due_target(Some(active), None, now), None);
    }

    #[test]
    fn partial_batch_reentry_keeps_commands_ahead_of_continuation() {
        let (sender, receiver) = mpsc::channel();
        sender.send(31).expect("receiver remains connected");
        // The pool arms another short deadline after a partial batch. At the
        // worker boundary, an already queued render still wins.
        assert_eq!(
            wait_for_command(&receiver, Some(Instant::now())),
            WaitOutcome::Command(31)
        );
    }
}
