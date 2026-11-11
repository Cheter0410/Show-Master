use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// Supported SMPTE frame rates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum FrameRate {
    #[serde(rename = "23.976")]
    F23976,
    #[serde(rename = "24")]
    F24,
    #[default]
    #[serde(rename = "25")]
    F25,
    /// 29.97 fps, drop-frame labelling.
    #[serde(rename = "29.97df")]
    F2997Df,
    /// 29.97 fps, non-drop labelling.
    #[serde(rename = "29.97")]
    F2997Ndf,
    #[serde(rename = "30")]
    F30,
}

impl FrameRate {
    pub const ALL: [FrameRate; 6] = [
        FrameRate::F23976,
        FrameRate::F24,
        FrameRate::F25,
        FrameRate::F2997Df,
        FrameRate::F2997Ndf,
        FrameRate::F30,
    ];

    /// Nominal (label) frames per second: the count base of the FF field.
    pub fn nominal(self) -> u32 {
        match self {
            FrameRate::F23976 | FrameRate::F24 => 24,
            FrameRate::F25 => 25,
            FrameRate::F2997Df | FrameRate::F2997Ndf | FrameRate::F30 => 30,
        }
    }

    /// Real frames per second as an exact rational `(num, den)`.
    pub fn rational(self) -> (u64, u64) {
        match self {
            FrameRate::F23976 => (24000, 1001),
            FrameRate::F24 => (24, 1),
            FrameRate::F25 => (25, 1),
            FrameRate::F2997Df | FrameRate::F2997Ndf => (30000, 1001),
            FrameRate::F30 => (30, 1),
        }
    }

    pub fn drop_frame(self) -> bool {
        matches!(self, FrameRate::F2997Df)
    }

    /// MTC rate code (bits 5-6 of the hours byte): 0=24, 1=25, 2=30 drop, 3=30.
    pub fn mtc_rate_code(self) -> u8 {
        match self {
            FrameRate::F23976 | FrameRate::F24 => 0,
            FrameRate::F25 => 1,
            FrameRate::F2997Df => 2,
            FrameRate::F2997Ndf | FrameRate::F30 => 3,
        }
    }

    /// Number of frame labels in 24 hours (timecode wraps after this many).
    pub fn frames_per_day(self) -> u64 {
        if self.drop_frame() {
            2_589_408
        } else {
            u64::from(self.nominal()) * 86_400
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FrameRate::F23976 => "23.976",
            FrameRate::F24 => "24",
            FrameRate::F25 => "25",
            FrameRate::F2997Df => "29.97df",
            FrameRate::F2997Ndf => "29.97",
            FrameRate::F30 => "30",
        }
    }
}

/// A timecode label `HH:MM:SS:FF`. Serialised as a string; drop-frame values
/// may be written with `;` before the frames but the rate is always supplied
/// by context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Timecode {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
}

impl Timecode {
    pub fn new(hours: u8, minutes: u8, seconds: u8, frames: u8) -> Self {
        Self {
            hours,
            minutes,
            seconds,
            frames,
        }
    }

    /// Convert a running frame number into a label. Wraps at 24 h.
    pub fn from_frame_number(n: u64, rate: FrameRate) -> Self {
        let mut n = n % rate.frames_per_day();
        if rate.drop_frame() {
            let d = n / 17_982;
            let m = n % 17_982;
            n += 18 * d;
            if m > 1 {
                n += 2 * ((m - 2) / 1_798);
            }
        }
        let fps = u64::from(rate.nominal());
        Self {
            frames: (n % fps) as u8,
            seconds: ((n / fps) % 60) as u8,
            minutes: ((n / (fps * 60)) % 60) as u8,
            hours: ((n / (fps * 3600)) % 24) as u8,
        }
    }

    /// Convert a label into a running frame number (0 at 00:00:00:00).
    /// Non-existent drop-frame labels map onto the next valid frame.
    pub fn to_frame_number(self, rate: FrameRate) -> u64 {
        let fps = u64::from(rate.nominal());
        let h = u64::from(self.hours % 24);
        let m = u64::from(self.minutes.min(59));
        let s = u64::from(self.seconds.min(59));
        let mut f = u64::from(self.frames).min(fps - 1);
        if rate.drop_frame() && s == 0 && m % 10 != 0 && f < 2 {
            f = 2;
        }
        let mut n = fps * (3600 * h + 60 * m + s) + f;
        if rate.drop_frame() {
            let total_min = 60 * h + m;
            n -= 2 * (total_min - total_min / 10);
        }
        n % rate.frames_per_day()
    }

    /// Format with the conventional separator (`;` before frames for drop-frame).
    pub fn format(self, rate: FrameRate) -> String {
        let sep = if rate.drop_frame() { ';' } else { ':' };
        format!(
            "{:02}:{:02}:{:02}{}{:02}",
            self.hours, self.minutes, self.seconds, sep, self.frames
        )
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02}:{:02}:{:02}:{:02}",
            self.hours, self.minutes, self.seconds, self.frames
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimecodeParseError(pub String);

impl fmt::Display for TimecodeParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid timecode '{}', expected HH:MM:SS:FF", self.0)
    }
}

