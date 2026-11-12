//! Audio input (capture), used for LTC timecode input.
//!
//! Separate from the output path on purpose: an input stream never touches
//! the mixer or its clock. The device callback extracts one channel, pushes
//! it as `f32` mono into a lock-free ring ([`InputTap`]) and records how many
//! samples had been captured at which [`Instant`]; a non-real-time reader
//! ([`InputReader`]) drains both. The callback never allocates, locks or
//! blocks (a full ring drops samples and counts them).
//!
//! * [`CpalInputBackend`] (feature `cpal-backend`): real capture devices.
//! * [`ManualInputBackend`]: a virtual device fed by hand (tests, tools).
//! * [`NoInputBackend`]: no devices.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::types::AudioError;

/// A capture device as listed by a backend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputDeviceInfo {
    /// Identifier for [`InputRequest::device_id`].
    pub id: String,
    pub name: String,
    pub channels: usize,
    pub default_sample_rate: u32,
    pub is_default: bool,
}

/// What to open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputRequest {
    /// `None` = the backend's default input.
    pub device_id: Option<String>,
    /// Zero-based channel to capture.
    pub channel: usize,
}

/// What was opened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputInfo {
    pub device_name: String,
    pub sample_rate: u32,
    pub channels: usize,
}

/// A family of capture devices.
pub trait AudioInputBackend: Send {
    fn name(&self) -> &str;
    fn input_devices(&self) -> Vec<InputDeviceInfo>;
    /// Open a stream; samples of the chosen channel arrive in the reader.
    fn open_input(
        &self,
        req: &InputRequest,
    ) -> Result<(Box<dyn InputStream>, InputReader), AudioError>;
}

/// A running capture stream; dropping it closes the device.
pub trait InputStream: Send {
    fn info(&self) -> InputInfo;
    fn error(&self) -> Option<String>;
}

/// Ring sizes: two seconds of audio at 96 kHz, callback stamps.
const RING_SECONDS: usize = 2;
const STAMPS: usize = 512;

/// Real-time side: owned by the device callback.
pub struct InputTap {
    samples: rtrb::Producer<f32>,
    stamps: rtrb::Producer<(u64, Instant)>,
    channel: usize,
    total: u64,
    dropped: Arc<AtomicU64>,
}

/// Reader side: drained by a normal thread.
pub struct InputReader {
    samples: rtrb::Consumer<f32>,
    stamps: rtrb::Consumer<(u64, Instant)>,
    sample_rate: u32,
    dropped: Arc<AtomicU64>,
}

/// A connected tap / reader pair for `sample_rate`, capturing `channel`.
pub fn input_pair(sample_rate: u32, channel: usize) -> (InputTap, InputReader) {
    let cap = (sample_rate.max(8000) as usize) * RING_SECONDS;
    let (sp, sc) = rtrb::RingBuffer::new(cap);
    let (tp, tc) = rtrb::RingBuffer::new(STAMPS);
    let dropped = Arc::new(AtomicU64::new(0));
    (
        InputTap {
            samples: sp,
            stamps: tp,
            channel,
            total: 0,
            dropped: dropped.clone(),
        },
        InputReader {
            samples: sc,
            stamps: tc,
            sample_rate,
            dropped,
        },
    )
}

impl InputTap {
    /// Push one interleaved device buffer captured "now". Real-time safe.
    pub fn push_interleaved(&mut self, data: &[f32], channels: usize) {
        self.push_at(data, channels, Instant::now());
    }

    /// Push one interleaved buffer whose last sample was captured at `at`.
    pub fn push_at(&mut self, data: &[f32], channels: usize, at: Instant) {
        if channels == 0 {
            return;
        }
        let frames = data.len() / channels;
        let mut lost = 0u64;
        for frame in data.chunks_exact(channels) {
            let x = frame.get(self.channel).copied().unwrap_or(0.0);
            if self.samples.push(x).is_err() {
                lost += 1;
            }
        }
        self.total += frames as u64;
        if lost > 0 {
            self.dropped.fetch_add(lost, Ordering::Relaxed);
        }
        // A full stamp ring only costs mapping accuracy, never correctness.
        let _ = self.stamps.push((self.total, at));
    }
}

impl InputReader {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Samples lost because the reader fell behind.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Take everything captured so far: appends samples to `out` and returns
    /// the `(samples captured in total, instant)` stamps since the last call.
    pub fn read(&mut self, out: &mut Vec<f32>) -> Vec<(u64, Instant)> {
        let n = self.samples.slots();
        if let Ok(chunk) = self.samples.read_chunk(n) {
            let (a, b) = chunk.as_slices();
            out.extend_from_slice(a);
            out.extend_from_slice(b);
            chunk.commit_all();
        }
        let mut stamps = Vec::new();
        while let Ok(s) = self.stamps.pop() {
            stamps.push(s);
        }
        stamps
    }
}

