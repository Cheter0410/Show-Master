//! Mapping between the audio sample clock and wall-clock instants, used to
//! schedule MIDI (which has no sample-accurate transport of its own).

use std::time::{Duration, Instant};

/// Tracks the relation `instant = base + offset + frame / sample_rate`.
///
/// The audio clock only advances once per device buffer, so individual
/// observations are late by up to one buffer. The earliest observation in a
/// sliding window is the least delayed one; using it removes most of that
/// jitter.
pub struct ClockMap {
    base: Instant,
    sample_rate: f64,
    last_frame: u64,
    best: Option<(f64, Instant)>,
}

const WINDOW: Duration = Duration::from_secs(2);

impl ClockMap {
    /// Engine samples per second.
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub fn new(sample_rate: u32) -> Self {
        Self {
            base: Instant::now(),
            sample_rate: f64::from(sample_rate.max(1)),
            last_frame: 0,
            best: None,
        }
    }

    /// Feed the current audio clock reading.
    pub fn observe(&mut self, frame: u64, now: Instant) {
        if frame == self.last_frame && self.best.is_some() {
            return;
        }
        self.last_frame = frame;
        let offset = now.duration_since(self.base).as_secs_f64() - frame as f64 / self.sample_rate;
        match self.best {
            Some((best, stamp)) if offset >= best && now.duration_since(stamp) < WINDOW => {}
            _ => self.best = Some((offset, now)),
        }
    }

    /// Audio clock frame rendered at `at` (inverse of [`ClockMap::instant_of`]).
    pub fn frame_of(&self, at: Instant) -> u64 {
        let offset = self.best.map(|b| b.0).unwrap_or(0.0);
        let secs = if at >= self.base {
            at.duration_since(self.base).as_secs_f64()
        } else {
            -self.base.duration_since(at).as_secs_f64()
        };
        ((secs - offset) * self.sample_rate).round().max(0.0) as u64
    }

    /// Wall-clock instant at which `frame` is (or was) rendered.
    pub fn instant_of(&self, frame: u64) -> Instant {
        let offset = self.best.map(|b| b.0).unwrap_or(0.0);
        let secs = (offset + frame as f64 / self.sample_rate).max(0.0);
        self.base + Duration::from_secs_f64(secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earliest_observation_wins() {
        let mut m = ClockMap::new(48_000);
        let t0 = m.base + Duration::from_millis(100);
        m.observe(4800, t0 + Duration::from_millis(4)); // late reading
        m.observe(9600, t0 + Duration::from_millis(100)); // on time: frame 9600 == t0 + 100 ms
        m.observe(14_400, t0 + Duration::from_millis(203)); // late again, ignored
        let at = m.instant_of(48_000);
        let expect = t0 + Duration::from_millis(900);
        let diff = if at > expect {
            at - expect
        } else {
            expect - at
        };
        assert!(diff < Duration::from_micros(50), "{diff:?}");
        assert!(m.frame_of(expect).abs_diff(48_000) <= 2);
        assert_eq!(m.frame_of(m.instant_of(123_456)), 123_456);
    }
}
