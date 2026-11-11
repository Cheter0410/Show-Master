//! AppleMIDI session-management packets (pure codec).
//!
//! Every packet starts with the signature `0xFFFF` followed by a two-letter
//! ASCII command. All fields are big-endian.
//!
//! | Command | Layout after signature + command                                   |
//! |---------|--------------------------------------------------------------------|
//! | `IN` `OK` `NO` `BY` | version `u32` (=2), initiator token `u32`, SSRC `u32`, optional NUL-terminated UTF-8 name |
//! | `CK`    | SSRC `u32`, count `u8`, 3 padding bytes, 3 × timestamp `u64` (100 µs units) |
//! | `RS`    | SSRC `u32`, sequence number `u16`, 2 padding bytes                 |
//! | `RL`    | SSRC `u32`, bitrate limit `u32`                                    |

use crate::CodecError;

/// Signature that starts every AppleMIDI session packet.
pub const SIGNATURE: u16 = 0xFFFF;
/// The only protocol version in use.
pub const PROTOCOL_VERSION: u32 = 2;

const CMD_IN: u16 = 0x494E;
const CMD_OK: u16 = 0x4F4B;
const CMD_NO: u16 = 0x4E4F;
const CMD_BY: u16 = 0x4259;
const CMD_CK: u16 = 0x434B;
const CMD_RS: u16 = 0x5253;
const CMD_RL: u16 = 0x524C;

/// Body shared by `IN`, `OK`, `NO` and `BY`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchange {
    /// Protocol version, [`PROTOCOL_VERSION`].
    pub version: u32,
    /// Random token chosen by the initiator, echoed by the responder.
    pub token: u32,
    /// SSRC of the sender of this packet.
    pub ssrc: u32,
    /// Session name (usually present in `IN`/`OK`, absent in `NO`/`BY`).
    pub name: Option<String>,
}

/// Body of a `CK` clock synchronisation packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sync {
    /// SSRC of the sender of this packet.
    pub ssrc: u32,
    /// 0, 1 or 2: number of the step in the three-way exchange.
    pub count: u8,
    /// Timestamps 1..3 in 100 µs units; entries not yet known are 0.
    pub timestamps: [u64; 3],
}

/// A decoded AppleMIDI session packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionPacket {
    /// `IN` – invitation.
    Invitation(Exchange),
    /// `OK` – invitation accepted.
    Accepted(Exchange),
    /// `NO` – invitation rejected.
    Rejected(Exchange),
    /// `BY` – end of session.
    Bye(Exchange),
    /// `CK` – clock synchronisation.
    Sync(Sync),
    /// `RS` – receiver feedback: highest RTP sequence number received.
    ReceiverFeedback {
        /// SSRC of the sender of this packet.
        ssrc: u32,
        /// Highest sequence number received so far.
        sequence: u16,
    },
    /// `RL` – bitrate receive limit (informational).
    BitrateLimit {
        /// SSRC of the sender of this packet.
        ssrc: u32,
        /// Limit in bits per second.
        limit: u32,
    },
}

/// `true` if `buf` starts with the AppleMIDI signature (as opposed to an RTP packet).
pub fn is_session_packet(buf: &[u8]) -> bool {
    matches!(buf, [0xFF, 0xFF, ..])
}

fn rd<const N: usize>(buf: &[u8], off: usize) -> Result<[u8; N], CodecError> {
    buf.get(off..off.saturating_add(N))
        .and_then(|s| <[u8; N]>::try_from(s).ok())
        .ok_or(CodecError::Truncated)
}

fn rd_u16(buf: &[u8], off: usize) -> Result<u16, CodecError> {
    rd(buf, off).map(u16::from_be_bytes)
}

fn rd_u32(buf: &[u8], off: usize) -> Result<u32, CodecError> {
    rd(buf, off).map(u32::from_be_bytes)
}

fn rd_u64(buf: &[u8], off: usize) -> Result<u64, CodecError> {
    rd(buf, off).map(u64::from_be_bytes)
}

