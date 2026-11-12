//! Chasing an external timecode (MTC or LTC).
//!
//! Pure state machine on the engine's sample clock. The I/O layer turns
//! incoming timecode into [`TcObservation`]s: "frame `frame` started at
//! engine sample `at`". The chaser decides when the engine's timecode
//! transport locks, re-anchors, jumps, freewheels and is lost:
//!
//! * **Lock** after `lock_frames` consecutive frames that are consistent
//!   with each other (each one where the previous one predicts it, within
//!   half a frame). The transport is anchored at the latest observation.
//! * While locked, an observation within one frame of the running transport
//!   is accepted as is (no correction: sub-frame jitter of the input is
//!   ignored). A larger deviation must be confirmed by a second observation
//!   consistent with the first before anything changes; up to
//!   [`JUMP_FRAMES`] it is treated as drift and the transport is re-anchored,
//!   beyond that it is a jump and the cue lists' jump policies apply.
//! * Without consistent input the transport keeps running (**freewheel**)
//!   until `freewheel` samples after the last good frame; then it stops and
//!   the input is reported **lost**.
//! * An MTC Full Frame (sent by a stopped or relocating source) stops the
//!   transport at that position immediately. An LTC source that repeats a
//!   still frame is treated the same way.

use serde::Serialize;
use sm_timecode::{FrameRate, TcTransport};

/// "Frame `frame` (running frame number at the show's rate) started at
/// engine sample `at`."
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TcObservation {
    pub frame: u64,
    pub at: u64,
}

/// Status of the engine's timecode, as shown on the timecode clock.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TcStatus {
    /// The internal generator is the source.
    #[default]
    Generator,
    /// External source selected, nothing received yet.
    NoSignal,
    /// Valid frames arrive; not enough yet to lock.
    Locking,
    /// Following the external timecode.
    Locked,
    /// Input interrupted; the timecode keeps running on its own.
    Freewheel,
    /// Input gone for longer than the freewheel time; timecode stopped.
    Lost,
    /// The source reports a stopped / located position.
    Stopped,
    /// Following was suspended by PANIC or a timecode stop/pause command.
    Held,
}

impl TcStatus {
    pub fn label(self) -> &'static str {
        match self {
            TcStatus::Generator => "generator",
            TcStatus::NoSignal => "no signal",
            TcStatus::Locking => "locking",
            TcStatus::Locked => "locked",
            TcStatus::Freewheel => "freewheel",
            TcStatus::Lost => "lost",
            TcStatus::Stopped => "stopped",
            TcStatus::Held => "held",
        }
    }
}

/// Deviations up to this many frames are corrected without applying the
/// jump policies (drift between the source clock and the audio clock).
pub const JUMP_FRAMES: f64 = 4.0;
/// Deviations up to this many frames are tolerated without re-anchoring.
pub const REANCHOR_FRAMES: f64 = 1.0;
/// Two observations are consistent when they agree within this many frames.
const CONSISTENT_FRAMES: f64 = 0.5;

/// What the engine has to do after feeding the chaser.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChaseStep {
    Nothing,
    /// Locked: start running with this transport (jump policies apply).
    Lock(TcTransport),
    /// Small correction while locked (deviation in frames).
    Reanchor(TcTransport, f64),
    /// Discontinuity while locked (jump policies apply).
    Jump(TcTransport),
    /// Back on track after a dropout.
    Relocked,
    /// The input stopped at `frame` (stop the transport there).
    Stop(u64),
    /// Input interrupted; the transport keeps running.
    Freewheel,
    /// Freewheel time is over: stop the transport where it is.
    Lost,
}

#[derive(Clone, Debug)]
pub struct Chaser {
    rate: FrameRate,
    sr: u32,
    lock_frames: u32,
    freewheel: u64,
    status: TcStatus,
    /// Running transport while locked / freewheeling.
    transport: Option<TcTransport>,
    /// Lock candidate and the number of consistent frames seen so far.
    cand: Option<(TcObservation, u32)>,
    /// A deviating observation waiting for confirmation.
    pending: Option<TcObservation>,
    last_good: Option<u64>,
    /// Previous observation and how often its frame repeated.
    prev: Option<TcObservation>,
    still: u32,
}

