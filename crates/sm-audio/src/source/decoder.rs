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
    Ok(Box::new(super::symph::SymDecoder::open(path)?))
}