// ---------------------------------------------------------------------------
// No input
// ---------------------------------------------------------------------------

/// A backend without devices.
pub struct NoInputBackend;

impl AudioInputBackend for NoInputBackend {
    fn name(&self) -> &str {
        "none"
    }
    fn input_devices(&self) -> Vec<InputDeviceInfo> {
        Vec::new()
    }
    fn open_input(
        &self,
        _req: &InputRequest,
    ) -> Result<(Box<dyn InputStream>, InputReader), AudioError> {
        Err(AudioError::DeviceNotFound("no audio input".into()))
    }
}

// ---------------------------------------------------------------------------
// Manual (virtual) input
// ---------------------------------------------------------------------------

type SharedTap = Arc<Mutex<Option<(InputTap, usize)>>>;

/// A virtual capture device fed through [`ManualInputFeed`].
pub struct ManualInputBackend {
    device: InputDeviceInfo,
    tap: SharedTap,
}

/// Feeds the stream opened on a [`ManualInputBackend`].
#[derive(Clone)]
pub struct ManualInputFeed {
    tap: SharedTap,
}

impl ManualInputBackend {
    /// A device `name` with `channels` channels at `sample_rate`.
    pub fn new(name: &str, channels: usize, sample_rate: u32) -> (Self, ManualInputFeed) {
        let tap: SharedTap = Arc::new(Mutex::new(None));
        (
            Self {
                device: InputDeviceInfo {
                    id: name.to_string(),
                    name: name.to_string(),
                    channels,
                    default_sample_rate: sample_rate,
                    is_default: true,
                },
                tap: tap.clone(),
            },
            ManualInputFeed { tap },
        )
    }
}

impl ManualInputFeed {
    /// True once a stream is open.
    pub fn is_open(&self) -> bool {
        self.tap.lock().map(|t| t.is_some()).unwrap_or(false)
    }

    /// Deliver an interleaved buffer whose last sample was captured at `at`.
    /// Returns false when no stream is open.
    pub fn push(&self, data: &[f32], at: Instant) -> bool {
        match self.tap.lock() {
            Ok(mut g) => match g.as_mut() {
                Some((tap, channels)) => {
                    let ch = *channels;
                    tap.push_at(data, ch, at);
                    true
                }
                None => false,
            },
            Err(_) => false,
        }
    }
}

struct ManualStream {
    info: InputInfo,
    tap: SharedTap,
}

impl InputStream for ManualStream {
    fn info(&self) -> InputInfo {
        self.info.clone()
    }
    fn error(&self) -> Option<String> {
        None
    }
}

impl Drop for ManualStream {
    fn drop(&mut self) {
        if let Ok(mut g) = self.tap.lock() {
            *g = None;
        }
    }
}

impl AudioInputBackend for ManualInputBackend {
    fn name(&self) -> &str {
        "manual"
    }
    fn input_devices(&self) -> Vec<InputDeviceInfo> {
        vec![self.device.clone()]
    }
    fn open_input(
        &self,
        req: &InputRequest,
    ) -> Result<(Box<dyn InputStream>, InputReader), AudioError> {
        if req
            .device_id
            .as_deref()
            .is_some_and(|d| d != self.device.id)
        {
            return Err(AudioError::DeviceNotFound(
                req.device_id.clone().unwrap_or_default(),
            ));
        }
        if req.channel >= self.device.channels {
            return Err(AudioError::InvalidParam(format!(
                "input channel {} does not exist ({} channels)",
                req.channel + 1,
                self.device.channels
            )));
        }
        let (tap, reader) = input_pair(self.device.default_sample_rate, req.channel);
        if let Ok(mut g) = self.tap.lock() {
            *g = Some((tap, self.device.channels));
        }
        Ok((
            Box::new(ManualStream {
                info: InputInfo {
                    device_name: self.device.name.clone(),
                    sample_rate: self.device.default_sample_rate,
                    channels: self.device.channels,
                },
                tap: self.tap.clone(),
            }),
            reader,
        ))
    }
}

/// The capture backends of this build (real hosts first).
pub fn default_input_backend() -> Box<dyn AudioInputBackend> {
    #[cfg(feature = "cpal-backend")]
    {
        Box::new(cpal_input::CpalInputBackend::default_host())
    }
    #[cfg(not(feature = "cpal-backend"))]
    {
        Box::new(NoInputBackend)
    }
}

#[cfg(feature = "cpal-backend")]
pub use cpal_input::CpalInputBackend;

