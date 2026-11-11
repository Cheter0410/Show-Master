//! Decoder for everything that is not plain PCM WAV, built on symphonia
//! (FLAC, MP3, Ogg Vorbis, AIFF, AAC / ALAC-less MP4, exotic WAV).

use std::fs::File;
use std::path::{Path, PathBuf};

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder as SymCodec, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::TimeBase;

use super::decoder::Decoder;
use crate::types::AudioError;

/// Frames decoded and thrown away before a seek target so codecs with
/// inter-packet state (MP3, AAC, Vorbis) have converged when we get there.
const SEEK_PREROLL_FRAMES: u64 = 8192;
/// Consecutive undecodable packets tolerated before giving up.
const MAX_DECODE_ERRORS: u32 = 64;

struct Opened {
    format: Box<dyn FormatReader>,
    codec: Box<dyn SymCodec>,
    track_id: u32,
    sample_rate: Option<u32>,
    channels: Option<usize>,
    frames: Option<u64>,
    time_base: Option<TimeBase>,
    codec_name: String,
}

fn dec_err(e: SymError) -> AudioError {
    match e {
        SymError::IoError(e) => AudioError::Io(e),
        SymError::Unsupported(s) => AudioError::Unsupported(s.to_string()),
        other => AudioError::Decode(other.to_string()),
    }
}

fn open_inner(path: &Path) -> Result<Opened, AudioError> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let format_opts = FormatOptions {
        enable_gapless: true,
        ..Default::default()
    };
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &MetadataOptions::default())
        .map_err(dec_err)?;
    let format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| AudioError::Unsupported("no audio track".into()))?;
    let params = track.codec_params.clone();
    let track_id = track.id;
    let codec = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(dec_err)?;
    let codec_name = symphonia::default::get_codecs()
        .get_codec(params.codec)
        .map(|d| d.short_name.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    Ok(Opened {
        format,
        codec,
        track_id,
        sample_rate: params.sample_rate,
        channels: params.channels.map(|c| c.count()),
        frames: params.n_frames,
        time_base: params.time_base,
        codec_name,
    })
}

pub(crate) struct SymDecoder {
    path: PathBuf,
    format: Box<dyn FormatReader>,
    codec: Box<dyn SymCodec>,
    track_id: u32,
    sample_rate: u32,
    channels: usize,
    frames: Option<u64>,
    time_base: Option<TimeBase>,
    codec_name: String,
    sbuf: Option<SampleBuffer<f32>>,
    /// Unread part of `sbuf`, in samples.
    pend_pos: usize,
    pend_len: usize,
    /// Frames to throw away before delivering (seek fine positioning).
    skip: u64,
    /// After a container seek: the frame we want to land on. Packets are
    /// dropped until the one whose timestamp range contains it.
    seek_target: Option<u64>,
}

impl SymDecoder {
    pub(crate) fn open(path: &Path) -> Result<Self, AudioError> {
        let mut o = open_inner(path)?;
        if o.sample_rate.is_none() || o.channels.is_none() {
            // Some containers only reveal the layout once a packet has been
            // decoded: decode one, then start over.
            let mut tries = 0;
            while tries < MAX_DECODE_ERRORS {
                tries += 1;
                let packet = o.format.next_packet().map_err(dec_err)?;
                if packet.track_id() != o.track_id {
                    continue;
                }
                if let Ok(decoded) = o.codec.decode(&packet) {
                    let spec = *decoded.spec();
                    let (rate, ch) = (spec.rate, spec.channels.count());
                    o = open_inner(path)?;
                    o.sample_rate = Some(rate);
                    o.channels = Some(ch);
                    break;
                }
            }
        }
        let sample_rate = o
            .sample_rate
            .filter(|r| *r > 0)
            .ok_or_else(|| AudioError::Unsupported("unknown sample rate".into()))?;
        let channels = o
            .channels
            .filter(|c| *c > 0)
            .ok_or_else(|| AudioError::Unsupported("unknown channel layout".into()))?;
        Ok(Self {
            path: path.to_path_buf(),
            format: o.format,
            codec: o.codec,
            track_id: o.track_id,
            sample_rate,
            channels,
            frames: o.frames,
            time_base: o.time_base,
            codec_name: o.codec_name,
            sbuf: None,
            pend_pos: 0,
            pend_len: 0,
            skip: 0,
            seek_target: None,
        })
    }

    fn reopen(&mut self) -> Result<(), AudioError> {
        let o = open_inner(&self.path)?;
        self.format = o.format;
        self.codec = o.codec;
        self.track_id = o.track_id;
        self.pend_pos = 0;
        self.pend_len = 0;
        self.seek_target = None;
        Ok(())
    }

    /// Track time base as (numerator, denominator) seconds per tick.
    fn time_base(&self) -> (u128, u128) {
        match self.time_base {
            Some(tb) if tb.numer > 0 && tb.denom > 0 => (tb.numer as u128, tb.denom as u128),
            // Without a time base, timestamps are frames.
            _ => (1, self.sample_rate as u128),
        }
    }

    fn ts_to_frames(&self, ts: u64) -> u64 {
        let (num, den) = self.time_base();
        (ts as u128 * self.sample_rate as u128 * num / den) as u64
    }

