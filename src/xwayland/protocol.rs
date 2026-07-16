use std::num::NonZeroU64;

pub(crate) const XWAYLAND_SHELL_V1_VERSION: u32 = 1;

/// The protocol names the low 32 bits first and the high 32 bits second.
pub fn serial_from_parts(serial_lo: u32, serial_hi: u32) -> Option<NonZeroU64> {
    NonZeroU64::new((u64::from(serial_hi) << 32) | u64::from(serial_lo))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_parts_use_protocol_low_then_high_ordering() {
        assert_eq!(
            serial_from_parts(0x0123_4567, 0x89ab_cdef).map(NonZeroU64::get),
            Some(0x89ab_cdef_0123_4567)
        );
        assert_eq!(serial_from_parts(0, 0), None);
    }
}
