//! Everything that comes *in*: MIDI (system ports, RTP-MIDI peers), MTC
//! and LTC timecode. Owned by [`crate::Io`] on the engine thread; all device
//! work happens on the input threads, [`Inputs::poll`] never blocks.

use crate::clock::ClockMap;
use crate::midi_in::{InboundMidi, MidiInputBackend, MidiInputHub, WantedInputs};
use crate::tc_in::{AudioInputWorker, LtcInput};
use serde::Serialize;
use sm_audio::input::{AudioInputBackend, InputDeviceInfo, InputRequest};
use sm_core::chase::TcObservation;
use sm_core::model::{MidiInputMode, Show, TcInputKind, TcSource};
use sm_rtpmidi::Session;
use sm_timecode::mtc_in::{mtc_rate_label, mtc_rate_matches, mtc_rate_of};
use sm_timecode::{FrameRate, MtcEvent, MtcReceiver};
use std::sync::Arc;
use std::time::Instant;

/// A received MIDI message (not timecode), stamped in engine time.
#[derive(Clone, Debug, PartialEq)]
pub struct MidiIn {
    pub port: String,
    pub bytes: Vec<u8>,
    pub frame: u64,
}

/// Timecode from the input chosen in the show.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TcInEvent {
    Frame(TcObservation),
    /// The source stopped / located (MTC Full Frame) at this frame.
    Stopped(u64),
}

/// What [`Inputs::poll`] collected.
#[derive(Default, Debug)]
pub struct InputBatch {
    pub midi: Vec<MidiIn>,
    pub tc: Vec<TcInEvent>,
    /// (level, message) for the event log.
    pub log: Vec<(&'static str, String)>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct AudioInputEntry {
    pub id: String,
    pub name: String,
    pub channels: usize,
    pub is_default: bool,
}

/// Input state for the UI (`state.inputs`).
#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct InputStatus {
    /// MIDI inputs that can be listened to (system ports and the RTP-MIDI session).
    pub midi_ports: Vec<String>,
    /// MIDI inputs currently open.
    pub midi_open: Vec<String>,
    /// Audio capture devices (`None` until scanned: only while LTC input is selected).
    pub audio_inputs: Option<Vec<AudioInputEntry>>,
    /// The LTC capture stream is open.
    pub ltc_open: bool,
    /// Peak level of the LTC input over the last second (dBFS).
    pub ltc_level_db: Option<f32>,
    /// Rate announced by the incoming MTC.
    pub mtc_rate: Option<String>,
    /// Rate the incoming LTC was found to run at.
    pub ltc_rate: Option<String>,
}

pub struct Inputs {
    midi: MidiInputHub,
    audio: AudioInputWorker,
    mtc: MtcReceiver,
    ltc: Option<LtcInput>,
    rtp: Option<Arc<Session>>,
    rtp_port: Option<String>,
    /// `Some` while chasing.
    tc_kind: Option<TcInputKind>,
    rate: FrameRate,
    /// Ports MTC is taken from; `None` = any open port.
    mtc_ports: Option<Vec<String>>,
    ltc_req: Option<InputRequest>,
    ltc_generation: u64,
    /// Audio input devices are being listed (LTC input selected).
    scanning: bool,
    mtc_rate: Option<u8>,
    mtc_rate_warned: bool,
    ltc_peak: f32,
    ltc_level: Option<f32>,
    level_since: Instant,
}

impl Inputs {
    pub fn new(midi: Box<dyn MidiInputBackend>, audio: Box<dyn AudioInputBackend>) -> Self {
        Self {
            midi: MidiInputHub::new(midi),
            audio: AudioInputWorker::new(audio),
            mtc: MtcReceiver::new(),
            ltc: None,
            rtp: None,
            rtp_port: None,
            tc_kind: None,
            rate: FrameRate::default(),
            mtc_ports: None,
            ltc_req: None,
            ltc_generation: 0,
            scanning: false,
            mtc_rate: None,
            mtc_rate_warned: false,
            ltc_peak: 0.0,
            ltc_level: None,
            level_since: Instant::now(),
        }
    }