#[cfg(feature = "cpal-backend")]
mod cpal_input {
    use super::{
        input_pair, AudioInputBackend, InputDeviceInfo, InputInfo, InputReader, InputRequest,
        InputStream, InputTap,
    };
    use crate::types::AudioError;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::SampleFormat;
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread::{self, JoinHandle};

    /// Frames of the conversion scratch buffer for integer formats.
    const SCRATCH_FRAMES: usize = 1024;

    /// Capture devices of one cpal host.
    pub struct CpalInputBackend {
        host_id: cpal::HostId,
        name: String,
    }

    impl CpalInputBackend {
        pub fn new(host_id: cpal::HostId) -> Self {
            Self {
                host_id,
                name: host_id.name().to_string(),
            }
        }

        pub fn default_host() -> Self {
            Self::new(cpal::default_host().id())
        }
    }

    fn convert_push<T: Copy>(
        tap: &mut InputTap,
        scratch: &mut [f32],
        data: &[T],
        channels: usize,
        f: impl Fn(T) -> f32,
    ) {
        let step = (scratch.len() / channels.max(1)) * channels.max(1);
        if step == 0 {
            return;
        }
        for chunk in data.chunks(step) {
            let s = &mut scratch[..chunk.len()];
            for (d, x) in s.iter_mut().zip(chunk) {
                *d = f(*x);
            }
            tap.push_interleaved(s, channels);
        }
    }

    type ErrorSlot = Arc<Mutex<Option<String>>>;

