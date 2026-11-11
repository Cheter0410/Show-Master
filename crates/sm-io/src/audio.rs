//! Audio device handling and the LTC real-time generator.

use serde::Serialize;
use sm_audio::{
    default_backends, engine_with_generators, Controller, DeviceInfo, EngineConfig, OutputStream,
    RtGenerator, StreamRequest,
};
use sm_core::model::AudioSettings;
use sm_timecode::{LtcEncoder, TcTransport};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

/// Lock-free LTC settings shared with the audio thread.
#[derive(Default)]
pub struct LtcShared {
    pub enabled: AtomicBool,
    pub channel: AtomicUsize,
    gain_bits: AtomicU32,
}

impl LtcShared {
    pub fn set_gain(&self, gain: f32) {
        self.gain_bits.store(gain.to_bits(), Ordering::Relaxed);
    }
    fn gain(&self) -> f32 {
        f32::from_bits(self.gain_bits.load(Ordering::Relaxed))
    }
}

const LTC_CHUNK: usize = 1024;

/// Writes LTC onto one device channel, sample-locked to the engine clock.
/// Runs on the audio thread: no allocation, no locks.
struct LtcGenerator {
    encoder: LtcEncoder,
    rx: rtrb::Consumer<TcTransport>,
    shared: Arc<LtcShared>,
    buf: [f32; LTC_CHUNK],
}

