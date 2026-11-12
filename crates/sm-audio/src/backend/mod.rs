//! Output backends: everything platform-specific lives behind
//! [`AudioBackend`] / [`OutputStream`].
//!
//! * [`NullBackend`] — a paced thread without a sound card (always there).
//! * [`CpalBackend`] — real devices (feature `cpal-backend`): CoreAudio,
//!   WASAPI, ALSA (PipeWire / PulseAudio through their ALSA plugins), ASIO
//!   (feature `asio`), JACK (feature `jack`).
//!
//! Offline rendering needs no backend at all: call
//! [`Renderer::render`](crate::Renderer::render) in a loop.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::renderer::Renderer;
use crate::types::AudioError;

#[cfg(feature = "cpal-backend")]
mod cpal_backend;
#[cfg(all(feature = "cpal-backend", target_os = "macos"))]
mod macos_rate;
mod null;

#[cfg(feature = "cpal-backend")]
pub use cpal_backend::CpalBackend;
pub use null::NullBackend;

/// An output device as listed by a backend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Identifier to pass in [`StreamRequest::device_id`] (stable for the
    /// lifetime of the device within one backend).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Largest output channel count the device offers.
    pub max_output_channels: usize,
    /// The device's preferred sample rate.
    pub default_sample_rate: u32,
    /// Common sample rates the device supports.
    pub sample_rates: Vec<u32>,
    /// Whether this is the backend's default output.
    pub is_default: bool,
}

/// What to open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamRequest {
    /// Device id from [`DeviceInfo::id`]; `None` = the backend's default.
    pub device_id: Option<String>,
    /// Must equal the renderer's sample rate (the engine does not resample
    /// its output).
    pub sample_rate: u32,
    /// Desired device channel count. When the device delivers a different
    /// count, the common channels are mapped 1:1 and the rest is silent.
    pub channels: usize,
    /// Desired buffer size in frames (`None` = device default).
    pub buffer_frames: Option<u32>,
}

/// What was actually opened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamInfo {
    /// Backend name.
    pub backend: String,
    /// Device name.
    pub device_name: String,
    /// Stream sample rate.
    pub sample_rate: u32,
    /// Channel count the device runs with.
    pub channels: usize,
    /// Buffer size in frames, when known.
    pub buffer_frames: Option<u32>,
    /// Something worth telling the user about the device set-up (e.g. it
    /// runs at another rate than the stream, so the system converts).
    pub note: Option<String>,
}

/// A family of output devices (one audio host API).
pub trait AudioBackend {
    /// Backend name, e.g. `"ALSA"`, `"CoreAudio"`, `"Null"`.
    fn name(&self) -> &str;
    /// Enumerate output devices. May take a moment on some hosts.
    fn devices(&self) -> Vec<DeviceInfo>;
    /// Open a stream and start pulling audio from `renderer`.
    fn open(
        &self,
        req: StreamRequest,
        renderer: Renderer,
    ) -> Result<Box<dyn OutputStream>, AudioError>;
}

/// A running output stream. Dropping it stops the stream (and drops the
/// renderer it owns).
pub trait OutputStream: Send {
    /// The parameters the stream runs with.
    fn info(&self) -> StreamInfo;
    /// Smoothed render time / block time, 0.0 – 1.0 (above 1.0 = overload).
    fn cpu_load(&self) -> f32;
    /// A device error or a caught panic of the audio callback, if any
    /// happened. After a callback panic the stream outputs silence.
    fn error(&self) -> Option<String>;
}

/// All backends available in this build, real hosts first, the null
/// backend last.
pub fn default_backends() -> Vec<Box<dyn AudioBackend>> {
    #[cfg(feature = "cpal-backend")]
    let real: Vec<Box<dyn AudioBackend>> = CpalBackend::available()
        .into_iter()
        .map(|b| Box::new(b) as Box<dyn AudioBackend>)
        .collect();
    #[cfg(not(feature = "cpal-backend"))]
    let real: Vec<Box<dyn AudioBackend>> = Vec::new();
    let null: Box<dyn AudioBackend> = Box::new(NullBackend::new());
    real.into_iter().chain(std::iter::once(null)).collect()
}

/// Status shared between a stream handle and its audio thread.
pub(crate) struct StreamStatus {
    load: AtomicU32,
    panicked: AtomicBool,
    error: Mutex<Option<String>>,
}

impl StreamStatus {
    pub(crate) fn new() -> Self {
        Self {
            load: AtomicU32::new(0),
            panicked: AtomicBool::new(false),
            error: Mutex::new(None),
        }
    }

    pub(crate) fn cpu_load(&self) -> f32 {
        f32::from_bits(self.load.load(Ordering::Relaxed))
    }

    /// Record a device error (called from non-real-time error callbacks).
    #[cfg_attr(not(feature = "cpal-backend"), allow(dead_code))]
    pub(crate) fn set_error(&self, message: String) {
        if let Ok(mut e) = self.error.try_lock() {
            *e = Some(message);
        }
    }

    pub(crate) fn error(&self) -> Option<String> {
        if self.panicked.load(Ordering::Relaxed) {
            return Some("audio callback panicked; output muted".to_string());
        }
        self.error.lock().ok().and_then(|e| e.clone())
    }
}

/// Wraps the renderer for use in a device callback: panic containment and
/// CPU load measurement.
pub(crate) struct Driver {
    renderer: Renderer,
    status: Arc<StreamStatus>,
    poisoned: bool,
    load: f32,
}

impl Driver {
    pub(crate) fn new(renderer: Renderer, status: Arc<StreamStatus>) -> Self {
        Self {
            renderer,
            status,
            poisoned: false,
            load: 0.0,
        }
    }

    /// Render one device buffer. Never panics; after a panic inside the
    /// renderer the output stays silent.
    pub(crate) fn process(&mut self, out: &mut [f32], channels: usize) {
        if self.poisoned || channels == 0 {
            out.fill(0.0);
            return;
        }
        let t0 = Instant::now();
        let renderer = &mut self.renderer;
        let ok = catch_unwind(AssertUnwindSafe(|| renderer.render(out, channels))).is_ok();
        if !ok {
            self.poisoned = true;
            self.status.panicked.store(true, Ordering::Relaxed);
            out.fill(0.0);
            return;
        }
        let frames = out.len() / channels;
        if frames > 0 {
            let block = frames as f32 / self.renderer.sample_rate() as f32;
            let load = t0.elapsed().as_secs_f32() / block;
            self.load += (load - self.load) * 0.1;
            self.status
                .load
                .store(self.load.to_bits(), Ordering::Relaxed);
        }
    }
}
