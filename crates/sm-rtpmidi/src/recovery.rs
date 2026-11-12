//! Receiver-side recovery (RFC 6295, section 4 and appendix A).
//!
//! When the RTP sequence number shows that packets were lost and the packet
//! that ends the loss carries a recovery journal (J=1), the journal is
//! compared with what this receiver knows about the stream ([`ReceiverState`]:
//! last program, controller values and sounding notes per channel). The
//! differences are turned into corrective MIDI messages that are delivered
//! *before* the commands of the packet itself.
//!
//! Evaluated: the recovery journal header, the system journal (skipped by its
//! LENGTH), channel journals with chapters P (program change, incl. bank
//! select), C (control change, value tool) and N (note logs and OFFBITS).
//! Chapters M and W are skipped by their size; E, T and A (which follow N) are
//! ignored because the channel journal LENGTH bounds them.
//!
//! Rules (following RFC 6295 A.1–A.6 and RFC 4696 §4.2):
//!
//! * **Single-packet loss:** if exactly one packet was lost, every structure
//!   with S=1 describes a packet that *was* received and is skipped (B=1 for
//!   the OFFBITS of chapter N).
//! * **Program:** a program (or bank) that differs from the receiver's last
//!   known one is sent again, with Bank Select MSB/LSB first when B=1.
//! * **Controllers:** controller values (value tool, A=0) that differ from the
//!   last known value are sent. Logs using the toggle/count tools (A=1) and the
//!   channel mode controllers 120–127 are not replayed (notes are repaired by
//!   chapter N).
//! * **Note off:** a note shown in OFFBITS that is still sounding at the
//!   receiver gets a NoteOff.
//! * **Note on:** a note log is only played when the sender marked it as
//!   recent (Y=1) and the note is not already sounding, or the log describes
//!   the lost packet `N - 1` (S=0) and the note is sounding from an older
//!   NoteOn (it was re-struck): then NoteOff + NoteOn. Y=0 logs are never
//!   played - a stale trigger fired late is worse than a missed one in show
//!   control.
//!
//! Recovery is not applied to the first packet of a link: the receiver has no
//! history to compare with and replaying the sender's past would fire stale
//! commands (a deliberate deviation from RFC 6295 §4, which asks receivers to
//! treat the first packet like the end of a loss event).

use crate::CodecError;

/// Program change log of chapter P.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramLog {
    /// S bit.
    pub s: bool,
    /// Program number.
    pub program: u8,
    /// Bank select (MSB, LSB) if the B bit is set.
    pub bank: Option<(u8, u8)>,
}

/// One controller log of chapter C.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControllerLog {
    /// S bit.
    pub s: bool,
    /// Controller number.
    pub number: u8,
    /// Value (value tool, A=0); `None` for the toggle/count tools (A=1).
    pub value: Option<u8>,
}

/// One note log of chapter N (the most recent command for the note was a NoteOn).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteLog {
    /// S bit.
    pub s: bool,
    /// Note number.
    pub note: u8,
    /// Sender's recommendation to play the NoteOn (recent) or skip it (stale).
    pub y: bool,
    /// Velocity (never 0 in a well-formed journal).
    pub velocity: u8,
}

/// Chapter N.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NoteChapter {
    /// B bit (S-bit function for the OFFBITS).
    pub b: bool,
    /// Note logs in journal order (oldest first).
    pub ons: Vec<NoteLog>,
    /// Notes whose most recent command was a NoteOff.
    pub offs: Vec<u8>,
}

/// One channel journal.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChannelLog {
    /// S bit of the channel journal header.
    pub s: bool,
    /// MIDI channel 0–15.
    pub channel: u8,
    /// Chapter P.
    pub program: Option<ProgramLog>,
    /// Chapter C logs.
    pub controllers: Vec<ControllerLog>,
    /// Chapter N.
    pub notes: Option<NoteChapter>,
}