    fn build(
        device: &cpal::Device,
        channel: usize,
        error: ErrorSlot,
    ) -> Result<(cpal::Stream, InputInfo, InputReader), AudioError> {
        let cfg = device
            .default_input_config()
            .map_err(|e| AudioError::Backend(e.to_string()))?;
        let channels = usize::from(cfg.channels());
        if channel >= channels {
            return Err(AudioError::InvalidParam(format!(
                "input channel {} does not exist ({channels} channels)",
                channel + 1
            )));
        }
        let sample_rate = cfg.sample_rate().0;
        let format = cfg.sample_format();
        let config: cpal::StreamConfig = cfg.into();
        let (mut tap, reader) = input_pair(sample_rate, channel);
        let on_error = move |e: cpal::StreamError| {
            tracing::error!("audio input error: {e}");
            if let Ok(mut s) = error.try_lock() {
                *s = Some(e.to_string());
            }
        };
        let mut scratch = vec![0.0f32; SCRATCH_FRAMES * channels];
        let stream = match format {
            SampleFormat::F32 => device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    tap.push_interleaved(data, channels)
                },
                on_error,
                None,
            ),
            SampleFormat::I16 => device.build_input_stream(
                &config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    convert_push(&mut tap, &mut scratch, data, channels, |x| {
                        f32::from(x) / 32_768.0
                    })
                },
                on_error,
                None,
            ),
            SampleFormat::I32 => device.build_input_stream(
                &config,
                move |data: &[i32], _: &cpal::InputCallbackInfo| {
                    convert_push(&mut tap, &mut scratch, data, channels, |x| {
                        (f64::from(x) / 2_147_483_648.0) as f32
                    })
                },
                on_error,
                None,
            ),
            SampleFormat::U16 => device.build_input_stream(
                &config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    convert_push(&mut tap, &mut scratch, data, channels, |x| {
                        (f32::from(x) - 32_768.0) / 32_768.0
                    })
                },
                on_error,
                None,
            ),
            other => return Err(AudioError::Unsupported(format!("sample format {other:?}"))),
        }
        .map_err(|e| AudioError::Backend(e.to_string()))?;
        stream
            .play()
            .map_err(|e| AudioError::Backend(e.to_string()))?;
        let info = InputInfo {
            device_name: device.name().unwrap_or_else(|_| "unknown".into()),
            sample_rate,
            channels,
        };
        Ok((stream, info, reader))
    }

    struct CpalInput {
        info: InputInfo,
        error: ErrorSlot,
        stop: Option<mpsc::Sender<()>>,
        thread: Option<JoinHandle<()>>,
    }

    impl InputStream for CpalInput {
        fn info(&self) -> InputInfo {
            self.info.clone()
        }
        fn error(&self) -> Option<String> {
            self.error.lock().ok().and_then(|e| e.clone())
        }
    }

    impl Drop for CpalInput {
        fn drop(&mut self) {
            drop(self.stop.take());
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    impl AudioInputBackend for CpalInputBackend {
        fn name(&self) -> &str {
            &self.name
        }

        fn input_devices(&self) -> Vec<InputDeviceInfo> {
            let Ok(host) = cpal::host_from_id(self.host_id) else {
                return Vec::new();
            };
            let default_name = host.default_input_device().and_then(|d| d.name().ok());
            let Ok(devices) = host.input_devices() else {
                return Vec::new();
            };
            devices
                .filter_map(|d| {
                    let name = d.name().ok()?;
                    let cfg = d.default_input_config().ok()?;
                    Some(InputDeviceInfo {
                        id: name.clone(),
                        is_default: default_name.as_deref() == Some(name.as_str()),
                        name,
                        channels: usize::from(cfg.channels()),
                        default_sample_rate: cfg.sample_rate().0,
                    })
                })
                .collect()
        }

        fn open_input(
            &self,
            req: &InputRequest,
        ) -> Result<(Box<dyn InputStream>, InputReader), AudioError> {
            let host_id = self.host_id;
            let req = req.clone();
            let error: ErrorSlot = Arc::new(Mutex::new(None));
            let err2 = error.clone();
            let (ready_tx, ready_rx) = mpsc::channel();
            let (stop_tx, stop_rx) = mpsc::channel::<()>();
            // cpal streams are not `Send` everywhere: the stream lives on its own thread.
            let thread = thread::Builder::new()
                .name("sm-audio-in-dev".into())
                .spawn(move || {
                    let opened = (|| {
                        let host = cpal::host_from_id(host_id)
                            .map_err(|e| AudioError::Backend(e.to_string()))?;
                        let device = match &req.device_id {
                            None => host.default_input_device().ok_or_else(|| {
                                AudioError::DeviceNotFound("default input".into())
                            })?,
                            Some(id) => host
                                .input_devices()
                                .map_err(|e| AudioError::Backend(e.to_string()))?
                                .find(|d| d.name().is_ok_and(|n| &n == id))
                                .ok_or_else(|| AudioError::DeviceNotFound(id.clone()))?,
                        };
                        build(&device, req.channel, err2)
                    })();
                    match opened {
                        Ok((stream, info, reader)) => {
                            if ready_tx.send(Ok((info, reader))).is_ok() {
                                let _ = stop_rx.recv();
                            }
                            drop(stream);
                        }
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                        }
                    }
                })
                .map_err(|e| AudioError::Backend(format!("cannot spawn input thread: {e}")))?;
            match ready_rx.recv() {
                Ok(Ok((info, reader))) => Ok((
                    Box::new(CpalInput {
                        info,
                        error,
                        stop: Some(stop_tx),
                        thread: Some(thread),
                    }),
                    reader,
                )),
                Ok(Err(e)) => {
                    let _ = thread.join();
                    Err(e)
                }
                Err(_) => {
                    let _ = thread.join();
                    Err(AudioError::Backend(
                        "input thread died while opening".into(),
                    ))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn tap_extracts_one_channel_and_stamps() {
        let (mut tap, mut reader) = input_pair(48_000, 1);
        let t0 = Instant::now();
        tap.push_at(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6], 2, t0);
        tap.push_at(&[0.7, 0.8], 2, t0 + Duration::from_millis(1));
        let mut out = Vec::new();
        let stamps = reader.read(&mut out);
        assert_eq!(out, vec![0.2, 0.4, 0.6, 0.8]);
        assert_eq!(stamps.iter().map(|s| s.0).collect::<Vec<_>>(), vec![3, 4]);
        assert_eq!(reader.dropped(), 0);
    }

    #[test]
    fn full_ring_drops_and_counts() {
        let (mut tap, mut reader) = input_pair(8000, 0);
        let big = vec![0.5f32; 8000 * 3];
        tap.push_at(&big, 1, Instant::now());
        assert_eq!(reader.dropped(), 8000);
        let mut out = Vec::new();
        reader.read(&mut out);
        assert_eq!(out.len(), 16_000);
    }

    #[test]
    fn manual_backend_feeds_the_open_stream() {
        let (backend, feed) = ManualInputBackend::new("Virtual", 2, 48_000);
        assert!(!feed.push(&[0.0, 0.0], Instant::now()));
        assert!(backend
            .open_input(&InputRequest {
                device_id: None,
                channel: 2
            })
            .is_err());
        let (stream, mut reader) = backend
            .open_input(&InputRequest {
                device_id: Some("Virtual".into()),
                channel: 0,
            })
            .unwrap();
        assert_eq!(stream.info().sample_rate, 48_000);
        assert!(feed.push(&[0.25, -1.0], Instant::now()));
        let mut out = Vec::new();
        reader.read(&mut out);
        assert_eq!(out, vec![0.25]);
        drop(stream);
        assert!(!feed.is_open());
    }
}
