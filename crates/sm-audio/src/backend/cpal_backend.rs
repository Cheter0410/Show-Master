//! Real audio devices through cpal.

use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SupportedBufferSize, SupportedStreamConfigRange};

use super::{
    AudioBackend, DeviceInfo, Driver, OutputStream, StreamInfo, StreamRequest, StreamStatus,
};
use crate::renderer::Renderer;
use crate::types::AudioError;

const COMMON_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];
/// Frames of the f32 scratch buffer used for integer sample formats.
const SCRATCH_FRAMES: usize = 1024;

/// One cpal host (CoreAudio, WASAPI, ASIO, ALSA, JACK).
pub struct CpalBackend {
    host_id: cpal::HostId,
    name: String,
}

impl CpalBackend {
    /// Backend for a specific cpal host.
    pub fn new(host_id: cpal::HostId) -> Self {
        Self {
            host_id,
            name: host_id.name().to_string(),
        }
    }

    /// The platform's default host.
    pub fn default_host() -> Self {
        Self::new(cpal::default_host().id())
    }

    /// Every host compiled in and available on this machine.
    pub fn available() -> Vec<Self> {
        cpal::available_hosts().into_iter().map(Self::new).collect()
    }

    fn host(&self) -> Result<cpal::Host, AudioError> {
        cpal::host_from_id(self.host_id).map_err(|e| AudioError::Backend(e.to_string()))
    }
}

fn format_rank(f: SampleFormat) -> Option<u32> {
    match f {
        SampleFormat::F32 => Some(0),
        SampleFormat::I32 => Some(1),
        SampleFormat::I16 => Some(2),
        SampleFormat::U16 => Some(3),
        _ => None,
    }
}

/// Pick the device configuration closest to the request: right sample
/// rate, then exact channel count (else the next larger, else the
/// largest), then the best sample format.
fn choose_config(
    device: &cpal::Device,
    req: &StreamRequest,
) -> Result<SupportedStreamConfigRange, AudioError> {
    let rate = cpal::SampleRate(req.sample_rate);
    let configs = device
        .supported_output_configs()
        .map_err(|e| AudioError::Backend(e.to_string()))?;
    let want = req.channels as i64;
    configs
        .filter(|c| c.min_sample_rate() <= rate && rate <= c.max_sample_rate())
        .filter_map(|c| {
            let rank = format_rank(c.sample_format())?;
            let ch = c.channels() as i64;
            let ch_score = if ch == want {
                0
            } else if ch > want {
                1 + (ch - want)
            } else {
                100_000 + (want - ch)
            };
            Some(((ch_score, rank), c))
        })
        .min_by_key(|(score, _)| *score)
        .map(|(_, c)| c)
        .ok_or_else(|| {
            AudioError::Unsupported(format!(
                "device does not support {} Hz output",
                req.sample_rate
            ))
        })
}

