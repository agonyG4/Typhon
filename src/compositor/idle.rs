use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleState {
    Active,
    Idle,
    DpmsOff,
}

#[derive(Debug, Clone)]
pub struct IdleManager {
    idle_timeout: Duration,
    dpms_timeout: Duration,
    last_activity: Instant,
    inhibited_count: usize,
}

impl IdleManager {
    pub fn new(idle_timeout: Duration, dpms_timeout: Duration, now: Instant) -> Self {
        Self {
            idle_timeout,
            dpms_timeout: dpms_timeout.max(idle_timeout),
            last_activity: now,
            inhibited_count: 0,
        }
    }

    pub fn notify_activity(&mut self) {
        self.notify_activity_at(Instant::now());
    }

    pub fn notify_activity_at(&mut self, now: Instant) {
        self.last_activity = now;
    }

    pub fn inhibit(&mut self) {
        self.inhibited_count = self.inhibited_count.saturating_add(1);
    }

    pub fn uninhibit(&mut self) {
        self.inhibited_count = self.inhibited_count.saturating_sub(1);
    }

    pub const fn is_inhibited(&self) -> bool {
        self.inhibited_count > 0
    }

    pub fn state_at(&self, now: Instant) -> IdleState {
        if self.is_inhibited() {
            return IdleState::Active;
        }

        let elapsed = now.saturating_duration_since(self.last_activity);
        if elapsed >= self.dpms_timeout {
            IdleState::DpmsOff
        } else if elapsed >= self.idle_timeout {
            IdleState::Idle
        } else {
            IdleState::Active
        }
    }
}

impl Default for IdleManager {
    fn default() -> Self {
        Self::new(
            Duration::from_secs(300),
            Duration::from_secs(600),
            Instant::now(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_manager_transitions_from_active_to_idle_to_dpms() {
        let start = Instant::now();
        let manager = IdleManager::new(Duration::from_secs(5), Duration::from_secs(10), start);

        assert_eq!(
            manager.state_at(start + Duration::from_secs(4)),
            IdleState::Active
        );
        assert_eq!(
            manager.state_at(start + Duration::from_secs(5)),
            IdleState::Idle
        );
        assert_eq!(
            manager.state_at(start + Duration::from_secs(10)),
            IdleState::DpmsOff
        );
    }

    #[test]
    fn idle_manager_activity_resets_timer() {
        let start = Instant::now();
        let mut manager = IdleManager::new(Duration::from_secs(5), Duration::from_secs(10), start);

        manager.notify_activity_at(start + Duration::from_secs(8));

        assert_eq!(
            manager.state_at(start + Duration::from_secs(12)),
            IdleState::Active
        );
        assert_eq!(
            manager.state_at(start + Duration::from_secs(13)),
            IdleState::Idle
        );
    }

    #[test]
    fn idle_manager_inhibition_keeps_state_active() {
        let start = Instant::now();
        let mut manager = IdleManager::new(Duration::from_secs(5), Duration::from_secs(10), start);

        manager.inhibit();

        assert_eq!(
            manager.state_at(start + Duration::from_secs(20)),
            IdleState::Active
        );
        manager.uninhibit();
        assert_eq!(
            manager.state_at(start + Duration::from_secs(20)),
            IdleState::DpmsOff
        );
    }
}
