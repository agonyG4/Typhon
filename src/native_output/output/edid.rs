//! Minimal EDID reader for the variable-refresh range of a connected sink.
//!
//! Only what VRR needs is parsed: the AMD FreeSync vendor-specific data block
//! (an explicit VRR range) and, as a weaker fallback, the base block's Display
//! Range Limits descriptor. Malformed input yields `None`, never a panic.

const EDID_BLOCK_LEN: usize = 128;
const EDID_HEADER: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];
const CTA_EXTENSION_TAG: u8 = 0x02;
const CTA_VENDOR_SPECIFIC_TAG: u8 = 3;
const AMD_OUI: u32 = 0x00_001a;
const RANGE_LIMITS_TAG: u8 = 0xfd;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdidVrrRangeSource {
    /// AMD FreeSync vendor-specific data block in a CTA-861 extension.
    FreeSyncVsdb,
    /// Base-block Display Range Limits; a monitor range, not a VRR promise.
    DisplayRangeLimits,
}

impl EdidVrrRangeSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::FreeSyncVsdb => "edid_freesync_vsdb",
            Self::DisplayRangeLimits => "edid_range_limits",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EdidVrrRange {
    pub(crate) min_hz: u32,
    pub(crate) max_hz: u32,
    pub(crate) source: EdidVrrRangeSource,
}

impl EdidVrrRange {
    fn new(min_hz: u32, max_hz: u32, source: EdidVrrRangeSource) -> Option<Self> {
        (min_hz > 0 && min_hz < max_hz).then_some(Self {
            min_hz,
            max_hz,
            source,
        })
    }
}

fn block_checksum_valid(block: &[u8]) -> bool {
    block.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) == 0
}

pub(crate) fn parse_edid_vrr_range(edid: &[u8]) -> Option<EdidVrrRange> {
    let base = edid.get(..EDID_BLOCK_LEN)?;
    if base[..8] != EDID_HEADER || !block_checksum_valid(base) {
        return None;
    }
    edid.chunks_exact(EDID_BLOCK_LEN)
        .skip(1)
        .filter(|block| block[0] == CTA_EXTENSION_TAG && block_checksum_valid(block))
        .find_map(cta_freesync_range)
        .or_else(|| base_range_limits(base))
}

fn cta_freesync_range(block: &[u8]) -> Option<EdidVrrRange> {
    // Byte 2 is the offset of the first detailed timing; data blocks sit
    // between byte 4 and that offset.
    let data_end = usize::from(block[2]).min(EDID_BLOCK_LEN - 1);
    let mut offset = 4;
    while offset < data_end {
        let header = block[offset];
        let tag = header >> 5;
        let len = usize::from(header & 0x1f);
        let payload = block.get(offset + 1..offset + 1 + len)?;
        if tag == CTA_VENDOR_SPECIFIC_TAG && len >= 7 {
            let oui = u32::from(payload[0])
                | (u32::from(payload[1]) << 8)
                | (u32::from(payload[2]) << 16);
            if oui == AMD_OUI {
                // payload[3] = version, [4] = feature caps, [5]/[6] = range.
                return EdidVrrRange::new(
                    u32::from(payload[5]),
                    u32::from(payload[6]),
                    EdidVrrRangeSource::FreeSyncVsdb,
                );
            }
        }
        offset += 1 + len;
    }
    None
}

fn base_range_limits(base: &[u8]) -> Option<EdidVrrRange> {
    base[54..126].chunks_exact(18).find_map(|descriptor| {
        if descriptor[..3] != [0, 0, 0] || descriptor[3] != RANGE_LIMITS_TAG {
            return None;
        }
        // EDID 1.4 rate offsets: bit 1 adds 255 to the maximum, bits 1:0 = 11
        // add 255 to the minimum as well.
        let offsets = descriptor[4] & 0b11;
        let max_offset = if offsets & 0b10 != 0 { 255 } else { 0 };
        let min_offset = if offsets == 0b11 { 255 } else { 0 };
        EdidVrrRange::new(
            u32::from(descriptor[5]) + min_offset,
            u32::from(descriptor[6]) + max_offset,
            EdidVrrRangeSource::DisplayRangeLimits,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRS24B: &[u8] = include_bytes!("testdata/crs24b-freesync.edid");

    fn fix_checksum(block: &mut [u8]) {
        let sum = block[..EDID_BLOCK_LEN - 1]
            .iter()
            .fold(0u8, |sum, byte| sum.wrapping_add(*byte));
        block[EDID_BLOCK_LEN - 1] = 0u8.wrapping_sub(sum);
    }

    #[test]
    fn real_freesync_monitor_reports_its_vsdb_range() {
        assert_eq!(
            parse_edid_vrr_range(CRS24B),
            Some(EdidVrrRange {
                min_hz: 48,
                max_hz: 165,
                source: EdidVrrRangeSource::FreeSyncVsdb,
            })
        );
    }

    #[test]
    fn base_block_alone_falls_back_to_range_limits() {
        assert_eq!(
            parse_edid_vrr_range(&CRS24B[..EDID_BLOCK_LEN]),
            Some(EdidVrrRange {
                min_hz: 48,
                max_hz: 165,
                source: EdidVrrRangeSource::DisplayRangeLimits,
            })
        );
    }

    #[test]
    fn corrupt_extension_is_ignored_but_corrupt_base_is_rejected() {
        let mut edid = CRS24B.to_vec();
        edid[EDID_BLOCK_LEN + 10] ^= 0xff;
        assert_eq!(
            parse_edid_vrr_range(&edid).map(|range| range.source),
            Some(EdidVrrRangeSource::DisplayRangeLimits)
        );

        let mut edid = CRS24B.to_vec();
        edid[20] ^= 0xff;
        assert_eq!(parse_edid_vrr_range(&edid), None);
    }

    #[test]
    fn malformed_input_never_panics() {
        assert_eq!(parse_edid_vrr_range(&[]), None);
        assert_eq!(parse_edid_vrr_range(&CRS24B[..100]), None);
        assert_eq!(parse_edid_vrr_range(&[0xff; 256]), None);

        // A CTA block whose data-block lengths run past the block end.
        let mut edid = CRS24B.to_vec();
        let cta = &mut edid[EDID_BLOCK_LEN..];
        cta[2] = 127;
        for byte in &mut cta[4..127] {
            *byte = 0x7f;
        }
        fix_checksum(cta);
        assert_eq!(
            parse_edid_vrr_range(&edid).map(|range| range.source),
            Some(EdidVrrRangeSource::DisplayRangeLimits)
        );
    }

    #[test]
    fn range_limit_offsets_and_inverted_ranges() {
        let mut base = CRS24B[..EDID_BLOCK_LEN].to_vec();
        let descriptor = (0..4)
            .map(|index| 54 + index * 18)
            .find(|offset| base[offset + 3] == RANGE_LIMITS_TAG)
            .unwrap();
        base[descriptor + 4] = 0b10;
        base[descriptor + 5] = 48;
        base[descriptor + 6] = 5;
        fix_checksum(&mut base);
        assert_eq!(
            parse_edid_vrr_range(&base).map(|range| (range.min_hz, range.max_hz)),
            Some((48, 260))
        );

        base[descriptor + 4] = 0;
        base[descriptor + 5] = 165;
        base[descriptor + 6] = 48;
        fix_checksum(&mut base);
        assert_eq!(parse_edid_vrr_range(&base), None);
    }
}
