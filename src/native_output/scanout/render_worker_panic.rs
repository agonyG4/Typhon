#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RenderWorkerPanic {
    Caught,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WorkerLoopFatalEventEvidence {
    pub(super) reason: &'static str,
    pub(super) gpu_execution_uncertain: bool,
}

impl RenderWorkerPanic {
    pub(super) const fn fatal_event_evidence(self) -> WorkerLoopFatalEventEvidence {
        match self {
            Self::Caught => WorkerLoopFatalEventEvidence {
                reason: "worker_loop_panic",
                gpu_execution_uncertain: true,
            },
        }
    }
}

pub(super) fn catch_render_worker_loop<T>(
    work: impl FnOnce() -> T,
) -> Result<T, RenderWorkerPanic> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
        .map_err(|_| RenderWorkerPanic::Caught)
}

#[cfg(test)]
mod tests {
    use super::{RenderWorkerPanic, catch_render_worker_loop};

    #[test]
    fn worker_loop_panic_is_converted_to_fatal_marker() {
        let result = catch_render_worker_loop(|| panic!("injected worker panic"));

        assert_eq!(result, Err(RenderWorkerPanic::Caught));
        let fatal = RenderWorkerPanic::Caught.fatal_event_evidence();
        assert_eq!(fatal.reason, "worker_loop_panic");
        assert!(fatal.gpu_execution_uncertain);
    }
}