impl Chaser {
    pub fn new(rate: FrameRate, sample_rate: u32, lock_frames: u32, freewheel_ms: u32) -> Self {
        let sr = sample_rate.max(1);
        Self {
            rate,
            sr,
            lock_frames: lock_frames.clamp(1, 100),
            freewheel: u64::from(freewheel_ms) * u64::from(sr) / 1000,
            status: TcStatus::NoSignal,
            transport: None,
            cand: None,
            pending: None,
            last_good: None,
            prev: None,
            still: 0,
        }
    }

    pub fn status(&self) -> TcStatus {
        self.status
    }

    pub fn rate(&self) -> FrameRate {
        self.rate
    }

    /// Samples per frame (fractional).
    fn frame_len(&self) -> f64 {
        let (num, den) = self.rate.rational();
        f64::from(self.sr) * den as f64 / num as f64
    }

    /// `a - b` in frames, wrapped into (-day/2, day/2].
    fn rel(&self, a: u64, b: u64) -> i64 {
        let day = self.rate.frames_per_day() as i64;
        let d = (a as i64 - b as i64).rem_euclid(day);
        if d > day / 2 {
            d - day
        } else {
            d
        }
    }

    /// How far `obs` lies from what `anchor` (running) predicts, in frames.
    fn deviation(&self, obs: TcObservation, anchor_frame: u64, anchor_at: u64) -> f64 {
        let elapsed = (obs.at as f64 - anchor_at as f64) / self.frame_len();
        self.rel(obs.frame, anchor_frame) as f64 - elapsed
    }

    fn running(&self, obs: TcObservation) -> TcTransport {
        TcTransport {
            rate: self.rate,
            running: true,
            anchor_sample: obs.at,
            anchor_frame: obs.frame % self.rate.frames_per_day(),
        }
    }

    /// Suspend following (PANIC, `tc_stop`, `tc_pause`).
    pub fn hold(&mut self) {
        self.status = TcStatus::Held;
        self.transport = None;
        self.cand = None;
        self.pending = None;
    }

    /// Resume following after [`Chaser::hold`].
    pub fn release(&mut self) {
        if self.status == TcStatus::Held {
            self.status = TcStatus::NoSignal;
        }
    }

    pub fn held(&self) -> bool {
        self.status == TcStatus::Held
    }

    /// The engine timecode is running under the chaser's control.
    pub fn running_transport(&self) -> Option<TcTransport> {
        self.transport
    }

    /// The sample at which the input counts as lost, while running.
    pub fn deadline(&self) -> Option<u64> {
        self.transport?;
        self.last_good.map(|g| g.saturating_add(self.freewheel))
    }

    /// Feed one received frame.
    pub fn observe(&mut self, obs: TcObservation) -> ChaseStep {
        if self.held() {
            return ChaseStep::Nothing;
        }
        // A frame label that repeats is a still frame (stopped source).
        let repeat = self.prev.is_some_and(|p| p.frame == obs.frame);
        self.prev = Some(obs);
        if repeat {
            self.still += 1;
            if self.still >= 2 && self.status != TcStatus::Stopped {
                return self.stop(obs.frame);
            }
            return ChaseStep::Nothing;
        }
        self.still = 0;
        match self.transport {
            Some(t) => self.observe_running(obs, t),
            None => self.observe_locking(obs),
        }
    }

    fn observe_running(&mut self, obs: TcObservation, t: TcTransport) -> ChaseStep {
        let dev = self.deviation(obs, t.anchor_frame, t.anchor_sample);
        if dev.abs() <= REANCHOR_FRAMES {
            self.pending = None;
            self.last_good = Some(obs.at);
            if self.status == TcStatus::Freewheel {
                self.status = TcStatus::Locked;
                return ChaseStep::Relocked;
            }
            return ChaseStep::Nothing;
        }
        let confirmed = self.pending.is_some_and(|p| {
            self.rel(obs.frame, p.frame) > 0
                && self.deviation(obs, p.frame, p.at).abs() <= CONSISTENT_FRAMES
        });
        if !confirmed {
            self.pending = Some(obs);
            return ChaseStep::Nothing;
        }
        self.pending = None;
        self.last_good = Some(obs.at);
        let new = self.running(obs);
        self.transport = Some(new);
        self.status = TcStatus::Locked;
        if dev.abs() <= JUMP_FRAMES {
            ChaseStep::Reanchor(new, dev)
        } else {
            ChaseStep::Jump(new)
        }
    }

