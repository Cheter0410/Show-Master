//! RTP-MIDI data packets (RFC 6295), pure codec.
//!
//! ```text
//! | RTP header (12 bytes) | MIDI command section | recovery journal (if J=1) |
//!
//! MIDI command section:   |B|J|Z|P|LEN.. | MIDI list (LEN bytes) |
//!   B=0: LEN is 4 bits (one header byte)   B=1: LEN is 12 bits (two header bytes)
//!   J: journal follows   Z: first command has a delta time   P: phantom status
//! MIDI list:              [delta0] cmd0  delta1 cmd1  delta2 cmd2 ...
//! ```
//!
//! SysEx segmentation: a complete SysEx is `F0 … F7`. A SysEx spread over
//! several commands is `F0 … F0`, then zero or more `F7 … F0`, then `F7 … F7`.
//! `F7 … F4` / `F0 … F4` cancels a SysEx in progress.

use crate::CodecError;
use std::ops::Range;

/// RTP payload type used by AppleMIDI.
pub const PAYLOAD_TYPE: u8 = 97;
/// Largest MIDI list the 12-bit length field can describe.
pub const MAX_LIST_LEN: usize = 0x0FFF;
/// Upper bound for a reassembled SysEx; larger messages are discarded.
pub const MAX_SYSEX_LEN: usize = 1 << 20;

/// The fixed RTP header fields relevant to RTP-MIDI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtpHeader {
    /// Marker bit. Apple sends 0; RFC 6295 says 1 for non-empty lists. Ignored on receive.
    pub marker: bool,
    /// Payload type, normally [`PAYLOAD_TYPE`].
    pub payload_type: u8,
    /// Sequence number.
    pub sequence: u16,
    /// Timestamp in session clock units (100 µs with AppleMIDI).
    pub timestamp: u32,
    /// Synchronisation source of the sender.
    pub ssrc: u32,
}

/// What a command in a MIDI list represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    /// A complete MIDI message; `data` starts with the status byte
    /// (running status already expanded). Complete SysEx is `F0 … F7`.
    Message,
    /// First segment of a SysEx; `data` is the payload without the framing `F0`s.
    SysexStart,
    /// Middle segment of a SysEx; `data` is the payload without framing.
    SysexContinue,
    /// Last segment of a SysEx; `data` is the payload without framing.
    SysexEnd,
    /// The SysEx in progress is cancelled.
    SysexCancel,
}

/// One entry of a MIDI list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// Delta time relative to the previous command (or the RTP timestamp).
    pub delta: u32,
    /// Kind of command.
    pub kind: CommandKind,
    /// Bytes, see [`CommandKind`].
    pub data: Vec<u8>,
}

/// A decoded RTP-MIDI packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataPacket<'a> {
    /// RTP header.
    pub header: RtpHeader,
    /// P flag: the status of the first command was not present in the source stream.
    pub phantom: bool,
    /// Commands of the MIDI list in order.
    pub commands: Vec<Command>,
    /// Raw recovery journal if J=1 (not interpreted).
    pub journal: Option<&'a [u8]>,
}

fn rd<const N: usize>(buf: &[u8], off: usize) -> Result<[u8; N], CodecError> {
    buf.get(off..off.saturating_add(N))
        .and_then(|s| <[u8; N]>::try_from(s).ok())
        .ok_or(CodecError::Truncated)
}

fn byte(buf: &[u8], off: usize) -> Result<u8, CodecError> {
    buf.get(off).copied().ok_or(CodecError::Truncated)
}

