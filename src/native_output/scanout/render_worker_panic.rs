#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RenderWorkerPanic {
    Caught,
}

impl RenderWorkerPanic {
    pub(super) const fn reason(self) -> &'static str {
        match self {
            Self::Caught => "worker_loop_panic",
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
        assert_eq!(RenderWorkerPanic::Caught.reason(), "worker_loop_panic");
    }
}
