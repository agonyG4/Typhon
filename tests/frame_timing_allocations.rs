//! Allocation regressions for the timing paths shared by both buffering modes.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::time::Duration;

use oblivion_one::native::adaptive_buffering::AdaptiveRenderJournal;
use oblivion_one::native::presentation_deadline::MonotonicTimestampNs;

// Exercise the binary's actual worker timing implementation without starting DRM.
#[path = "../src/native_output/kms_worker/timing.rs"]
mod worker_timing;

thread_local! {
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

struct CountingAllocator;

fn count_allocation() {
    let _ = ALLOCATIONS.try_with(|count| {
        if let Some(value) = count.get() {
            count.set(Some(value + 1));
        }
    });
}

// SAFETY: Every operation forwards the original pointer/layout to System.
// Counting uses constant-initialized thread-local storage and cannot allocate.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count_allocation();
        unsafe { System.realloc(ptr, layout, size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn allocations_during(f: impl FnOnce()) -> usize {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ALLOCATIONS.set(None);
        }
    }
    ALLOCATIONS.set(Some(0));
    let _reset = Reset;
    f();
    ALLOCATIONS.get().unwrap()
}

#[test]
fn populated_render_prediction_does_not_allocate() {
    let mut journal = AdaptiveRenderJournal::default();
    for value in 1..=241 {
        journal.record_render_sample(value, MonotonicTimestampNs::new(value));
        journal.record_wake_lateness(value);
        journal.record_atomic_submit(value);
        journal.record_worker_queue_residency(value);
        journal.record_worker_submit_wake_lateness(value);
        journal.record_worker_pre_submit(value);
        journal.record_worker_dispatch(value);
        journal.record_submission_budget(value);
        journal.record_target_slip(value);
    }
    let allocations = allocations_during(|| {
        black_box(black_box(&journal).prediction(Duration::from_millis(6)));
    });
    assert_eq!(allocations, 0);
}

#[test]
fn populated_worker_dispatch_budget_does_not_allocate() {
    let mut model = worker_timing::KmsWorkerDispatchModel::default();
    for value in 1..=241 {
        model.record(value, value * 2, value * 3);
    }
    let allocations = allocations_during(|| {
        black_box(black_box(&model).budget());
    });
    assert_eq!(allocations, 0);
}
