//! Timecode input from audio (LTC): the capture device runs on a worker
//! thread (`sm-audio-in`), its samples are decoded on the caller's thread
//! and every decoded frame is stamped with the engine time at which the
//! *next* frame begins (the end of the decoded frame's sync word).
//!
//! Input and output devices run on different clocks. Captured samples are
//! mapped to wall-clock instants ([`InputClock`]), and instants to engine
//! frames through the output [`crate::clock::ClockMap`]. Both mappings use
//! the least-delayed observation of a sliding window, which removes most of
//! the callback jitter; the remaining error is of the order of a device
//! buffer plus the (unknown) converter latency of the input.

use sm_audio::input::{AudioInputBackend, InputDeviceInfo, InputReader, InputRequest, InputStream};
use sm_core::chase::TcObservation;
use sm_timecode::{DecodedLtcFrame, FrameRate, LtcDecoder};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Maps captured sample counts to instants: `instant = base + offset + n / rate`.
pub struct InputClock {
    base: Instant,
    rate: f64,
    best: Option<(f64, Instant)>,
}

const WINDOW: Duration = Duration::from_secs(2);

impl InputClock {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            base: Instant::now(),
            rate: f64::from(sample_rate.max(1)),
            best: None,
        }
    }

    /// `count` samples had been captured at `at`.
    pub fn observe(&mut self, count: u64, at: Instant) {
        let secs = if at >= self.base {
            at.duration_since(self.base).as_secs_f64()
        } else {
            -self.base.duration_since(at).as_secs_f64()
        };
        // Callbacks run after the capture: the smallest offset is the most accurate.
        let offset = secs - count as f64 / self.rate;
        match self.best {
            Some((best, stamp)) if offset >= best && at.duration_since(stamp) < WINDOW => {}
            _ => self.best = Some((offset, at)),
        }
    }

    pub fn ready(&self) -> bool {
        self.best.is_some()
    }

    /// Instant at which sample `n` was captured.
    pub fn instant_of(&self, n: u64) -> Instant {
        let offset = self.best.map(|b| b.0).unwrap_or(0.0);
        let secs = offset + n as f64 / self.rate;
        if secs >= 0.0 {
            self.base + Duration::from_secs_f64(secs)
        } else {
            self.base
                .checked_sub(Duration::from_secs_f64(-secs))
                .unwrap_or(self.base)
        }
    }
}

/// Decodes LTC from an open capture stream.
pub struct LtcInput {
    reader: InputReader,
    decoder: LtcDecoder,
    clock: InputClock,
    buf: Vec<f32>,
    frames: Vec<DecodedLtcFrame>,
    peak: f32,
    dropped_reported: u64,
    df_warned: bool,
    rate: LtcRateDetector,
}

/// Works out the frame rate of incoming LTC from the spacing of its frames
/// (sample-exact in the capture stream) and the drop-frame flag: LTC itself
/// does not say whether it runs at 24, 25 or 30 fps.
#[derive(Default)]
pub struct LtcRateDetector {
    last_end: Option<u64>,
    deltas: std::collections::VecDeque<u64>,
    detected: Option<FrameRate>,
}

impl LtcRateDetector {
    const KEEP: usize = 32;
    const NEED: usize = 8;

    /// One decoded frame ending at `end_sample` (capture samples).
    pub fn push(
        &mut self,
        end_sample: u64,
        drop_frame: bool,
        sample_rate: u32,
    ) -> Option<FrameRate> {
        let sr = f64::from(sample_rate.max(1));
        if let Some(prev) = self.last_end {
            let d = end_sample.saturating_sub(prev);
            // Only neighbouring frames (between ~20 and ~36 fps).
            if (d as f64) > sr / 36.0 && (d as f64) < sr / 20.0 {
                self.deltas.push_back(d);
                if self.deltas.len() > Self::KEEP {
                    self.deltas.pop_front();
                }
            } else {
                self.deltas.clear();
            }
        }
        self.last_end = Some(end_sample);
        if self.deltas.len() >= Self::NEED {
            let mean = self.deltas.iter().sum::<u64>() as f64 / self.deltas.len() as f64;
            let fps = sr / mean;
            let nominal = [24.0, 25.0, 30.0]
                .into_iter()
                .min_by(|a: &f64, b: &f64| (a - fps).abs().total_cmp(&(b - fps).abs()))
                .unwrap_or(25.0);
            self.detected = Some(if drop_frame && nominal > 27.0 {
                FrameRate::F2997Df
            } else if nominal < 24.5 {
                FrameRate::F24
            } else if nominal < 27.0 {
                FrameRate::F25
            } else {
                FrameRate::F30
            });
        }
        self.detected
    }

