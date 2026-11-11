//! A backend without a sound card: a thread renders at real-time pace and
//! throws the audio away. Used when no device exists and for soak tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::{
    AudioBackend, DeviceInfo, Driver, OutputStream, StreamInfo, StreamRequest, StreamStatus,
};
use crate::renderer::Renderer;
use crate::types::AudioError;

const DEVICE_ID: &str = "null";
const DEVICE_NAME: &str = "Null output (no sound)";
const MAX_CHANNELS: usize = 64;
const DEFAULT_BUFFER: u32 = 512;

/// The always-available silent backend.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullBackend;

impl NullBackend {
    /// Create the backend.
    pub fn new() -> Self {
        Self
    }
}

impl AudioBackend for NullBackend {
    fn name(&self) -> &str {
        "Null"
    }

    fn devices(&self) -> Vec<DeviceInfo> {
        vec![DeviceInfo {
            id: DEVICE_ID.to_string(),
            name: DEVICE_NAME.to_string(),
            max_output_channels: MAX_CHANNELS,
            default_sample_rate: 48_000,
            sample_rates: vec![44_100, 48_000, 88_200, 96_000, 176_400, 192_000],
            is_default: true,
        }]
    }

    fn open(
        &self,
        req: StreamRequest,
        renderer: Renderer,
    ) -> Result<Box<dyn OutputStream>, AudioError> {
        if let Some(id) = &req.device_id {
            if id != DEVICE_ID {
                return Err(AudioError::DeviceNotFound(id.clone()));
            }
        }
        if req.sample_rate != renderer.sample_rate() {
            return Err(AudioError::InvalidParam(format!(
                "stream rate {} differs from engine rate {}",
                req.sample_rate,
                renderer.sample_rate()
            )));
        }
        if req.channels == 0 || req.channels > MAX_CHANNELS {
            return Err(AudioError::InvalidParam(format!(
                "null output supports 1..={MAX_CHANNELS} channels"
            )));
        }
        let buffer = req
            .buffer_frames
            .unwrap_or(DEFAULT_BUFFER)
            .clamp(16, 16_384);
        let info = StreamInfo {
            backend: "Null".to_string(),
            device_name: DEVICE_NAME.to_string(),
            sample_rate: req.sample_rate,
            channels: req.channels,
            buffer_frames: Some(buffer),
        };
        let status = Arc::new(StreamStatus::new());
        let stop = Arc::new(AtomicBool::new(false));
        let mut driver = Driver::new(renderer, Arc::clone(&status));
        let stop2 = Arc::clone(&stop);
        let (rate, channels) = (req.sample_rate as u64, req.channels);
        let thread = thread::Builder::new()
            .name("sm-audio-null".into())
            .spawn(move || {
                let mut buf = vec![0.0f32; buffer as usize * channels];
                let mut epoch = Instant::now();
                let mut blocks: u64 = 0;
                while !stop2.load(Ordering::Acquire) {
                    driver.process(&mut buf, channels);
                    blocks += 1;
                    let due =
                        epoch + Duration::from_nanos(blocks * buffer as u64 * 1_000_000_000 / rate);
                    let now = Instant::now();
                    if due > now {
                        thread::sleep(due - now);
                    } else if now - due > Duration::from_millis(250) {
                        // Stalled (suspend, debugger): do not try to catch up.
                        epoch = now;
                        blocks = 0;
                    }
                }
            })
            .map_err(|e| AudioError::Backend(format!("cannot spawn null audio thread: {e}")))?;
        Ok(Box::new(NullStream {
            info,
            status,
            stop,
            thread: Some(thread),
        }))
    }
}

struct NullStream {
    info: StreamInfo,
    status: Arc<StreamStatus>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl OutputStream for NullStream {
    fn info(&self) -> StreamInfo {
        self.info.clone()
    }

    fn cpu_load(&self) -> f32 {
        self.status.cpu_load()
    }

    fn error(&self) -> Option<String> {
        self.status.error()
    }
}

impl Drop for NullStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            // A panic of the audio thread is already contained by `Driver`.
            let _ = t.join();
        }
    }
}
