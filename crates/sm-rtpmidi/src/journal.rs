//! Sender-side recovery journal (RFC 6295, section 5 and appendix A).
//!
//! The journal appended to packet `N` describes the *checkpoint history*: every
//! packet from the checkpoint packet up to `N - 1`. The checkpoint advances when
//! the receiver acknowledges packets with an AppleMIDI `RS` message.
//!
//! Implemented structures (everything else is never emitted):
//!
//! ```text
//! Journal header (3 octets):   |S|Y|A|H|TOTCHAN| checkpoint packet seqnum (16) |
//! Channel header (3 octets):   |S| CHAN  |H|   LENGTH (10)   |P|C|M|W|N|E|T|A|
//! Chapter P (3 octets):        |S| PROGRAM |B| BANK-MSB |X| BANK-LSB |
//! Chapter C:                   |S|   LEN   | then LEN+1 logs: |S| NUMBER |A| VALUE |
//! Chapter W (2 octets):        |S| FIRST |R| SECOND |   (last pitch wheel, LSB / MSB)
//! Chapter N:                   |B|   LEN   | LOW | HIGH | LEN logs: |S| NOTENUM |Y| VELOCITY |
//!                              followed by HIGH-LOW+1 OFFBITS octets (if LOW <= HIGH)
//! Chapter T (1 octet):         |S| PRESSURE |              (last channel aftertouch)
//! Chapter A:                   |S|   LEN   | then LEN+1 logs: |S| NOTENUM |X| PRESSURE |
//! ```
//!
//! System journal (only chapter X, SysEx, list tool), after the header:
//! ```text
//! System header (2 octets):    |S|D|V|Q|F|X|   LENGTH (10)   |   (only X set)
//! Chapter X logs:              |S|T|C|F|D|L| STA | DATA... (F0/F7 omitted, last octet MSB=1)
//! ```
//! Logs are in sending order; the first log's S bit is the chapter's S bit
//! (RFC 6295 B.5). At most ~1000 octets of the most recent commands are kept.
//!
//! S bits are 0 for structures that describe a command of packet `N - 1` (so a
//! receiver that lost exactly one packet can skip everything with S=1) and 1
//! otherwise. `Y` (system journal) and both `H` bits are always 0. In chapter N
//! the `Y` bit of a note log (play the NoteOn on recovery) is only set for
//! NoteOns of packet `N - 1`: late-firing an old trigger is worse than missing
//! it in a show-control context.

use std::collections::BTreeMap;

const FLAG_P: u8 = 0x80;
const FLAG_C: u8 = 0x40;
const FLAG_W: u8 = 0x10;
const FLAG_N: u8 = 0x08;
const FLAG_T: u8 = 0x02;
const FLAG_A: u8 = 0x01;

#[derive(Debug, Clone, Copy)]
struct Program {
    seq: u64,
    program: u8,
    bank: Option<(u8, u8)>,
}

#[derive(Debug, Clone, Copy)]
struct Logged {
    seq: u64,
    value: u8,
}

/// (controller or note number, log entry), ascending by number.
type Logs = Vec<(u8, Logged)>;

#[derive(Debug, Default, Clone)]
struct Channel {
    program: Option<Program>,
    bank_msb: Option<u8>,
    bank_lsb: Option<u8>,
    controllers: BTreeMap<u8, Logged>,
    /// value = velocity; 0 means the most recent event was a NoteOff.
    notes: BTreeMap<u8, Logged>,
    /// Last pitch wheel: (seq, LSB, MSB).
    pitch: Option<(u64, u8, u8)>,
    /// Last channel aftertouch.
    pressure: Option<Logged>,
    /// Last poly aftertouch per note.
    poly: BTreeMap<u8, Logged>,
}

/// History of journalled commands, keyed by the (extended, non-wrapping)
/// sequence number of the packet that carried them.
#[derive(Debug, Default, Clone)]
pub struct JournalHistory {
    channels: [Channel; 16],
    /// SysEx commands: (seq, data octets without F0 / F7).
    sysex: Vec<(u64, Vec<u8>)>,
}

