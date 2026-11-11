//! Minimal, strict-enough RIFF/WAVE (and RF64 / BW64) PCM reader.
//!
//! Show audio is overwhelmingly PCM WAV, often with many channels and a
//! zero or unusual channel mask (DAW "direct out" exports). Handling it
//! natively keeps those files independent of codec-library quirks and makes
//! seeking trivially sample-accurate. Anything else (ADPCM, A-law, …)
//! returns `None` and is left to symphonia.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use super::decoder::Decoder;
use crate::types::AudioError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    U8,
    I16,
    I24,
    I32,
    F32,
    F64,
}

impl Format {
    fn bytes(self) -> usize {
        match self {
            Format::U8 => 1,
            Format::I16 => 2,
            Format::I24 => 3,
            Format::I32 | Format::F32 => 4,
            Format::F64 => 8,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Format::U8 => "pcm_u8",
            Format::I16 => "pcm_s16le",
            Format::I24 => "pcm_s24le",
            Format::I32 => "pcm_s32le",
            Format::F32 => "pcm_f32le",
            Format::F64 => "pcm_f64le",
        }
    }
}

pub(crate) struct WavDecoder {
    reader: BufReader<File>,
    data_start: u64,
    frames: u64,
    pos: u64,
    sample_rate: u32,
    channels: usize,
    format: Format,
    bytes: Vec<u8>,
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn u64_at(b: &[u8], i: usize) -> u64 {
    (u32_at(b, i) as u64) | ((u32_at(b, i + 4) as u64) << 32)
}

impl WavDecoder {
    /// `Ok(None)`: not a WAV file, or a WAV flavour this reader does not
    /// handle.
    pub(crate) fn open(path: &Path) -> Result<Option<Self>, AudioError> {
        let file = File::open(path)?;
        let file_len = file.metadata()?.len();
        let mut reader = BufReader::with_capacity(64 * 1024, file);
        let mut head = [0u8; 12];
        if reader.read_exact(&mut head).is_err() {
            return Ok(None);
        }
        let rf64 = matches!(&head[0..4], b"RF64" | b"BW64");
        if !(rf64 || &head[0..4] == b"RIFF") || &head[8..12] != b"WAVE" {
            return Ok(None);
        }

        let mut pos = 12u64;
        let mut ds64_data: Option<u64> = None;
        let mut fmt: Option<(Format, usize, u32, usize)> = None;
        loop {
            let mut ch = [0u8; 8];
            if reader.read_exact(&mut ch).is_err() {
                return Ok(None); // no data chunk
            }
            pos += 8;
            let size = u32_at(&ch, 4) as u64;
            match &ch[0..4] {
                b"ds64" => {
                    let mut b = vec![0u8; size.min(64) as usize];
                    if reader.read_exact(&mut b).is_err() || b.len() < 16 {
                        return Ok(None);
                    }
                    ds64_data = Some(u64_at(&b, 8));
                    reader.seek(SeekFrom::Start(pos + size + (size & 1)))?;
                }
                b"fmt " => {
                    let mut b = vec![0u8; size.min(64) as usize];
                    if reader.read_exact(&mut b).is_err() || b.len() < 16 {
                        return Ok(None);
                    }
                    let mut tag = u16_at(&b, 0);
                    let channels = u16_at(&b, 2) as usize;
                    let sample_rate = u32_at(&b, 4);
                    let block_align = u16_at(&b, 12) as usize;
                    let bits = u16_at(&b, 14);
                    if tag == 0xFFFE {
                        if b.len() < 26 {
                            return Ok(None);
                        }
                        tag = u16_at(&b, 24); // first field of the sub-format GUID
                    }
                    let format = match (tag, bits) {
                        (1, 8) => Format::U8,
                        (1, 16) => Format::I16,
                        (1, 24) => Format::I24,
                        (1, 32) => Format::I32,
                        (3, 32) => Format::F32,
                        (3, 64) => Format::F64,
                        _ => return Ok(None),
                    };
                    if channels == 0 || sample_rate == 0 || block_align != channels * format.bytes()
                    {
                        return Ok(None);
                    }
                    fmt = Some((format, channels, sample_rate, block_align));
                    reader.seek(SeekFrom::Start(pos + size + (size & 1)))?;
                }
                b"data" => {
                    let Some((format, channels, sample_rate, block_align)) = fmt else {
                        return Ok(None);
                    };
                    let available = file_len.saturating_sub(pos);
                    let mut len = if size == u32::MAX as u64 {
                        ds64_data.unwrap_or(available)
                    } else {
                        size
                    };
                    // 0 = still being written / streamed; too long = truncated file.
                    if len == 0 || len > available {
                        len = available;
                    }
                    return Ok(Some(Self {
                        reader,
                        data_start: pos,
                        frames: len / block_align as u64,
                        pos: 0,
                        sample_rate,
                        channels,
                        format,
                        bytes: Vec::new(),
                    }));
                }
                _ => {
                    reader.seek(SeekFrom::Start(pos + size + (size & 1)))?;
                }
            }
            pos += size + (size & 1);
            if pos >= file_len {
                return Ok(None);
            }
        }
    }
}

impl Decoder for WavDecoder {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn frames(&self) -> Option<u64> {
        Some(self.frames)
    }

    fn codec(&self) -> String {
        self.format.name().to_string()
    }

    fn seek(&mut self, frame: u64) -> Result<(), AudioError> {
        let frame = frame.min(self.frames);
        let block = (self.channels * self.format.bytes()) as u64;
        self.reader
            .seek(SeekFrom::Start(self.data_start + frame * block))?;
        self.pos = frame;
        Ok(())
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, AudioError> {
        let bps = self.format.bytes();
        let block = self.channels * bps;
        let want = ((out.len() / self.channels) as u64).min(self.frames - self.pos) as usize;
        if want == 0 {
            return Ok(0);
        }
        self.bytes.resize(want * block, 0);
        let mut got = 0;
        while got < self.bytes.len() {
            match self.reader.read(&mut self.bytes[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        let frames = got / block;
        let src = &self.bytes[..frames * block];
        let dst = &mut out[..frames * self.channels];
        match self.format {
            Format::U8 => {
                for (d, s) in dst.iter_mut().zip(src) {
                    *d = (*s as f32 - 128.0) / 128.0;
                }
            }
            Format::I16 => {
                for (d, s) in dst.iter_mut().zip(src.chunks_exact(2)) {
                    *d = i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0;
                }
            }
            Format::I24 => {
                for (d, s) in dst.iter_mut().zip(src.chunks_exact(3)) {
                    let v = i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8;
                    *d = v as f32 / 8_388_608.0;
                }
            }
            Format::I32 => {
                for (d, s) in dst.iter_mut().zip(src.chunks_exact(4)) {
                    *d = (i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f64 / 2_147_483_648.0)
                        as f32;
                }
            }
            Format::F32 => {
                for (d, s) in dst.iter_mut().zip(src.chunks_exact(4)) {
                    *d = f32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                }
            }
            Format::F64 => {
                for (d, s) in dst.iter_mut().zip(src.chunks_exact(8)) {
                    *d =
                        f64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]) as f32;
                }
            }
        }
        self.pos += frames as u64;
        Ok(frames)
    }
}