/// Decodes one AppleMIDI session packet.
pub fn parse(buf: &[u8]) -> Result<SessionPacket, CodecError> {
    if rd_u16(buf, 0)? != SIGNATURE {
        return Err(CodecError::BadSignature);
    }
    let command = rd_u16(buf, 2)?;
    match command {
        CMD_IN | CMD_OK | CMD_NO | CMD_BY => {
            let ex = Exchange {
                version: rd_u32(buf, 4)?,
                token: rd_u32(buf, 8)?,
                ssrc: rd_u32(buf, 12)?,
                name: parse_name(buf.get(16..).unwrap_or_default()),
            };
            Ok(match command {
                CMD_IN => SessionPacket::Invitation(ex),
                CMD_OK => SessionPacket::Accepted(ex),
                CMD_NO => SessionPacket::Rejected(ex),
                _ => SessionPacket::Bye(ex),
            })
        }
        CMD_CK => Ok(SessionPacket::Sync(Sync {
            ssrc: rd_u32(buf, 4)?,
            count: rd::<1>(buf, 8)?[0],
            timestamps: [rd_u64(buf, 12)?, rd_u64(buf, 20)?, rd_u64(buf, 28)?],
        })),
        CMD_RS => Ok(SessionPacket::ReceiverFeedback {
            ssrc: rd_u32(buf, 4)?,
            sequence: rd_u16(buf, 8)?,
        }),
        CMD_RL => Ok(SessionPacket::BitrateLimit {
            ssrc: rd_u32(buf, 4)?,
            limit: rd_u32(buf, 8)?,
        }),
        other => Err(CodecError::UnknownCommand(other)),
    }
}

fn parse_name(rest: &[u8]) -> Option<String> {
    if rest.is_empty() {
        return None;
    }
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    rest.get(..end)
        .map(|s| String::from_utf8_lossy(s).into_owned())
}

/// Encodes one AppleMIDI session packet.
pub fn encode(packet: &SessionPacket) -> Vec<u8> {
    let mut out = Vec::with_capacity(40);
    out.extend_from_slice(&SIGNATURE.to_be_bytes());
    match packet {
        SessionPacket::Invitation(ex) => encode_exchange(&mut out, CMD_IN, ex),
        SessionPacket::Accepted(ex) => encode_exchange(&mut out, CMD_OK, ex),
        SessionPacket::Rejected(ex) => encode_exchange(&mut out, CMD_NO, ex),
        SessionPacket::Bye(ex) => encode_exchange(&mut out, CMD_BY, ex),
        SessionPacket::Sync(sync) => {
            out.extend_from_slice(&CMD_CK.to_be_bytes());
            out.extend_from_slice(&sync.ssrc.to_be_bytes());
            out.push(sync.count);
            out.extend_from_slice(&[0, 0, 0]);
            for ts in sync.timestamps {
                out.extend_from_slice(&ts.to_be_bytes());
            }
        }
        SessionPacket::ReceiverFeedback { ssrc, sequence } => {
            out.extend_from_slice(&CMD_RS.to_be_bytes());
            out.extend_from_slice(&ssrc.to_be_bytes());
            out.extend_from_slice(&sequence.to_be_bytes());
            out.extend_from_slice(&[0, 0]);
        }
        SessionPacket::BitrateLimit { ssrc, limit } => {
            out.extend_from_slice(&CMD_RL.to_be_bytes());
            out.extend_from_slice(&ssrc.to_be_bytes());
            out.extend_from_slice(&limit.to_be_bytes());
        }
    }
    out
}