/// Decodes an RTP-MIDI packet. CSRC lists, header extensions and padding are skipped.
pub fn parse_data_packet(buf: &[u8]) -> Result<DataPacket<'_>, CodecError> {
    let b0 = byte(buf, 0)?;
    if b0 >> 6 != 2 {
        return Err(CodecError::BadRtpVersion(b0 >> 6));
    }
    let b1 = byte(buf, 1)?;
    let header = RtpHeader {
        marker: b1 & 0x80 != 0,
        payload_type: b1 & 0x7F,
        sequence: u16::from_be_bytes(rd(buf, 2)?),
        timestamp: u32::from_be_bytes(rd(buf, 4)?),
        ssrc: u32::from_be_bytes(rd(buf, 8)?),
    };
    let mut pos = 12 + 4 * usize::from(b0 & 0x0F);
    if b0 & 0x10 != 0 {
        let words = u16::from_be_bytes(rd(buf, pos + 2)?);
        pos += 4 + 4 * usize::from(words);
    }
    let mut end = buf.len();
    if b0 & 0x20 != 0 {
        let pad = usize::from(byte(buf, end.saturating_sub(1))?);
        if pad == 0 || pad > end.saturating_sub(pos) {
            return Err(CodecError::Truncated);
        }
        end -= pad;
    }
    let payload = buf.get(pos..end).ok_or(CodecError::Truncated)?;

    let flags = byte(payload, 0)?;
    let (len, start) = if flags & 0x80 != 0 {
        (
            (usize::from(flags & 0x0F) << 8) | usize::from(byte(payload, 1)?),
            2,
        )
    } else {
        (usize::from(flags & 0x0F), 1)
    };
    let list = payload
        .get(start..start + len)
        .ok_or(CodecError::Truncated)?;
    let journal = if flags & 0x40 != 0 {
        payload.get(start + len..)
    } else {
        None
    };
    Ok(DataPacket {
        header,
        phantom: flags & 0x10 != 0,
        commands: parse_midi_list(list, flags & 0x20 != 0)?,
        journal,
    })
}