fn build_stream(
    device: &cpal::Device,
    req: &StreamRequest,
    mut driver: Driver,
    status: Arc<StreamStatus>,
) -> Result<(cpal::Stream, usize, Option<u32>), AudioError> {
    let range = choose_config(device, req)?;
    let format = range.sample_format();
    let channels = range.channels() as usize;
    let buffer_frames = req.buffer_frames.map(|b| match range.buffer_size() {
        SupportedBufferSize::Range { min, max } => b.clamp(*min, *max),
        SupportedBufferSize::Unknown => b,
    });
    let config = cpal::StreamConfig {
        channels: range.channels(),
        sample_rate: cpal::SampleRate(req.sample_rate),
        buffer_size: match buffer_frames {
            Some(b) => cpal::BufferSize::Fixed(b),
            None => cpal::BufferSize::Default,
        },
    };
    let on_error = move |e: cpal::StreamError| {
        tracing::error!("audio stream error: {e}");
        status.set_error(e.to_string());
    };
    let mut scratch = vec![0.0f32; SCRATCH_FRAMES * channels];
    let stream = match format {
        SampleFormat::F32 => device.build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| driver.process(data, channels),
            on_error,
            None,
        ),
        SampleFormat::I32 => device.build_output_stream(
            &config,
            move |data: &mut [i32], _: &cpal::OutputCallbackInfo| {
                for chunk in data.chunks_mut(scratch.len()) {
                    let s = &mut scratch[..chunk.len()];
                    driver.process(s, channels);
                    for (d, x) in chunk.iter_mut().zip(s.iter()) {
                        *d = (*x as f64 * 2_147_483_647.0) as i32;
                    }
                }
            },
            on_error,
            None,
        ),
        SampleFormat::I16 => device.build_output_stream(
            &config,
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                for chunk in data.chunks_mut(scratch.len()) {
                    let s = &mut scratch[..chunk.len()];
                    driver.process(s, channels);
                    for (d, x) in chunk.iter_mut().zip(s.iter()) {
                        *d = (*x * 32_767.0) as i16;
                    }
                }
            },
            on_error,
            None,
        ),
        SampleFormat::U16 => device.build_output_stream(
            &config,
            move |data: &mut [u16], _: &cpal::OutputCallbackInfo| {
                for chunk in data.chunks_mut(scratch.len()) {
                    let s = &mut scratch[..chunk.len()];
                    driver.process(s, channels);
                    for (d, x) in chunk.iter_mut().zip(s.iter()) {
                        *d = ((*x * 0.5 + 0.5) * 65_535.0) as u16;
                    }
                }
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
    Ok((stream, channels, buffer_frames))
}

impl AudioBackend for CpalBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn devices(&self) -> Vec<DeviceInfo> {
        let Ok(host) = self.host() else {
            return Vec::new();
        };
        let default_name = host.default_output_device().and_then(|d| d.name().ok());
        let Ok(devices) = host.output_devices() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for d in devices {
            let Ok(name) = d.name() else { continue };
            let Ok(configs) = d.supported_output_configs() else {
                continue;
            };
            let configs: Vec<_> = configs.collect();
            if configs.is_empty() {
                continue;
            }
            let max_output_channels = configs
                .iter()
                .map(|c| c.channels() as usize)
                .max()
                .unwrap_or(0);
            let sample_rates: Vec<u32> = COMMON_RATES
                .iter()
                .copied()
                .filter(|r| {
                    configs
                        .iter()
                        .any(|c| c.min_sample_rate().0 <= *r && *r <= c.max_sample_rate().0)
                })
                .collect();
            let default_sample_rate = d
                .default_output_config()
                .map(|c| c.sample_rate().0)
                .ok()
                .or_else(|| sample_rates.first().copied())
                .unwrap_or(48_000);
            out.push(DeviceInfo {
                id: name.clone(),
                is_default: default_name.as_deref() == Some(name.as_str()),
                name,
                max_output_channels,
                default_sample_rate,
                sample_rates,
            });
        }
        out
    }

    fn open(
        &self,
        req: StreamRequest,
        renderer: Renderer,
    ) -> Result<Box<dyn OutputStream>, AudioError> {
        if req.sample_rate != renderer.sample_rate() {
            return Err(AudioError::InvalidParam(format!(
                "stream rate {} differs from engine rate {}",
                req.sample_rate,
                renderer.sample_rate()
            )));
        }
        if req.channels == 0 {
            return Err(AudioError::InvalidParam("0 channels requested".into()));
        }
        let status = Arc::new(StreamStatus::new());
        let driver = Driver::new(renderer, Arc::clone(&status));
        let host_id = self.host_id;
        let backend = self.name.clone();
        let status2 = Arc::clone(&status);
        let (ready_tx, ready_rx) = mpsc::channel::<Result<StreamInfo, AudioError>>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();

        // cpal streams are not `Send` on every platform, so the stream
        // lives on its own thread for its whole life.
        let thread = thread::Builder::new()
            .name("sm-audio-device".into())
            .spawn(move || {
                let opened = (|| {
                    let host = cpal::host_from_id(host_id)
                        .map_err(|e| AudioError::Backend(e.to_string()))?;
                    let device = match &req.device_id {
                        None => host
                            .default_output_device()
                            .ok_or_else(|| AudioError::DeviceNotFound("default output".into()))?,
                        Some(id) => host
                            .output_devices()
                            .map_err(|e| AudioError::Backend(e.to_string()))?
                            .find(|d| d.name().is_ok_and(|n| &n == id))
                            .ok_or_else(|| AudioError::DeviceNotFound(id.clone()))?,
                    };
                    let device_name = device.name().unwrap_or_else(|_| "unknown".to_string());
                    let (stream, channels, buffer_frames) =
                        build_stream(&device, &req, driver, status2)?;
                    let info = StreamInfo {
                        backend,
                        device_name,
                        sample_rate: req.sample_rate,
                        channels,
                        buffer_frames,
                    };
                    Ok::<_, AudioError>((stream, info))
                })();
                match opened {
                    Ok((stream, info)) => {
                        if ready_tx.send(Ok(info)).is_ok() {
                            // Park until the handle is dropped.
                            let _ = stop_rx.recv();
                        }
                        drop(stream);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            })
            .map_err(|e| AudioError::Backend(format!("cannot spawn device thread: {e}")))?;

        match ready_rx.recv() {
            Ok(Ok(info)) => Ok(Box::new(CpalStream {
                info,
                status,
                stop: Some(stop_tx),
                thread: Some(thread),
            })),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(AudioError::Backend(
                    "device thread died while opening".into(),
                ))
            }
        }
    }
}

struct CpalStream {
    info: StreamInfo,
    status: Arc<StreamStatus>,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl OutputStream for CpalStream {
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

impl Drop for CpalStream {
    fn drop(&mut self) {
        drop(self.stop.take()); // wakes the device thread
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
