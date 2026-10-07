use crate::system_action::AstreaSystemAction;

pub(crate) const PROTOCOL_MAJOR: u8 = 1;
pub(crate) const PROTOCOL_MINOR: u8 = 0;
pub(crate) const PACKET_SIZE: usize = 32;

const MAGIC: [u8; 4] = *b"ASTY";
const KIND_HELLO: u8 = 1;
const KIND_WELCOME: u8 = 2;
const KIND_CAPABILITIES_CHANGED: u8 = 3;
const KIND_ACTION: u8 = 4;
const KIND_REJECT: u8 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SystemActionMessage {
    Hello {
        major: u8,
        minor: u8,
        capabilities: u64,
    },
    Welcome {
        major: u8,
        minor: u8,
        capabilities: u64,
    },
    CapabilitiesChanged {
        capabilities: u64,
    },
    Action {
        sequence: u64,
        action: AstreaSystemAction,
        occurrences: u32,
    },
    Reject {
        reason: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SystemActionProtocolError {
    InvalidSize,
    InvalidMagic,
    UnknownMessage,
    InvalidReservedField,
    InvalidVersion,
    InvalidAction,
    InvalidOccurrenceCount,
}

pub(crate) fn encode(message: SystemActionMessage) -> [u8; PACKET_SIZE] {
    let mut packet = [0; PACKET_SIZE];
    packet[..4].copy_from_slice(&MAGIC);
    match message {
        SystemActionMessage::Hello {
            major,
            minor,
            capabilities,
        } => {
            packet[4] = KIND_HELLO;
            packet[5] = major;
            packet[6] = minor;
            packet[16..24].copy_from_slice(&capabilities.to_le_bytes());
        }
        SystemActionMessage::Welcome {
            major,
            minor,
            capabilities,
        } => {
            packet[4] = KIND_WELCOME;
            packet[5] = major;
            packet[6] = minor;
            packet[16..24].copy_from_slice(&capabilities.to_le_bytes());
        }
        SystemActionMessage::CapabilitiesChanged { capabilities } => {
            packet[4] = KIND_CAPABILITIES_CHANGED;
            packet[5] = PROTOCOL_MAJOR;
            packet[6] = PROTOCOL_MINOR;
            packet[16..24].copy_from_slice(&capabilities.to_le_bytes());
        }
        SystemActionMessage::Action {
            sequence,
            action,
            occurrences,
        } => {
            packet[4] = KIND_ACTION;
            packet[5] = PROTOCOL_MAJOR;
            packet[6] = PROTOCOL_MINOR;
            packet[8..16].copy_from_slice(&sequence.to_le_bytes());
            packet[16..18].copy_from_slice(&action.wire_code().to_le_bytes());
            packet[18..22].copy_from_slice(&occurrences.to_le_bytes());
        }
        SystemActionMessage::Reject { reason } => {
            packet[4] = KIND_REJECT;
            packet[5] = PROTOCOL_MAJOR;
            packet[6] = PROTOCOL_MINOR;
            packet[16..18].copy_from_slice(&reason.to_le_bytes());
        }
    }
    packet
}

pub(crate) fn decode(packet: &[u8]) -> Result<SystemActionMessage, SystemActionProtocolError> {
    if packet.len() != PACKET_SIZE {
        return Err(SystemActionProtocolError::InvalidSize);
    }
    if packet[..4] != MAGIC {
        return Err(SystemActionProtocolError::InvalidMagic);
    }
    if packet[7] != 0 {
        return Err(SystemActionProtocolError::InvalidReservedField);
    }
    let major = packet[5];
    let minor = packet[6];
    match packet[4] {
        KIND_HELLO | KIND_WELCOME | KIND_CAPABILITIES_CHANGED => {
            if packet[4] != KIND_HELLO && (major != PROTOCOL_MAJOR || minor > PROTOCOL_MINOR) {
                return Err(SystemActionProtocolError::InvalidVersion);
            }
            if packet[8..16].iter().any(|byte| *byte != 0)
                || packet[24..].iter().any(|byte| *byte != 0)
            {
                return Err(SystemActionProtocolError::InvalidReservedField);
            }
            let capabilities = u64::from_le_bytes([
                packet[16], packet[17], packet[18], packet[19], packet[20], packet[21], packet[22],
                packet[23],
            ]);
            match packet[4] {
                KIND_HELLO => Ok(SystemActionMessage::Hello {
                    major,
                    minor,
                    capabilities,
                }),
                KIND_WELCOME => Ok(SystemActionMessage::Welcome {
                    major,
                    minor,
                    capabilities,
                }),
                _ => Ok(SystemActionMessage::CapabilitiesChanged { capabilities }),
            }
        }
        KIND_ACTION => {
            if major != PROTOCOL_MAJOR || minor > PROTOCOL_MINOR {
                return Err(SystemActionProtocolError::InvalidVersion);
            }
            if packet[22..].iter().any(|byte| *byte != 0) {
                return Err(SystemActionProtocolError::InvalidReservedField);
            }
            let action_code = u16::from_le_bytes([packet[16], packet[17]]);
            let action = AstreaSystemAction::from_wire_code(action_code)
                .ok_or(SystemActionProtocolError::InvalidAction)?;
            let occurrences = u32::from_le_bytes([packet[18], packet[19], packet[20], packet[21]]);
            if occurrences == 0 {
                return Err(SystemActionProtocolError::InvalidOccurrenceCount);
            }
            Ok(SystemActionMessage::Action {
                sequence: u64::from_le_bytes([
                    packet[8], packet[9], packet[10], packet[11], packet[12], packet[13],
                    packet[14], packet[15],
                ]),
                action,
                occurrences,
            })
        }
        KIND_REJECT => {
            if major != PROTOCOL_MAJOR || minor > PROTOCOL_MINOR {
                return Err(SystemActionProtocolError::InvalidVersion);
            }
            if packet[8..16].iter().any(|byte| *byte != 0)
                || packet[18..].iter().any(|byte| *byte != 0)
            {
                return Err(SystemActionProtocolError::InvalidReservedField);
            }
            Ok(SystemActionMessage::Reject {
                reason: u16::from_le_bytes([packet[16], packet[17]]),
            })
        }
        _ => Err(SystemActionProtocolError::UnknownMessage),
    }
}

#[cfg(test)]
mod tests {
    use crate::system_action::{AstreaSystemAction, AstreaSystemActionCapabilities};

    use super::{PACKET_SIZE, PROTOCOL_MAJOR, PROTOCOL_MINOR, SystemActionMessage, decode, encode};

    #[test]
    fn every_v1_message_round_trips() {
        let capabilities =
            AstreaSystemActionCapabilities::for_action(AstreaSystemAction::OutputVolumeUp).union(
                AstreaSystemActionCapabilities::for_action(AstreaSystemAction::MediaNext),
            );
        let messages = [
            SystemActionMessage::Hello {
                major: PROTOCOL_MAJOR,
                minor: PROTOCOL_MINOR,
                capabilities: capabilities.wire_bits(),
            },
            SystemActionMessage::Welcome {
                major: PROTOCOL_MAJOR,
                minor: PROTOCOL_MINOR,
                capabilities: capabilities.wire_bits(),
            },
            SystemActionMessage::CapabilitiesChanged {
                capabilities: capabilities.wire_bits(),
            },
            SystemActionMessage::Action {
                sequence: 0x0102_0304_0506_0708,
                action: AstreaSystemAction::OutputVolumeUp,
                occurrences: 3,
            },
            SystemActionMessage::Reject { reason: 2 },
        ];
        for message in messages {
            let packet = encode(message);
            assert_eq!(packet.len(), PACKET_SIZE);
            assert_eq!(decode(&packet), Ok(message));
        }
    }

    #[test]
    fn decoder_rejects_bad_headers_shapes_codes_counts_and_reserved_bytes() {
        let valid = encode(SystemActionMessage::Hello {
            major: PROTOCOL_MAJOR,
            minor: PROTOCOL_MINOR,
            capabilities: 1,
        });
        let oversized = [0_u8; PACKET_SIZE + 1];
        for malformed in [&valid[..PACKET_SIZE - 1], &oversized[..]] {
            assert!(decode(malformed).is_err());
        }

        let mut packet = valid;
        packet[0] ^= 1;
        assert!(decode(&packet).is_err());

        let mut packet = valid;
        packet[4] = u8::MAX;
        assert!(decode(&packet).is_err());

        let mut packet = valid;
        packet[7] = 1;
        assert!(decode(&packet).is_err());

        let mut packet = encode(SystemActionMessage::Action {
            sequence: 1,
            action: AstreaSystemAction::MediaNext,
            occurrences: 1,
        });
        packet[16..18].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(decode(&packet).is_err());

        let mut packet = encode(SystemActionMessage::Action {
            sequence: 1,
            action: AstreaSystemAction::MediaNext,
            occurrences: 1,
        });
        packet[18..22].copy_from_slice(&0_u32.to_le_bytes());
        assert!(decode(&packet).is_err());
    }

    #[test]
    fn decoder_enforces_version_rules_for_every_versioned_packet() {
        let mut action = encode(SystemActionMessage::Action {
            sequence: 1,
            action: AstreaSystemAction::MediaNext,
            occurrences: 1,
        });
        action[5] = PROTOCOL_MAJOR + 1;
        assert!(decode(&action).is_err());

        let mut reject = encode(SystemActionMessage::Reject { reason: 1 });
        reject[6] = PROTOCOL_MINOR + 1;
        assert!(decode(&reject).is_err());

        let mut welcome = encode(SystemActionMessage::Welcome {
            major: PROTOCOL_MAJOR,
            minor: PROTOCOL_MINOR,
            capabilities: 0,
        });
        welcome[6] = PROTOCOL_MINOR + 1;
        assert!(decode(&welcome).is_err());
    }
}
