//! Gain ramp used for every gain in the real-time path.

use crate::types::FadeCurve;

/// A gain that only ever changes through a ramp.
///
/// `pos` counts frames since the ramp started; the gain applied to the frame
/// at `pos` is `from + (to - from) * shape(pos / len)`, so the frame at
/// `pos == len` (and every later one) gets exactly `to`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ramp {
    from: f32,
    to: f32,
    cur: f32,
    pos: u64,
    len: u64,
    curve: FadeCurve,
}

impl Ramp {
    pub(crate) fn constant(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            cur: value,
            pos: 0,
            len: 0,
            curve: FadeCurve::Linear,
        }
    }

    /// Current value (gain of the next frame).
    #[inline]
    pub(crate) fn value(&self) -> f32 {
        self.cur
    }

    #[inline]
    pub(crate) fn is_ramping(&self) -> bool {
        self.pos < self.len
    }

    /// Jump to `value` without a ramp.
    #[inline]
    pub(crate) fn jump(&mut self, value: f32) {
        *self = Self::constant(value);
    }

    /// Start a ramp from the current value to `target` over `len` frames.
    #[inline]
    pub(crate) fn set(&mut self, target: f32, len: u64, curve: FadeCurve) {
        if len == 0 {
            self.jump(target);
        } else {
            self.from = self.cur;
            self.to = target;
            self.pos = 0;
            self.len = len;
            self.curve = curve;
        }
    }

    /// Clamp a segment length so it does not run past the end of this ramp.
    #[inline]
    pub(crate) fn limit(&self, n: usize) -> usize {
        if self.is_ramping() {
            let rem = self.len - self.pos;
            if rem < n as u64 {
                return rem as usize;
            }
        }
        n
    }

    /// Advance by `n` frames. Returns the gain at the first frame of the
    /// segment and the gain at the frame just after it; the caller
    /// interpolates linearly in between.
    #[inline]
    pub(crate) fn step(&mut self, n: usize) -> (f32, f32) {
        let start = self.cur;
        if self.is_ramping() {
            self.pos = self.pos.saturating_add(n as u64);
            if self.pos >= self.len {
                self.pos = self.len;
                self.cur = self.to;
            } else {
                let t = (self.pos as f64 / self.len as f64) as f32;
                let k = self.curve.shape(t, self.to >= self.from);
                self.cur = self.from + (self.to - self.from) * k;
            }
        }
        (start, self.cur)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_ramp_hits_endpoints() {
        let mut r = Ramp::constant(0.0);
        r.set(1.0, 100, FadeCurve::Linear);
        let (a, b) = r.step(50);
        assert_eq!(a, 0.0);
        assert!((b - 0.5).abs() < 1e-6);
        assert_eq!(r.limit(64), 50);
        let (_, c) = r.step(50);
        assert_eq!(c, 1.0);
        assert!(!r.is_ramping());
        assert_eq!(r.limit(64), 64);
    }

    #[test]
    fn equal_power_midpoint_both_directions() {
        let mut up = Ramp::constant(0.0);
        up.set(1.0, 1000, FadeCurve::EqualPower);
        let (_, m) = up.step(500);
        assert!((m - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4);
        let mut down = Ramp::constant(1.0);
        down.set(0.0, 1000, FadeCurve::EqualPower);
        let (_, m) = down.step(500);
        assert!((m - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4);
    }

    #[test]
    fn s_curve_is_symmetric() {
        let mut r = Ramp::constant(0.0);
        r.set(1.0, 1000, FadeCurve::SCurve);
        let (_, q) = r.step(250);
        let (_, m) = r.step(250);
        let (_, t) = r.step(250);
        assert!((m - 0.5).abs() < 1e-5);
        assert!((q + t - 1.0).abs() < 1e-5);
        assert!(q < 0.25);
    }
}
