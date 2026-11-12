//! Receiver-side recovery (RFC 6295, section 4 and appendix A).
//!
//! When the RTP sequence number shows that packets were lost and the packet
//! that ends the loss carries a recovery journal (J=1), the journal is
//! compared with what this receiver knows about the stream ([`ReceiverState`]:
//! last program, controller values and sounding notes per channel). The
//! differences are turned into corrective MIDI messages that are delivered
//! *before* the commands of the packet itself.
//!
//! Evaluated: the recovery journal header, chapter X (SysEx) of the system
//! journal (chapters D, V, Q, F are skipped), and channel journals with
//! chapters P (program change, incl. bank select), C (control change, value
//! tool), W (pitch wheel), N (note logs and OFFBITS), T (channel aftertouch)
//! and A (poly aftertouch). Chapters M and E are skipped by their size.
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
//! * **Pitch wheel / channel aftertouch:** sent when the journal's last value
//!   differs from the receiver's.
//! * **Poly aftertouch:** only for notes sounding at the receiver, not when
//!   X=1 (the note was silenced after it).
//! * **SysEx (MSC, ...):** a complete, finished command is replayed only when
//!   its log is known to describe the lost packet `N - 1` (S=0; for the first
//!   log, whose bit is the chapter's, only if it is the only log). A command
//!   that may have arrived is never sent twice - a doubled MSC GO would fire
//!   the next cue.
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

/// One poly aftertouch log of chapter A.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolyLog {
    /// S bit.
    pub s: bool,
    pub note: u8,
    /// X bit: the note was turned off after this pressure (do not replay).
    pub x: bool,
    pub pressure: u8,
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
    /// Chapter W: (S, first data octet = LSB, second = MSB).
    pub pitch: Option<(bool, u8, u8)>,
    /// Chapter T: (S, pressure).
    pub pressure: Option<(bool, u8)>,
    /// Chapter A logs.
    pub poly: Vec<PolyLog>,
}

/// One logged SysEx command of chapter X (system journal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysexLog {
    /// S bit as sent. For the first log it doubles as the chapter's S bit
    /// (RFC 6295 B.5), so on its own it does not say which packet the first
    /// log describes.
    pub s: bool,
    /// Finished (STA 2 or 3) and complete (F=0, D=1): `data` is the whole command.
    pub complete: bool,
    /// Data octets without F0 / F7.
    pub data: Vec<u8>,
}

/// A parsed recovery journal (the parts this receiver evaluates).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Journal {
    /// S bit of the journal header.
    pub s: bool,
    /// Sequence number of the checkpoint packet.
    pub checkpoint: u16,
    /// `true` if a system journal was present.
    pub has_system: bool,
    /// Chapter X logs of the system journal (other system chapters are skipped).
    pub sysex: Vec<SysexLog>,
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
const TOC_E: u8 = 0x04;
const TOC_T: u8 = 0x02;
const TOC_A: u8 = 0x01;

