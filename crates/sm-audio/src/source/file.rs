//! File playback source: region (start / end), loops, load-to-time offset
//! and sample-rate conversion to the engine rate.

use std::path::Path;

use rubato::{FftFixedIn, Resampler as _};
use serde::{Deserialize, Serialize};

use super::decoder::{open_decoder, Decoder};
use super::SampleSource;
use crate::types::AudioError;

/// Input chunk of the resampler, in source frames.
const RESAMPLE_CHUNK: usize = 1024;

/// Basic facts about a media file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaInfo {
    /// Native sample rate of the file.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: usize,
    /// Length in frames at the native rate, if the container knows it.
    pub frames: Option<u64>,
    /// `frames / sample_rate`.
    pub duration_seconds: Option<f64>,
    /// Short codec name, e.g. `pcm_s24le`, `flac`, `mp3`.
    pub codec: String,
}

/// Read the header of `path`.
pub fn probe(path: impl AsRef<Path>) -> Result<MediaInfo, AudioError> {
    let dec = open_decoder(path.as_ref())?;
    let frames = dec.frames();
    Ok(MediaInfo {
        sample_rate: dec.sample_rate(),
        channels: dec.channels(),
        frames,
        duration_seconds: frames.map(|f| f as f64 / dec.sample_rate() as f64),
        codec: dec.codec(),
    })
}

/// Decode the whole file and reduce it to `buckets` `(min, max)` pairs for
/// a waveform display. Min / max are taken over **all channels** of the
/// frames in a bucket. Blocking; run it on a worker thread.
pub fn waveform_peaks(
    path: impl AsRef<Path>,
    buckets: usize,
) -> Result<Vec<(f32, f32)>, AudioError> {
    let path = path.as_ref();
    if buckets == 0 {
        return Ok(Vec::new());
    }
    let mut dec = open_decoder(path)?;
    let ch = dec.channels();
    let mut buf = vec![0.0f32; 8192 * ch];
    let total = match dec.frames() {
        Some(f) => f,
        None => {
            // Length unknown: count frames with a first decoding pass.
            let mut n = 0u64;
            loop {
                let got = dec.read(&mut buf)?;
                if got == 0 {
                    break;
                }
                n += got as u64;
            }
            dec = open_decoder(path)?;
            n
        }
    };
    let mut out = vec![(0.0f32, 0.0f32); buckets];
    if total == 0 {
        return Ok(out);
    }
    let mut touched = vec![false; buckets];
    let mut frame = 0u64;
    loop {
        let got = dec.read(&mut buf)?;
        if got == 0 {
            break;
        }
        for f in buf[..got * ch].chunks_exact(ch) {
            let b = ((frame as u128 * buckets as u128 / total as u128) as usize).min(buckets - 1);
            let (lo, hi) = &mut out[b];
            if !touched[b] {
                touched[b] = true;
                *lo = f32::INFINITY;
                *hi = f32::NEG_INFINITY;
            }
            for &s in f {
                *lo = lo.min(s);
                *hi = hi.max(s);
            }
            frame += 1;
        }
    }
    // More buckets than frames: repeat the previous value.
    let mut prev = (0.0, 0.0);
    for (o, t) in out.iter_mut().zip(&touched) {
        if *t {
            prev = *o;
        } else {
            *o = prev;
        }
    }
    Ok(out)
}

/// How to play a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileSourceOpts {
    /// Sample rate the engine runs at; the file is resampled when its own
    /// rate differs.
    pub engine_sample_rate: u32,
    /// Region start in seconds of file time.
    pub start_seconds: f64,
    /// Region end in seconds of file time (`None` = end of file).
    pub end_seconds: Option<f64>,
    /// Region start in source frames; overrides `start_seconds` when set.
    pub start_frame: Option<u64>,
    /// Region end (exclusive) in source frames; overrides `end_seconds`.
    pub end_frame: Option<u64>,
    /// How many times the region plays: 1 = once, 0 = forever.
    pub loop_count: u32,
    /// Initial position inside the (looped) region in seconds of file time,
    /// for "load to time". Offsets beyond one pass consume whole loops.
    pub offset_seconds: f64,
}

impl FileSourceOpts {
    /// Whole file, once, from the top.
    pub fn new(engine_sample_rate: u32) -> Self {
        Self {
            engine_sample_rate,
            start_seconds: 0.0,
            end_seconds: None,
            start_frame: None,
            end_frame: None,
            loop_count: 1,
            offset_seconds: 0.0,
        }
    }
}

impl Default for FileSourceOpts {
    fn default() -> Self {
        Self::new(48_000)
    }
}

fn seconds_to_frames(seconds: f64, rate: u32) -> u64 {
    if seconds.is_finite() && seconds > 0.0 {
        (seconds * rate as f64).round() as u64
    } else {
        0
    }
}