impl JournalHistory {
    /// Creates an empty history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one complete MIDI message sent in packet `seq`.
    /// Messages not covered by chapters P, C and N are ignored.
    pub fn record(&mut self, seq: u64, message: &[u8]) {
        if message.first() == Some(&0xF0) {
            let data: Vec<u8> = message[1..]
                .iter()
                .copied()
                .take_while(|b| *b != 0xF7)
                .map(|b| b & 0x7F)
                .collect();
            self.sysex.push((seq, data));
            if self.sysex.len() > 64 {
                self.sysex.remove(0);
            }
            return;
        }
        let (Some(&status), Some(&d1)) = (message.first(), message.get(1)) else {
            return;
        };
        let Some(ch) = self.channels.get_mut(usize::from(status & 0x0F)) else {
            return;
        };
        let d1 = d1 & 0x7F;
        let d2 = message.get(2).copied().unwrap_or(0) & 0x7F;
        match status & 0xF0 {
            0x80 => {
                ch.notes.insert(d1, Logged { seq, value: 0 });
            }
            0x90 => {
                ch.notes.insert(d1, Logged { seq, value: d2 });
            }
            0xB0 => {
                match d1 {
                    0 => ch.bank_msb = Some(d2),
                    32 => ch.bank_lsb = Some(d2),
                    _ => {}
                }
                ch.controllers.insert(d1, Logged { seq, value: d2 });
            }
            0xE0 => ch.pitch = Some((seq, d1, d2)),
            0xD0 => ch.pressure = Some(Logged { seq, value: d1 }),
            0xA0 => {
                ch.poly.insert(d1, Logged { seq, value: d2 });
            }
            0xC0 => {
                let bank = (ch.bank_msb.is_some() || ch.bank_lsb.is_some())
                    .then(|| (ch.bank_msb.unwrap_or(0), ch.bank_lsb.unwrap_or(0)));
                ch.program = Some(Program {
                    seq,
                    program: d1,
                    bank,
                });
            }
            _ => {}
        }
    }

    /// Forgets everything sent in packets before `seq` (acknowledged by all receivers).
    pub fn trim(&mut self, seq: u64) {
        self.sysex.retain(|(s, _)| *s >= seq);
        for ch in &mut self.channels {
            if ch.program.is_some_and(|p| p.seq < seq) {
                ch.program = None;
            }
            ch.controllers.retain(|_, l| l.seq >= seq);
            ch.notes.retain(|_, l| l.seq >= seq);
            if ch.pitch.is_some_and(|p| p.0 < seq) {
                ch.pitch = None;
            }
            if ch.pressure.is_some_and(|p| p.seq < seq) {
                ch.pressure = None;
            }
            ch.poly.retain(|_, l| l.seq >= seq);
        }
    }

    /// Appends the recovery journal for packet `current`, covering packets
    /// `checkpoint ..= current - 1`, to `out`. Always emits at least the
    /// three-octet journal header.
    pub fn encode(&self, checkpoint: u64, current: u64, out: &mut Vec<u8>) {
        let prev = current.wrapping_sub(1);
        let mut body = Vec::new();
        let mut count = 0u8;
        let mut s_all = true;
        for (number, ch) in (0u8..).zip(&self.channels) {
            if let Some(s) = ch.encode(number, checkpoint, current, &mut body) {
                count += 1;
                s_all &= s;
            }
        }
        let system = self.encode_system(checkpoint, current);
        if let Some((_, s)) = &system {
            s_all &= *s;
        }
        let a = count > 0;
        let y = system.is_some();
        out.push(
            (u8::from(s_all) << 7)
                | (u8::from(y) << 6)
                | (u8::from(a) << 5)
                | (count.saturating_sub(1) & 0x0F),
        );
        // Truncation to 16 bits is the wire format of the sequence number.
        out.extend_from_slice(&(checkpoint.min(prev) as u16).to_be_bytes());
        if let Some((sys, _)) = system {
            out.extend_from_slice(&sys);
        }
        out.extend_from_slice(&body);
    }