/// Parses a recovery journal (the bytes after the MIDI list of a J=1 packet).
pub fn parse_journal(bytes: &[u8]) -> Result<Journal, CodecError> {
    let mut c = Cursor(bytes);
    let b0 = c.u8()?;
    let checkpoint = u16::from_be_bytes([c.u8()?, c.u8()?]);
    let mut journal = Journal {
        s: b0 & 0x80 != 0,
        checkpoint,
        has_system: b0 & 0x40 != 0,
        sysex: Vec::new(),
        channels: Vec::new(),
    };
    if journal.has_system {
        // |S|D|V|Q|F|X| LENGTH (10) | - LENGTH includes this header.
        let length = (usize::from(c.peek(0)? & 0x03) << 8) | usize::from(c.peek(1)?);
        if length < 2 {
            return Err(CodecError::Truncated);
        }
        let system = c.take(length)?;
        // Best effort: a system journal this parser does not understand only
        // costs the SysEx recovery, never the channel journals.
        journal.sysex = parse_system(system).unwrap_or_default();
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

/// Skips chapters D, V, Q and F of a system journal and returns the
/// chapter X logs (RFC 6295 appendix B).
fn parse_system(bytes: &[u8]) -> Result<Vec<SysexLog>, CodecError> {
    let mut c = Cursor(bytes);
    let h0 = c.u8()?;
    c.u8()?; // rest of LENGTH
    let (d, v, q, f, x) = (
        h0 & 0x40 != 0,
        h0 & 0x20 != 0,
        h0 & 0x10 != 0,
        h0 & 0x08 != 0,
        h0 & 0x04 != 0,
    );
    if d {
        let toc = c.u8()?;
        // B, G, H: one octet each.
        for bit in [0x40u8, 0x20, 0x10] {
            if toc & bit != 0 {
                c.u8()?;
            }
        }
        // J, K: |S|C|V|L|DSZ| LENGTH (10) |, LENGTH includes the header.
        for bit in [0x08u8, 0x04] {
            if toc & bit != 0 {
                let len = (usize::from(c.peek(0)? & 0x03) << 8) | usize::from(c.peek(1)?);
                c.take(len.max(2))?;
            }
        }
        // Y, Z: |S|C|L| LENGTH (5) |, LENGTH includes the header.
        for bit in [0x02u8, 0x01] {
            if toc & bit != 0 {
                let len = usize::from(c.peek(0)? & 0x1F);
                c.take(len.max(1))?;
            }
        }
    }
    if v {
        c.u8()?;
    }
    if q {
        let h = c.u8()?;
        c.take(2 * usize::from(h & 0x10 != 0) + 3 * usize::from(h & 0x08 != 0))?;
    }
    if f {
        let h = c.u8()?;
        c.take(4 * usize::from(h & 0x40 != 0) + 4 * usize::from(h & 0x20 != 0))?;
    }
    let mut logs = Vec::new();
    if !x {
        return Ok(logs);
    }
    // Chapter X: logs without a chapter header, up to the end of the journal.
    while !c.0.is_empty() {
        let h = c.u8()?;
        let (t, cnt, first, dat, sta) = (
            h & 0x40 != 0,
            h & 0x20 != 0,
            h & 0x10 != 0,
            h & 0x08 != 0,
            h & 0x03,
        );
        if t {
            c.u8()?;
        }
        if cnt {
            c.u8()?;
        }
        if first {
            while c.u8()? & 0x80 != 0 {}
        }
        let mut data = Vec::new();
        if dat {
            loop {
                let b = c.u8()?;
                data.push(b & 0x7F);
                if b & 0x80 != 0 {
                    break;
                }
            }
        }
        logs.push(SysexLog {
            s: h & 0x80 != 0,
            complete: dat && !first && sta >= 2,
            data,
        });
    }
    Ok(logs)
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
        let (first, second) = (c.u8()?, c.u8()?);
        ch.pitch = Some((first & 0x80 != 0, first & 0x7F, second & 0x7F));
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
    if toc & TOC_E != 0 {
        // |S| LEN | then LEN+1 two-octet logs (note extras: not evaluated).
        let len = usize::from(c.u8()? & 0x7F) + 1;
        c.take(len * 2)?;
    }
    if toc & TOC_T != 0 {
        let t = c.u8()?;
        ch.pressure = Some((t & 0x80 != 0, t & 0x7F));
    }
    if toc & TOC_A != 0 {
        let head = c.u8()?;
        for _ in 0..=(head & 0x7F) {
            let (n, v) = (c.u8()?, c.u8()?);
            ch.poly.push(PolyLog {
                s: n & 0x80 != 0,
                note: n & 0x7F,
                x: v & 0x80 != 0,
                pressure: v & 0x7F,
            });
        }
    }
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
    /// Last pitch wheel (LSB, MSB).
    pitch: Option<(u8, u8)>,
    /// Last channel aftertouch.
    pressure: Option<u8>,
    /// Last poly aftertouch per note.
    poly: [Option<u8>; 128],
}

impl Default for ChannelState {
    fn default() -> Self {
        Self {
            program: None,
            bank_msb: None,
            bank_lsb: None,
            controllers: [None; 128],
            notes: [None; 128],
            pitch: None,
            pressure: None,
            poly: [None; 128],
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
            0xD0 => ch.pressure = Some(d1),
            0xE0 => ch.pitch = Some((d1, d2)),
            0xA0 => {
                if let Some(slot) = ch.poly.get_mut(n) {
                    *slot = Some(d2);
                }
            }
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
        // SysEx (show control: MSC GO, ...) is only replayed when the log is
        // known to describe the lost packet N-1 (S=0): replaying a command
        // that did arrive would fire a cue twice.
        // The first log's bit is the chapter's: trust it only when it is the
        // only log (then both mean the same).
        let n = journal.sysex.len();
        for (i, x) in journal.sysex.iter().enumerate() {
            if !x.s && x.complete && (i > 0 || n == 1) {
                let mut m = Vec::with_capacity(x.data.len() + 2);
                m.push(0xF0);
                m.extend_from_slice(&x.data);
                m.push(0xF7);
                out.push(m);
            }
        }
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

            if let Some((s, first, second)) = log.pitch.filter(|p| !skip(p.0)) {
                let known = self.channels.get(usize::from(ch)).and_then(|s| s.pitch);
                if known != Some((first, second)) {
                    emit(self, vec![0xE0 | ch, first, second]);
                }
                let _ = s;
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

            if let Some((_, pressure)) = log.pressure.filter(|p| !skip(p.0)) {
                let known = self.channels.get(usize::from(ch)).and_then(|s| s.pressure);
                if known != Some(pressure) {
                    emit(self, vec![0xD0 | ch, pressure]);
                }
            }

            for a in &log.poly {
                // Only for notes that sound now: pressure on a silent note is meaningless.
                if skip(a.s) || a.x || self.sounding(ch, a.note).is_none() {
                    continue;
                }
                let known = self
                    .channels
                    .get(usize::from(ch))
                    .and_then(|s| s.poly.get(usize::from(a.note)).copied())
                    .flatten();
                if known != Some(a.pressure) {
                    emit(self, vec![0xA0 | ch, a.note, a.pressure]);
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
    fn pitch_wheel_and_aftertouch_are_recovered() {
        let mut h = JournalHistory::new();
        let mut rx = ReceiverState::new();
        for m in [
            [0x90u8, 60, 100].as_slice(),
            &[0xE0, 0, 64],
            &[0xD0, 10],
            &[0xA0, 60, 20],
        ] {
            h.record(10, m);
            rx.record(m);
        }
        // Lost packets 11 and 12.
        h.record(11, &[0xE0, 5, 70]);
        h.record(11, &[0xD0, 50]);
        h.record(12, &[0xA0, 60, 90]);
        h.record(12, &[0xA0, 61, 33]); // note 61 does not sound: not replayed
        let out = recover(&mut rx, &encode(&h, 10, 13), false);
        assert_eq!(
            out,
            vec![vec![0xE0, 5, 70], vec![0xD0, 50], vec![0xA0, 60, 90]]
        );
        assert!(recover(&mut rx, &encode(&h, 10, 14), false).is_empty());
        // The parser sees the chapters as encoded.
        let j = parse_journal(&encode(&h, 10, 13)).unwrap();
        assert_eq!(j.channels[0].pitch, Some((true, 5, 70)));
        assert_eq!(j.channels[0].pressure, Some((true, 50)));
        assert_eq!(j.channels[0].poly.len(), 2);
    }

    #[test]
    fn lost_sysex_is_replayed_only_when_it_was_in_the_lost_packet() {
        let msc_go = |q: u8| vec![0xF0, 0x7F, 0x01, 0x02, 0x01, 0x01, q, 0xF7];
        // Packet 12 (N-1, certainly lost) carried GO 3; 11 carried GO 2.
        let mut h = JournalHistory::new();
        h.record(11, &msc_go(0x32));
        h.record(12, &msc_go(0x33));
        let mut rx = ReceiverState::new();
        let out = recover(&mut rx, &encode(&h, 11, 13), false);
        // GO 2 may have arrived (packet 11 is not known to be lost): only GO 3.
        assert_eq!(out, vec![msc_go(0x33)]);
        // Single SysEx in the lost packet.
        let mut h = JournalHistory::new();
        h.record(20, &msc_go(0x35));
        assert_eq!(
            recover(&mut rx, &encode(&h, 20, 21), true),
            vec![msc_go(0x35)]
        );
        // ... and not when it was in an older packet.
        assert!(recover(&mut rx, &encode(&h, 20, 22), false).is_empty());
        let j = parse_journal(&encode(&h, 20, 21)).unwrap();
        assert!(j.has_system);
        assert_eq!(j.sysex[0].data, msc_go(0x35)[1..7].to_vec());
    }

    #[test]
    fn system_chapters_before_x_are_skipped() {
        // System journal: D (B log) + V + Q (C) + F (C) + X (one finished log).
        let sys = [
            0x00, 0x00, // header, LENGTH filled below; D V Q F X
            0x40, 0x81, // D: TOC B, B log
            0x82, // V
            0x10, 0x00, 0x05, // Q: C=1, CLOCK
            0x40, 0x01, 0x02, 0x03, 0x04, // F: C=1, COMPLETE
            0x0B, 0x41, 0xC2, // X: S=0 D=1 L=0 STA=3, DATA 41 42
        ];
        let mut bytes = vec![0x40, 0x00, 0x07];
        let mut s = sys.to_vec();
        s[0] = 0x7C; // S=0 D V Q F X, LENGTH high 0
        s[1] = sys.len() as u8;
        bytes.extend_from_slice(&s);
        let j = parse_journal(&bytes).unwrap();
        assert_eq!(
            j.sysex,
            vec![SysexLog {
                s: false,
                complete: true,
                data: vec![0x41, 0x42]
            }]
        );
        let mut rx = ReceiverState::new();
        assert_eq!(
            recover(&mut rx, &bytes, false),
            vec![vec![0xF0, 0x41, 0x42, 0xF7]]
        );
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
