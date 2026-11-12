//! MIDI Timecode (MTC) receiver: quarter-frame reassembly and Full Frame
//! parsing.
//!
//! Pure: the caller stamps every message with the time it arrived (any
//! monotonic unit, normally engine sample frames) and gets back complete
//! timecode labels with the time at which the labelled frame started.
//!
//! # Quarter frames
//!
//! A sender transmits eight quarter-frame messages (`F1 nd`, piece `n` =
//! 0..=7) over two frames. Piece 0 is sent at the start of the frame whose
//! label the cycle carries; the label is complete when piece 7 arrives,
//! 1.75 frames later, so "the current frame" at that moment is the label
//! plus two frames (the usual receiver rule). This receiver reports the
//! label together with the arrival time of piece 0, which is exactly the
//! start of the labelled frame and needs no rate arithmetic.
//!
//! Cycles with a missing, duplicated or out-of-order piece are discarded
//! (reverse playback sends pieces 7..0 and is therefore ignored).

use crate::types::{FrameRate, Timecode};

/// Something the receiver recognised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MtcEvent {
    /// A complete quarter-frame cycle. The frame labelled `timecode`
    /// started at `at` (arrival time of piece 0).
    Frame {
        timecode: Timecode,
        /// MTC rate code: 0 = 24, 1 = 25, 2 = 30 drop-frame, 3 = 30 fps.
        rate_code: u8,
        at: u64,
    },
    /// A Full Frame message (`F0 7F <dev> 01 01 hr mn sc fr F7`): the
    /// sender located to `timecode` (sent when stopped or relocating).
    FullFrame {
        timecode: Timecode,
        rate_code: u8,
        at: u64,
    },
}

/// Nominal frames per second of an MTC rate code.
pub fn mtc_rate_fps(code: u8) -> u8 {
    match code & 3 {
        0 => 24,
        1 => 25,
        _ => 30,
    }
}

/// Does the incoming rate code fit the configured rate?
pub fn mtc_rate_matches(code: u8, rate: FrameRate) -> bool {
    rate.mtc_rate_code() == (code & 3)
}

/// A readable name of an MTC rate code.
pub fn mtc_rate_label(code: u8) -> &'static str {
    match code & 3 {
        0 => "24 fps",
        1 => "25 fps",
        2 => "30 fps drop-frame",
        _ => "30 fps",
    }
}

fn label(hours: u8, minutes: u8, seconds: u8, frames: u8, rate_code: u8) -> Option<Timecode> {
    (hours <= 23 && minutes <= 59 && seconds <= 59 && frames < mtc_rate_fps(rate_code))
        .then(|| Timecode::new(hours, minutes, seconds, frames))
}

/// Parse an MTC Full Frame SysEx message.
pub fn parse_full_frame(msg: &[u8]) -> Option<(Timecode, u8)> {
    match msg {
        [0xF0, 0x7F, _, 0x01, 0x01, hr, mn, sc, fr, 0xF7] => {
            let rate_code = (hr >> 5) & 3;
            label(hr & 0x1F, *mn, *sc, *fr, rate_code).map(|tc| (tc, rate_code))
        }
        _ => None,
    }
}

/// Reassembles quarter frames; see the module documentation.
#[derive(Clone, Debug, Default)]
pub struct MtcReceiver {
    nibbles: [u8; 8],
    /// Next expected piece; `None` = waiting for piece 0.
    next: Option<u8>,
    /// Arrival time of the current cycle's piece 0.
    start: u64,
}