    /// The system journal with chapter X, and its S bit; `None` without SysEx
    /// in the checkpoint history.
    fn encode_system(&self, checkpoint: u64, current: u64) -> Option<(Vec<u8>, bool)> {
        let prev = current.wrapping_sub(1);
        let covered: Vec<&(u64, Vec<u8>)> = self
            .sysex
            .iter()
            .filter(|(s, _)| *s >= checkpoint && *s < current)
            .collect();
        // Newest first until the 10-bit LENGTH is full, then N-1 logs first.
        let mut picked: Vec<&(u64, Vec<u8>)> = Vec::new();
        let mut size = 2;
        for log in covered.iter().rev() {
            let n = 1 + log.1.len();
            if size + n > 1023 {
                break;
            }
            size += n;
            picked.push(log);
        }
        if picked.is_empty() {
            return None;
        }
        picked.reverse();
        let s_all = picked.iter().all(|(s, _)| *s != prev);
        let mut out = vec![0u8, 0u8];
        for (i, (seq, data)) in picked.iter().enumerate() {
            let d = !data.is_empty();
            let s = if i == 0 { s_all } else { *seq != prev };
            // T=0 C=0 F=0, D, L=1 (list tool), STA=3 (finished).
            out.push((u8::from(s) << 7) | (u8::from(d) << 3) | 0x04 | 0x03);
            if let Some((last, rest)) = data.split_last() {
                out.extend(rest.iter().map(|b| b & 0x7F));
                out.push(0x80 | (last & 0x7F));
            }
        }
        let length = out.len();
        out[0] = (u8::from(s_all) << 7) | 0x04 | ((length >> 8) & 0x03) as u8;
        out[1] = (length & 0xFF) as u8;
        Some((out, s_all))
    }
}