/// Region + loop logic at the source rate.
struct Region {
    dec: Box<dyn Decoder>,
    start: u64,
    end: Option<u64>,
    pos: u64,
    /// Passes left including the current one; `None` = infinite.
    loops_left: Option<u32>,
    done: bool,
}

impl Region {
    fn read(&mut self, buf: &mut [f32]) -> Result<usize, AudioError> {
        let ch = self.dec.channels();
        let want = buf.len() / ch;
        let mut filled = 0;
        while filled < want && !self.done {
            let room = want - filled;
            let limit = match self.end {
                Some(e) => (e.saturating_sub(self.pos)).min(room as u64) as usize,
                None => room,
            };
            let n = if limit > 0 {
                self.dec
                    .read(&mut buf[filled * ch..(filled + limit) * ch])?
            } else {
                0
            };
            if n > 0 {
                self.pos += n as u64;
                filled += n;
                continue;
            }
            // End of the region (or the file turned out shorter).
            if self.end.is_none_or(|e| self.pos < e) {
                self.end = Some(self.pos);
            }
            if self.pos <= self.start {
                self.done = true; // empty region: never spin
                break;
            }
            match self.loops_left {
                Some(0) | Some(1) => self.done = true,
                more => {
                    self.loops_left = more.map(|k| k - 1);
                    self.dec.seek(self.start)?;
                    self.pos = self.start;
                }
            }
        }
        Ok(filled)
    }
}

/// Streaming resampler stage (source rate → engine rate), delay-compensated
/// so the first output frame corresponds to the first input frame and the
/// output length is exactly `ceil(input_frames * out_rate / in_rate)`.
struct Resampled {
    rs: FftFixedIn<f32>,
    rate_in: u64,
    rate_out: u64,
    channels: usize,
    tmp: Vec<f32>,
    inbuf: Vec<Vec<f32>>,
    outbuf: Vec<Vec<f32>>,
    out_pos: usize,
    out_len: usize,
    skip: usize,
    in_total: u64,
    in_done: bool,
    emitted: u64,
    finished: bool,
}

impl Resampled {
    fn new(rate_in: u32, rate_out: u32, channels: usize) -> Result<Self, AudioError> {
        let rs = FftFixedIn::<f32>::new(
            rate_in as usize,
            rate_out as usize,
            RESAMPLE_CHUNK,
            2,
            channels,
        )
        .map_err(|e| AudioError::Unsupported(format!("resampler: {e}")))?;
        let outbuf = rs.output_buffer_allocate(true);
        let skip = rs.output_delay();
        Ok(Self {
            rs,
            rate_in: rate_in as u64,
            rate_out: rate_out as u64,
            channels,
            tmp: Vec::new(),
            inbuf: vec![Vec::new(); channels],
            outbuf,
            out_pos: 0,
            out_len: 0,
            skip,
            in_total: 0,
            in_done: false,
            emitted: 0,
            finished: false,
        })
    }

    fn out_frames_for(&self, in_frames: u64) -> u64 {
        (in_frames as u128 * self.rate_out as u128).div_ceil(self.rate_in as u128) as u64
    }

    /// Run the resampler once. Returns false when the stream is complete.
    fn refill(&mut self, region: &mut Region) -> Result<bool, AudioError> {
        if self.finished {
            return Ok(false);
        }
        let ch = self.channels;
        let need = self.rs.input_frames_next();
        self.tmp.resize(need * ch, 0.0);
        let mut got = 0;
        while !self.in_done && got < need {
            let n = region.read(&mut self.tmp[got * ch..])?;
            if n == 0 {
                self.in_done = true;
            }
            got += n;
        }
        self.in_total += got as u64;
        for (c, plane) in self.inbuf.iter_mut().enumerate() {
            plane.clear();
            plane.extend(self.tmp[..got * ch].chunks_exact(ch).map(|f| f[c]));
            plane.resize(need, 0.0); // zero padding flushes the tail
        }
        let (_, produced) = self
            .rs
            .process_into_buffer(&self.inbuf, &mut self.outbuf, None)
            .map_err(|e| AudioError::Decode(format!("resampler: {e}")))?;
        let drop = self.skip.min(produced);
        self.skip -= drop;
        self.out_pos = drop;
        self.out_len = produced;
        if self.in_done {
            let remaining = self
                .out_frames_for(self.in_total)
                .saturating_sub(self.emitted);
            if (self.out_len - self.out_pos) as u64 >= remaining {
                self.out_len = self.out_pos + remaining as usize;
                self.finished = true;
            }
        }
        Ok(true)
    }