    pub fn detected(&self) -> Option<FrameRate> {
        self.detected
    }
}

impl LtcInput {
    pub fn new(reader: InputReader) -> Self {
        let sr = reader.sample_rate();
        Self {
            reader,
            decoder: LtcDecoder::new(sr),
            clock: InputClock::new(sr),
            buf: Vec::with_capacity(sr as usize / 5),
            frames: Vec::with_capacity(32),
            peak: 0.0,
            dropped_reported: 0,
            df_warned: false,
            rate: LtcRateDetector::default(),
        }
    }

    /// Decode everything captured so far. `to_engine` maps an instant to
    /// engine time. Returns observations and log messages.
    pub fn poll(
        &mut self,
        rate: FrameRate,
        engine_rate: f64,
        to_engine: impl Fn(Instant) -> u64,
    ) -> (Vec<TcObservation>, Vec<String>) {
        let mut log = Vec::new();
        self.buf.clear();
        for (count, at) in self.reader.read(&mut self.buf) {
            self.clock.observe(count, at);
        }
        let peak = self.buf.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        self.peak = self.peak.max(peak);
        self.frames.clear();
        self.decoder.push(&self.buf, &mut self.frames);
        let dropped = self.reader.dropped();
        if dropped > self.dropped_reported {
            log.push(format!(
                "LTC input: {} samples lost (engine thread too slow)",
                dropped - self.dropped_reported
            ));
            self.dropped_reported = dropped;
        }
        if !self.clock.ready() {
            return (Vec::new(), log);
        }
        let day = rate.frames_per_day();
        let in_rate = self.reader.sample_rate();
        let mut out = Vec::with_capacity(self.frames.len());
        for f in &self.frames {
            let detected = self.rate.push(f.end_sample, f.drop_frame, in_rate);
            // Until the rate is known, frames that fit the show's rate are used as they are.
            let src = match detected {
                Some(d) if d.nominal() != rate.nominal() || d.drop_frame() != rate.drop_frame() => {
                    d
                }
                _ => rate,
            };
            if u32::from(f.timecode.frames) >= src.nominal() {
                continue; // not known yet / not this rate
            }
            if src != rate && !self.df_warned {
                self.df_warned = true;
                log.push(format!(
                    "LTC input runs at {}, the show at {}: converting by timecode value",
                    src.label(),
                    rate.label()
                ));
            }
            // Forward: the frame *after* the decoded one starts at `end_sample`.
            // Backwards: the source is back at the start of the decoded frame.
            let label = f.timecode.to_frame_number(src);
            let next = if f.reverse {
                label
            } else {
                (label + 1) % src.frames_per_day()
            };
            let (frame, offset) = sm_timecode::convert_frame(next, src, rate);
            let at = to_engine(self.clock.instant_of(f.end_sample));
            out.push(TcObservation {
                frame: frame % day,
                at: at.saturating_sub((offset * engine_rate) as u64),
            });
        }
        (out, log)
    }

    /// The rate the incoming LTC was found to run at.
    pub fn detected_rate(&self) -> Option<FrameRate> {
        self.rate.detected()
    }

    /// Peak level since the last call (linear) and reset.
    pub fn take_peak(&mut self) -> f32 {
        std::mem::take(&mut self.peak)
    }

    pub fn sample_rate(&self) -> u32 {
        self.reader.sample_rate()
    }
}

// ---------------------------------------------------------------------------
// Capture device worker
// ---------------------------------------------------------------------------

enum Cmd {
    Open(InputRequest, u64),
    Close,
    Scan(bool),
    Quit,
}

/// Result of an open request (generation, reader or error).
pub type Opened = (u64, Result<InputReader, String>);

#[derive(Default)]
struct Shared {
    devices: Option<Vec<InputDeviceInfo>>,
    messages: Vec<String>,
}

