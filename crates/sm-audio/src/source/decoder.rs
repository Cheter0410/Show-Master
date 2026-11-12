//! Internal decoder abstraction: frames at the file's own sample rate.

use std::path::Path;

use crate::types::AudioError;

pub(crate) trait Decoder: Send {
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> usize;
    /// Total frames in the file, if known.
    fn frames(&self) -> Option<u64>;
    fn codec(&self) -> String;
    /// Position the decoder so the next `read` starts exactly at `frame`.
    fn seek(&mut self, frame: u64) -> Result<(), AudioError>;
    /// Decode up to `out.len() / channels` interleaved frames; 0 = end.
    fn read(&mut self, out: &mut [f32]) -> Result<usize, AudioError>;
}

/// Open `path` with the native WAV reader when possible (any channel count,
/// any channel mask, RF64), otherwise with symphonia.
pub(crate) fn open_decoder(path: &Path) -> Result<Box<dyn Decoder>, AudioError> {
    if let Some(wav) = super::wav::WavDecoder::open(path)? {
        return Ok(Box::new(wav));
    }
    let sym = super::symph::SymDecoder::open(path)?;
    if sym.codec() == "aac" {
        if let Some(g) = super::mp4_gapless::read(path, sym.sample_rate()) {
            return Ok(Box::new(Trimmed::new(Box::new(sym), g.delay, g.frames)?));
        }
    }
    Ok(Box::new(sym))
}

/// Hides the encoder delay at the start and the padding at the end
/// (AAC in MP4, see `mp4_gapless`): frame 0 is the first real frame.
struct Trimmed {
    inner: Box<dyn Decoder>,
    delay: u64,
    frames: Option<u64>,
    pos: u64,
}

impl Trimmed {
    fn new(
        mut inner: Box<dyn Decoder>,
        delay: u64,
        frames: Option<u64>,
    ) -> Result<Self, AudioError> {
        inner.seek(delay)?;
        let frames = frames.or_else(|| inner.frames().map(|f| f.saturating_sub(delay)));
        Ok(Self {
            inner,
            delay,
            frames,
            pos: 0,
        })
    }
}

impl Decoder for Trimmed {
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn channels(&self) -> usize {
        self.inner.channels()
    }

    fn frames(&self) -> Option<u64> {
        self.frames
    }

    fn codec(&self) -> String {
        self.inner.codec()
    }

    fn seek(&mut self, frame: u64) -> Result<(), AudioError> {
        self.inner.seek(frame + self.delay)?;
        self.pos = frame;
        Ok(())
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, AudioError> {
        let ch = self.channels().max(1);
        let mut want = out.len() / ch;
        if let Some(total) = self.frames {
            want = want.min(total.saturating_sub(self.pos) as usize);
        }
        if want == 0 {
            return Ok(0);
        }
        let n = self.inner.read(&mut out[..want * ch])?;
        self.pos += n as u64;
        Ok(n)
    }
}