impl std::error::Error for TimecodeParseError {}

impl FromStr for Timecode {
    type Err = TimecodeParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || TimecodeParseError(s.to_string());
        let parts: Vec<&str> = s.trim().split([':', ';', '.']).collect();
        if parts.len() != 4 {
            return Err(err());
        }
        let mut v = [0u8; 4];
        for (slot, p) in v.iter_mut().zip(&parts) {
            *slot = p.parse::<u8>().map_err(|_| err())?;
        }
        if v[0] > 23 || v[1] > 59 || v[2] > 59 || v[3] > 29 {
            return Err(err());
        }
        Ok(Timecode::new(v[0], v[1], v[2], v[3]))
    }
}

impl Serialize for Timecode {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Timecode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// State of a timecode generator, anchored to the audio sample clock.
///
/// From `anchor_sample` onwards the generator shows frame `anchor_frame`
/// and, if `running`, advances at exactly `rate` relative to the sample
/// clock. A new transport value replaces the old one at its `anchor_sample`
/// (start, stop, pause and relocation are all expressed this way).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TcTransport {
    pub rate: FrameRate,
    pub running: bool,
    pub anchor_sample: u64,
    pub anchor_frame: u64,
}

impl TcTransport {
    pub fn stopped(rate: FrameRate) -> Self {
        Self {
            rate,
            running: false,
            anchor_sample: 0,
            anchor_frame: 0,
        }
    }

    /// Frames elapsed since the anchor at `sample` (not wrapped).
    pub fn frames_elapsed(&self, sample: u64, sample_rate: u32) -> u64 {
        if !self.running || sample <= self.anchor_sample || sample_rate == 0 {
            return 0;
        }
        let (num, den) = self.rate.rational();
        let d = u128::from(sample - self.anchor_sample);
        (d * u128::from(num) / (u128::from(sample_rate) * u128::from(den))) as u64
    }

    /// Frame number shown at `sample` (wrapped at 24 h).
    pub fn frame_at(&self, sample: u64, sample_rate: u32) -> u64 {
        (self.anchor_frame + self.frames_elapsed(sample, sample_rate)) % self.rate.frames_per_day()
    }

    pub fn timecode_at(&self, sample: u64, sample_rate: u32) -> Timecode {
        Timecode::from_frame_number(self.frame_at(sample, sample_rate), self.rate)
    }

    /// First sample at which `frames_elapsed() >= k`. `None` when stopped.
    pub fn sample_after_frames(&self, k: u64, sample_rate: u32) -> Option<u64> {
        if !self.running {
            return None;
        }
        let (num, den) = self.rate.rational();
        let n = u128::from(k) * u128::from(sample_rate) * u128::from(den);
        let s = n.div_ceil(u128::from(num));
        Some(self.anchor_sample + s as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_rates() {
        for rate in FrameRate::ALL {
            let day = rate.frames_per_day();
            let mut n = 0u64;
            while n < day {
                let tc = Timecode::from_frame_number(n, rate);
                assert_eq!(tc.to_frame_number(rate), n, "{rate:?} {tc}");
                n += 997;
            }
            let last = Timecode::from_frame_number(day - 1, rate);
            assert_eq!((last.hours, last.minutes, last.seconds), (23, 59, 59));
        }
    }

    #[test]
    fn drop_frame_skips_labels() {
        let r = FrameRate::F2997Df;
        assert_eq!(
            Timecode::from_frame_number(1799, r),
            Timecode::new(0, 0, 59, 29)
        );
        assert_eq!(
            Timecode::from_frame_number(1800, r),
            Timecode::new(0, 1, 0, 2)
        );
        assert_eq!(
            Timecode::from_frame_number(17_982, r),
            Timecode::new(0, 10, 0, 0)
        );
        assert_eq!(Timecode::new(0, 1, 0, 0).to_frame_number(r), 1800);
    }

    #[test]
    fn parse_and_serde() {
        let tc: Timecode = "01:02:03;04".parse().unwrap();
        assert_eq!(tc, Timecode::new(1, 2, 3, 4));
        assert!("1:2:3".parse::<Timecode>().is_err());
        let json = serde_json::to_string(&tc).unwrap();
        assert_eq!(json, "\"01:02:03:04\"");
        assert_eq!(serde_json::from_str::<Timecode>(&json).unwrap(), tc);
        assert_eq!(
            serde_json::to_string(&FrameRate::F2997Df).unwrap(),
            "\"29.97df\""
        );
    }

    #[test]
    fn transport_is_exact() {
        let t = TcTransport {
            rate: FrameRate::F2997Df,
            running: true,
            anchor_sample: 1000,
            anchor_frame: 10,
        };
        // One hour of samples at 48k = 172_800_000 samples -> 107892 frames at 30000/1001.
        assert_eq!(t.frames_elapsed(1000 + 172_800_000, 48_000), 107_892);
        for k in [0u64, 1, 2, 1000, 107_892] {
            let s = t.sample_after_frames(k, 48_000).unwrap();
            assert_eq!(t.frames_elapsed(s, 48_000), k);
            if k > 0 {
                assert_eq!(t.frames_elapsed(s - 1, 48_000), k - 1);
            }
        }
    }
}