fn read_delta(list: &[u8], pos: &mut usize) -> Result<u32, CodecError> {
    let mut value = 0u32;
    for _ in 0..4 {
        let b = byte(list, *pos)?;
        *pos += 1;
        value = (value << 7) | u32::from(b & 0x7F);
        if b & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(CodecError::DeltaTooLong)
}

/// Decodes a MIDI list. `first_has_delta` is the Z flag of the command section.
pub fn parse_midi_list(list: &[u8], first_has_delta: bool) -> Result<Vec<Command>, CodecError> {
    let mut commands = Vec::new();
    let mut pos = 0usize;
    let mut running: Option<u8> = None;
    let mut first = true;
    while pos < list.len() {
        let delta = if first && !first_has_delta {
            0
        } else {
            read_delta(list, &mut pos)?
        };
        first = false;
        let Some(&b) = list.get(pos) else {
            break; // trailing delta time without a command
        };
        let status = if b < 0x80 {
            running.ok_or(CodecError::NoRunningStatus)?
        } else {
            pos += 1;
            b
        };
        match status {
            0x80..=0xEF => {
                running = Some(status);
                let n = if matches!(status & 0xF0, 0xC0 | 0xD0) {
                    1
                } else {
                    2
                };
                let data = read_message(list, &mut pos, status, n)?;
                commands.push(Command {
                    delta,
                    kind: CommandKind::Message,
                    data,
                });
            }
            0xF0 | 0xF7 => {
                running = None;
                let mut data = Vec::new();
                let terminator = loop {
                    let d = byte(list, pos)?;
                    pos += 1;
                    match d {
                        0x00..=0x7F => data.push(d),
                        0xF0 | 0xF4 | 0xF7 => break d,
                        // Real-time bytes should not be embedded, but are harmless.
                        0xF8..=0xFF => commands.push(Command {
                            delta,
                            kind: CommandKind::Message,
                            data: vec![d],
                        }),
                        other => return Err(CodecError::UnexpectedStatus(other)),
                    }
                };
                let kind = match (status, terminator) {
                    (_, 0xF4) => {
                        data.clear();
                        CommandKind::SysexCancel
                    }
                    (0xF0, 0xF7) => {
                        data.insert(0, 0xF0);
                        data.push(0xF7);
                        CommandKind::Message
                    }
                    (0xF0, _) => CommandKind::SysexStart,
                    (_, 0xF7) => CommandKind::SysexEnd,
                    _ => CommandKind::SysexContinue,
                };
                commands.push(Command { delta, kind, data });
            }
            0xF1..=0xF6 => {
                running = None;
                let n = match status {
                    0xF1 | 0xF3 => 1,
                    0xF2 => 2,
                    _ => 0,
                };
                let data = read_message(list, &mut pos, status, n)?;
                // F4/F5 are undefined system common messages: skip them.
                if !matches!(status, 0xF4 | 0xF5) {
                    commands.push(Command {
                        delta,
                        kind: CommandKind::Message,
                        data,
                    });
                }
            }
            _ => commands.push(Command {
                delta,
                kind: CommandKind::Message,
                data: vec![status],
            }),
        }
    }
    Ok(commands)
}

fn read_message(
    list: &[u8],
    pos: &mut usize,
    status: u8,
    data_len: usize,
) -> Result<Vec<u8>, CodecError> {
    let mut msg = Vec::with_capacity(1 + data_len);
    msg.push(status);
    for _ in 0..data_len {
        let d = byte(list, *pos)?;
        if d >= 0x80 {
            return Err(CodecError::UnexpectedStatus(d));
        }
        *pos += 1;
        msg.push(d);
    }
    Ok(msg)
}

/// Reassembles segmented SysEx messages across commands and packets.
#[derive(Debug, Default)]
pub struct SysexAssembler {
    buf: Option<Vec<u8>>,
}

impl SysexAssembler {
    /// Creates an empty assembler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Discards a partially received SysEx (call after packet loss).
    pub fn reset(&mut self) {
        self.buf = None;
    }

    /// Feeds one command; returns a complete MIDI message when one is available.
    pub fn push(&mut self, command: Command) -> Option<Vec<u8>> {
        match command.kind {
            CommandKind::Message => {
                // Real-time messages may interleave with SysEx segments;
                // anything else terminates a SysEx in progress.
                if command.data.first().is_some_and(|&s| s < 0xF8) {
                    self.buf = None;
                }
                Some(command.data)
            }
            CommandKind::SysexStart => {
                let mut buf = Vec::with_capacity(command.data.len() + 1);
                buf.push(0xF0);
                buf.extend_from_slice(&command.data);
                self.buf = Some(buf);
                None
            }
            CommandKind::SysexContinue => {
                let too_long = match &mut self.buf {
                    Some(buf) => {
                        buf.extend_from_slice(&command.data);
                        buf.len() > MAX_SYSEX_LEN
                    }
                    None => false,
                };
                if too_long {
                    self.buf = None;
                }
                None
            }
            CommandKind::SysexEnd => {
                let mut buf = self.buf.take()?;
                buf.extend_from_slice(&command.data);
                buf.push(0xF7);
                Some(buf)
            }
            CommandKind::SysexCancel => {
                self.buf = None;
                None
            }
        }
    }
}

/// Appends a variable-length delta time (1–4 octets, 28 bits).
pub fn encode_delta(delta: u32, out: &mut Vec<u8>) {
    let delta = delta & 0x0FFF_FFFF;
    for shift in [21u32, 14, 7] {
        if delta >> shift != 0 {
            out.push(0x80 | ((delta >> shift) & 0x7F) as u8);
        }
    }
    out.push((delta & 0x7F) as u8);
}

/// Encodes a MIDI list from `(delta, command bytes)` pairs.
///
/// Returns the list and the Z flag (true if the first command carries a delta
/// time, which is only the case when its delta is non-zero). The command bytes
/// are copied verbatim, so SysEx segments can be passed pre-framed.
pub fn encode_midi_list(commands: &[(u32, &[u8])]) -> (Vec<u8>, bool) {
    let mut out = Vec::new();
    let mut z = false;
    for (i, (delta, bytes)) in commands.iter().enumerate() {
        if i == 0 {
            if *delta != 0 {
                z = true;
                encode_delta(*delta, &mut out);
            }
        } else {
            encode_delta(*delta, &mut out);
        }
        out.extend_from_slice(bytes);
    }
    (out, z)
}

/// Encodes a complete RTP-MIDI packet.
///
/// `z` is the Z flag belonging to `list` (see [`encode_midi_list`]). If
/// `journal` is `Some`, J=1 and the journal bytes are appended.
pub fn encode_data_packet(
    header: &RtpHeader,
    z: bool,
    list: &[u8],
    journal: Option<&[u8]>,
) -> Result<Vec<u8>, CodecError> {
    if list.len() > MAX_LIST_LEN {
        return Err(CodecError::ListTooLong(list.len()));
    }
    let journal_len = journal.map_or(0, <[u8]>::len);
    let mut out = Vec::with_capacity(14 + list.len() + journal_len);
    out.push(0x80);
    out.push((u8::from(header.marker) << 7) | (header.payload_type & 0x7F));
    out.extend_from_slice(&header.sequence.to_be_bytes());
    out.extend_from_slice(&header.timestamp.to_be_bytes());
    out.extend_from_slice(&header.ssrc.to_be_bytes());
    let flags = (u8::from(journal.is_some()) << 6) | (u8::from(z) << 5);
    if list.len() <= 0x0F {
        out.push(flags | list.len() as u8);
    } else {
        out.push(0x80 | flags | (list.len() >> 8) as u8);
        out.push((list.len() & 0xFF) as u8);
    }
    out.extend_from_slice(list);
    if let Some(journal) = journal {
        out.extend_from_slice(journal);
    }
    Ok(out)
}

/// Splits a byte stream into complete MIDI messages.
///
/// Every message must start with its status byte (no running status); SysEx
/// must be complete (`F0 … F7`).
pub fn split_midi(bytes: &[u8]) -> Result<Vec<&[u8]>, &'static str> {
    let mut messages = Vec::new();
    let mut pos = 0usize;
    while let Some(&status) = bytes.get(pos) {
        let len = match status {
            0x00..=0x7F => return Err("data byte without status byte"),
            0xC0..=0xDF | 0xF1 | 0xF3 => 2,
            0x80..=0xEF | 0xF2 => 3,
            0xF0 => {
                let rest = bytes.get(pos + 1..).unwrap_or_default();
                match rest.iter().position(|&b| b >= 0x80) {
                    Some(i) if rest.get(i) == Some(&0xF7) => i + 2,
                    Some(_) => return Err("status byte inside SysEx"),
                    None => return Err("unterminated SysEx"),
                }
            }
            0xF4 | 0xF5 | 0xF7 => return Err("undefined or stray system common status"),
            _ => 1,
        };
        let msg = bytes.get(pos..pos + len).ok_or("truncated MIDI message")?;
        if status != 0xF0 && msg.iter().skip(1).any(|&b| b >= 0x80) {
            return Err("status byte inside MIDI message");
        }
        messages.push(msg);
        pos += len;
    }
    Ok(messages)
}

