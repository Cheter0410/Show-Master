//! Linear Timecode (SMPTE 12M LTC): frame bit layout, a real-time safe
//! sample-accurate encoder and an independent audio-domain decoder.
//!
//! # Bit numbering
//!
//! An LTC frame is 80 bits, transmitted bit 0 first. Frames are represented
//! as `[u8; 10]` where LTC bit `i` is bit `i % 8` (LSB first) of byte
//! `i / 8`. The sync word (bits 64..=79, `0011 1111 1111 1101` in
//! transmission order) therefore appears as bytes `0xFC, 0xBF`.

use crate::queue::TransportQueue;
use crate::types::{FrameRate, TcTransport, Timecode};

/// Bits per LTC frame.
const BITS_PER_FRAME: u64 = 80;
/// Half-bit cells per LTC frame (the encoder's time resolution).
const HALF_BITS_PER_FRAME: u64 = BITS_PER_FRAME * 2;
/// Sync word as it appears in bits 64..=79 with bit 64 as the LSB.
const SYNC_WORD_LSB_FIRST: u128 = 0xBFFC;

/// Build the 80-bit LTC frame for `tc`.
///
/// BCD time fields sit at the standard positions (frames 0-3/8-9, seconds
/// 16-19/24-26, minutes 32-35/40-42, hours 48-51/56-57), bit 10 is the
/// drop-frame flag (set for 29.97df), user bits and the remaining flags are
/// zero, bits 64..=79 hold the sync word, and the biphase polarity
/// correction bit (bit 27 for 24/30 fps based rates, bit 59 for 25 fps) is
/// set so that the frame contains an even number of zero bits.
pub fn encode_frame_bits(tc: Timecode, rate: FrameRate) -> [u8; 10] {
    let field = |value: u8, pos: u32| u128::from(value) << pos;
    let mut v: u128 = field(tc.frames % 10, 0)
        | field((tc.frames / 10) & 0x3, 8)
        | field(u8::from(rate.drop_frame()), 10)
        | field(tc.seconds % 10, 16)
        | field((tc.seconds / 10) & 0x7, 24)
        | field(tc.minutes % 10, 32)
        | field((tc.minutes / 10) & 0x7, 40)
        | field(tc.hours % 10, 48)
        | field((tc.hours / 10) & 0x3, 56)
        | (SYNC_WORD_LSB_FIRST << 64);
    if v.count_ones() % 2 == 1 {
        let correction_bit = if rate.nominal() == 25 { 59 } else { 27 };
        v |= 1u128 << correction_bit;
    }
    let mut out = [0u8; 10];
    for (i, b) in out.iter_mut().enumerate() {
        *b = (v >> (8 * i)) as u8;
    }
    out
}

/// Biphase-mark levels of the 160 half-bit cells of one frame, as a bitset
/// (cell `j` is bit `j % 128` of word `j / 128`; set = high).
///
/// There is a transition at every bit boundary and an additional mid-bit
/// transition for a one. Because every frame has an even number of
/// transitions (polarity correction), each frame starts from the same
/// level, which makes the waveform a pure function of the frame bits.
fn half_bit_levels(bits: [u8; 10]) -> [u128; 2] {
    let mut words = [0u128; 2];
    let mut level = false;
    let mut cell = 0u32;
    let mut put = |level: bool, cell: u32| {
        if level {
            if cell < 128 {
                words[0] |= 1u128 << cell;
            } else {
                words[1] |= 1u128 << ((cell - 128) & 127);
            }
        }
    };
    for byte in bits {
        for b in 0..8 {
            level = !level;
            put(level, cell);
            if (byte >> b) & 1 == 1 {
                level = !level;
            }
            put(level, cell + 1);
            cell += 2;
        }
    }
    words
}

/// Incremental position of the encoder inside the running transport.
#[derive(Clone, Copy, Debug)]
struct Cursor {
    valid: bool,
    /// Absolute sample this cursor describes.
    sample: u64,
    /// Frames since the anchor.
    frame: u64,
    /// Half-bit cell inside the frame, `0..160`.
    cell: u64,
    /// `((sample - anchor) * step) mod period`, always `< period`.
    acc: u64,
    /// Waveform of the current frame.
    levels: [u128; 2],
}