impl Channel {
    /// Appends this channel's journal; returns its S bit, or `None` if the
    /// channel has nothing in the checkpoint history.
    fn encode(&self, number: u8, checkpoint: u64, current: u64, out: &mut Vec<u8>) -> Option<bool> {
        let prev = current.wrapping_sub(1);
        let covered = |seq: u64| seq >= checkpoint && seq < current;
        let s_bit = |seq: u64| u8::from(seq != prev) << 7;
        let mut flags = 0u8;
        let mut s_all = true;
        let mut body = Vec::new();

        if let Some(p) = self.program.filter(|p| covered(p.seq)) {
            flags |= FLAG_P;
            s_all &= p.seq != prev;
            body.push(s_bit(p.seq) | p.program);
            match p.bank {
                Some((msb, lsb)) => body.extend_from_slice(&[0x80 | msb, lsb]),
                None => body.extend_from_slice(&[0, 0]),
            }
        }

        let logs: Logs = self
            .controllers
            .iter()
            .filter(|(_, l)| covered(l.seq))
            .map(|(n, l)| (*n, *l))
            .collect();
        if !logs.is_empty() {
            flags |= FLAG_C;
            let chapter_s = logs.iter().all(|(_, l)| l.seq != prev);
            s_all &= chapter_s;
            body.push((u8::from(chapter_s) << 7) | ((logs.len() - 1) & 0x7F) as u8);
            for (number, log) in &logs {
                body.push(s_bit(log.seq) | number);
                body.push(log.value); // A=0: plain value
            }
        }

        if let Some((seq, first, second)) = self.pitch.filter(|p| covered(p.0)) {
            flags |= FLAG_W;
            s_all &= seq != prev;
            body.push(s_bit(seq) | (first & 0x7F));
            body.push(second & 0x7F); // R = 0
        }

        let (ons, offs): (Logs, Logs) = self
            .notes
            .iter()
            .filter(|(_, l)| covered(l.seq))
            .map(|(n, l)| (*n, *l))
            .partition(|(_, l)| l.value > 0);
        if !ons.is_empty() || !offs.is_empty() {
            flags |= FLAG_N;
            let off_in_prev = offs.iter().any(|(_, l)| l.seq == prev);
            s_all &= !off_in_prev && ons.iter().all(|(_, l)| l.seq != prev);
            let (low, high) = match (offs.first(), offs.last()) {
                (Some((lo, _)), Some((hi, _))) => (lo / 8, hi / 8),
                // No OFFBITS: LOW > HIGH. (15, 0) with LEN=127 means 128 logs,
                // so exactly 127 logs must use the alternative (15, 1).
                _ if ons.len() == 127 => (15, 1),
                _ => (15, 0),
            };
            body.push((u8::from(!off_in_prev) << 7) | ons.len().min(127) as u8);
            body.push((low << 4) | high);
            for (note, log) in &ons {
                body.push(s_bit(log.seq) | note);
                body.push((u8::from(log.seq == prev) << 7) | log.value);
            }
            if !offs.is_empty() {
                let mut bits = vec![0u8; usize::from(high - low) + 1];
                for (note, _) in &offs {
                    if let Some(octet) = bits.get_mut(usize::from(note / 8 - low)) {
                        *octet |= 0x80 >> (note % 8);
                    }
                }
                body.extend_from_slice(&bits);
            }
        }

        if let Some(p) = self.pressure.filter(|p| covered(p.seq)) {
            flags |= FLAG_T;
            s_all &= p.seq != prev;
            body.push(s_bit(p.seq) | (p.value & 0x7F));
        }

        let poly: Logs = self
            .poly
            .iter()
            .filter(|(_, l)| covered(l.seq))
            .map(|(n, l)| (*n, *l))
            .collect();
        if !poly.is_empty() {
            flags |= FLAG_A;
            let chapter_s = poly.iter().all(|(_, l)| l.seq != prev);
            s_all &= chapter_s;
            body.push((u8::from(chapter_s) << 7) | ((poly.len() - 1) & 0x7F) as u8);
            for (note, log) in &poly {
                body.push(s_bit(log.seq) | note);
                body.push(log.value & 0x7F); // X = 0
            }
        }

        if flags == 0 {
            return None;
        }
        // < 1024 by construction: P 3 + C 257 + W 2 + N 272 + T 1 + A 257 + 3.
        let length = body.len() + 3;
        out.push((u8::from(s_all) << 7) | (number << 3) | ((length >> 8) & 0x03) as u8);
        out.push((length & 0xFF) as u8);
        out.push(flags);
        out.extend_from_slice(&body);
        Some(s_all)
    }
}

/// Decoder for the emitted subset; only used to verify the encoder in tests.
#[cfg(test)]
pub(crate) mod decode {
    pub type ProgramLog = (bool, u8, Option<(u8, u8)>);

    #[derive(Debug, PartialEq, Eq, Default)]
    pub struct Journal {
        pub s: bool,
        pub checkpoint: u16,
        pub channels: Vec<ChannelJournal>,
    }

    #[derive(Debug, PartialEq, Eq, Default)]
    pub struct ChannelJournal {
        pub s: bool,
        pub channel: u8,
        /// (S, program, bank (msb, lsb) if B=1)
        pub program: Option<ProgramLog>,
        /// (S, number, value)
        pub controllers: Vec<(bool, u8, u8)>,
        /// (S, note, Y, velocity)
        pub note_ons: Vec<(bool, u8, bool, u8)>,
        pub note_offs: Vec<u8>,
        /// B bit of chapter N, if the chapter is present.
        pub note_b: Option<bool>,
    }