    fn observe_locking(&mut self, obs: TcObservation) -> ChaseStep {
        let count = match self.cand {
            Some((c, n))
                if self.rel(obs.frame, c.frame) > 0
                    && self.deviation(obs, c.frame, c.at).abs() <= CONSISTENT_FRAMES =>
            {
                n.saturating_add(self.rel(obs.frame, c.frame) as u32)
            }
            _ => 1,
        };
        if count >= self.lock_frames {
            let t = self.running(obs);
            self.cand = None;
            self.pending = None;
            self.transport = Some(t);
            self.last_good = Some(obs.at);
            self.status = TcStatus::Locked;
            return ChaseStep::Lock(t);
        }
        self.cand = Some((obs, count));
        self.status = TcStatus::Locking;
        ChaseStep::Nothing
    }

    /// The source located / stopped at `frame` (MTC Full Frame).
    pub fn full_frame(&mut self, frame: u64) -> ChaseStep {
        if self.held() {
            return ChaseStep::Nothing;
        }
        self.prev = None;
        self.still = 0;
        self.stop(frame)
    }

    fn stop(&mut self, frame: u64) -> ChaseStep {
        self.transport = None;
        self.cand = None;
        self.pending = None;
        self.status = TcStatus::Stopped;
        ChaseStep::Stop(frame % self.rate.frames_per_day())
    }