/// Real-time safe LTC generator locked to the audio sample clock.
///
/// After construction no method allocates, locks, performs I/O or panics.
///
/// The half-bit cell shown at absolute sample `s` is
/// `floor((s - anchor_sample) * num * 160 / (sample_rate * den))`, computed
/// with exact integer arithmetic, so the signal never drifts against the
/// sample clock regardless of block sizes. Frame `k` after the anchor is
/// labelled `Timecode::from_frame_number(anchor_frame + k, rate)`.
///
/// The output is an unfiltered square wave at `±gain` (silence while the
/// transport is stopped).
#[derive(Clone, Debug)]
pub struct LtcEncoder {
    sample_rate: u32,
    current: TcTransport,
    /// Invert the waveform of the current transport (see `activate`).
    invert: bool,
    pending: TransportQueue,
    cursor: Cursor,
    /// End of the last rendered block.
    next_sample: u64,
    /// Level of the last rendered sample: `Some(high)`, or `None` for
    /// silence / unknown.
    last_level: Option<bool>,
}

impl LtcEncoder {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            current: TcTransport::stopped(FrameRate::default()),
            invert: false,
            pending: TransportQueue::new(),
            cursor: Cursor {
                valid: false,
                sample: 0,
                frame: 0,
                cell: 0,
                acc: 0,
                levels: [0; 2],
            },
            next_sample: 0,
            last_level: None,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The transport that was in effect at the end of the last rendered
    /// block.
    pub fn transport(&self) -> TcTransport {
        self.current
    }

    /// Schedule a transport change. It takes effect exactly at
    /// `t.anchor_sample`, also in the middle of a rendered block. If the
    /// anchor lies before the end of the last rendered block it takes
    /// effect immediately (the signal continues as if the transport had
    /// been in effect since its anchor).
    ///
    /// Up to [`crate::PENDING_CAPACITY`] transports with future anchors can
    /// be queued; a transport with the same anchor as a queued one replaces
    /// it; if the queue overflows the entry with the latest anchor is lost.
    pub fn set_transport(&mut self, t: TcTransport) {
        if t.anchor_sample < self.next_sample {
            self.current = t;
            self.invert = false;
            self.cursor.valid = false;
        } else {
            self.pending.insert(t);
        }
    }

    fn activate(&mut self, t: TcTransport, at: u64) {
        self.current = t;
        // When a running transport takes over exactly at its anchor while
        // the line is high, invert it so that there is a signal transition
        // right at the anchor (a frame otherwise starts high). Biphase mark
        // is polarity agnostic, so this is inaudible to decoders but gives
        // them a clean edge to lock onto.
        self.invert = t.running && t.anchor_sample == at && self.last_level == Some(true);
        self.cursor.valid = false;
    }

    /// Write mono LTC for absolute samples
    /// `start_sample .. start_sample + out.len()` into `out` (overwriting).
    pub fn render(&mut self, start_sample: u64, out: &mut [f32], gain: f32) {
        if start_sample != self.next_sample {
            self.cursor.valid = false;
            self.last_level = None;
        }
        let len = out.len();
        let mut i = 0usize;
        while i < len {
            let s = start_sample.wrapping_add(i as u64);
            while let Some(p) = self.pending.first() {
                if p.anchor_sample > s {
                    break;
                }
                self.pending.pop_front();
                self.activate(p, s);
            }
            let mut end = len;
            if let Some(p) = self.pending.first() {
                let until_change = p.anchor_sample.saturating_sub(s);
                if until_change < (len - i) as u64 {
                    end = i.saturating_add(until_change as usize).max(i + 1);
                }
            }
            let live = self.current.running && self.sample_rate != 0;
            let audible = live && s >= self.current.anchor_sample;
            if live && !audible {
                // Rendering from before the anchor of the current transport
                // (only after a backwards jump): silent up to the anchor.
                let until_anchor = self.current.anchor_sample - s;
                if until_anchor < (end - i) as u64 {
                    end = i.saturating_add(until_anchor as usize).max(i + 1);
                }
            }
            let Some(seg) = out.get_mut(i..end) else {
                break;
            };
            if audible {
                self.render_running(s, seg, gain);
            } else {
                seg.fill(0.0);
                self.last_level = None;
            }
            i = end;
        }
        self.next_sample = start_sample.wrapping_add(len as u64);
    }

    fn frame_levels(&self, frame: u64) -> [u128; 2] {
        let rate = self.current.rate;
        let day = rate.frames_per_day();
        let n = (self.current.anchor_frame % day + frame % day) % day;
        half_bit_levels(encode_frame_bits(
            Timecode::from_frame_number(n, rate),
            rate,
        ))
    }

    /// Position the cursor on absolute sample `s` (`s >= anchor`).
    fn seek(&mut self, s: u64, step: u64, period: u64) {
        let d = u128::from(s.saturating_sub(self.current.anchor_sample));
        let total = d * u128::from(step);
        let cells = total / u128::from(period);
        let frame = (cells / u128::from(HALF_BITS_PER_FRAME)) as u64;
        self.cursor = Cursor {
            valid: true,
            sample: s,
            frame,
            cell: (cells % u128::from(HALF_BITS_PER_FRAME)) as u64,
            acc: (total % u128::from(period)) as u64,
            levels: self.frame_levels(frame),
        };
    }

