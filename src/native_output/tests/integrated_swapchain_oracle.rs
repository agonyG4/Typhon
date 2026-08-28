//! Deterministic client/output swapchain integration oracle.
//!
//! This model deliberately keeps client content, physical rendered slots,
//! submitted transactions, and confirmed presentation state separate. It is
//! test evidence for the ownership boundaries. The final test also models a
//! fixed-refresh client/output cadence without using wall-clock sleeps.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Damage {
    start: usize,
    end: usize,
}

impl Damage {
    const FULL: Self = Self { start: 0, end: 4 };
    const EMPTY: Self = Self { start: 0, end: 0 };

    const fn is_empty(self) -> bool {
        self.start == self.end
    }

    const fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        Self {
            start: if self.start < other.start {
                self.start
            } else {
                other.start
            },
            end: if self.end > other.end {
                self.end
            } else {
                other.end
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClientCommit {
    sequence: u64,
    buffer_id: u8,
    image: [u8; 4],
    damage: Damage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputSlotState {
    Available,
    Quarantined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OutputSlot {
    physical_pixels: [u8; 4],
    last_presented_serial: Option<u64>,
    state: OutputSlotState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RenderedCandidate {
    commit: ClientCommit,
    slot: usize,
    physical_pixels: [u8; 4],
    sampled_damage: Damage,
    buffer_age: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SubmittedCandidate {
    transaction_id: u64,
    candidate: RenderedCandidate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PresentedState {
    slot: usize,
    serial: u64,
    logical_reference_image: [u8; 4],
    surface_commit: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PresentedDamage {
    serial: u64,
    damage: Damage,
    image: [u8; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PresentedOutput {
    state: PresentedState,
    candidate: RenderedCandidate,
    physical_pixels: [u8; 4],
}

struct IntegratedSwapchainOracle {
    slots: [OutputSlot; 3],
    presented_history: Vec<PresentedDamage>,
    history_capacity: usize,
    client_commits: Vec<ClientCommit>,
    current_client: ClientCommit,
    next_surface_commit: u64,
    next_presentation_serial: u64,
    next_transaction_id: u64,
    presented_surface_commit: u64,
    presented: Option<PresentedState>,
    retry_candidate: Option<SubmittedCandidate>,
}

impl IntegratedSwapchainOracle {
    fn new(initial: [u8; 4]) -> Self {
        Self {
            slots: [
                OutputSlot {
                    physical_pixels: [0; 4],
                    last_presented_serial: None,
                    state: OutputSlotState::Available,
                },
                OutputSlot {
                    physical_pixels: [0; 4],
                    last_presented_serial: None,
                    state: OutputSlotState::Available,
                },
                OutputSlot {
                    physical_pixels: [0; 4],
                    last_presented_serial: None,
                    state: OutputSlotState::Available,
                },
            ],
            presented_history: Vec::new(),
            history_capacity: 2,
            client_commits: Vec::new(),
            current_client: ClientCommit {
                sequence: 0,
                buffer_id: 0,
                image: initial,
                damage: Damage::FULL,
            },
            next_surface_commit: 0,
            next_presentation_serial: 0,
            next_transaction_id: 0,
            presented_surface_commit: 0,
            presented: None,
            retry_candidate: None,
        }
    }

    fn commit(&mut self, buffer_id: u8, image: [u8; 4], damage: Damage) -> ClientCommit {
        self.next_surface_commit += 1;
        self.current_client = ClientCommit {
            sequence: self.next_surface_commit,
            buffer_id,
            image,
            damage,
        };
        self.client_commits.push(self.current_client);
        self.current_client
    }

    fn pending_damage_through(&self, sequence: u64) -> Damage {
        self.client_commits
            .iter()
            .filter(|commit| {
                commit.sequence > self.presented_surface_commit && commit.sequence <= sequence
            })
            .fold(Damage::EMPTY, |damage, commit| damage.union(commit.damage))
    }

    fn settle_no_visual_change(&mut self) {
        assert!(
            self.pending_damage_through(self.current_client.sequence)
                .is_empty()
        );
        self.presented_surface_commit = self
            .presented_surface_commit
            .max(self.current_client.sequence);
    }

    fn full_reference_image_through(&self, sequence: u64) -> [u8; 4] {
        self.client_commits
            .iter()
            .filter(|commit| commit.sequence <= sequence)
            .max_by_key(|commit| commit.sequence)
            .map(|commit| commit.image)
            .expect("the reference contains the rendered client commit")
    }

    fn submit(&mut self, candidate: RenderedCandidate) -> SubmittedCandidate {
        self.next_transaction_id += 1;
        SubmittedCandidate {
            transaction_id: self.next_transaction_id,
            candidate,
        }
    }

    fn render(&mut self, slot: usize) -> RenderedCandidate {
        let slot_state = self.slots[slot];
        assert_eq!(slot_state.state, OutputSlotState::Available);
        let buffer_age = slot_state
            .last_presented_serial
            .map(|serial| {
                usize::try_from(self.next_presentation_serial.saturating_sub(serial) + 1)
                    .expect("test presentation serial fits usize")
            })
            .unwrap_or(0);

        let mut rendered = slot_state.physical_pixels;
        if buffer_age == 0 || buffer_age > self.history_capacity + 1 {
            rendered = self.current_client.image;
        } else if let Some(last_serial) = slot_state.last_presented_serial {
            for repair in self
                .presented_history
                .iter()
                .filter(|repair| repair.serial > last_serial)
            {
                apply_damage(&mut rendered, repair.image, repair.damage);
            }
        }
        let sampled_damage = self.pending_damage_through(self.current_client.sequence);
        apply_damage(&mut rendered, self.current_client.image, sampled_damage);

        // Rendering writes the physical output slot before KMS submission.
        // Rejection must therefore leave these pixels present but unpresented.
        self.slots[slot].physical_pixels = rendered;
        RenderedCandidate {
            commit: self.current_client,
            slot,
            physical_pixels: rendered,
            sampled_damage,
            buffer_age,
        }
    }

    fn reject_and_quarantine(&mut self, submitted: SubmittedCandidate) {
        assert!(submitted.transaction_id > 0);
        self.slots[submitted.candidate.slot].state = OutputSlotState::Quarantined;
    }

    fn reject_for_retry(&mut self, submitted: SubmittedCandidate) {
        assert!(self.retry_candidate.is_none());
        self.retry_candidate = Some(submitted);
    }

    fn take_retry(&mut self) -> SubmittedCandidate {
        self.retry_candidate
            .take()
            .expect("a rejected candidate is available for retry")
    }

    fn present(&mut self, submitted: SubmittedCandidate) -> PresentedOutput {
        assert!(submitted.transaction_id > 0);
        let candidate = submitted.candidate;
        assert_eq!(
            self.slots[candidate.slot].physical_pixels,
            candidate.physical_pixels
        );
        assert_eq!(self.slots[candidate.slot].state, OutputSlotState::Available);

        self.next_presentation_serial += 1;
        let serial = self.next_presentation_serial;
        self.slots[candidate.slot].last_presented_serial = Some(serial);
        self.presented_history.push(PresentedDamage {
            serial,
            damage: candidate.sampled_damage,
            image: candidate.commit.image,
        });
        if self.presented_history.len() > self.history_capacity {
            self.presented_history.remove(0);
        }
        self.presented_surface_commit =
            self.presented_surface_commit.max(candidate.commit.sequence);
        let state = PresentedState {
            slot: candidate.slot,
            serial,
            logical_reference_image: candidate.commit.image,
            surface_commit: self.presented_surface_commit,
        };
        self.presented = Some(state);
        PresentedOutput {
            state,
            candidate,
            physical_pixels: candidate.physical_pixels,
        }
    }
}

fn apply_damage(target: &mut [u8; 4], source: [u8; 4], damage: Damage) {
    if damage.is_empty() {
        return;
    }
    target[damage.start..damage.end].copy_from_slice(&source[damage.start..damage.end]);
}

#[test]
fn rendered_physical_slot_is_distinct_from_submitted_and_presented_state() {
    let mut oracle = IntegratedSwapchainOracle::new([1, 1, 1, 1]);
    let commit = oracle.commit(0, [1, 1, 1, 1], Damage::FULL);
    let candidate = oracle.render(0);
    assert_eq!(oracle.slots[0].physical_pixels, candidate.physical_pixels);
    assert_eq!(oracle.presented, None);
    assert_eq!(oracle.presented_surface_commit, 0);

    let submitted = oracle.submit(candidate);
    assert_eq!(oracle.presented_surface_commit, 0);
    oracle.reject_and_quarantine(submitted);
    assert_eq!(oracle.slots[0].physical_pixels, [1, 1, 1, 1]);
    assert_eq!(oracle.slots[0].state, OutputSlotState::Quarantined);
    assert_eq!(oracle.presented, None);
    assert_eq!(oracle.current_client.sequence, commit.sequence);
}

#[test]
fn client_and_output_swapchains_match_full_reference_across_rotation_empty_partial_and_aging() {
    let mut oracle = IntegratedSwapchainOracle::new([1, 1, 1, 1]);
    let sequence = [
        (0, [1, 1, 1, 1], Damage::FULL, 0),
        (1, [1, 1, 1, 1], Damage::EMPTY, 0),
        (2, [2, 1, 1, 1], Damage { start: 0, end: 1 }, 1),
        (0, [2, 1, 1, 1], Damage::EMPTY, 0),
        (1, [2, 3, 1, 1], Damage { start: 1, end: 2 }, 2),
        (2, [2, 3, 1, 1], Damage::EMPTY, 1),
        (0, [2, 3, 4, 1], Damage { start: 2, end: 3 }, 0),
        (1, [2, 3, 4, 1], Damage::EMPTY, 2),
        (2, [2, 3, 4, 5], Damage { start: 3, end: 4 }, 1),
        (0, [2, 3, 4, 5], Damage::EMPTY, 0),
    ];
    let mut observed_ages = Vec::new();

    for (buffer_id, image, damage, slot) in sequence {
        let commit = oracle.commit(buffer_id, image, damage);
        let candidate = oracle.render(slot);
        observed_ages.push(candidate.buffer_age);
        let submitted = oracle.submit(candidate);
        let output = oracle.present(submitted);
        let reference = oracle.full_reference_image_through(commit.sequence);
        assert_eq!(output.physical_pixels, reference);
        assert_eq!(output.physical_pixels, image);
        assert_eq!(output.state.logical_reference_image, image);
        assert_eq!(oracle.presented_surface_commit, commit.sequence);
        assert_eq!(oracle.current_client.buffer_id, buffer_id);
    }

    assert!(
        observed_ages.contains(&0),
        "initial slots require a full paint"
    );
    assert!(
        observed_ages.contains(&1),
        "immediate reuse exercises age 1"
    );
    assert!(
        observed_ages.contains(&2),
        "two-generation reuse exercises age 2"
    );
    assert!(
        observed_ages.iter().any(|age| *age >= 3),
        "cyclic reuse exercises age 3+"
    );
}

#[test]
fn rejected_rendered_candidate_does_not_advance_history_and_retry_reuses_exact_pixels() {
    let mut oracle = IntegratedSwapchainOracle::new([1, 1, 1, 1]);
    let first = oracle.commit(0, [1, 1, 1, 1], Damage::FULL);
    let first_candidate = oracle.render(0);
    let first_submitted = oracle.submit(first_candidate);
    oracle.present(first_submitted);

    let rejected_commit = oracle.commit(1, [2, 1, 1, 1], Damage { start: 0, end: 1 });
    let rejected_candidate = oracle.render(1);
    let rejected_pixels = rejected_candidate.physical_pixels;
    let rejected = oracle.submit(rejected_candidate);
    oracle.reject_for_retry(rejected);
    assert_eq!(oracle.slots[1].physical_pixels, rejected_pixels);
    assert_eq!(oracle.presented_surface_commit, first.sequence);
    assert_eq!(oracle.presented_history.len(), 1);

    let retry = oracle.take_retry();
    let output = oracle.present(retry);
    let reference = oracle.full_reference_image_through(rejected_commit.sequence);
    assert_eq!(output.physical_pixels, reference);
    assert_eq!(output.physical_pixels, rejected_commit.image);
    assert_eq!(oracle.presented_surface_commit, rejected_commit.sequence);
    assert_eq!(oracle.presented_history.len(), 2);
}

#[test]
fn rejected_old_frame_keeps_newer_client_commit_pending_until_later_presentation() {
    let mut oracle = IntegratedSwapchainOracle::new([1, 1, 1, 1]);
    let old = oracle.commit(0, [1, 1, 1, 1], Damage::FULL);
    let old_candidate = oracle.render(0);
    let old_submitted = oracle.submit(old_candidate);
    let newer = oracle.commit(1, [2, 1, 1, 1], Damage { start: 0, end: 1 });

    oracle.reject_and_quarantine(old_submitted);
    assert_eq!(oracle.presented_surface_commit, 0);
    assert_eq!(oracle.current_client.sequence, newer.sequence);

    let later = oracle.render(1);
    let later_submitted = oracle.submit(later);
    let output = oracle.present(later_submitted);
    let reference = oracle.full_reference_image_through(newer.sequence);
    assert_eq!(output.physical_pixels, reference);
    assert_eq!(output.candidate.commit.sequence, newer.sequence);
    assert_eq!(output.physical_pixels, newer.image);
    assert_eq!(oracle.presented_surface_commit, newer.sequence);
    assert_ne!(old.sequence, output.candidate.commit.sequence);
}

#[test]
fn confirmed_old_frame_settles_only_its_frozen_client_commit() {
    let mut oracle = IntegratedSwapchainOracle::new([1, 1, 1, 1]);
    let old = oracle.commit(0, [1, 1, 1, 1], Damage::FULL);
    let old_candidate = oracle.render(0);
    let old_submitted = oracle.submit(old_candidate);
    let newer = oracle.commit(1, [2, 1, 1, 1], Damage { start: 0, end: 1 });

    let output = oracle.present(old_submitted);
    assert_eq!(output.candidate.commit.sequence, old.sequence);
    assert_eq!(oracle.presented_surface_commit, old.sequence);
    assert_eq!(oracle.current_client.sequence, newer.sequence);

    let newer_candidate = oracle.render(1);
    let newer_submitted = oracle.submit(newer_candidate);
    oracle.present(newer_submitted);
    assert_eq!(oracle.presented_surface_commit, newer.sequence);
    assert_eq!(oracle.presented.unwrap().surface_commit, newer.sequence);
}

#[test]
fn direct_scanout_buffer_identity_is_separate_from_composited_empty_damage() {
    let mut oracle = IntegratedSwapchainOracle::new([7, 7, 7, 7]);
    let first = oracle.commit(0, [7, 7, 7, 7], Damage::FULL);
    let first_candidate = oracle.render(0);
    let first_submitted = oracle.submit(first_candidate);
    let first_output = oracle.present(first_submitted);
    assert_eq!(first_output.physical_pixels, first.image);

    // Composited logical damage is Empty and the logical image is unchanged,
    // but the direct-scanout resource is a new candidate because its buffer
    // identity changed.
    let direct_b = oracle.commit(1, [7, 7, 7, 7], Damage::EMPTY);
    assert_ne!(first.buffer_id, direct_b.buffer_id);
    let direct_candidate = oracle.render(1);
    assert!(direct_candidate.sampled_damage.is_empty());
    let direct_submitted = oracle.submit(direct_candidate);
    let direct_output = oracle.present(direct_submitted);
    let reference = oracle.full_reference_image_through(direct_b.sequence);
    assert_eq!(direct_output.physical_pixels, reference);
    assert_eq!(direct_output.physical_pixels, [7, 7, 7, 7]);
    assert_eq!(direct_output.candidate.commit.buffer_id, 1);
}

#[test]
fn no_visual_change_advances_surface_accounting_without_physical_presentation() {
    let mut oracle = IntegratedSwapchainOracle::new([7, 7, 7, 7]);
    let first = oracle.commit(0, [7, 7, 7, 7], Damage::FULL);
    let first_candidate = oracle.render(0);
    let first_submitted = oracle.submit(first_candidate);
    let first_output = oracle.present(first_submitted);
    assert_eq!(first_output.state.serial, 1);
    assert_eq!(oracle.presented_surface_commit, first.sequence);

    let empty = oracle.commit(1, [7, 7, 7, 7], Damage::EMPTY);
    let serial_before = oracle.next_presentation_serial;
    let physical_before = oracle.presented.unwrap();
    oracle.settle_no_visual_change();

    assert_eq!(oracle.next_presentation_serial, serial_before);
    assert_eq!(oracle.presented, Some(physical_before));
    assert_eq!(oracle.presented_surface_commit, empty.sequence);
    assert!(oracle.pending_damage_through(empty.sequence).is_empty());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum O1FrameState {
    Pending,
    Ready,
}

struct TwoBufferShmO1Oracle {
    now_ns: u64,
    frames: Vec<O1FrameState>,
    available: [bool; 2],
    next_buffer: usize,
    commits: u64,
    materializations: u64,
    releases: [u64; 2],
    render_ahead_successes: u64,
    refreshes_with_pending_and_ready: u64,
}

impl TwoBufferShmO1Oracle {
    const OUTPUT_INTERVAL_NS: u64 = 6_060_606;

    fn new() -> Self {
        Self {
            now_ns: 0,
            frames: Vec::new(),
            available: [true; 2],
            next_buffer: 0,
            commits: 0,
            materializations: 0,
            releases: [0; 2],
            render_ahead_successes: 0,
            refreshes_with_pending_and_ready: 0,
        }
    }

    fn client_commit_one_refresh(&mut self) {
        let buffer = self.next_buffer;
        self.next_buffer = (self.next_buffer + 1) % self.available.len();
        assert!(self.available[buffer], "SHM client is backpressured");
        self.available[buffer] = false;
        self.commits += 1;

        // This is the ownership boundary under test: the exact pixels are
        // copied before the client lease is returned, independently of frame
        // presentation state.
        self.materializations += 1;
        self.available[buffer] = true;
        self.releases[buffer] += 1;
    }

    fn output_refresh(&mut self) {
        let had_frame = !self.frames.is_empty();
        self.client_commit_one_refresh();
        if self.frames.is_empty() {
            self.frames.push(O1FrameState::Pending);
        } else if self.frames.len() == 1 {
            self.frames.push(O1FrameState::Ready);
            self.render_ahead_successes += 1;
        }
        if self.frames.len() == 2
            && self.frames[0] == O1FrameState::Pending
            && self.frames[1] == O1FrameState::Ready
        {
            self.refreshes_with_pending_and_ready += 1;
        }
        if had_frame && self.frames.first() == Some(&O1FrameState::Pending) {
            self.frames.remove(0);
        }
        if had_frame && self.frames.first() == Some(&O1FrameState::Ready) {
            self.frames[0] = O1FrameState::Pending;
        }
        self.now_ns += Self::OUTPUT_INTERVAL_NS;
    }
}

#[test]
fn two_buffer_shm_client_stays_unblocked_with_o1_pending_and_ready_frames() {
    let mut oracle = TwoBufferShmO1Oracle::new();
    for _ in 0..120 {
        oracle.output_refresh();
    }

    assert!(oracle.render_ahead_successes > 0);
    assert!(oracle.refreshes_with_pending_and_ready > 0);
    assert_eq!(oracle.commits, 120);
    assert_eq!(oracle.materializations, oracle.commits);
    assert_eq!(oracle.releases, [60, 60]);
    assert_eq!(
        oracle.now_ns,
        120 * TwoBufferShmO1Oracle::OUTPUT_INTERVAL_NS
    );
}
