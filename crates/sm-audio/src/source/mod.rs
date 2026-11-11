//! Sample sources: where a voice's audio comes from.

mod decoder;
mod file;
mod symph;
mod wav;

pub use file::{probe, waveform_peaks, FileSource, FileSourceOpts, MediaInfo};

/// A stream of interleaved f32 frames **at the engine sample rate**.
///
/// Sources are pulled by the streamer thread (never by the audio callback),
/// so they may block, allocate and do I/O. A source that blocks for long
/// stalls streaming for every voice and will cause underruns.
pub trait SampleSource: Send {
    /// Number of channels of the stream (constant).
    fn channels(&self) -> usize;

    /// Total length in frames at engine rate, if known and finite.
    fn total_frames(&self) -> Option<u64>;

    /// Read up to `buf.len() / channels` frames into `buf` (interleaved).
    /// Returns the number of frames written; `0` means end of stream.
    fn read(&mut self, buf: &mut [f32]) -> usize;

    /// If the stream ended because of an error, the message (once). The
    /// streamer calls this after `read` returned 0 and reports it as
    /// [`crate::AudioEvent::SourceError`].
    fn take_error(&mut self) -> Option<String> {
        None
    }
}

/// An in-memory source (tests, short samples, generated signals).
#[derive(Debug, Clone)]
pub struct MemorySource {
    samples: Vec<f32>,
    channels: usize,
    pos: usize,
    /// Remaining passes; `None` = infinite.
    loops_left: Option<u32>,
}

impl MemorySource {
    /// `samples` is interleaved with `channels` channels and is assumed to
    /// be at the engine sample rate. A trailing partial frame is ignored.
    pub fn new(samples: Vec<f32>, channels: usize) -> Self {
        let channels = channels.max(1);
        let mut samples = samples;
        samples.truncate(samples.len() / channels * channels);
        Self {
            samples,
            channels,
            pos: 0,
            loops_left: Some(1),
        }
    }

    /// Play the buffer `loop_count` times (0 = forever, 1 = once).
    pub fn with_loops(mut self, loop_count: u32) -> Self {
        self.loops_left = if loop_count == 0 {
            None
        } else {
            Some(loop_count)
        };
        self
    }
}

impl SampleSource for MemorySource {
    fn channels(&self) -> usize {
        self.channels
    }

    fn total_frames(&self) -> Option<u64> {
        let frames = (self.samples.len() / self.channels) as u64;
        self.loops_left.map(|l| frames * l as u64)
    }

    fn read(&mut self, buf: &mut [f32]) -> usize {
        let want = buf.len() / self.channels * self.channels;
        let mut filled = 0;
        if self.samples.is_empty() {
            return 0;
        }
        while filled < want {
            if self.pos >= self.samples.len() {
                match self.loops_left {
                    Some(0) | Some(1) => {
                        self.loops_left = Some(0);
                        break;
                    }
                    Some(n) => self.loops_left = Some(n - 1),
                    None => {}
                }
                self.pos = 0;
            }
            let n = (want - filled).min(self.samples.len() - self.pos);
            buf[filled..filled + n].copy_from_slice(&self.samples[self.pos..self.pos + n]);
            self.pos += n;
            filled += n;
        }
        filled / self.channels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_source_loops() {
        let mut s = MemorySource::new(vec![1.0, 2.0, 3.0], 1).with_loops(2);
        assert_eq!(s.total_frames(), Some(6));
        let mut buf = [0.0; 4];
        assert_eq!(s.read(&mut buf), 4);
        assert_eq!(buf, [1.0, 2.0, 3.0, 1.0]);
        assert_eq!(s.read(&mut buf), 2);
        assert_eq!(&buf[..2], &[2.0, 3.0]);
        assert_eq!(s.read(&mut buf), 0);
    }
}