/// A parsed recovery journal (the parts this receiver evaluates).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Journal {
    /// S bit of the journal header.
    pub s: bool,
    /// Sequence number of the checkpoint packet.
    pub checkpoint: u16,
    /// `true` if a system journal was present (it is skipped).
    pub has_system: bool,
    /// Channel journals.
    pub channels: Vec<ChannelLog>,
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn u8(&mut self) -> Result<u8, CodecError> {
        let (first, rest) = self.0.split_first().ok_or(CodecError::Truncated)?;
        self.0 = rest;
        Ok(*first)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], CodecError> {
        if n > self.0.len() {
            return Err(CodecError::Truncated);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn peek(&self, offset: usize) -> Result<u8, CodecError> {
        self.0.get(offset).copied().ok_or(CodecError::Truncated)
    }
}

const TOC_P: u8 = 0x80;
const TOC_C: u8 = 0x40;
const TOC_M: u8 = 0x20;
const TOC_W: u8 = 0x10;
const TOC_N: u8 = 0x08;

/// Parses a recovery journal (the bytes after the MIDI list of a J=1 packet).
pub fn parse_journal(bytes: &[u8]) -> Result<Journal, CodecError> {
    let mut c = Cursor(bytes);
    let b0 = c.u8()?;
    let checkpoint = u16::from_be_bytes([c.u8()?, c.u8()?]);
    let mut journal = Journal {
        s: b0 & 0x80 != 0,
        checkpoint,
        has_system: b0 & 0x40 != 0,
        channels: Vec::new(),
    };
    if journal.has_system {
        // |S|D|V|Q|F|X| LENGTH (10) | - LENGTH includes this header.
        let length = (usize::from(c.peek(0)? & 0x03) << 8) | usize::from(c.peek(1)?);
        if length < 2 {
            return Err(CodecError::Truncated);
        }
        c.take(length)?;
    }
    if b0 & 0x20 != 0 {
        let count = usize::from(b0 & 0x0F) + 1;
        for _ in 0..count {
            let h0 = c.peek(0)?;
            let length = (usize::from(h0 & 0x03) << 8) | usize::from(c.peek(1)?);
            if length < 3 {
                return Err(CodecError::Truncated);
            }
            let body = c.take(length)?;
            journal.channels.push(parse_channel(body)?);
        }
    }
    Ok(journal)
}

fn parse_channel(bytes: &[u8]) -> Result<ChannelLog, CodecError> {
    let mut c = Cursor(bytes);
    let h0 = c.u8()?;
    c.u8()?; // rest of LENGTH
    let toc = c.u8()?;
    let mut ch = ChannelLog {
        s: h0 & 0x80 != 0,
        channel: (h0 >> 3) & 0x0F,
        ..ChannelLog::default()
    };
    if toc & TOC_P != 0 {
        let (p, msb, lsb) = (c.u8()?, c.u8()?, c.u8()?);
        ch.program = Some(ProgramLog {
            s: p & 0x80 != 0,
            program: p & 0x7F,
            bank: (msb & 0x80 != 0).then_some((msb & 0x7F, lsb & 0x7F)),
        });
    }
    if toc & TOC_C != 0 {
        let head = c.u8()?;
        for _ in 0..=(head & 0x7F) {
            let (n, v) = (c.u8()?, c.u8()?);
            ch.controllers.push(ControllerLog {
                s: n & 0x80 != 0,
                number: n & 0x7F,
                value: (v & 0x80 == 0).then_some(v & 0x7F),
            });
        }
    }
    if toc & TOC_M != 0 {
        // |S|P|E|U|W|Z| LENGTH (10) | - LENGTH includes the header.
        let length = (usize::from(c.peek(0)? & 0x03) << 8) | usize::from(c.peek(1)?);
        if length < 2 {
            return Err(CodecError::Truncated);
        }
        c.take(length)?;
    }
    if toc & TOC_W != 0 {
        c.take(2)?;
    }
    if toc & TOC_N != 0 {
        let head = c.u8()?;
        let lh = c.u8()?;
        let (low, high) = (lh >> 4, lh & 0x0F);
        let mut len = usize::from(head & 0x7F);
        if len == 127 && low == 15 && high == 0 {
            len = 128;
        }
        let mut notes = NoteChapter {
            b: head & 0x80 != 0,
            ..NoteChapter::default()
        };
        for _ in 0..len {
            let (n, v) = (c.u8()?, c.u8()?);
            notes.ons.push(NoteLog {
                s: n & 0x80 != 0,
                note: n & 0x7F,
                y: v & 0x80 != 0,
                velocity: v & 0x7F,
            });
        }
        if low <= high {
            for octet in low..=high {
                let bits = c.u8()?;
                for bit in 0..8u8 {
                    if bits & (0x80 >> bit) != 0 {
                        notes.offs.push(octet * 8 + bit);
                    }
                }
            }
        }
        ch.notes = Some(notes);
    }
    // Chapters E, T and A are not evaluated; LENGTH already bounded them.
    Ok(ch)
}

#[derive(Debug, Clone)]
struct ChannelState {
    program: Option<u8>,
    bank_msb: Option<u8>,
    bank_lsb: Option<u8>,
    controllers: [Option<u8>; 128],
    /// Velocity of the sounding NoteOn, `None` = off / unknown.
    notes: [Option<u8>; 128],
}

impl Default for ChannelState {
    fn default() -> Self {
        Self {
            program: None,
            bank_msb: None,
            bank_lsb: None,
            controllers: [None; 128],
            notes: [None; 128],
        }
    }
}

/// What the receiver knows about one incoming stream: the last program, bank,
/// controller values and the sounding notes of each channel.
#[derive(Debug, Clone, Default)]
pub struct ReceiverState {
    channels: Box<[ChannelState; 16]>,
}

impl ReceiverState {
    /// Creates an empty state (nothing known).
    pub fn new() -> Self {
        Self::default()
    }

    /// Updates the state with one delivered MIDI message.
    pub fn record(&mut self, message: &[u8]) {
        let (Some(&status), Some(&d1)) = (message.first(), message.get(1)) else {
            // A System Reset clears everything (RFC 6295 "Reset State" command).
            if message.first() == Some(&0xFF) {
                *self = Self::default();
            }
            return;
        };
        if !(0x80..0xF0).contains(&status) {
            return;
        }
        let Some(ch) = self.channels.get_mut(usize::from(status & 0x0F)) else {
            return;
        };
        let d1 = d1 & 0x7F;
        let d2 = message.get(2).copied().unwrap_or(0) & 0x7F;
        let n = usize::from(d1);
        match status & 0xF0 {
            0x80 => {
                if let Some(slot) = ch.notes.get_mut(n) {
                    *slot = None;
                }
            }
            0x90 => {
                if let Some(slot) = ch.notes.get_mut(n) {
                    *slot = (d2 > 0).then_some(d2);
                }
            }
            0xB0 => {
                match d1 {
                    0 => ch.bank_msb = Some(d2),
                    32 => ch.bank_lsb = Some(d2),
                    // All Sound Off and the All Notes Off family silence the channel.
                    120 | 123..=127 => ch.notes = [None; 128],
                    121 => {
                        // Reset All Controllers: values are no longer known.
                        let (msb, lsb) = (ch.bank_msb, ch.bank_lsb);
                        ch.controllers = [None; 128];
                        ch.bank_msb = msb;
                        ch.bank_lsb = lsb;
                    }
                    _ => {}
                }
                if let Some(slot) = ch.controllers.get_mut(n) {
                    *slot = Some(d2);
                }
            }
            0xC0 => ch.program = Some(d1),
            _ => {}
        }
    }

    /// Appends the corrective MIDI messages for `journal` to `out` and applies
    /// them to the state. `single_loss` is `true` if exactly one packet was
    /// lost (structures with S=1 are then skipped). Returns the number of
    /// messages appended.
    pub fn recover(
        &mut self,
        journal: &Journal,
        single_loss: bool,
        out: &mut Vec<Vec<u8>>,
    ) -> usize {
        let start = out.len();
        if single_loss && journal.s {
            return 0;
        }
        let skip = |s: bool| single_loss && s;
        for log in &journal.channels {
            if skip(log.s) {
                continue;
            }
            let ch = log.channel & 0x0F;
            let mut emit = |state: &mut Self, message: Vec<u8>| {
                state.record(&message);
                out.push(message);
            };

            if let Some(p) = log.program.filter(|p| !skip(p.s)) {
                let Some(state) = self.channels.get(usize::from(ch)) else {
                    continue;
                };
                let bank_differs = p.bank.is_some_and(|(msb, lsb)| {
                    state.bank_msb != Some(msb) || state.bank_lsb != Some(lsb)
                });
                if bank_differs || state.program != Some(p.program) {
                    if let (true, Some((msb, lsb))) = (bank_differs, p.bank) {
                        emit(self, vec![0xB0 | ch, 0, msb]);
                        emit(self, vec![0xB0 | ch, 32, lsb]);
                    }
                    emit(self, vec![0xC0 | ch, p.program]);
                }
            }

            for c in &log.controllers {
                let Some(value) = c.value else { continue };
                if skip(c.s) || c.number >= 120 {
                    continue;
                }
                let known = self
                    .channels
                    .get(usize::from(ch))
                    .and_then(|s| s.controllers.get(usize::from(c.number)).copied())
                    .flatten();
                if known != Some(value) {
                    emit(self, vec![0xB0 | ch, c.number, value]);
                }
            }

            if let Some(notes) = &log.notes {
                if !skip(notes.b) {
                    for &note in &notes.offs {
                        if self.sounding(ch, note).is_some() {
                            emit(self, vec![0x80 | ch, note & 0x7F, 0]);
                        }
                    }
                }
                for n in &notes.ons {
                    if skip(n.s) {
                        continue;
                    }
                    let note = n.note & 0x7F;
                    if n.velocity == 0 {
                        // Malformed (velocity 0 belongs in OFFBITS): treat as NoteOff.
                        if self.sounding(ch, note).is_some() {
                            emit(self, vec![0x80 | ch, note, 0]);
                        }
                        continue;
                    }
                    match self.sounding(ch, note) {
                        None if n.y => emit(self, vec![0x90 | ch, note, n.velocity]),
                        // Re-struck in the lost packet N-1 while still sounding from before.
                        Some(_) if n.y && !n.s => {
                            emit(self, vec![0x80 | ch, note, 0]);
                            emit(self, vec![0x90 | ch, note, n.velocity]);
                        }
                        _ => {}
                    }
                }
            }
        }
        out.len() - start
    }

    fn sounding(&self, ch: u8, note: u8) -> Option<u8> {
        self.channels
            .get(usize::from(ch))
            .and_then(|s| s.notes.get(usize::from(note)).copied())
            .flatten()
    }
}

/// `true` if the journal's checkpoint history covers every packet after
/// `last_received` (RFC 6295 §5: checkpoint <= last received + 1, modulo 2^16).
pub fn covers_loss(checkpoint: u16, last_received: u16) -> bool {
    last_received.wrapping_add(1).wrapping_sub(checkpoint) < 0x8000
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::journal::JournalHistory;

    fn encode(h: &JournalHistory, checkpoint: u64, current: u64) -> Vec<u8> {
        let mut out = Vec::new();
        h.encode(checkpoint, current, &mut out);
        out
    }

    fn recover(state: &mut ReceiverState, bytes: &[u8], single: bool) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        state.recover(&parse_journal(bytes).unwrap(), single, &mut out);
        out
    }

    #[test]
    fn parses_hand_built_journal() {
        // Header S=0 Y=0 A=1 TOTCHAN=1, checkpoint 0x1234.
        // Channel 3: P (program 5, bank 1/2), C (7=100 A=0, 64 A=1 toggle), N (60 Y=1 vel 90, off 62).
        // Channel 9: W (skipped) + N (no logs, off 36).
        let bytes = [
            0x21, 0x12, 0x34, // journal header
            0x18, 0x10, 0xC8, // S=0 CHAN=3 LENGTH=16, TOC P C N
            0x05, 0x81, 0x02, // P: S=0 program 5, B=1 MSB 1, X=0 LSB 2
            0x01, 0x07, 0x64, 0xC0, 0x83, // C: LEN=1, 7=100, S=1 64 A=1 ALT=3
            0x01, 0x77, 0x3C, 0xDA, 0x02, // N: B=0 LEN=1 LOW=7 HIGH=7, 60 Y=1 90, OFFBITS 62
            0xC8, 0x08, 0x18, // S=1 CHAN=9 LENGTH=8, TOC W N
            0x40, 0x00, // W
            0x80, 0x44, 0x08, // N: B=1 LEN=0 LOW=4 HIGH=4, OFFBITS 36
        ];
        let j = parse_journal(&bytes).unwrap();
        assert!(!j.s);
        assert_eq!(j.checkpoint, 0x1234);
        assert_eq!(j.channels.len(), 2);
        let c3 = &j.channels[0];
        assert_eq!(c3.channel, 3);
        assert_eq!(
            c3.program,
            Some(ProgramLog {
                s: false,
                program: 5,
                bank: Some((1, 2))
            })
        );
        assert_eq!(
            c3.controllers,
            vec![
                ControllerLog {
                    s: false,
                    number: 7,
                    value: Some(100)
                },
                ControllerLog {
                    s: true,
                    number: 64,
                    value: None
                },
            ]
        );
        let n = c3.notes.as_ref().unwrap();
        assert!(!n.b);
        assert_eq!(
            n.ons,
            vec![NoteLog {
                s: false,
                note: 60,
                y: true,
                velocity: 90
            }]
        );
        assert_eq!(n.offs, vec![62]);
        let c9 = &j.channels[1];
        assert!(c9.s);
        assert_eq!(c9.notes.as_ref().unwrap().offs, vec![36]);

        // Truncated input is an error, never a panic.
        for cut in 0..bytes.len() {
            assert!(parse_journal(&bytes[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn system_journal_and_chapter_m_are_skipped() {
        let bytes = [
            0x60, 0x00, 0x01, // S=0 Y=1 A=1 TOTCHAN=0
            0x00, 0x04, 0xAA, 0xBB, // system journal, LENGTH=4
            0x00, 0x0A, 0x60, // CHAN=0 LENGTH=10, TOC C M
            0x00, 0x07, 0x10, // C: one log 7=16
            0x00, 0x04, 0x11, 0x22, // M: LENGTH=4
        ];
        let j = parse_journal(&bytes).unwrap();
        assert!(j.has_system);
        assert_eq!(j.channels[0].controllers.len(), 1);
        let mut st = ReceiverState::new();
        assert_eq!(recover(&mut st, &bytes, false), vec![vec![0xB0, 7, 16]]);
    }

    #[test]
    fn empty_journal_recovers_nothing() {
        let mut st = ReceiverState::new();
        assert!(recover(&mut st, &[0x80, 0x00, 0x05], false).is_empty());
        assert!(parse_journal(&[0x80, 0x00]).is_err());
    }

    #[test]
    fn missed_note_off_cc_and_program_from_encoder_output() {
        // Sender history: packet 10 (received): note on 60, CC7=20, program 1.
        // Packets 11, 12 (lost): note off 60, CC7=90, bank 0/3 + program 4, note on 64.
        let mut h = JournalHistory::new();
        let mut rx = ReceiverState::new();
        for m in [[0x90u8, 60, 100].as_slice(), &[0xB0, 7, 20], &[0xC0, 1]] {
            h.record(10, m);
            rx.record(m);
        }
        h.record(11, &[0x80, 60, 0]);
        h.record(11, &[0xB0, 7, 90]);
        h.record(11, &[0xB0, 0, 0]);
        h.record(11, &[0xB0, 32, 3]);
        h.record(12, &[0xC0, 4]);
        h.record(12, &[0x90, 64, 70]);
        let bytes = encode(&h, 10, 13);
        let out = recover(&mut rx, &bytes, false);
        assert_eq!(
            out,
            vec![
                vec![0xB0, 0, 0],
                vec![0xB0, 32, 3],
                vec![0xC0, 4],
                vec![0xB0, 7, 90],
                vec![0x80, 60, 0],
                vec![0x90, 64, 70], // Y=1: packet 12 is N-1
            ]
        );
        // The state is now in sync: a later journal of the same history
        // (everything S=1, Y=0) changes nothing.
        assert!(recover(&mut rx, &encode(&h, 10, 14), false).is_empty());
    }

    #[test]
    fn stale_note_on_is_not_played() {
        // The NoteOn was two packets before the current one: Y=0.
        let mut h = JournalHistory::new();
        h.record(11, &[0x90, 64, 70]);
        let mut rx = ReceiverState::new();
        let bytes = encode(&h, 10, 13);
        assert!(recover(&mut rx, &bytes, false).is_empty());
    }

    #[test]
    fn single_loss_skips_s1_structures() {
        let mut h = JournalHistory::new();
        h.record(10, &[0xB0, 7, 20]); // received (S=1 in packet 12's journal)
        h.record(11, &[0xB0, 1, 5]); // lost (S=0)
        let bytes = encode(&h, 10, 12);
        // The receiver missed the CC7 too (e.g. it joined late): a single loss
        // still only repairs packet 11 by design.
        let mut rx = ReceiverState::new();
        assert_eq!(recover(&mut rx, &bytes, true), vec![vec![0xB0, 1, 5]]);
        let mut rx = ReceiverState::new();
        assert_eq!(
            recover(&mut rx, &bytes, false),
            vec![vec![0xB0, 1, 5], vec![0xB0, 7, 20]]
        );
    }

    #[test]
    fn restruck_note_in_lost_packet_is_retriggered() {
        let mut rx = ReceiverState::new();
        rx.record(&[0x92, 50, 10]);
        // Channel 2, N: B=1 LEN=1 no OFFBITS, S=0 note 50 Y=1 vel 99.
        let bytes = [0x20, 0, 1, 0x10, 0x07, 0x08, 0x81, 0xF0, 0x32, 0xE3];
        assert_eq!(
            recover(&mut rx, &bytes, false),
            vec![vec![0x82, 50, 0], vec![0x92, 50, 99]]
        );
        // The same log with S=1 (an older packet) is assumed already played.
        let bytes = [0xA0, 0, 1, 0x90, 0x07, 0x08, 0x81, 0xF0, 0xB2, 0xE3];
        assert!(recover(&mut rx, &bytes, false).is_empty());
    }

    #[test]
    fn mode_and_toggle_controllers_are_not_replayed() {
        let bytes = [0x20, 0, 1, 0x00, 0x08, 0x40, 0x01, 0x7B, 0x00, 0x40, 0x81];
        let mut rx = ReceiverState::new();
        assert!(recover(&mut rx, &bytes, false).is_empty());
    }

    #[test]
    fn all_notes_off_and_reset_clear_the_state() {
        let mut rx = ReceiverState::new();
        rx.record(&[0x90, 60, 1]);
        rx.record(&[0xB0, 123, 0]);
        assert_eq!(rx.sounding(0, 60), None);
        rx.record(&[0x91, 61, 1]);
        rx.record(&[0xFF]);
        assert_eq!(rx.sounding(1, 61), None);
        rx.record(&[0x92, 62, 0]); // velocity 0 = off
        assert_eq!(rx.sounding(2, 62), None);
    }

    #[test]
    fn checkpoint_coverage() {
        assert!(covers_loss(10, 9));
        assert!(covers_loss(5, 9));
        assert!(!covers_loss(11, 9));
        assert!(covers_loss(0xFFFF, 0xFFFF));
        assert!(covers_loss(0, 0xFFFF));
        assert!(!covers_loss(1, 0xFFFF));
    }

    #[test]
    fn full_chapter_n_roundtrip_through_parser() {
        let mut h = JournalHistory::new();
        for n in 0..128u8 {
            h.record(1, &[0x91, n, 1 + n % 127]);
        }
        let j = parse_journal(&encode(&h, 1, 2)).unwrap();
        assert_eq!(j.channels[0].notes.as_ref().unwrap().ons.len(), 128);
        for n in 0..128u8 {
            h.record(2, &[0x81, n, 0]);
        }
        let j = parse_journal(&encode(&h, 1, 3)).unwrap();
        let notes = j.channels[0].notes.as_ref().unwrap();
        assert!(notes.ons.is_empty());
        assert_eq!(notes.offs.len(), 128);
    }
}