impl MtcReceiver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget a partly received cycle (after a dropout or port change).
    pub fn reset(&mut self) {
        self.next = None;
    }

    /// Feed one complete MIDI message that arrived at `at`. Messages that
    /// are not MTC are ignored.
    pub fn push(&mut self, msg: &[u8], at: u64) -> Option<MtcEvent> {
        if let [0xF1, data] = msg {
            return self.quarter(*data, at);
        }
        let (timecode, rate_code) = parse_full_frame(msg)?;
        self.next = None;
        Some(MtcEvent::FullFrame {
            timecode,
            rate_code,
            at,
        })
    }

    fn quarter(&mut self, data: u8, at: u64) -> Option<MtcEvent> {
        let piece = (data >> 4) & 7;
        let nibble = data & 0x0F;
        if piece == 0 {
            self.nibbles[0] = nibble;
            self.start = at;
            self.next = Some(1);
            return None;
        }
        if self.next != Some(piece) {
            // Lost, repeated or reversed piece: wait for the next cycle.
            self.next = None;
            return None;
        }
        self.nibbles[usize::from(piece)] = nibble;
        if piece < 7 {
            self.next = Some(piece + 1);
            return None;
        }
        self.next = None;
        let n = &self.nibbles;
        let rate_code = (n[7] >> 1) & 3;
        let timecode = label(
            n[6] | ((n[7] & 1) << 4),
            n[4] | ((n[5] & 3) << 4),
            n[2] | ((n[3] & 3) << 4),
            n[0] | ((n[1] & 1) << 4),
            rate_code,
        )?;
        Some(MtcEvent::Frame {
            timecode,
            rate_code,
            at: self.start,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mtc::{full_frame_message, MtcGenerator};
    use crate::types::TcTransport;

    fn run(g: &mut MtcGenerator, until: u64) -> Vec<(u64, Vec<u8>)> {
        let mut out = Vec::new();
        let mut pos = g.position();
        while pos < until {
            pos = (pos + 256).min(until);
            g.advance(pos, &mut |t, m| out.push((t, m.to_vec())));
        }
        out
    }

    fn running(rate: FrameRate, anchor_sample: u64, anchor_frame: u64) -> TcTransport {
        TcTransport {
            rate,
            running: true,
            anchor_sample,
            anchor_frame,
        }
    }

    #[test]
    fn reassembles_generator_output_for_every_rate() {
        for sr in [44_100u32, 48_000, 96_000] {
            for rate in FrameRate::ALL {
                // Start just before a drop-frame minute boundary and the day wrap.
                for start in [
                    Timecode::new(0, 0, 59, 20).to_frame_number(rate),
                    rate.frames_per_day() - 7,
                ] {
                    let t = running(rate, 1234, start);
                    let mut g = MtcGenerator::new(sr);
                    g.set_transport(t);
                    let msgs = run(&mut g, 1234 + u64::from(sr) * 3);
                    let mut rx = MtcReceiver::new();
                    let mut frames = 0;
                    for (at, m) in &msgs {
                        match rx.push(m, *at) {
                            Some(MtcEvent::Frame {
                                timecode,
                                rate_code,
                                at,
                            }) => {
                                frames += 1;
                                assert_eq!(rate_code, rate.mtc_rate_code());
                                assert!(mtc_rate_matches(rate_code, rate));
                                // The label is the generator position at `at`.
                                assert_eq!(
                                    timecode,
                                    t.timecode_at(at, sr),
                                    "{rate:?} sr {sr} at {at}"
                                );
                                // ... and `at` is the first sample of that frame.
                                if at > t.anchor_sample {
                                    assert_ne!(t.frame_at(at - 1, sr), t.frame_at(at, sr));
                                }
                            }
                            Some(MtcEvent::FullFrame { timecode, .. }) => {
                                assert_eq!(timecode, Timecode::from_frame_number(start, rate));
                            }
                            None => {}
                        }
                    }
                    // Three seconds: one cycle every two frames, the last may be incomplete.
                    let expect = u64::from(rate.nominal()) * 3 / 2;
                    assert!(frames + 2 >= expect, "{rate:?}: {frames} cycles");
                }
            }
        }
    }

    #[test]
    fn full_frame_relocation_restarts_the_cycle() {
        let sr = 48_000;
        let rate = FrameRate::F25;
        let mut g = MtcGenerator::new(sr);
        g.set_transport(running(rate, 0, 0));
        // Relocate mid-cycle.
        let reloc = 480 * 13 + 7;
        let to = Timecode::new(1, 0, 0, 0).to_frame_number(rate);
        g.set_transport(running(rate, reloc, to));
        let msgs = run(&mut g, reloc + 48_000);
        let mut rx = MtcReceiver::new();
        let events: Vec<MtcEvent> = msgs.iter().filter_map(|(t, m)| rx.push(m, *t)).collect();
        let ff: Vec<&MtcEvent> = events
            .iter()
            .filter(|e| matches!(e, MtcEvent::FullFrame { .. }))
            .collect();
        assert_eq!(ff.len(), 2);
        assert_eq!(
            *ff[1],
            MtcEvent::FullFrame {
                timecode: Timecode::new(1, 0, 0, 0),
                rate_code: 1,
                at: reloc
            }
        );
        // The interrupted cycle before the relocation yields nothing wrong;
        // the first frame after it is the relocation target at the anchor.
        let after: Vec<&MtcEvent> = events
            .iter()
            .filter(|e| matches!(e, MtcEvent::Frame { at, .. } if *at >= reloc))
            .collect();
        assert_eq!(
            *after[0],
            MtcEvent::Frame {
                timecode: Timecode::new(1, 0, 0, 0),
                rate_code: 1,
                at: reloc
            }
        );
        for e in &events {
            if let MtcEvent::Frame { timecode, at, .. } = e {
                let t = if *at >= reloc {
                    running(rate, reloc, to)
                } else {
                    running(rate, 0, 0)
                };
                assert_eq!(*timecode, t.timecode_at(*at, sr));
            }
        }
    }

    #[test]
    fn dropouts_discard_only_the_broken_cycles() {
        let sr = 48_000;
        let rate = FrameRate::F30;
        let t = running(rate, 0, 3000);
        let mut g = MtcGenerator::new(sr);
        g.set_transport(t);
        let msgs = run(&mut g, 48_000);
        let mut rx = MtcReceiver::new();
        let mut good = 0;
        // Drop every 13th message (and a burst in the middle).
        for (i, (at, m)) in msgs.iter().enumerate() {
            if i % 13 == 5 || (200..240).contains(&i) {
                continue;
            }
            if let Some(MtcEvent::Frame { timecode, at, .. }) = rx.push(m, *at) {
                assert_eq!(timecode, t.timecode_at(at, sr));
                good += 1;
            }
        }
        assert!(good > 3, "{good}");
        // Reverse order (pieces 7..0) never produces a label.
        let mut rx = MtcReceiver::new();
        let qf: Vec<&(u64, Vec<u8>)> = msgs.iter().filter(|(_, m)| m[0] == 0xF1).collect();
        for (at, m) in qf.iter().rev() {
            assert!(rx.push(m, *at).is_none());
        }
    }

    #[test]
    fn full_frame_parsing_and_rejects() {
        let m = full_frame_message(Timecode::new(10, 20, 30, 29), FrameRate::F2997Df);
        assert_eq!(
            parse_full_frame(&m),
            Some((Timecode::new(10, 20, 30, 29), 2))
        );
        // Frames out of range for 25 fps.
        let mut bad = full_frame_message(Timecode::new(1, 0, 0, 0), FrameRate::F25);
        bad[8] = 27;
        assert_eq!(parse_full_frame(&bad), None);
        assert_eq!(
            parse_full_frame(&[0xF0, 0x7F, 0x7F, 0x01, 0x01, 0xF7]),
            None
        );
        let mut rx = MtcReceiver::new();
        assert_eq!(rx.push(&[0x90, 60, 100], 5), None);
        assert_eq!(rx.push(&[0xF1], 5), None);
        assert_eq!(mtc_rate_label(1), "25 fps");
    }
}
