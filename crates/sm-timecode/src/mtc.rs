//! MIDI Timecode (MTC) generator.
//!
//! Pure and sample-timestamped: the generator performs no I/O, it only
//! reports which MIDI messages are due at which absolute sample time. All
//! message times derive from exact integer arithmetic on the sample clock,
//! so MTC cannot drift against audio (or against the LTC encoder, which
//! uses the same anchors).

use crate::queue::TransportQueue;
use crate::types::{FrameRate, TcTransport, Timecode};

/// MTC Full Frame message: `F0 7F 7F 01 01 hr mn sc fr F7`, with the MTC
/// rate code in bits 5-6 of `hr`.
pub fn full_frame_message(tc: Timecode, rate: FrameRate) -> [u8; 10] {
    [
        0xF0,
        0x7F,
        0x7F,
        0x01,
        0x01,
        (rate.mtc_rate_code() << 5) | (tc.hours & 0x1F),
        tc.minutes & 0x3F,
        tc.seconds & 0x3F,
        tc.frames & 0x1F,
        0xF7,
    ]
}

/// Data byte of quarter-frame piece `piece` (0..=7) for label `tc`.
fn quarter_frame_data(piece: u8, tc: Timecode, rate: FrameRate) -> u8 {
    let nibble = match piece & 7 {
        0 => tc.frames & 0x0F,
        1 => (tc.frames >> 4) & 0x01,
        2 => tc.seconds & 0x0F,
        3 => (tc.seconds >> 4) & 0x03,
        4 => tc.minutes & 0x0F,
        5 => (tc.minutes >> 4) & 0x03,
        6 => tc.hours & 0x0F,
        _ => ((tc.hours >> 4) & 0x01) | (rate.mtc_rate_code() << 1),
    };
    ((piece & 7) << 4) | nibble
}

/// MTC generator driven by the audio sample clock.
///
/// * Whenever a transport becomes effective (start, stop, relocation, rate
///   change) a Full Frame message is emitted at its anchor sample.
/// * While running, quarter-frame messages (`F1 nd`) are emitted four per
///   frame; quarter-frame `q` after the anchor is stamped
///   `anchor + ceil(q * sample_rate * den / (num * 4))`. Piece numbers cycle
///   0..=7 starting with piece 0 at the anchor; each 8-piece cycle encodes
///   the label of the frame that was current when its piece 0 was sent
///   (receivers add two frames, as usual for MTC).
/// * While stopped, nothing but the Full Frame is sent.
///
/// A transport whose anchor already lies before the generator position
/// takes effect immediately: the Full Frame is stamped with the current
/// position and carries the label current at that time, and quarter frames
/// continue on the grid defined by the (past) anchor.
///
/// No allocation, locking or I/O; safe to call from a real-time thread
/// (provided `emit` is).
#[derive(Clone, Debug)]
pub struct MtcGenerator {
    sample_rate: u32,
    pos: u64,
    current: TcTransport,
    next_q: u64,
    pending: TransportQueue,
}