    /// Decode the next packet into `sbuf`. `Ok(false)` = end of stream.
    fn next_packet(&mut self) -> Result<bool, AudioError> {
        let mut errors = 0;
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(false)
                }
                Err(SymError::ResetRequired) => return Ok(false),
                Err(e) => return Err(dec_err(e)),
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let packet_at = self.ts_to_frames(packet.ts());
            if self.seek_target.is_some_and(|target| packet_at > target) {
                // The container landed after the target: decode from the
                // top instead (slow but exact).
                let target = self.seek_target.take().unwrap_or(0);
                self.reopen()?;
                self.skip = target;
                continue;
            }
            match self.codec.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    if spec.channels.count() != self.channels {
                        return Err(AudioError::Decode(
                            "channel layout changed mid-stream".into(),
                        ));
                    }
                    let need = decoded.capacity() * self.channels;
                    if self.sbuf.as_ref().is_none_or(|b| b.capacity() < need) {
                        self.sbuf = Some(SampleBuffer::new(decoded.capacity() as u64, spec));
                    }
                    let frames = decoded.frames() as u64;
                    if let Some(target) = self.seek_target {
                        // Position by packet timestamps, not by counting:
                        // the first packets after a decoder reset may
                        // produce fewer frames than they span (Vorbis).
                        if packet_at + frames <= target {
                            continue;
                        }
                        self.seek_target = None;
                        self.skip = target - packet_at;
                    }
                    if let Some(buf) = self.sbuf.as_mut() {
                        buf.copy_interleaved_ref(decoded);
                        self.pend_len = buf.len();
                        self.pend_pos = 0;
                    }
                    return Ok(true);
                }
                Err(SymError::DecodeError(msg)) => {
                    errors += 1;
                    if errors > MAX_DECODE_ERRORS {
                        return Err(AudioError::Decode(msg.to_string()));
                    }
                }
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Ok(false)
                }
                Err(e) => return Err(dec_err(e)),
            }
        }
    }
}

impl Decoder for SymDecoder {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn frames(&self) -> Option<u64> {
        self.frames
    }

    fn codec(&self) -> String {
        self.codec_name.clone()
    }

    fn seek(&mut self, frame: u64) -> Result<(), AudioError> {
        let sr = self.sample_rate as u128;
        let (num, den) = self.time_base();
        let target = frame.saturating_sub(SEEK_PREROLL_FRAMES);
        let ts = (target as u128 * den / (sr * num)) as u64;
        // Start of file: reopening is exact for every format.
        let seeked = target > 0
            && self
                .format
                .seek(
                    SeekMode::Accurate,
                    SeekTo::TimeStamp {
                        ts,
                        track_id: self.track_id,
                    },
                )
                .is_ok();
        self.pend_pos = 0;
        self.pend_len = 0;
        if seeked {
            self.codec.reset();
            self.skip = 0;
            self.seek_target = Some(frame);
        } else {
            self.reopen()?;
            self.seek_target = None;
            self.skip = frame;
        }
        Ok(())
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, AudioError> {
        let ch = self.channels;
        let want = out.len() / ch * ch;
        let mut filled = 0;
        while filled < want {
            if self.pend_pos >= self.pend_len {
                if !self.next_packet()? {
                    break;
                }
                continue;
            }
            let avail = self.pend_len - self.pend_pos;
            if self.skip > 0 {
                let s = (self.skip.min((avail / ch) as u64)) as usize;
                self.pend_pos += s * ch;
                self.skip -= s as u64;
                continue;
            }
            let n = avail.min(want - filled);
            if let Some(buf) = self.sbuf.as_ref() {
                out[filled..filled + n]
                    .copy_from_slice(&buf.samples()[self.pend_pos..self.pend_pos + n]);
            }
            self.pend_pos += n;
            filled += n;
        }
        Ok(filled / ch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The symphonia path on a WAV file (normally handled natively), so the
    /// generic packet / seek logic is covered without external encoders.
    #[test]
    fn symphonia_wav_decode_and_seek() {
        let path = std::env::temp_dir().join(format!("sm-audio-symph-{}.wav", std::process::id()));
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let samples: Vec<i16> = (0..2 * 50_000)
            .map(|i| ((i * 7919) % 30_000 - 15_000) as i16)
            .collect();
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for s in &samples {
            w.write_sample(*s).unwrap();
        }
        w.finalize().unwrap();
        let expect: Vec<f32> = samples.iter().map(|s| *s as f32 / 32768.0).collect();

        let mut d = SymDecoder::open(&path).unwrap();
        assert_eq!(d.sample_rate(), 44_100);
        assert_eq!(d.channels(), 2);
        assert_eq!(d.frames(), Some(50_000));
        let mut buf = vec![0.0f32; 2 * 50_001];
        assert_eq!(d.read(&mut buf).unwrap(), 50_000);
        assert_eq!(&buf[..100_000], &expect[..]);
        assert_eq!(d.read(&mut buf).unwrap(), 0);

        for target in [33_333u64, 5, 0, 49_999, 20_000] {
            d.seek(target).unwrap();
            let mut b = vec![0.0f32; 2 * 777];
            let n = d.read(&mut b).unwrap();
            let want = (50_000 - target as usize).min(777);
            assert_eq!(n, want, "seek {target}");
            let at = target as usize * 2;
            assert_eq!(&b[..n * 2], &expect[at..at + n * 2], "seek {target}");
        }
        let _ = std::fs::remove_file(&path);
    }
}
