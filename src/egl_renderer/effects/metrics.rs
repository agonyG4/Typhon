#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct EffectResourceMetrics {
    pub current_bytes: u64,
    pub peak_bytes: u64,
    pub budget_bytes: u64,
    pub cached_key_count: usize,
    pub cached_texture_count: usize,
    pub checked_out_texture_count: usize,
    pub eviction_count: usize,
}