impl MtcGenerator {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            pos: 0,
            current: TcTransport::stopped(FrameRate::default()),
            next_q: 0,
            pending: TransportQueue::new(),
        }
    }

    /// Sample position up to which messages have been emitted.
    pub fn position(&self) -> u64 {
        self.pos
    }

    /// Schedule a transport; it becomes effective at `t.anchor_sample`.
    /// Up to [`crate::PENDING_CAPACITY`] transports may be pending.
    pub fn set_transport(&mut self, t: TcTransport) {
        self.pending.insert(t);
    }

    /// Sample time of quarter-frame `q` of the current transport.
    fn quarter_time(&self, q: u64) -> u64 {
        let (num, den) = self.current.rate.rational();
        let n = u128::from(q) * u128::from(self.sample_rate) * u128::from(den);
        let off = n.div_ceil(u128::from(num) * 4);
        self.current
            .anchor_sample
            .saturating_add(u64::try_from(off).unwrap_or(u64::MAX))
    }

    fn activate(&mut self, t: TcTransport, at: u64, emit: &mut dyn FnMut(u64, &[u8])) {
        self.current = t;
        let label = t.timecode_at(at, self.sample_rate);
        emit(at, &full_frame_message(label, t.rate));
        self.next_q = 0;
        if t.running && at > t.anchor_sample && self.sample_rate != 0 {
            // Late start: continue on the anchor's grid with the first
            // quarter frame that is not in the past.
            let (num, den) = t.rate.rational();
            let d = u128::from(at - t.anchor_sample);
            let q = d * u128::from(num) * 4 / (u128::from(self.sample_rate) * u128::from(den));
            let q = u64::try_from(q).unwrap_or(u64::MAX);
            self.next_q = if self.quarter_time(q) < at {
                q.saturating_add(1)
            } else {
                q
            };
        }
    }

    /// Emit every message with a timestamp in `[position, until_sample)`,
    /// in timestamp order, then move the position to `until_sample`.
    pub fn advance(&mut self, until_sample: u64, emit: &mut dyn FnMut(u64, &[u8])) {
        let mut pos = self.pos;
        while pos < until_sample {
            while let Some(p) = self.pending.first() {
                if p.anchor_sample > pos {
                    break;
                }
                self.pending.pop_front();
                self.activate(p, pos, emit);
            }
            let seg_end = match self.pending.first() {
                Some(p) => p.anchor_sample.min(until_sample),
                None => until_sample,
            };
            if self.current.running && self.sample_rate != 0 {
                let rate = self.current.rate;
                let day = rate.frames_per_day();
                loop {
                    let q = self.next_q;
                    let t = self.quarter_time(q);
                    if t >= seg_end {
                        break;
                    }
                    let cycle = q / 8;
                    let frame = (self.current.anchor_frame % day + (cycle % day) * 2) % day;
                    let label = Timecode::from_frame_number(frame, rate);
                    emit(t, &[0xF1, quarter_frame_data((q % 8) as u8, label, rate)]);
                    self.next_q = q.saturating_add(1);
                }
            }
            pos = seg_end;
        }
        self.pos = self.pos.max(until_sample);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Msg = (u64, Vec<u8>);

    fn collect(g: &mut MtcGenerator, until: u64, out: &mut Vec<Msg>) {
        g.advance(until, &mut |t, m| out.push((t, m.to_vec())));
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
    fn full_frame_bytes() {
        let m = full_frame_message(Timecode::new(1, 2, 3, 4), FrameRate::F25);
        assert_eq!(
            m,
            [0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x21, 0x02, 0x03, 0x04, 0xF7]
        );
        let m = full_frame_message(Timecode::new(23, 59, 58, 29), FrameRate::F2997Df);
        assert_eq!(
            m,
            [0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x40 | 23, 59, 58, 29, 0xF7]
        );
        assert_eq!(
            full_frame_message(Timecode::new(10, 0, 0, 0), FrameRate::F30)[5],
            0x60 | 10
        );
        assert_eq!(
            full_frame_message(Timecode::new(10, 0, 0, 0), FrameRate::F24)[5],
            10
        );
    }

    /// Reassemble 8 quarter-frame data bytes into (label, rate code).
    fn reassemble(pieces: &[u8]) -> (Timecode, u8) {
        assert_eq!(pieces.len(), 8);
        for (i, p) in pieces.iter().enumerate() {
            assert_eq!((p >> 4) as usize, i, "piece order");
        }
        let n = |i: usize| pieces[i] & 0x0F;
        let tc = Timecode::new(
            n(6) | ((n(7) & 1) << 4),
            n(4) | (n(5) << 4),
            n(2) | (n(3) << 4),
            n(0) | (n(1) << 4),
        );
        (tc, n(7) >> 1)
    }

    #[test]
    fn quarter_frames_exact_for_ten_minutes() {
        let sr = 48_000u32;
        for rate in [FrameRate::F25, FrameRate::F2997Df] {
            let (num, den) = rate.rational();
            let anchor = 12_345u64;
            // Start shortly before a drop-frame minute boundary.
            let anchor_frame = Timecode::new(0, 0, 59, 20).to_frame_number(rate);
            let mut g = MtcGenerator::new(sr);
            g.set_transport(running(rate, anchor, anchor_frame));
            let until = anchor + u64::from(sr) * 600;
            let mut msgs = Vec::new();
            let mut pos = 0;
            while pos < until {
                pos = (pos + 480).min(until);
                collect(&mut g, pos, &mut msgs);
            }
            assert_eq!(msgs[0].0, anchor);
            assert_eq!(
                msgs[0].1,
                full_frame_message(Timecode::from_frame_number(anchor_frame, rate), rate)
            );
            let qf = &msgs[1..];
            // Exactly the quarter frames whose time lies before `until`.
            let expected_count = (u128::from(until - anchor) * u128::from(num) * 4)
                .div_ceil(u128::from(sr) * u128::from(den))
                as usize;
            assert_eq!(qf.len(), expected_count, "{rate:?}");
            for (q, (t, m)) in qf.iter().enumerate() {
                let ideal = (q as u128 * u128::from(sr) * u128::from(den))
                    .div_ceil(u128::from(num) * 4) as u64;
                assert_eq!(*t, anchor + ideal, "{rate:?} q={q}");
                assert_eq!(m.len(), 2);
                assert_eq!(m[0], 0xF1);
                assert_eq!((m[1] >> 4) as usize, q % 8);
            }
            for (c, chunk) in qf.chunks_exact(8).enumerate() {
                let data: Vec<u8> = chunk.iter().map(|(_, m)| m[1]).collect();
                let (tc, code) = reassemble(&data);
                let want = Timecode::from_frame_number(anchor_frame + 2 * c as u64, rate);
                assert_eq!(tc, want, "{rate:?} cycle {c}");
                assert_eq!(code, rate.mtc_rate_code());
            }
        }
    }

    #[test]
    fn reassembly_all_rates_and_day_wrap() {
        for rate in FrameRate::ALL {
            let anchor_frame = rate.frames_per_day() - 5;
            let mut g = MtcGenerator::new(44_100);
            g.set_transport(running(rate, 0, anchor_frame));
            let mut msgs = Vec::new();
            collect(&mut g, 44_100, &mut msgs);
            let data: Vec<u8> = msgs[1..].iter().map(|(_, m)| m[1]).collect();
            for (c, chunk) in data.chunks_exact(8).enumerate() {
                let (tc, code) = reassemble(chunk);
                assert_eq!(
                    tc,
                    Timecode::from_frame_number(anchor_frame + 2 * c as u64, rate)
                );
                assert_eq!(code, rate.mtc_rate_code());
            }
            assert!(data.len() >= 8 * 10);
        }
    }

    #[test]
    fn stopped_emits_only_full_frame() {
        let mut g = MtcGenerator::new(48_000);
        let mut msgs = Vec::new();
        collect(&mut g, 1000, &mut msgs);
        assert!(msgs.is_empty());
        g.set_transport(TcTransport {
            rate: FrameRate::F30,
            running: false,
            anchor_sample: 2000,
            anchor_frame: 90,
        });
        collect(&mut g, 100_000, &mut msgs);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, 2000);
        assert_eq!(
            msgs[0].1,
            full_frame_message(Timecode::new(0, 0, 3, 0), FrameRate::F30)
        );
    }

    #[test]
    fn relocation_and_stop_emit_full_frame_and_restart_cycle() {
        let sr = 48_000u32;
        let rate = FrameRate::F25;
        let mut g = MtcGenerator::new(sr);
        g.set_transport(running(rate, 0, 0));
        // Relocate in the middle of a cycle (and not on a quarter-frame time).
        let reloc = 480 * 13 + 7;
        let new_frame = Timecode::new(1, 0, 0, 0).to_frame_number(rate);
        g.set_transport(running(rate, reloc, new_frame));
        let stop = reloc + 48_000;
        g.set_transport(TcTransport {
            rate,
            running: false,
            anchor_sample: stop,
            anchor_frame: 7,
        });
        let mut msgs = Vec::new();
        collect(&mut g, stop + 48_000, &mut msgs);

        let before: Vec<&Msg> = msgs.iter().filter(|(t, _)| *t < reloc).collect();
        assert_eq!(before.len(), 1 + 14);
        assert_eq!(before.last().map(|(t, _)| *t), Some(480 * 13));

        let at: Vec<&Msg> = msgs
            .iter()
            .filter(|(t, _)| *t >= reloc && *t < stop)
            .collect();
        assert_eq!(at[0].0, reloc);
        assert_eq!(at[0].1, full_frame_message(Timecode::new(1, 0, 0, 0), rate));
        // Cycle restarts with piece 0 exactly at the new anchor.
        for (q, (t, m)) in at[1..].iter().enumerate() {
            assert_eq!(*t, reloc + 480 * q as u64);
            assert_eq!((m[1] >> 4) as usize, q % 8);
        }
        assert_eq!(at.len(), 1 + 100);
        let data: Vec<u8> = at[1..9].iter().map(|(_, m)| m[1]).collect();
        assert_eq!(reassemble(&data).0, Timecode::new(1, 0, 0, 0));

        let after: Vec<&Msg> = msgs.iter().filter(|(t, _)| *t >= stop).collect();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].0, stop);
        assert_eq!(
            after[0].1,
            full_frame_message(Timecode::new(0, 0, 0, 7), rate)
        );
        assert!(msgs.windows(2).all(|w| w[0].0 <= w[1].0));
    }

    #[test]
    fn step_size_does_not_change_output() {
        let sr = 44_100u32;
        let setup = |g: &mut MtcGenerator| {
            g.set_transport(running(FrameRate::F2997Df, 1000, 1790));
            g.set_transport(running(FrameRate::F23976, 30_001, 5));
            g.set_transport(TcTransport {
                rate: FrameRate::F23976,
                running: false,
                anchor_sample: 60_000,
                anchor_frame: 99,
            });
            g.set_transport(running(FrameRate::F30, 70_000, 123_456));
        };
        let until = 120_000u64;

        let mut huge = MtcGenerator::new(sr);
        setup(&mut huge);
        let mut a = Vec::new();
        collect(&mut huge, until, &mut a);

        for step in [1u64, 7, 64, 4096] {
            let mut g = MtcGenerator::new(sr);
            setup(&mut g);
            let mut b = Vec::new();
            let mut pos = 0;
            while pos < until {
                pos = (pos + step).min(until);
                collect(&mut g, pos, &mut b);
            }
            assert_eq!(a, b, "step {step}");
        }
        assert!(a.len() > 250);
        assert_eq!(a.iter().filter(|(_, m)| m.len() == 10).count(), 4);
    }

    #[test]
    fn late_transport_takes_effect_immediately() {
        let sr = 48_000u32;
        let rate = FrameRate::F25;
        let mut g = MtcGenerator::new(sr);
        let mut msgs = Vec::new();
        collect(&mut g, 10_000, &mut msgs);
        assert!(msgs.is_empty());
        // Anchor 3 frames + 100 samples in the past.
        g.set_transport(running(rate, 10_000 - 3 * 1920 - 100, 50));
        collect(&mut g, 20_000, &mut msgs);
        assert_eq!(msgs[0].0, 10_000);
        assert_eq!(
            msgs[0].1,
            full_frame_message(Timecode::new(0, 0, 2, 3), rate)
        );
        // Quarter frames stay on the anchor's grid: next one is q=13.
        assert_eq!(msgs[1].0, 10_000 - 100 + 480);
        assert_eq!(msgs[1].1[1] >> 4, 5);
    }
}