/// Opens / closes the capture device and lists devices on its own thread.
pub struct AudioInputWorker {
    cmd: mpsc::Sender<Cmd>,
    opened: mpsc::Receiver<Opened>,
    shared: Arc<Mutex<Shared>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

const SCAN_EVERY: Duration = Duration::from_secs(3);

fn worker(
    backend: Box<dyn AudioInputBackend>,
    cmd: mpsc::Receiver<Cmd>,
    opened: mpsc::Sender<Opened>,
    shared: Arc<Mutex<Shared>>,
) {
    let mut stream: Option<Box<dyn InputStream>> = None;
    let mut scan = false;
    let mut last_scan: Option<Instant> = None;
    let mut reported_error = false;
    let say = |m: String| {
        if let Ok(mut s) = shared.lock() {
            if s.messages.len() < 32 {
                s.messages.push(m);
            }
        }
    };
    loop {
        if scan && last_scan.is_none_or(|t| t.elapsed() >= SCAN_EVERY) {
            last_scan = Some(Instant::now());
            let list = backend.input_devices();
            if let Ok(mut s) = shared.lock() {
                s.devices = Some(list);
            }
        }
        if let Some(e) = stream.as_ref().and_then(|s| s.error()) {
            if !reported_error {
                reported_error = true;
                say(format!("LTC input device error: {e}"));
            }
        }
        match cmd.recv_timeout(Duration::from_millis(250)) {
            Ok(Cmd::Open(req, generation)) => {
                stream = None;
                reported_error = false;
                let result = match backend.open_input(&req) {
                    Ok((s, reader)) => {
                        let info = s.info();
                        say(format!(
                            "LTC input: {} channel {} @ {} Hz",
                            info.device_name,
                            req.channel + 1,
                            info.sample_rate
                        ));
                        stream = Some(s);
                        Ok(reader)
                    }
                    Err(e) => Err(e.to_string()),
                };
                if opened.send((generation, result)).is_err() {
                    return;
                }
            }
            Ok(Cmd::Close) => stream = None,
            Ok(Cmd::Scan(on)) => {
                scan = on;
                if on {
                    last_scan = None;
                }
            }
            Ok(Cmd::Quit) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

impl AudioInputWorker {
    pub fn new(backend: Box<dyn AudioInputBackend>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (op_tx, op_rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("sm-audio-in".into())
            .spawn(move || worker(backend, cmd_rx, op_tx, s))
            .map_err(|e| tracing::error!("cannot start audio input thread: {e}"))
            .ok();
        Self {
            cmd: cmd_tx,
            opened: op_rx,
            shared,
            thread,
        }
    }

    /// Ask for a stream; the reader arrives through [`AudioInputWorker::poll_opened`].
    pub fn open(&self, req: InputRequest, generation: u64) {
        let _ = self.cmd.send(Cmd::Open(req, generation));
    }

    pub fn close(&self) {
        let _ = self.cmd.send(Cmd::Close);
    }

    /// Keep the device list up to date (only while LTC input is selected).
    pub fn scan(&self, on: bool) {
        let _ = self.cmd.send(Cmd::Scan(on));
    }

    pub fn poll_opened(&self) -> Vec<Opened> {
        self.opened.try_iter().collect()
    }

    /// The last device scan; `None` while not scanned.
    pub fn devices(&self) -> Option<Vec<InputDeviceInfo>> {
        self.shared.lock().ok().and_then(|s| s.devices.clone())
    }

    pub fn take_messages(&self) -> Vec<String> {
        self.shared
            .lock()
            .map(|mut s| std::mem::take(&mut s.messages))
            .unwrap_or_default()
    }
}

impl Drop for AudioInputWorker {
    fn drop(&mut self) {
        let _ = self.cmd.send(Cmd::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sm_audio::input::{input_pair, ManualInputBackend};
    use sm_timecode::{LtcEncoder, TcTransport, Timecode};

    #[test]
    fn ltc_rate_is_detected_from_frame_spacing() {
        let detect = |spacing: f64, df: bool| {
            let mut d = LtcRateDetector::default();
            let mut r = None;
            for i in 0..20u64 {
                r = d.push((1000.0 + i as f64 * spacing) as u64, df, 48_000);
            }
            r
        };
        assert_eq!(detect(2000.0, false), Some(FrameRate::F24));
        assert_eq!(detect(1920.0, false), Some(FrameRate::F25));
        assert_eq!(detect(1600.0, false), Some(FrameRate::F30));
        assert_eq!(detect(1601.6, true), Some(FrameRate::F2997Df));
        // 25 fps played 3 % fast is still 25.
        assert_eq!(detect(1920.0 / 1.03, false), Some(FrameRate::F25));
        // Not enough frames yet / a gap restarts the measurement.
        let mut d = LtcRateDetector::default();
        assert_eq!(d.push(0, false, 48_000), None);
        assert_eq!(d.push(1920, false, 48_000), None);
    }

    #[test]
    fn input_clock_takes_the_least_delayed_stamp() {
        let mut c = InputClock::new(48_000);
        let t0 = c.base + Duration::from_millis(50);
        // Sample n is captured at t0 + n / 48 kHz; callbacks come late.
        c.observe(480, t0 + Duration::from_millis(13));
        c.observe(960, t0 + Duration::from_millis(20)); // on time
        c.observe(1440, t0 + Duration::from_millis(33));
        let at = c.instant_of(48_960);
        let want = t0 + Duration::from_millis(1020);
        let diff = if at > want { at - want } else { want - at };
        assert!(diff < Duration::from_micros(20), "{diff:?}");
    }

    /// LTC through the capture path: observations carry the exact start
    /// sample of each frame (mapped 1:1 here) and the right frame number.
    #[test]
    fn ltc_through_the_input_path() {
        for rate in [FrameRate::F25, FrameRate::F2997Df, FrameRate::F30] {
            let sr = 48_000u32;
            let (mut tap, reader) = input_pair(sr, 1);
            let mut ltc = LtcInput::new(reader);
            let start = Timecode::new(10, 0, 0, 0).to_frame_number(rate);
            let src = TcTransport {
                rate,
                running: true,
                anchor_sample: 777,
                anchor_frame: start,
            };
            let mut enc = LtcEncoder::new(sr);
            enc.set_transport(src);
            let t0 = Instant::now();
            // Engine time = captured sample index + 10 000 (some other clock origin).
            let to_engine = |at: Instant| {
                let secs = if at >= t0 {
                    at.duration_since(t0).as_secs_f64()
                } else {
                    -t0.duration_since(at).as_secs_f64()
                };
                (secs * f64::from(sr)).round() as u64 + 10_000
            };
            let mut mono = vec![0.0f32; 512];
            let mut obs = Vec::new();
            let mut pos = 0u64;
            while pos < u64::from(sr) * 2 {
                enc.render(pos, &mut mono, 0.4);
                // Stereo device, LTC on the second channel.
                let stereo: Vec<f32> = mono.iter().flat_map(|x| [0.0, *x]).collect();
                pos += 512;
                // The callback for samples ..pos runs at the instant of sample pos.
                let at = t0 + Duration::from_secs_f64(pos as f64 / f64::from(sr));
                tap.push_at(&stereo, 2, at);
                let (o, log) = ltc.poll(rate, 48_000.0, to_engine);
                assert!(log.is_empty(), "{log:?}");
                obs.extend(o);
            }
            assert!(obs.len() as u64 >= u64::from(rate.nominal()) * 2 - 3);
            for o in &obs {
                let k = o.frame - start;
                let want = src.sample_after_frames(k, sr).unwrap() + 10_000;
                assert!(
                    o.at.abs_diff(want) <= 2,
                    "{rate:?} frame {k}: {} vs {want}",
                    o.at
                );
            }
            assert!(ltc.take_peak() > 0.3);
        }
    }

    #[test]
    fn worker_opens_lists_and_reports() {
        let (backend, feed) = ManualInputBackend::new("Virtual", 2, 48_000);
        let w = AudioInputWorker::new(Box::new(backend));
        w.scan(true);
        w.open(
            InputRequest {
                device_id: Some("Virtual".into()),
                channel: 1,
            },
            7,
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = None;
        while got.is_none() && Instant::now() < deadline {
            got = w.poll_opened().pop();
            std::thread::sleep(Duration::from_millis(5));
        }
        let (generation, reader) = got.unwrap();
        assert_eq!(generation, 7);
        let mut reader = reader.unwrap();
        assert!(feed.push(&[0.0, 0.5], Instant::now()));
        let mut out = Vec::new();
        reader.read(&mut out);
        assert_eq!(out, vec![0.5]);
        while w.devices().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(w.devices().unwrap()[0].name, "Virtual");
        assert!(w.take_messages().iter().any(|m| m.contains("Virtual")));
        w.close();
        while feed.is_open() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!feed.is_open());
    }
}