    fn render_running(&mut self, s: u64, seg: &mut [f32], gain: f32) {
        let (num, den) = self.current.rate.rational();
        // Phase advance per sample and phase span of one half-bit cell.
        let step = num * HALF_BITS_PER_FRAME;
        let period = u64::from(self.sample_rate) * den;
        if !(self.cursor.valid && self.cursor.sample == s) {
            self.seek(s, step, period);
        }
        let len = seg.len();
        let mut j = 0usize;
        let mut high = false;
        while j < len {
            let c = self.cursor;
            // Samples left in this cell (>= 1 because acc < period).
            let left = period.saturating_sub(c.acc).div_ceil(step).max(1);
            let run = left.min((len - j) as u64);
            let word = if c.cell < 128 {
                c.levels[0]
            } else {
                c.levels[1]
            };
            high = ((word >> (c.cell & 127)) & 1 == 1) != self.invert;
            let value = if high { gain } else { -gain };
            let run_end = j.saturating_add(run as usize).min(len);
            if let Some(cells) = seg.get_mut(j..run_end) {
                cells.fill(value);
            }
            j = run_end;
            let mut acc = c.acc.wrapping_add(run.wrapping_mul(step));
            if acc >= period {
                let mut cell = c.cell + acc / period;
                acc %= period;
                if cell >= HALF_BITS_PER_FRAME {
                    let frame = c.frame.wrapping_add(cell / HALF_BITS_PER_FRAME);
                    cell %= HALF_BITS_PER_FRAME;
                    self.cursor.frame = frame;
                    self.cursor.levels = self.frame_levels(frame);
                }
                self.cursor.cell = cell;
            }
            self.cursor.acc = acc;
        }
        self.cursor.sample = s.wrapping_add(len as u64);
        if len > 0 {
            self.last_level = Some(high);
        }
    }
}

/// One frame recovered by [`LtcDecoder`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodedLtcFrame {
    pub timecode: Timecode,
    /// State of the drop-frame flag (bit 10).
    pub drop_frame: bool,
    /// Index, counted in samples pushed into the decoder so far, of the
    /// sample right after the frame's last bit (the end of the sync word),
    /// i.e. the first sample of the following frame.
    pub end_sample: u64,
}

/// Lowest detection threshold; signals below roughly -70 dBFS are ignored.
const MIN_THRESHOLD: f32 = 1.0e-4;
/// Detection threshold relative to the tracked signal peak.
const THRESHOLD_RATIO: f32 = 0.3;

/// LTC decoder working purely on the audio signal (forward direction).
///
/// The signal is sliced with a hysteresis comparator whose threshold
/// follows the signal peak, the intervals between level changes are
/// classified as half or full bit cells against an adaptively tracked bit
/// period, and the resulting bit stream is searched for the sync word.
/// It is independent of signal polarity and absolute level and tolerates
/// mild additive noise. Any frame rate from 23.976 to 30 fps is accepted
/// without configuration.
///
/// # Latency / accuracy
///
/// `end_sample` is the index of the first sample after the level change
/// that terminates the frame; for a clean square-wave signal this is exact
/// (zero offset). There is no fixed reporting delay: a frame is emitted by
/// the `push` call that contains sample `end_sample`. The one exception is
/// a frame directly followed by silence: its end is only recognised once
/// the line has been quiet for about a quarter bit period, so it is emitted
/// that many samples late (`end_sample` itself is still exact).
///
/// After an abrupt level drop of more than about 3:1 the comparator needs
/// some tens of milliseconds to re-arm, so a few frames may be lost.
///
/// This type is intended for verification and input monitoring; `push`
/// appends to a `Vec` and is therefore not real-time safe unless the
/// vector has spare capacity.
#[derive(Clone, Debug)]
pub struct LtcDecoder {
    count: u64,
    envelope: f32,
    decay: f32,
    /// Comparator state: 1 high, -1 low, 0 silence.
    state: i8,
    /// Consecutive samples inside the dead zone while `state != 0`.
    quiet: u32,
    quiet_limit: u32,
    /// Tracked bit period in samples and its allowed range.
    period: f32,
    min_period: f32,
    max_period: f32,
    last_edge: Option<u64>,
    bit_start: u64,
    half_pending: bool,
    shift: u128,
    bits_seen: u32,
}