    struct Cursor<'a>(&'a [u8]);

    impl Cursor<'_> {
        fn u8(&mut self) -> u8 {
            let (first, rest) = self.0.split_first().expect("journal truncated");
            self.0 = rest;
            *first
        }
    }

    pub fn journal(bytes: &[u8]) -> Journal {
        let mut c = Cursor(bytes);
        let b0 = c.u8();
        assert_eq!(b0 & 0x50, 0, "Y and H must be 0");
        let mut j = Journal {
            s: b0 & 0x80 != 0,
            checkpoint: u16::from_be_bytes([c.u8(), c.u8()]),
            channels: Vec::new(),
        };
        let count = if b0 & 0x20 != 0 { (b0 & 0x0F) + 1 } else { 0 };
        for _ in 0..count {
            let h0 = c.u8();
            let length = (usize::from(h0 & 0x03) << 8) | usize::from(c.u8());
            let flags = c.u8();
            assert_eq!(h0 & 0x04, 0, "H must be 0");
            assert_eq!(flags & !0xC8, 0, "only chapters P, C, N");
            let (body, rest) = c.0.split_at(length - 3);
            c.0 = rest;
            let mut b = Cursor(body);
            let mut ch = ChannelJournal {
                s: h0 & 0x80 != 0,
                channel: (h0 >> 3) & 0x0F,
                ..Default::default()
            };
            if flags & 0x80 != 0 {
                let (p, msb, lsb) = (b.u8(), b.u8(), b.u8());
                assert_eq!(lsb & 0x80, 0, "X must be 0");
                let bank = (msb & 0x80 != 0).then_some((msb & 0x7F, lsb));
                ch.program = Some((p & 0x80 != 0, p & 0x7F, bank));
            }
            if flags & 0x40 != 0 {
                let head = b.u8();
                let mut all_s = true;
                for _ in 0..=(head & 0x7F) {
                    let (n, v) = (b.u8(), b.u8());
                    assert_eq!(v & 0x80, 0, "A must be 0");
                    all_s &= n & 0x80 != 0;
                    ch.controllers.push((n & 0x80 != 0, n & 0x7F, v));
                }
                assert_eq!(head & 0x80 != 0, all_s, "chapter C S bit");
            }
            if flags & 0x08 != 0 {
                let head = b.u8();
                let lh = b.u8();
                let (low, high) = (lh >> 4, lh & 0x0F);
                let mut len = usize::from(head & 0x7F);
                if len == 127 && low == 15 && high == 0 {
                    len = 128;
                }
                ch.note_b = Some(head & 0x80 != 0);
                for _ in 0..len {
                    let (n, v) = (b.u8(), b.u8());
                    ch.note_ons
                        .push((n & 0x80 != 0, n & 0x7F, v & 0x80 != 0, v & 0x7F));
                }
                if low <= high {
                    for octet in low..=high {
                        let bits = b.u8();
                        for bit in 0..8 {
                            if bits & (0x80 >> bit) != 0 {
                                ch.note_offs.push(octet * 8 + bit);
                            }
                        }
                    }
                }
            }
            assert!(b.0.is_empty(), "channel LENGTH mismatch");
            j.channels.push(ch);
        }
        assert!(c.0.is_empty(), "trailing journal bytes");
        j
    }
}

#[cfg(test)]
mod tests {
    use super::decode::{journal as decode, ChannelJournal};
    use super::*;

    fn encode(h: &JournalHistory, checkpoint: u64, current: u64) -> Vec<u8> {
        let mut out = Vec::new();
        h.encode(checkpoint, current, &mut out);
        out
    }

    #[test]
    fn empty_journal_is_header_only() {
        let h = JournalHistory::new();
        // S=1, Y=0, A=0, H=0, TOTCHAN=0; checkpoint = previous packet.
        assert_eq!(encode(&h, 10, 10), [0x80, 0x00, 0x09]);
        assert_eq!(encode(&h, 0x1_0005, 0x1_0010), [0x80, 0x00, 0x05]);
    }

