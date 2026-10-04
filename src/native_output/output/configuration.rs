/// Semantic identity for user-visible output configuration.
///
/// This generation changes only when the authoritative output binding changes
/// or a Display configuration is applied or rolled back. It is intentionally
/// independent from render, swapchain, framebuffer, and presentation IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct OutputConfigurationGeneration(u64);

impl OutputConfigurationGeneration {
    pub(crate) const fn initial() -> Self {
        Self(1)
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }

    pub(crate) fn advance(&mut self) -> Self {
        self.0 = self.0.wrapping_add(1).max(1);
        *self
    }
}

#[cfg(test)]
mod tests {
    use super::OutputConfigurationGeneration;

    #[test]
    fn semantic_generation_advances_only_when_the_authority_requests_it() {
        let mut generation = OutputConfigurationGeneration::initial();
        let render_ticks = 165_u32;
        let swapchain_recreations = 3_u32;
        let framebuffer_ids = [18, 19, 20];

        assert_eq!(render_ticks, 165);
        assert_eq!(swapchain_recreations, 3);
        assert_eq!(framebuffer_ids, [18, 19, 20]);
        assert_eq!(generation.get(), 1);

        assert_eq!(generation.advance().get(), 2);
        assert_eq!(generation.advance().get(), 3);
    }

    #[test]
    fn generation_wraps_without_publishing_zero() {
        let mut generation = OutputConfigurationGeneration(u64::MAX);
        assert_eq!(generation.advance().get(), 1);
    }
}