fn encode_exchange(out: &mut Vec<u8>, command: u16, ex: &Exchange) {
    out.extend_from_slice(&command.to_be_bytes());
    out.extend_from_slice(&ex.version.to_be_bytes());
    out.extend_from_slice(&ex.token.to_be_bytes());
    out.extend_from_slice(&ex.ssrc.to_be_bytes());
    if let Some(name) = &ex.name {
        out.extend(name.bytes().filter(|&b| b != 0));
        out.push(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Layout of an invitation as sent by macOS (Audio MIDI Setup, session
    /// "MacBook"): signature, "IN", version 2, token, SSRC, name, NUL.
    const MACOS_IN: [u8; 24] = [
        0xFF, 0xFF, 0x49, 0x4E, // signature, "IN"
        0x00, 0x00, 0x00, 0x02, // protocol version 2
        0x19, 0x49, 0x5C, 0xD2, // initiator token
        0x3A, 0x11, 0xD6, 0x0E, // SSRC
        b'M', b'a', b'c', b'B', b'o', b'o', b'k', 0x00,
    ];

    #[test]
    fn parse_macos_invitation() {
        let pkt = parse(&MACOS_IN).unwrap();
        assert_eq!(
            pkt,
            SessionPacket::Invitation(Exchange {
                version: 2,
                token: 0x1949_5CD2,
                ssrc: 0x3A11_D60E,
                name: Some("MacBook".into()),
            })
        );
        assert_eq!(encode(&pkt), MACOS_IN);
    }

    #[test]
    fn exchange_without_name() {
        let bytes = [
            0xFF, 0xFF, 0x42, 0x59, 0, 0, 0, 2, 0xDE, 0xAD, 0xBE, 0xEF, 1, 2, 3, 4,
        ];
        let pkt = parse(&bytes).unwrap();
        let SessionPacket::Bye(ex) = &pkt else {
            panic!("not BY: {pkt:?}");
        };
        assert_eq!(ex.token, 0xDEAD_BEEF);
        assert_eq!(ex.ssrc, 0x0102_0304);
        assert_eq!(ex.name, None);
        assert_eq!(encode(&pkt), bytes);
    }

    #[test]
    fn ok_and_no_roundtrip() {
        for make in [SessionPacket::Accepted, SessionPacket::Rejected] {
            let pkt = make(Exchange {
                version: 2,
                token: 7,
                ssrc: 9,
                name: Some("Bühne 1".into()),
            });
            assert_eq!(parse(&encode(&pkt)).unwrap(), pkt);
        }
        assert_eq!(
            encode(&SessionPacket::Accepted(Exchange {
                version: 2,
                token: 1,
                ssrc: 2,
                name: Some("A".into())
            })),
            [0xFF, 0xFF, b'O', b'K', 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 2, b'A', 0]
        );
    }

    #[test]
    fn name_without_terminator_is_accepted() {
        let mut bytes = MACOS_IN.to_vec();
        bytes.pop();
        let SessionPacket::Invitation(ex) = parse(&bytes).unwrap() else {
            panic!("not IN");
        };
        assert_eq!(ex.name.as_deref(), Some("MacBook"));
    }

    #[test]
    fn sync_byte_exact() {
        let mut bytes = vec![
            0xFF, 0xFF, 0x43, 0x4B, 0x11, 0x22, 0x33, 0x44, 0x01, 0, 0, 0,
        ];
        bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0x12, 0x34]);
        bytes.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0]);
        bytes.extend_from_slice(&[0; 8]);
        assert_eq!(bytes.len(), 36);
        let pkt = parse(&bytes).unwrap();
        assert_eq!(
            pkt,
            SessionPacket::Sync(Sync {
                ssrc: 0x1122_3344,
                count: 1,
                timestamps: [0x1234, 0x1_0000_0000, 0],
            })
        );
        assert_eq!(encode(&pkt), bytes);
    }

    #[test]
    fn receiver_feedback_byte_exact() {
        let bytes = [
            0xFF, 0xFF, 0x52, 0x53, 0xAA, 0xBB, 0xCC, 0xDD, 0x12, 0x34, 0, 0,
        ];
        let pkt = parse(&bytes).unwrap();
        assert_eq!(
            pkt,
            SessionPacket::ReceiverFeedback {
                ssrc: 0xAABB_CCDD,
                sequence: 0x1234
            }
        );
        assert_eq!(encode(&pkt), bytes);
    }

    #[test]
    fn bitrate_limit_parses() {
        let bytes = [0xFF, 0xFF, 0x52, 0x4C, 0, 0, 0, 5, 0, 0, 0x13, 0x88];
        let pkt = parse(&bytes).unwrap();
        assert_eq!(
            pkt,
            SessionPacket::BitrateLimit {
                ssrc: 5,
                limit: 5000
            }
        );
        assert_eq!(encode(&pkt), bytes);
    }

    #[test]
    fn malformed_packets_are_errors_not_panics() {
        assert_eq!(parse(&[]), Err(CodecError::Truncated));
        assert_eq!(parse(&[0xFF]), Err(CodecError::Truncated));
        assert_eq!(parse(&[0x80, 0x61, 0, 0]), Err(CodecError::BadSignature));
        assert_eq!(
            parse(&[0xFF, 0xFF, b'X', b'Y', 0, 0]),
            Err(CodecError::UnknownCommand(0x5859))
        );
        for len in 0..MACOS_IN.len() {
            let r = parse(&MACOS_IN[..len]);
            assert_eq!(r.is_err(), len < 16, "len {len}");
        }
        assert_eq!(
            parse(&[0xFF, 0xFF, 0x43, 0x4B, 0, 0, 0, 1, 0]),
            Err(CodecError::Truncated)
        );
        assert!(is_session_packet(&MACOS_IN));
        assert!(!is_session_packet(&[0x80, 0x61]));
        assert!(!is_session_packet(&[0xFF]));
    }
}