    #[test]
    fn note_on_in_previous_packet_byte_exact() {
        let mut h = JournalHistory::new();
        h.record(9, &[0x90, 60, 100]);
        assert_eq!(
            encode(&h, 9, 10),
            [
                0x20, 0x00, 0x09, // S=0 A=1 TOTCHAN=0, checkpoint 9
                0x00, 0x07, 0x08, // S=0 CHAN=0 H=0 LENGTH=7, chapter N only
                0x81, 0xF0, // B=1 LEN=1, LOW=15 HIGH=0 (no OFFBITS)
                0x3C, 0xE4, // S=0 note 60, Y=1 velocity 100
            ]
        );
        // Two packets later the same note is "old": every S bit is 1, Y=0.
        assert_eq!(
            encode(&h, 9, 12),
            [0xA0, 0x00, 0x09, 0x80, 0x07, 0x08, 0x81, 0xF0, 0xBC, 0x64]
        );
        // Once acknowledged it is no longer part of the history.
        assert_eq!(encode(&h, 10, 12), [0x80, 0x00, 0x0A]);
    }

    #[test]
    fn note_off_sets_offbits_byte_exact() {
        let mut h = JournalHistory::new();
        h.record(4, &[0x93, 60, 100]);
        h.record(5, &[0x83, 60, 0]); // note off 60 → octet 7, bit 4
        h.record(5, &[0x93, 73, 0]); // note on vel 0 = off → octet 9, bit 1
        assert_eq!(
            encode(&h, 4, 6),
            [
                0x20, 0x00, 0x04, // header, checkpoint 4
                0x18, 0x08, 0x08, // S=0 CHAN=3 LENGTH=8, chapter N
                0x00, 0x79, // B=0 (NoteOff in previous packet) LEN=0, LOW=7 HIGH=9
                0x08, 0x00, 0x40, // OFFBITS for notes 56-63, 64-71, 72-79
            ]
        );
        // One packet later: B=1 and S=1 everywhere.
        assert_eq!(
            encode(&h, 4, 7),
            [0xA0, 0x00, 0x04, 0x98, 0x08, 0x08, 0x80, 0x79, 0x08, 0x00, 0x40]
        );
    }

    #[test]
    fn program_change_byte_exact() {
        let mut h = JournalHistory::new();
        h.record(1, &[0xC2, 5]);
        assert_eq!(
            encode(&h, 1, 2),
            [
                0x20, 0x00, 0x01, // header
                0x10, 0x06, 0x80, // S=0 CHAN=2 LENGTH=6, chapter P
                0x05, 0x00, 0x00, // S=0 program 5, B=0, X=0
            ]
        );
        // With bank select seen before the program change: B=1 + MSB/LSB.
        h.record(2, &[0xB2, 0, 3]);
        h.record(2, &[0xB2, 32, 9]);
        h.record(3, &[0xC2, 7]);
        assert_eq!(
            encode(&h, 3, 5),
            [
                0xA0, 0x00, 0x03, // S=1 (nothing from packet 4)
                0x90, 0x06, 0x80, // S=1 CHAN=2 LENGTH=6, chapter P
                0x87, 0x83, 0x09, // S=1 program 7, B=1 MSB 3, X=0 LSB 9
            ]
        );
    }

    #[test]
    fn control_change_byte_exact() {
        let mut h = JournalHistory::new();
        h.record(20, &[0xB0, 7, 100]);
        h.record(21, &[0xB0, 1, 64]);
        h.record(21, &[0xB0, 7, 101]); // overwrites the older value
        assert_eq!(
            encode(&h, 20, 22),
            [
                0x20, 0x00, 0x14, // header, checkpoint 20
                0x00, 0x08, 0x40, // S=0 CHAN=0 LENGTH=8, chapter C
                0x01, // S=0 LEN=1 (two logs)
                0x01, 0x40, // S=0 controller 1, A=0 value 64
                0x07, 0x65, // S=0 controller 7, A=0 value 101
            ]
        );
    }