impl LtcDecoder {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(1) as f32;
        // 80 bits per frame at 30 fps .. 23.976 fps, with 20 % margin.
        let min_period = sr / 2400.0 * 0.8;
        let max_period = sr / 1918.0 * 1.2;
        let period = sr / 2160.0;
        Self {
            count: 0,
            envelope: 0.0,
            decay: 1.0 - 1.0 / (0.05 * sr).max(2.0),
            state: 0,
            quiet: 0,
            quiet_limit: ((period / 4.0) as u32).max(2),
            period,
            min_period,
            max_period,
            last_edge: None,
            bit_start: 0,
            half_pending: false,
            shift: 0,
            bits_seen: 0,
        }
    }

    /// Number of samples pushed so far.
    pub fn samples_pushed(&self) -> u64 {
        self.count
    }

    /// Feed audio; every frame completed inside `samples` is appended to
    /// `out`.
    pub fn push(&mut self, samples: &[f32], out: &mut Vec<DecodedLtcFrame>) {
        for &x in samples {
            let n = self.count;
            self.count += 1;
            let a = x.abs();
            self.envelope = if a > self.envelope {
                a
            } else {
                self.envelope * self.decay
            };
            let thr = (self.envelope * THRESHOLD_RATIO).max(MIN_THRESHOLD);
            let level: i8 = if x > thr {
                1
            } else if x < -thr {
                -1
            } else {
                0
            };
            if level != 0 {
                self.quiet = 0;
                if level != self.state {
                    self.state = level;
                    self.edge(n, out);
                }
            } else if self.state != 0 {
                self.quiet += 1;
                if self.quiet > self.quiet_limit {
                    // The line went silent: the first quiet sample ends the
                    // last cell.
                    let first_quiet = n + 1 - u64::from(self.quiet);
                    self.edge(first_quiet, out);
                    self.state = 0;
                    self.quiet = 0;
                    self.last_edge = None;
                }
            }
        }
    }

    fn resync(&mut self, at: u64) {
        self.bits_seen = 0;
        self.half_pending = false;
        self.bit_start = at;
    }

    /// A level change whose new level starts at sample `t`.
    fn edge(&mut self, t: u64, out: &mut Vec<DecodedLtcFrame>) {
        let Some(last) = self.last_edge else {
            self.last_edge = Some(t);
            self.resync(t);
            return;
        };
        self.last_edge = Some(t);
        let interval = (t - last) as f32;
        let period = self.period;
        if interval > 1.6 * period || interval < 0.25 * period {
            self.resync(t);
        } else if interval < 0.75 * period {
            if self.half_pending {
                self.half_pending = false;
                self.bit(true, t, out);
            } else {
                self.half_pending = true;
            }
        } else if self.half_pending {
            // A full cell after a lone half cell: the bit phase was off by
            // half a cell (start-up mid-stream, or the generator relocated).
            // Up to here only ones can have been received since the slip,
            // and their number is the same in either phase, so the bits
            // collected so far stay valid: drop the stray half cell and
            // carry on from the start of this cell.
            self.half_pending = false;
            self.bit_start = last;
            self.bit(false, t, out);
        } else {
            self.bit(false, t, out);
        }
    }

    /// A complete bit cell ending at sample `t`.
    fn bit(&mut self, one: bool, t: u64, out: &mut Vec<DecodedLtcFrame>) {
        let measured = (t - self.bit_start) as f32;
        self.bit_start = t;
        self.period =
            (self.period + (measured - self.period) / 16.0).clamp(self.min_period, self.max_period);
        self.quiet_limit = ((self.period / 4.0) as u32).max(2);
        self.shift = (self.shift >> 1) | (u128::from(one) << 79);
        self.bits_seen = (self.bits_seen + 1).min(80);
        if self.bits_seen == 80 && (self.shift >> 64) & 0xFFFF == SYNC_WORD_LSB_FIRST {
            if let Some((timecode, drop_frame)) = decode_fields(self.shift) {
                out.push(DecodedLtcFrame {
                    timecode,
                    drop_frame,
                    end_sample: t,
                });
            }
        }
    }
}

/// Extract the time fields from 80 received bits (first received = bit 0).
fn decode_fields(word: u128) -> Option<(Timecode, bool)> {
    let field = |pos: u32, width: u32| ((word >> pos) & ((1u128 << width) - 1)) as u8;
    let units = [field(0, 4), field(16, 4), field(32, 4), field(48, 4)];
    if units.iter().any(|&u| u > 9) {
        return None;
    }
    let [frame_units, second_units, minute_units, hour_units] = units;
    let timecode = Timecode::new(
        field(56, 2) * 10 + hour_units,
        field(40, 3) * 10 + minute_units,
        field(24, 3) * 10 + second_units,
        field(8, 2) * 10 + frame_units,
    );
    Some((timecode, field(10, 1) == 1))
}

#[cfg(test)]
#[path = "ltc_tests.rs"]
mod tests;
