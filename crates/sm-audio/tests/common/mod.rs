//! Helpers shared by the integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sm_audio::{Controller, Renderer, VoiceId};

/// A unique temp directory, removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!(
            "sm-audio-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Write a 16-bit PCM WAV from interleaved i16 samples.
pub fn write_wav_i16(path: &Path, rate: u32, channels: u16, samples: &[i16]) {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for s in samples {
        w.write_sample(*s).unwrap();
    }
    w.finalize().unwrap();
}

/// Write a 24-bit PCM WAV from interleaved samples (24-bit range).
pub fn write_wav_i24(path: &Path, rate: u32, channels: u16, samples: &[i32]) {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 24,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for s in samples {
        w.write_sample(*s).unwrap();
    }
    w.finalize().unwrap();
}

/// Write a 32-bit float WAV from interleaved samples.
pub fn write_wav_f32(path: &Path, rate: u32, channels: u16, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for s in samples {
        w.write_sample(*s).unwrap();
    }
    w.finalize().unwrap();
}

/// Deterministic pseudo-random i16 test signal.
pub fn noise_i16(n: usize, seed: u32) -> Vec<i16> {
    let mut x = seed.wrapping_mul(2654435761).max(1);
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            ((x >> 8) as i16) / 4
        })
        .collect()
}

pub fn i16_to_f32(s: &[i16]) -> Vec<f32> {
    s.iter().map(|v| *v as f32 / 32768.0).collect()
}

pub fn wait_primed(ctl: &Controller, voices: &[VoiceId]) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !voices.iter().all(|v| ctl.is_primed(*v)) {
        assert!(Instant::now() < deadline, "voices did not prime in time");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Wait until the streamer has `frames` ready for every voice.
pub fn wait_buffered(ctl: &Controller, frames: u64) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ctl.all_buffered(frames) {
        assert!(Instant::now() < deadline, "streamer did not keep up");
        std::thread::yield_now();
    }
}

/// Render `frames` frames offline, cycling through `blocks` as block sizes,
/// never outrunning the streamer. Returns interleaved output.
pub fn render(
    ctl: &Controller,
    r: &mut Renderer,
    channels: usize,
    frames: usize,
    blocks: &[usize],
) -> Vec<f32> {
    let mut out = vec![0.0f32; frames * channels];
    let mut done = 0;
    let mut i = 0;
    while done < frames {
        let n = blocks[i % blocks.len()].min(frames - done);
        i += 1;
        wait_buffered(ctl, n as u64);
        r.render(&mut out[done * channels..(done + n) * channels], channels);
        done += n;
    }
    out
}

/// One channel of an interleaved buffer.
pub fn channel(buf: &[f32], channels: usize, c: usize) -> Vec<f32> {
    buf.chunks_exact(channels).map(|f| f[c]).collect()
}