/// One outgoing MIDI list produced by [`packetize`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Encoded MIDI list (all delta times 0, Z=0).
    pub list: Vec<u8>,
    /// Indices of the messages completely contained in this chunk
    /// (empty for SysEx segments).
    pub messages: Range<usize>,
}

/// Distributes complete MIDI messages over MIDI lists of at most `max_list`
/// bytes. A SysEx that does not fit into an empty list is segmented, one
/// segment per list; everything else is never split.
pub fn packetize(messages: &[&[u8]], max_list: usize) -> Vec<Chunk> {
    let max_list = max_list.clamp(8, MAX_LIST_LEN);
    let mut chunks = Vec::new();
    let mut list: Vec<u8> = Vec::new();
    let mut first = 0usize;
    for (i, msg) in messages.iter().enumerate() {
        let needed = msg.len() + usize::from(!list.is_empty());
        if list.len() + needed > max_list && !list.is_empty() {
            chunks.push(Chunk {
                list: std::mem::take(&mut list),
                messages: first..i,
            });
            first = i;
        }
        if msg.len() <= max_list {
            if !list.is_empty() {
                list.push(0x00); // delta time 0
            }
            list.extend_from_slice(msg);
            continue;
        }
        // Oversized: only a SysEx can be this long. Strip F0/F7 and segment.
        let body = msg.get(1..msg.len() - 1).unwrap_or_default();
        let mut parts = body.chunks(max_list - 2).peekable();
        let mut lead = 0xF0u8;
        while let Some(part) = parts.next() {
            let mut seg = Vec::with_capacity(part.len() + 2);
            seg.push(lead);
            seg.extend_from_slice(part);
            seg.push(if parts.peek().is_some() { 0xF0 } else { 0xF7 });
            chunks.push(Chunk {
                list: seg,
                messages: i..i,
            });
            lead = 0xF7;
        }
        first = i + 1;
    }
    if !list.is_empty() {
        chunks.push(Chunk {
            list,
            messages: first..messages.len(),
        });
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(delta: u32, data: &[u8]) -> Command {
        Command {
            delta,
            kind: CommandKind::Message,
            data: data.to_vec(),
        }
    }

    /// Note-on as sent by macOS: V=2, PT=97, M=0, no journal, short length.
    const ONE_NOTE: [u8; 16] = [
        0x80, 0x61, 0x2B, 0x67, // V=2, PT 97, seq 0x2B67
        0x00, 0x12, 0xD6, 0x87, // timestamp
        0x3A, 0x11, 0xD6, 0x0E, // SSRC
        0x03, 0x90, 0x3C, 0x64, // LEN=3, note on C4 vel 100
    ];

    #[test]
    fn single_message_packet_byte_exact() {
        let pkt = parse_data_packet(&ONE_NOTE).unwrap();
        assert_eq!(
            pkt.header,
            RtpHeader {
                marker: false,
                payload_type: 97,
                sequence: 0x2B67,
                timestamp: 0x0012_D687,
                ssrc: 0x3A11_D60E,
            }
        );
        assert!(!pkt.phantom);
        assert_eq!(pkt.journal, None);
        assert_eq!(pkt.commands, vec![msg(0, &[0x90, 0x3C, 0x64])]);

        let (list, z) = encode_midi_list(&[(0, &[0x90, 0x3C, 0x64])]);
        assert!(!z);
        assert_eq!(
            encode_data_packet(&pkt.header, z, &list, None).unwrap(),
            ONE_NOTE
        );
    }

    #[test]
    fn multi_message_packet_with_delta_times_and_running_status() {
        // Z=1: first command has delta 0x10; second uses running status with a
        // two-octet delta (0x81 0x00 = 128); third is a CC with delta 0; then a
        // real-time clock byte and a program change.
        let list = [
            0x10, 0x90, 0x3C, 0x64, // delta 16, note on
            0x81, 0x00, 0x40, 0x00, // delta 128, running status note on 0x40 vel 0
            0x00, 0xB0, 0x07, 0x7F, // delta 0, CC 7
            0x05, 0xF8, // delta 5, timing clock
            0x00, 0x0A, 0x01, // delta 0, running status (CC survives real-time)
            0x00, 0xC1, 0x05, // program change ch 2
            0x00, 0xF1, 0x23, // MTC quarter frame
        ];
        let mut bytes = vec![0x80, 0xE1, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3];
        bytes.push(0x80 | 0x20); // B=1, Z=1, LEN high nibble 0
        bytes.push(list.len() as u8);
        bytes.extend_from_slice(&list);
        let pkt = parse_data_packet(&bytes).unwrap();
        assert!(pkt.header.marker);
        assert_eq!(
            pkt.commands,
            vec![
                msg(16, &[0x90, 0x3C, 0x64]),
                msg(128, &[0x90, 0x40, 0x00]),
                msg(0, &[0xB0, 0x07, 0x7F]),
                msg(5, &[0xF8]),
                msg(0, &[0xB0, 0x0A, 0x01]),
                msg(0, &[0xC1, 0x05]),
                msg(0, &[0xF1, 0x23]),
            ]
        );
    }

    #[test]
    fn encode_multi_message_with_deltas_byte_exact() {
        let (list, z) = encode_midi_list(&[
            (0, &[0x90, 0x3C, 0x64]),
            (200, &[0x80, 0x3C, 0x00]),
            (0, &[0xB0, 0x40, 0x7F]),
        ]);
        assert!(!z);
        assert_eq!(
            list,
            [0x90, 0x3C, 0x64, 0x81, 0x48, 0x80, 0x3C, 0x00, 0x00, 0xB0, 0x40, 0x7F]
        );
        let header = RtpHeader {
            marker: false,
            payload_type: PAYLOAD_TYPE,
            sequence: 1,
            timestamp: 2,
            ssrc: 3,
        };
        let bytes = encode_data_packet(&header, z, &list, None).unwrap();
        assert_eq!(bytes[12], 0x0C);
        let pkt = parse_data_packet(&bytes).unwrap();
        assert_eq!(pkt.commands[1], msg(200, &[0x80, 0x3C, 0x00]));
        assert_eq!(pkt.commands.len(), 3);

        let (list, z) = encode_midi_list(&[(3, &[0xFA])]);
        assert!(z);
        assert_eq!(list, [0x03, 0xFA]);
        let bytes = encode_data_packet(&header, z, &list, None).unwrap();
        assert_eq!(bytes[12], 0x22);
        assert_eq!(
            parse_data_packet(&bytes).unwrap().commands,
            vec![msg(3, &[0xFA])]
        );
    }

    #[test]
    fn delta_time_encoding() {
        for (value, expect) in [
            (0u32, vec![0x00]),
            (0x7F, vec![0x7F]),
            (0x80, vec![0x81, 0x00]),
            (0x3FFF, vec![0xFF, 0x7F]),
            (0x4000, vec![0x81, 0x80, 0x00]),
            (0x0FFF_FFFF, vec![0xFF, 0xFF, 0xFF, 0x7F]),
        ] {
            let mut out = Vec::new();
            encode_delta(value, &mut out);
            assert_eq!(out, expect, "value {value:#x}");
            let mut pos = 0;
            assert_eq!(read_delta(&out, &mut pos).unwrap(), value);
            assert_eq!(pos, out.len());
        }
        let mut pos = 0;
        assert_eq!(
            read_delta(&[0x80, 0x80, 0x80, 0x80, 0x00], &mut pos),
            Err(CodecError::DeltaTooLong)
        );
    }

    #[test]
    fn long_form_length() {
        // 20 note-ons = 60 bytes + 19 delta bytes = 79 > 15 → B=1.
        let note: &[u8] = &[0x90, 0x3C, 0x64];
        let cmds: Vec<(u32, &[u8])> = (0..20).map(|_| (0, note)).collect();
        let (list, z) = encode_midi_list(&cmds);
        assert_eq!(list.len(), 79);
        let header = RtpHeader {
            marker: false,
            payload_type: PAYLOAD_TYPE,
            sequence: 0xFFFF,
            timestamp: 0xFFFF_FFFF,
            ssrc: 0x0102_0304,
        };
        let bytes = encode_data_packet(&header, z, &list, None).unwrap();
        assert_eq!(&bytes[..2], &[0x80, 0x61]);
        assert_eq!(&bytes[12..14], &[0x80, 79]);
        assert_eq!(bytes.len(), 14 + 79);
        let pkt = parse_data_packet(&bytes).unwrap();
        assert_eq!(pkt.header, header);
        assert_eq!(pkt.commands.len(), 20);
        assert!(pkt.commands.iter().all(|c| c.data == note));

        // 12-bit length with a non-zero high nibble.
        let big = vec![0xF8u8; 1];
        let cmds: Vec<(u32, &[u8])> = (0..400).map(|_| (0, big.as_slice())).collect();
        let (list, _) = encode_midi_list(&cmds);
        assert_eq!(list.len(), 799);
        let bytes = encode_data_packet(&header, false, &list, None).unwrap();
        assert_eq!(&bytes[12..14], &[0x83, 0x1F]);
        assert_eq!(parse_data_packet(&bytes).unwrap().commands.len(), 400);

        assert_eq!(
            encode_data_packet(&header, false, &vec![0xF8; 4096], None),
            Err(CodecError::ListTooLong(4096))
        );
    }

    #[test]
    fn journal_is_skipped() {
        let mut bytes = ONE_NOTE.to_vec();
        bytes[12] = 0x43; // J=1, LEN=3
        let journal = [
            0x20, 0x2B, 0x66, 0x00, 0x00, 0x07, 0x08, 0x81, 0xF0, 0x3C, 0xE4,
        ];
        bytes.extend_from_slice(&journal);
        let pkt = parse_data_packet(&bytes).unwrap();
        assert_eq!(pkt.commands, vec![msg(0, &[0x90, 0x3C, 0x64])]);
        assert_eq!(pkt.journal, Some(&journal[..]));
        let again = encode_data_packet(&pkt.header, false, &[0x90, 0x3C, 0x64], pkt.journal);
        assert_eq!(again.unwrap(), bytes);
    }

    #[test]
    fn empty_list_and_rtp_options() {
        // Empty MIDI list (keep-alive style packet).
        let bytes = [0x80, 0x61, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0x00];
        assert!(parse_data_packet(&bytes).unwrap().commands.is_empty());

        // One CSRC, a one-word header extension and 2 bytes of padding.
        let mut bytes = vec![
            0x80 | 0x20 | 0x10 | 0x01,
            0x61,
            0,
            1,
            0,
            0,
            0,
            2,
            0,
            0,
            0,
            3,
        ];
        bytes.extend_from_slice(&[9, 9, 9, 9]); // CSRC
        bytes.extend_from_slice(&[0xBE, 0xDE, 0, 1, 1, 2, 3, 4]); // extension
        bytes.extend_from_slice(&[0x02, 0xC0, 0x07]); // command section
        bytes.extend_from_slice(&[0x00, 0x02]); // padding
        assert_eq!(
            parse_data_packet(&bytes).unwrap().commands,
            vec![msg(0, &[0xC0, 0x07])]
        );
    }

    #[test]
    fn malformed_packets_are_errors_not_panics() {
        assert_eq!(parse_data_packet(&[]), Err(CodecError::Truncated));
        assert_eq!(
            parse_data_packet(&[0x40, 0x61, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            Err(CodecError::BadRtpVersion(1))
        );
        for len in 0..ONE_NOTE.len() {
            assert!(parse_data_packet(&ONE_NOTE[..len]).is_err(), "len {len}");
        }
        assert_eq!(
            parse_midi_list(&[0x3C, 0x64], false),
            Err(CodecError::NoRunningStatus)
        );
        assert_eq!(
            parse_midi_list(&[0x90, 0x3C, 0x90], false),
            Err(CodecError::UnexpectedStatus(0x90))
        );
        assert_eq!(
            parse_midi_list(&[0xF0, 0x01, 0x02], false),
            Err(CodecError::Truncated)
        );
        // System common cancels running status.
        assert_eq!(
            parse_midi_list(&[0x90, 1, 2, 0x00, 0xF6, 0x00, 3, 4], false),
            Err(CodecError::NoRunningStatus)
        );
        // Every prefix and every single-byte corruption must not panic.
        let list = [
            0x90, 1, 2, 0, 0xF0, 1, 2, 0xF7, 0, 0xF2, 1, 2, 0x81, 0, 0xE0, 1, 2,
        ];
        for len in 0..list.len() {
            let _ = parse_midi_list(&list[..len], false);
            let _ = parse_midi_list(&list[..len], true);
        }
        for i in 0..list.len() {
            for v in [0x00, 0x7F, 0x80, 0xF0, 0xF4, 0xF7, 0xFF] {
                let mut l = list;
                l[i] = v;
                let _ = parse_midi_list(&l, false);
            }
        }
    }

    #[test]
    fn unsegmented_sysex() {
        let cmds = parse_midi_list(&[0xF0, 0x7F, 0x7F, 0x01, 0xF7, 0x00, 0xFE], false).unwrap();
        assert_eq!(
            cmds,
            vec![msg(0, &[0xF0, 0x7F, 0x7F, 0x01, 0xF7]), msg(0, &[0xFE])]
        );
    }

    #[test]
    fn segmented_sysex_parse_rfc_framing() {
        let first = parse_midi_list(&[0xF0, 1, 2, 0xF0], false).unwrap();
        let middle = parse_midi_list(&[0xF7, 3, 4, 0xF0], false).unwrap();
        let last = parse_midi_list(&[0xF7, 5, 0xF7], false).unwrap();
        assert_eq!(first[0].kind, CommandKind::SysexStart);
        assert_eq!(middle[0].kind, CommandKind::SysexContinue);
        assert_eq!(last[0].kind, CommandKind::SysexEnd);
        let mut asm = SysexAssembler::new();
        assert_eq!(asm.push(first[0].clone()), None);
        // A real-time message between segments does not disturb reassembly.
        assert_eq!(asm.push(msg(0, &[0xF8])), Some(vec![0xF8]));
        assert_eq!(asm.push(middle[0].clone()), None);
        assert_eq!(
            asm.push(last[0].clone()),
            Some(vec![0xF0, 1, 2, 3, 4, 5, 0xF7])
        );

        // Cancel, and segments without a start, yield nothing.
        let cancel = parse_midi_list(&[0xF7, 9, 0xF4], false).unwrap();
        assert_eq!(cancel[0].kind, CommandKind::SysexCancel);
        asm.push(first[0].clone());
        assert_eq!(asm.push(cancel[0].clone()), None);
        assert_eq!(asm.push(last[0].clone()), None);
        assert_eq!(asm.push(middle[0].clone()), None);
        // A channel message aborts a SysEx in progress.
        asm.push(first[0].clone());
        assert_eq!(asm.push(msg(0, &[0x90, 1, 2])), Some(vec![0x90, 1, 2]));
        assert_eq!(asm.push(last[0].clone()), None);
    }

    #[test]
    fn split_midi_messages() {
        let bytes = [
            0x90, 60, 100, 0xB0, 7, 127, 0xC0, 5, 0xF1, 0x23, 0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x21,
            0x02, 0x03, 0x04, 0xF7, 0xF8, 0xF2, 1, 2, 0xD3, 9, 0xE0, 0, 64,
        ];
        let msgs = split_midi(&bytes).unwrap();
        assert_eq!(msgs.len(), 9);
        assert_eq!(msgs[3], &[0xF1, 0x23]);
        assert_eq!(msgs[4].len(), 10);
        assert_eq!(msgs.concat(), bytes);
        assert!(split_midi(&[]).unwrap().is_empty());
        assert!(split_midi(&[60, 100]).is_err());
        assert!(split_midi(&[0x90, 60]).is_err());
        assert!(split_midi(&[0x90, 60, 0x90]).is_err());
        assert!(split_midi(&[0xF0, 1, 2]).is_err());
        assert!(split_midi(&[0xF0, 1, 0xF8, 2, 0xF7]).is_err());
        assert!(split_midi(&[0xF7]).is_err());
        assert!(split_midi(&[0xF4]).is_err());
    }

    fn roundtrip(messages: &[&[u8]], max_list: usize) -> (Vec<Chunk>, Vec<Vec<u8>>) {
        let chunks = packetize(messages, max_list);
        let mut asm = SysexAssembler::new();
        let mut out = Vec::new();
        let header = RtpHeader {
            marker: false,
            payload_type: PAYLOAD_TYPE,
            sequence: 0,
            timestamp: 0,
            ssrc: 1,
        };
        for chunk in &chunks {
            assert!(chunk.list.len() <= max_list);
            let bytes = encode_data_packet(&header, false, &chunk.list, None).unwrap();
            for cmd in parse_data_packet(&bytes).unwrap().commands {
                out.extend(asm.push(cmd));
            }
        }
        (chunks, out)
    }

    #[test]
    fn packetize_small_messages_into_one_list() {
        let (chunks, out) = roundtrip(&[&[0x90, 1, 2], &[0xF1, 0x05], &[0xF0, 1, 0xF7]], 64);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].messages, 0..3);
        assert_eq!(
            chunks[0].list,
            [0x90, 1, 2, 0x00, 0xF1, 0x05, 0x00, 0xF0, 1, 0xF7]
        );
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn packetize_splits_between_messages() {
        let note: &[u8] = &[0x90, 1, 2];
        let msgs: Vec<&[u8]> = vec![note; 5];
        let (chunks, out) = roundtrip(&msgs, 8);
        // 3 + (1+3) = 7 bytes per list → 2 messages per chunk.
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].messages, 0..2);
        assert_eq!(chunks[1].messages, 2..4);
        assert_eq!(chunks[2].messages, 4..5);
        assert_eq!(out, vec![note.to_vec(); 5]);
    }

    #[test]
    fn sysex_segmentation_roundtrip() {
        let mut sysex = vec![0xF0];
        sysex.extend((0..100u8).map(|i| i & 0x7F));
        sysex.push(0xF7);
        let before: &[u8] = &[0x90, 60, 100];
        let after: &[u8] = &[0xB0, 1, 2];

        // Fits: sent unsegmented together with its neighbours.
        let (chunks, out) = roundtrip(&[before, &sysex, after], 200);
        assert_eq!(chunks.len(), 1);
        assert_eq!(out, vec![before.to_vec(), sysex.clone(), after.to_vec()]);

        // Fits an empty list but not the remainder: own list, unsegmented.
        let (chunks, out) = roundtrip(&[before, &sysex, after], 104);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[1].list, sysex);
        assert_eq!(chunks[1].messages, 1..2);
        assert_eq!(out, vec![before.to_vec(), sysex.clone(), after.to_vec()]);

        // Does not fit: segmented with RFC 6295 framing.
        let (chunks, out) = roundtrip(&[before, &sysex, after], 42);
        assert_eq!(out, vec![before.to_vec(), sysex.clone(), after.to_vec()]);
        assert_eq!(chunks.len(), 5); // note, 3 segments (40+40+20), cc
        assert_eq!(chunks[0].messages, 0..1);
        assert_eq!(chunks[4].messages, 2..3);
        let seg = |i: usize| (&chunks[i].list, chunks[i].messages.clone());
        let (s1, r1) = seg(1);
        assert_eq!((s1[0], s1[41], s1.len()), (0xF0, 0xF0, 42));
        assert!(r1.is_empty());
        let (s2, _) = seg(2);
        assert_eq!((s2[0], s2[41], s2.len()), (0xF7, 0xF0, 42));
        let (s3, _) = seg(3);
        assert_eq!((s3[0], s3[21], s3.len()), (0xF7, 0xF7, 22));

        // Body length an exact multiple of the segment size.
        let mut exact = vec![0xF0];
        exact.extend(std::iter::repeat_n(0x55, 80));
        exact.push(0xF7);
        let (chunks, out) = roundtrip(&[&exact], 42);
        assert_eq!(chunks.len(), 2);
        assert_eq!(out, vec![exact]);
    }
}