impl RtGenerator for LtcGenerator {
    fn render(&mut self, start_frame: u64, out: &mut [f32], channels: usize) {
        while let Ok(t) = self.rx.pop() {
            self.encoder.set_transport(t);
        }
        let channel = self.shared.channel.load(Ordering::Relaxed);
        if !self.shared.enabled.load(Ordering::Relaxed) || channels == 0 || channel >= channels {
            return;
        }
        let gain = self.shared.gain();
        let mut frame = start_frame;
        for block in out.chunks_mut(LTC_CHUNK * channels) {
            let n = block.len() / channels;
            let Some(buf) = self.buf.get_mut(..n) else {
                return;
            };
            self.encoder.render(frame, buf, gain);
            for (dst, src) in block.chunks_exact_mut(channels).zip(buf.iter()) {
                if let Some(slot) = dst.get_mut(channel) {
                    *slot = *src;
                }
            }
            frame += n as u64;
        }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct DeviceEntry {
    pub backend: String,
    pub id: String,
    pub name: String,
    pub max_output_channels: usize,
    pub sample_rates: Vec<u32>,
    pub is_default: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct AudioStatus {
    pub backend: String,
    pub device: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub buffer_frames: Option<u32>,
    /// True when running on the silent null output.
    pub is_null: bool,
    pub error: Option<String>,
    pub devices: Vec<DeviceEntry>,
}

/// An open output stream with its engine controller.
pub struct AudioSystem {
    pub controller: Controller,
    stream: Box<dyn OutputStream>,
    ltc_tx: rtrb::Producer<TcTransport>,
    pub ltc: Arc<LtcShared>,
    status: AudioStatus,
}

fn is_null(name: &str) -> bool {
    name.to_ascii_lowercase().contains("null")
}

pub fn list_devices() -> Vec<DeviceEntry> {
    let mut out = Vec::new();
    for b in default_backends() {
        let backend = b.name().to_string();
        for d in b.devices() {
            let DeviceInfo {
                id,
                name,
                max_output_channels,
                sample_rates,
                is_default,
                ..
            } = d;
            out.push(DeviceEntry {
                backend: backend.clone(),
                id,
                name,
                max_output_channels,
                sample_rates,
                is_default,
            });
        }
    }
    out
}

impl AudioSystem {
    /// Open the configured device. Falls back to other backends and finally
    /// to the null output, so the show clock always runs; the reason for a
    /// fallback is kept in `status().error`.
    pub fn open(settings: &AudioSettings) -> Self {
        let mut first_error: Option<String> = None;
        let wanted = settings.backend.as_deref();
        let backends = default_backends();
        // Preferred backend first, then the rest; the null backend is last in the list.
        let mut order: Vec<usize> = (0..backends.len()).collect();
        if let Some(w) = wanted {
            order.sort_by_key(|i| {
                !backends
                    .get(*i)
                    .is_some_and(|b| b.name().eq_ignore_ascii_case(w))
            });
        }
        for i in order {
            let Some(backend) = backends.get(i) else {
                continue;
            };
            let preferred = wanted.is_none_or(|w| backend.name().eq_ignore_ascii_case(w));
            let null = is_null(backend.name());
            let channels = usize::from(settings.channels.max(1));
            let (tx, rx) = rtrb::RingBuffer::new(64);
            let ltc = Arc::new(LtcShared::default());
            let generator = LtcGenerator {
                encoder: LtcEncoder::new(settings.sample_rate),
                rx,
                shared: ltc.clone(),
                buf: [0.0; LTC_CHUNK],
            };
            let (controller, renderer) = engine_with_generators(
                EngineConfig::new(settings.sample_rate, channels),
                vec![Box::new(generator)],
            );
            let req = StreamRequest {
                device_id: if preferred && !null {
                    settings.device.clone()
                } else {
                    None
                },
                sample_rate: settings.sample_rate,
                channels,
                buffer_frames: settings.buffer_frames,
            };
            match backend.open(req, renderer) {
                Ok(stream) => {
                    let info = stream.info();
                    if null && first_error.is_none() && wanted.is_none_or(|w| !is_null(w)) {
                        first_error = Some("no audio device available".into());
                    }
                    let status = AudioStatus {
                        backend: info.backend,
                        device: info.device_name,
                        sample_rate: info.sample_rate,
                        channels: info.channels,
                        buffer_frames: info.buffer_frames,
                        is_null: null,
                        error: first_error,
                        devices: list_devices(),
                    };
                    tracing::info!(backend = %status.backend, device = %status.device, rate = status.sample_rate, channels = status.channels, "audio stream open");
                    return Self {
                        controller,
                        stream,
                        ltc_tx: tx,
                        ltc,
                        status,
                    };
                }
                Err(e) => {
                    tracing::warn!("audio backend {}: {e}", backend.name());
                    if first_error.is_none() {
                        first_error = Some(format!("{}: {e}", backend.name()));
                    }
                }
            }
        }
        // Unreachable in practice (the null backend cannot fail), but never panic:
        // run without a stream; the clock then stands still and the UI shows the error.
        let (tx, _rx) = rtrb::RingBuffer::new(1);
        let (controller, renderer) = engine_with_generators(
            EngineConfig::new(settings.sample_rate, usize::from(settings.channels.max(1))),
            Vec::new(),
        );
        let status = AudioStatus {
            backend: "none".into(),
            device: String::new(),
            sample_rate: settings.sample_rate,
            channels: usize::from(settings.channels),
            buffer_frames: None,
            is_null: true,
            error: first_error.or(Some("no audio backend".into())),
            devices: Vec::new(),
        };
        Self {
            controller,
            stream: Box::new(Dead(renderer)),
            ltc_tx: tx,
            ltc: Arc::new(LtcShared::default()),
            status,
        }
    }

    /// An audio system without a device: the caller drives the returned
    /// renderer itself (offline bounce, tests).
    pub fn offline(settings: &AudioSettings) -> (Self, sm_audio::Renderer) {
        let channels = usize::from(settings.channels.max(1));
        let (tx, rx) = rtrb::RingBuffer::new(64);
        let ltc = Arc::new(LtcShared::default());
        let generator = LtcGenerator {
            encoder: LtcEncoder::new(settings.sample_rate),
            rx,
            shared: ltc.clone(),
            buf: [0.0; LTC_CHUNK],
        };
        let (controller, renderer) = engine_with_generators(
            EngineConfig::new(settings.sample_rate, channels),
            vec![Box::new(generator)],
        );
        let status = AudioStatus {
            backend: "offline".into(),
            device: "offline".into(),
            sample_rate: settings.sample_rate,
            channels,
            buffer_frames: None,
            is_null: true,
            error: None,
            devices: Vec::new(),
        };
        let system = Self {
            controller,
            stream: Box::new(Offline),
            ltc_tx: tx,
            ltc,
            status,
        };
        (system, renderer)
    }

    pub fn status(&self) -> AudioStatus {
        let mut s = self.status.clone();
        if let Some(e) = self.stream.error() {
            s.error = Some(e);
        }
        s
    }

    pub fn cpu_load(&self) -> f32 {
        self.stream.cpu_load()
    }

    pub fn set_transport(&mut self, t: TcTransport) {
        if self.ltc_tx.push(t).is_err() {
            tracing::warn!("LTC transport queue full");
        }
    }
}

struct Dead(#[allow(dead_code)] sm_audio::Renderer);
impl OutputStream for Dead {
    fn info(&self) -> sm_audio::StreamInfo {
        sm_audio::StreamInfo {
            backend: "none".into(),
            device_name: String::new(),
            sample_rate: 0,
            channels: 0,
            buffer_frames: None,
        }
    }
    fn cpu_load(&self) -> f32 {
        0.0
    }
    fn error(&self) -> Option<String> {
        Some("no audio backend".into())
    }
}

struct Offline;
impl OutputStream for Offline {
    fn info(&self) -> sm_audio::StreamInfo {
        sm_audio::StreamInfo {
            backend: "offline".into(),
            device_name: "offline".into(),
            sample_rate: 0,
            channels: 0,
            buffer_frames: None,
        }
    }
    fn cpu_load(&self) -> f32 {
        0.0
    }
    fn error(&self) -> Option<String> {
        None
    }
}