    fn read(&mut self, region: &mut Region, buf: &mut [f32]) -> Result<usize, AudioError> {
        let ch = self.channels;
        let want = buf.len() / ch;
        let mut filled = 0;
        while filled < want {
            if self.out_pos >= self.out_len {
                if !self.refill(region)? {
                    break;
                }
                continue;
            }
            let n = (self.out_len - self.out_pos).min(want - filled);
            for (c, plane) in self.outbuf.iter().enumerate() {
                let src = &plane[self.out_pos..self.out_pos + n];
                for (i, s) in src.iter().enumerate() {
                    buf[(filled + i) * ch + c] = *s;
                }
            }
            self.out_pos += n;
            self.emitted += n as u64;
            filled += n;
        }
        Ok(filled)
    }
}

/// A [`SampleSource`] that streams an audio file.
///
/// * decodes WAV (native reader: PCM 8/16/24/32-bit, float 32/64, any
///   channel count, RF64) and, through symphonia, FLAC, MP3, Ogg Vorbis,
///   AIFF and AAC/MP4;
/// * honours the start / end points sample-accurately at the source rate;
/// * loops seamlessly inside the source, so the engine sees one continuous
///   stream;
/// * resamples to the engine rate when needed (the resampler runs across
///   loop boundaries, so loops stay seamless).
pub struct FileSource {
    region: Region,
    resampler: Option<Resampled>,
    channels: usize,
    total: Option<u64>,
    info: MediaInfo,
    error: Option<String>,
    reported: bool,
}

impl FileSource {
    /// Open `path` for playback with `opts`.
    pub fn open(path: impl AsRef<Path>, opts: FileSourceOpts) -> Result<Self, AudioError> {
        if opts.engine_sample_rate == 0 {
            return Err(AudioError::InvalidParam("engine sample rate is 0".into()));
        }
        let mut dec = open_decoder(path.as_ref())?;
        let rate = dec.sample_rate();
        let channels = dec.channels();
        let file_frames = dec.frames();
        let info = MediaInfo {
            sample_rate: rate,
            channels,
            frames: file_frames,
            duration_seconds: file_frames.map(|f| f as f64 / rate as f64),
            codec: dec.codec(),
        };

        let mut start = opts
            .start_frame
            .unwrap_or_else(|| seconds_to_frames(opts.start_seconds, rate));
        let mut end = opts
            .end_frame
            .or_else(|| opts.end_seconds.map(|s| seconds_to_frames(s, rate)));
        end = match (end, file_frames) {
            (Some(e), Some(f)) => Some(e.min(f)),
            (None, f) => f,
            (e, None) => e,
        };
        if let Some(e) = end {
            start = start.min(e);
        }

        let mut loops_left = if opts.loop_count == 0 {
            None
        } else {
            Some(opts.loop_count)
        };
        let mut offset = seconds_to_frames(opts.offset_seconds, rate);
        let mut done = false;
        if let Some(len) = end.map(|e| e - start) {
            match offset.checked_div(len) {
                // empty region
                None => {
                    done = true;
                    offset = 0;
                }
                Some(skipped_loops) => {
                    offset %= len;
                    if let Some(l) = loops_left {
                        if skipped_loops >= l as u64 {
                            done = true;
                            offset = 0;
                        } else {
                            loops_left = Some(l - skipped_loops as u32);
                        }
                    }
                }
            }
        }
        let pos = start + offset;
        if pos != 0 && !done {
            dec.seek(pos)?;
        }

        let src_total = match (end, loops_left) {
            _ if done => Some(0),
            (Some(e), Some(l)) => Some((e - start) * l as u64 - offset),
            _ => None,
        };
        let region = Region {
            dec,
            start,
            end,
            pos,
            loops_left,
            done,
        };
        let resampler = if rate != opts.engine_sample_rate {
            Some(Resampled::new(rate, opts.engine_sample_rate, channels)?)
        } else {
            None
        };
        let total = match &resampler {
            Some(r) => src_total.map(|t| r.out_frames_for(t)),
            None => src_total,
        };
        Ok(Self {
            region,
            resampler,
            channels,
            total,
            info,
            error: None,
            reported: false,
        })
    }

    /// Facts about the underlying file (native rate, not engine rate).
    pub fn info(&self) -> &MediaInfo {
        &self.info
    }
}

impl SampleSource for FileSource {
    fn channels(&self) -> usize {
        self.channels
    }

    fn total_frames(&self) -> Option<u64> {
        self.total
    }

    fn read(&mut self, buf: &mut [f32]) -> usize {
        if self.error.is_some() {
            return 0;
        }
        let result = match self.resampler.as_mut() {
            Some(r) => r.read(&mut self.region, buf),
            None => self.region.read(buf),
        };
        match result {
            Ok(n) => n,
            Err(e) => {
                self.error = Some(e.to_string());
                self.region.done = true;
                0
            }
        }
    }

    fn take_error(&mut self) -> Option<String> {
        // Keep `error` set so later reads stay at end-of-stream.
        if self.reported {
            return None;
        }
        self.reported = self.error.is_some();
        self.error.clone()
    }
}