    /// Follow the show settings (ports to open, timecode input).
    pub fn configure(&mut self, show: &Show, rtp: Option<Arc<Session>>) {
        let mi = &show.midi_input;
        let tc = &show.timecode;
        let external = tc.source == TcSource::External;
        let system = |name: &str| -> Option<String> {
            let device = show.resolve_midi_port(name);
            (!device.is_empty() && !device.starts_with(crate::midi::RTP_PREFIX))
                .then(|| device.to_string())
        };
        let mtc_port = (external && tc.input.kind == TcInputKind::Mtc)
            .then(|| system(&tc.input.mtc_port))
            .flatten();
        let wanted = match mi.mode {
            MidiInputMode::None => WantedInputs::Named(mtc_port.iter().cloned().collect()),
            MidiInputMode::All => WantedInputs::All {
                extra: mtc_port.iter().cloned().collect(),
            },
            MidiInputMode::Selected => WantedInputs::Named(
                mi.ports
                    .iter()
                    .filter_map(|p| system(p))
                    .chain(mtc_port.iter().cloned())
                    .collect(),
            ),
        };
        self.midi.set_wanted(wanted);

        let kind = external.then_some(tc.input.kind);
        if kind != self.tc_kind || tc.rate != self.rate {
            self.mtc.reset();
            self.mtc_rate = None;
            self.mtc_rate_warned = false;
        }
        self.tc_kind = kind;
        self.rate = tc.rate;
        self.mtc_ports = (!tc.input.mtc_port.is_empty()).then(|| {
            vec![
                tc.input.mtc_port.clone(),
                show.resolve_midi_port(&tc.input.mtc_port).to_string(),
            ]
        });
        let ltc_req = (kind == Some(TcInputKind::Ltc)).then(|| InputRequest {
            device_id: tc.input.ltc_device.clone(),
            channel: usize::from(tc.input.ltc_channel),
        });
        if ltc_req != self.ltc_req {
            self.ltc = None;
            self.ltc_generation += 1;
            match &ltc_req {
                Some(r) => self.audio.open(r.clone(), self.ltc_generation),
                None => self.audio.close(),
            }
            self.ltc_req = ltc_req;
        }
        let scan = tc.input.kind == TcInputKind::Ltc;
        if scan != self.scanning {
            self.scanning = scan;
            self.audio.scan(scan);
        }

        // RTP-MIDI peers deliver into the same channel as system ports.
        let same = match (&self.rtp, &rtp) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.rtp_port = rtp
                .as_ref()
                .map(|s| format!("{}{}", crate::midi::RTP_PREFIX, s.name()));
            if let (Some(s), Some(port)) = (&rtp, &self.rtp_port) {
                let tx = self.midi.sender();
                let port = port.clone();
                s.set_receiver(Box::new(move |bytes| {
                    if !crate::midi_in::is_noise(bytes) {
                        let _ = tx.send(InboundMidi {
                            port: port.clone(),
                            bytes: bytes.to_vec(),
                            at: Instant::now(),
                        });
                    }
                }));
            }
            self.rtp = rtp;
        }
    }

    fn mtc_from(&self, port: &str) -> bool {
        self.tc_kind == Some(TcInputKind::Mtc)
            && self
                .mtc_ports
                .as_ref()
                .is_none_or(|p| p.iter().any(|x| x == port))
    }

    /// Collect everything received since the last call.
    pub fn poll(&mut self, clock: &ClockMap, base: u64) -> InputBatch {
        let mut batch = InputBatch::default();
        for m in self.midi.take_messages() {
            let level = if m.ends_with("connected") {
                "info"
            } else {
                "warn"
            };
            batch.log.push((level, m));
        }
        for m in self.audio.take_messages() {
            let level = if m.contains("error") { "error" } else { "info" };
            batch.log.push((level, m));
        }
        for (generation, result) in self.audio.poll_opened() {
            if generation != self.ltc_generation {
                continue;
            }
            match result {
                Ok(reader) => self.ltc = Some(LtcInput::new(reader)),
                Err(e) => batch
                    .log
                    .push(("error", format!("LTC input cannot be opened: {e}"))),
            }
        }
        let to_engine = |at: Instant| base + clock.frame_of(at);
        let rate = self.rate;
        let day = rate.frames_per_day();
        for m in self.midi.drain() {
            let frame = to_engine(m.at);
            let mtc = matches!(m.bytes.first(), Some(0xF1))
                || sm_timecode::mtc_in::parse_full_frame(&m.bytes).is_some();
            if !mtc {
                batch.midi.push(MidiIn {
                    port: m.port,
                    bytes: m.bytes,
                    frame,
                });
                continue;
            }
            if !self.mtc_from(&m.port) {
                continue;
            }
            let (tc, code, stopped) = match self.mtc.push(&m.bytes, frame) {
                Some(MtcEvent::Frame {
                    timecode,
                    rate_code,
                    at,
                }) => (timecode, rate_code, Some(at)),
                Some(MtcEvent::FullFrame {
                    timecode,
                    rate_code,
                    ..
                }) => (timecode, rate_code, None),
                None => continue,
            };
            self.mtc_rate = Some(code);
            if !mtc_rate_matches(code, rate) && !self.mtc_rate_warned {
                self.mtc_rate_warned = true;
                batch.log.push((
                    "info",
                    format!(
                        "Incoming MTC is {}, the show runs at {}: converting by timecode value",
                        mtc_rate_label(code),
                        rate.label()
                    ),
                ));
            }
            // The source's own rate (from the MTC rate code), mapped onto the show's.
            let src = if mtc_rate_matches(code, rate) {
                rate
            } else {
                mtc_rate_of(code)
            };
            let (n, offset) = sm_timecode::convert_frame(
                tc.to_frame_number(src) % src.frames_per_day(),
                src,
                rate,
            );
            let n = n % day;
            let back = (offset * clock.sample_rate()) as u64;
            batch.tc.push(match stopped {
                Some(at) => TcInEvent::Frame(TcObservation {
                    frame: n,
                    at: at.saturating_sub(back),
                }),
                None => TcInEvent::Stopped(n),
            });
        }
        if let Some(ltc) = self.ltc.as_mut() {
            let (obs, log) = ltc.poll(rate, clock.sample_rate(), to_engine);
            batch.tc.extend(obs.into_iter().map(TcInEvent::Frame));
            batch.log.extend(log.into_iter().map(|m| ("warn", m)));
            self.ltc_peak = self.ltc_peak.max(ltc.take_peak());
        }
        if self.level_since.elapsed().as_millis() >= 1000 {
            self.level_since = Instant::now();
            self.ltc_level = self.ltc.as_ref().map(|_| {
                if self.ltc_peak > 1.0e-6 {
                    20.0 * self.ltc_peak.log10()
                } else {
                    -120.0
                }
            });
            self.ltc_peak = 0.0;
        }
        batch
    }

    pub fn status(&self) -> InputStatus {
        let mut ports = self.midi.available();
        let mut open = self.midi.open_ports();
        if let Some(p) = &self.rtp_port {
            ports.push(p.clone());
            open.push(p.clone());
        }
        InputStatus {
            midi_ports: ports,
            midi_open: open,
            audio_inputs: self.audio.devices().map(|d| {
                d.into_iter()
                    .map(
                        |InputDeviceInfo {
                             id,
                             name,
                             channels,
                             is_default,
                             ..
                         }| AudioInputEntry {
                            id,
                            name,
                            channels,
                            is_default,
                        },
                    )
                    .collect()
            }),
            ltc_open: self.ltc.is_some(),
            ltc_level_db: self.ltc_level,
            mtc_rate: self.mtc_rate.map(|c| mtc_rate_label(c).to_string()),
            ltc_rate: self
                .ltc
                .as_ref()
                .and_then(LtcInput::detected_rate)
                .map(|r| r.label().to_string()),
        }
    }
}