    #[test]
    fn multiple_channels_and_chapters_roundtrip() {
        let mut h = JournalHistory::new();
        h.record(100, &[0xC0, 10]);
        h.record(101, &[0xB0, 7, 90]);
        h.record(102, &[0x90, 60, 80]);
        h.record(103, &[0x90, 64, 81]);
        h.record(103, &[0x80, 60, 0]);
        h.record(103, &[0xB9, 64, 127]);
        h.record(103, &[0xF1, 0x12]); // not journalled
        h.record(104, &[0x9F, 127, 1]);
        let bytes = encode(&h, 101, 105);
        // Header: S=0 (channel 15 has data from packet 104), A=1, TOTCHAN=2.
        assert_eq!(&bytes[..3], &[0x22, 0x00, 101]);
        let j = decode(&bytes);
        assert!(!j.s);
        assert_eq!(j.checkpoint, 101);
        assert_eq!(
            j.channels,
            vec![
                ChannelJournal {
                    s: true,
                    channel: 0,
                    program: None, // packet 100 is before the checkpoint
                    controllers: vec![(true, 7, 90)],
                    note_ons: vec![(true, 64, false, 81)],
                    note_offs: vec![60],
                    note_b: Some(true),
                },
                ChannelJournal {
                    s: true,
                    channel: 9,
                    controllers: vec![(true, 64, 127)],
                    ..Default::default()
                },
                ChannelJournal {
                    s: false,
                    channel: 15,
                    note_ons: vec![(false, 127, true, 1)],
                    note_b: Some(true),
                    ..Default::default()
                },
            ]
        );
        // Channel 0: header 3 + C (1+2) + N (2+2+1) = 11 octets, flags C|N.
        assert_eq!(&bytes[3..6], &[0x80, 11, 0x48]);
    }

    #[test]
    fn full_chapters_roundtrip() {
        let mut h = JournalHistory::new();
        for n in 0..128u8 {
            h.record(1, &[0x91, n, 1 + n % 127]);
            h.record(1, &[0xB1, n, n]);
        }
        let j = decode(&encode(&h, 1, 3));
        assert_eq!(j.channels[0].note_ons.len(), 128);
        assert_eq!(j.channels[0].controllers.len(), 128);
        assert!(j.channels[0].note_offs.is_empty());

        // 127 note logs without OFFBITS must not be mistaken for 128.
        h.trim(2);
        for n in 0..127u8 {
            h.record(2, &[0x91, n, 5]);
        }
        let bytes = encode(&h, 2, 3);
        assert_eq!(&bytes[6..8], &[0xFF, 0xF1]);
        assert_eq!(decode(&bytes).channels[0].note_ons.len(), 127);

        // All notes off: 16 OFFBITS octets, LOW=0 HIGH=15.
        for n in 0..128u8 {
            h.record(3, &[0x81, n, 0]);
        }
        let bytes = encode(&h, 3, 4);
        assert_eq!(&bytes[6..8], &[0x00, 0x0F]);
        assert_eq!(&bytes[8..], &[0xFF; 16]);
        assert_eq!(decode(&bytes).channels[0].note_offs.len(), 128);
    }

    #[test]
    fn trim_and_sequence_wrap() {
        let mut h = JournalHistory::new();
        h.record(0xFFFF, &[0x90, 1, 1]);
        h.record(0x1_0000, &[0xB0, 2, 2]);
        h.record(0x1_0000, &[0xC0, 3]);
        let j = decode(&encode(&h, 0xFFFF, 0x1_0001));
        assert_eq!(j.checkpoint, 0xFFFF);
        assert_eq!(j.channels[0].note_ons, vec![(true, 1, false, 1)]);
        assert_eq!(j.channels[0].program, Some((false, 3, None)));
        h.trim(0x1_0000);
        let j = decode(&encode(&h, 0xFFFF, 0x1_0001));
        assert!(j.channels[0].note_ons.is_empty());
        assert_eq!(j.channels[0].controllers, vec![(false, 2, 2)]);
        h.trim(0x1_0001);
        assert_eq!(encode(&h, 0xFFFF, 0x1_0001), [0x80, 0xFF, 0xFF]);
        // Malformed input is ignored.
        h.record(5, &[]);
        h.record(5, &[0x90]);
        h.record(5, &[0xF0, 1, 0xF7]);
    }
}