    /// Time passes: freewheel / lost detection.
    pub fn tick(&mut self, now: u64) -> ChaseStep {
        let Some(last) = self.last_good else {
            return ChaseStep::Nothing;
        };
        let gap = now.saturating_sub(last);
        if self.transport.is_some() {
            if gap >= self.freewheel {
                self.transport = None;
                self.pending = None;
                self.status = TcStatus::Lost;
                return ChaseStep::Lost;
            }
            // MTC delivers a label every two frames, 1.75 frames late; allow
            // for that plus scheduling before calling it a dropout.
            let dropout = (self.frame_len() * 6.0) as u64 + u64::from(self.sr) / 20;
            if gap > dropout && self.status == TcStatus::Locked {
                self.status = TcStatus::Freewheel;
                return ChaseStep::Freewheel;
            }
        } else if self.status == TcStatus::Locking {
            if let Some((c, _)) = self.cand {
                if now.saturating_sub(c.at) >= self.freewheel.max(u64::from(self.sr) / 2) {
                    self.cand = None;
                    self.status = TcStatus::NoSignal;
                }
            }
        }
        ChaseStep::Nothing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn obs(frame: u64, at: u64) -> TcObservation {
        TcObservation { frame, at }
    }

    #[test]
    fn locks_after_n_consistent_frames() {
        let mut c = Chaser::new(FrameRate::F25, SR, 4, 2000);
        assert_eq!(c.status(), TcStatus::NoSignal);
        for i in 0..3 {
            assert_eq!(c.observe(obs(100 + i, 1920 * i)), ChaseStep::Nothing);
            assert_eq!(c.status(), TcStatus::Locking);
        }
        let ChaseStep::Lock(t) = c.observe(obs(103, 1920 * 3)) else {
            panic!("no lock")
        };
        assert_eq!((t.anchor_frame, t.anchor_sample), (103, 5760));
        assert_eq!(c.status(), TcStatus::Locked);
    }

    #[test]
    fn inconsistent_frames_restart_locking() {
        let mut c = Chaser::new(FrameRate::F25, SR, 3, 2000);
        c.observe(obs(10, 0));
        c.observe(obs(11, 1920));
        // Jumped: start over.
        c.observe(obs(500, 3840));
        c.observe(obs(501, 5760));
        assert!(matches!(c.observe(obs(502, 7680)), ChaseStep::Lock(_)));
    }

    #[test]
    fn mtc_style_two_frame_steps_lock() {
        let mut c = Chaser::new(FrameRate::F30, SR, 4, 2000);
        assert_eq!(c.observe(obs(0, 0)), ChaseStep::Nothing);
        assert_eq!(c.observe(obs(2, 3200)), ChaseStep::Nothing);
        assert!(matches!(c.observe(obs(4, 6400)), ChaseStep::Lock(_)));
    }

    #[test]
    fn drift_reanchors_and_jumps_need_confirmation() {
        let mut c = Chaser::new(FrameRate::F25, SR, 2, 2000);
        c.observe(obs(0, 0));
        assert!(matches!(c.observe(obs(1, 1920)), ChaseStep::Lock(_)));
        // Within a frame: tolerated.
        assert_eq!(c.observe(obs(2, 1920 * 2 + 1500)), ChaseStep::Nothing);
        // 2 frames ahead, confirmed by the next one: re-anchor.
        assert_eq!(c.observe(obs(5, 1920 * 3)), ChaseStep::Nothing);
        let ChaseStep::Reanchor(t, dev) = c.observe(obs(6, 1920 * 4)) else {
            panic!()
        };
        assert!((dev - 2.0).abs() < 1e-9, "{dev}");
        assert_eq!(t.anchor_frame, 6);
        // A single wild frame is ignored ...
        assert_eq!(c.observe(obs(9000, 1920 * 5)), ChaseStep::Nothing);
        assert_eq!(c.observe(obs(8, 1920 * 6)), ChaseStep::Nothing);
        // ... a confirmed one is a jump.
        assert_eq!(c.observe(obs(9000, 1920 * 7)), ChaseStep::Nothing);
        assert!(matches!(
            c.observe(obs(9001, 1920 * 8)),
            ChaseStep::Jump(TcTransport {
                anchor_frame: 9001,
                ..
            })
        ));
    }

    #[test]
    fn freewheel_then_lost_then_relock() {
        let mut c = Chaser::new(FrameRate::F25, SR, 2, 1000);
        c.observe(obs(0, 0));
        c.observe(obs(1, 1920));
        assert_eq!(c.deadline(), Some(1920 + 48_000));
        assert_eq!(c.tick(1920 * 3), ChaseStep::Nothing);
        assert_eq!(c.tick(1920 + 20_000), ChaseStep::Freewheel);
        assert_eq!(c.status(), TcStatus::Freewheel);
        // Input back in time: locked again.
        assert_eq!(c.observe(obs(12, 1920 * 12)), ChaseStep::Relocked);
        assert_eq!(c.tick(1920 * 12 + 48_000), ChaseStep::Lost);
        assert_eq!(c.status(), TcStatus::Lost);
        assert_eq!(c.deadline(), None);
        c.observe(obs(100, 500_000));
        assert!(matches!(c.observe(obs(101, 501_920)), ChaseStep::Lock(_)));
    }

    #[test]
    fn full_frame_and_still_frames_stop() {
        let mut c = Chaser::new(FrameRate::F25, SR, 2, 2000);
        c.observe(obs(0, 0));
        c.observe(obs(1, 1920));
        assert_eq!(c.full_frame(90_000), ChaseStep::Stop(90_000));
        assert_eq!(c.status(), TcStatus::Stopped);
        let mut c = Chaser::new(FrameRate::F25, SR, 2, 2000);
        c.observe(obs(0, 0));
        c.observe(obs(1, 1920));
        assert_eq!(c.observe(obs(1, 3840)), ChaseStep::Nothing);
        assert_eq!(c.observe(obs(1, 5760)), ChaseStep::Stop(1));
        assert_eq!(c.observe(obs(1, 7680)), ChaseStep::Nothing);
    }

    #[test]
    fn hold_ignores_input_until_released() {
        let mut c = Chaser::new(FrameRate::F25, SR, 2, 2000);
        c.hold();
        assert_eq!(c.observe(obs(0, 0)), ChaseStep::Nothing);
        assert_eq!(c.observe(obs(1, 1920)), ChaseStep::Nothing);
        assert_eq!(c.status(), TcStatus::Held);
        c.release();
        c.observe(obs(2, 3840));
        assert!(matches!(c.observe(obs(3, 5760)), ChaseStep::Lock(_)));
    }

    #[test]
    fn day_wrap_is_continuous() {
        let rate = FrameRate::F2997Df;
        let day = rate.frames_per_day();
        let mut c = Chaser::new(rate, SR, 3, 2000);
        let len = 1601.6;
        let at = |i: u64| (i as f64 * len) as u64;
        c.observe(obs(day - 2, at(0)));
        c.observe(obs(day - 1, at(1)));
        assert!(matches!(c.observe(obs(0, at(2))), ChaseStep::Lock(_)));
        assert_eq!(c.observe(obs(1, at(3))), ChaseStep::Nothing);
    }
}
