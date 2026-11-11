//! Glue between the cue engine and the outside world.

use crate::audio::{AudioStatus, AudioSystem};
use crate::clock::ClockMap;
use crate::midi::{default_backend, MidiBackend, MidiHub};
use serde::Serialize;
use sm_audio::{FadeCurve, FileSource, FileSourceOpts, VoiceParams};
use sm_core::db_to_lin;
use sm_core::engine::{Action, VoiceId, VoiceSpec};
use sm_core::model::{AudioSettings, Curve, Show};
use sm_core::showfile::resolve_media;
use sm_timecode::MtcGenerator;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Instant;

fn curve(c: Curve) -> FadeCurve {
    match c {
        Curve::Linear => FadeCurve::Linear,
        Curve::EqualPower => FadeCurve::EqualPower,
        Curve::SCurve => FadeCurve::SCurve,
    }
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct IoStatus {
    pub cpu: f32,
    pub underruns: u64,
    pub voices: usize,
}

/// Owns the audio stream, MIDI outputs and timecode generators.
pub struct Io {
    audio: AudioSystem,
    audio_settings: AudioSettings,
    pub midi: MidiHub,
    mtc: MtcGenerator,
    mtc_port: Option<String>,
    clock: ClockMap,
    /// Engine time = `base` + device clock (the device clock restarts at 0
    /// whenever the stream is reopened; engine time never goes backwards).
    base: u64,
    voices: HashMap<VoiceId, sm_audio::VoiceId>,
    /// Voices that have been started (stop-all only touches these, so the
    /// pre-opened standby voices stay ready for the next GO).
    started: std::collections::HashSet<VoiceId>,
    show_dir: Option<PathBuf>,
    underruns_before: u64,
}

impl Io {
    pub fn new(show: &Show, show_dir: Option<PathBuf>) -> Self {
        Self::with_midi_backend(show, show_dir, default_backend())
    }

    pub fn with_midi_backend(
        show: &Show,
        show_dir: Option<PathBuf>,
        midi: Box<dyn MidiBackend>,
    ) -> Self {
        let audio = AudioSystem::open(&show.settings.audio);
        Self::with_audio(show, show_dir, midi, audio)
    }

    /// Build on an already opened audio system (offline rendering, tests).
    pub fn with_audio(
        show: &Show,
        show_dir: Option<PathBuf>,
        midi: Box<dyn MidiBackend>,
        audio: AudioSystem,
    ) -> Self {
        let sr = audio.controller.sample_rate();
        let mut io = Self {
            audio,
            audio_settings: show.settings.audio.clone(),
            midi: MidiHub::new(midi),
            mtc: MtcGenerator::new(sr),
            mtc_port: None,
            clock: ClockMap::new(sr),
            base: 0,
            voices: HashMap::new(),
            started: Default::default(),
            show_dir,
            underruns_before: 0,
        };
        io.configure(show);
        io
    }

    /// True once every prepared voice has its pre-roll buffered.
    pub fn all_primed(&self) -> bool {
        self.voices
            .values()
            .all(|v| self.audio.controller.is_primed(*v))
    }

    pub fn sample_rate(&self) -> u32 {
        self.audio.controller.sample_rate()
    }

    /// Current engine time in frames.
    pub fn now(&mut self) -> u64 {
        let c = self.audio.controller.clock();
        self.clock.observe(c, Instant::now());
        self.base + c
    }

    pub fn set_show_dir(&mut self, dir: Option<PathBuf>) {
        self.show_dir = dir;
    }

    /// Apply show settings that live in the I/O layer (LTC, MTC, RTP-MIDI).
    /// Returns warnings to show in the log.
    pub fn configure(&mut self, show: &Show) -> Vec<String> {
        let mut warnings = Vec::new();
        let tc = &show.timecode;
        self.audio
            .ltc
            .channel
            .store(usize::from(tc.ltc_channel), Ordering::Relaxed);
        self.audio.ltc.set_gain(db_to_lin(tc.ltc_level_db).min(1.0));
        self.audio
            .ltc
            .enabled
            .store(tc.ltc_enabled, Ordering::Relaxed);
        self.mtc_port = (tc.mtc_enabled && !tc.mtc_port.is_empty())
            .then(|| show.resolve_midi_port(&tc.mtc_port).to_string());
        if let Err(e) = self.midi.configure_rtp(&show.rtp_midi) {
            warnings.push(e);
        }
        warnings
    }

    /// Does the stream need reopening for these settings?
    pub fn audio_settings_changed(&self, settings: &AudioSettings) -> bool {
        self.audio_settings != *settings
    }

    /// Close and reopen the audio stream. All voices are lost; the caller
    /// must reset the engine's running cues first. Returns true when the
    /// sample rate changed (the engine must then be rebuilt).
    pub fn reopen_audio(&mut self, show: &Show) -> bool {
        let old_sr = self.sample_rate();
        self.base = self.now();
        self.voices.clear();
        self.started.clear();
        self.audio = AudioSystem::open(&show.settings.audio);
        self.audio_settings = show.settings.audio.clone();
        let sr = self.sample_rate();
        if sr != old_sr {
            self.base = 0;
        }
        self.clock = ClockMap::new(sr);
        self.mtc = MtcGenerator::new(sr);
        self.underruns_before = 0;
        self.configure(show);
        sr != old_sr
    }

    pub fn audio_status(&self) -> AudioStatus {
        self.audio.status()
    }

    pub fn status(&self) -> IoStatus {
        IoStatus {
            cpu: self.audio.cpu_load(),
            underruns: self.audio.controller.underruns(),
            voices: self.audio.controller.active_voices(),
        }
    }

    pub fn meters(&self) -> sm_audio::Meters {
        self.audio.controller.meters()
    }

    pub fn reset_clip(&self) {
        self.audio.controller.reset_clip();
    }

    fn local(&self, frame: u64) -> u64 {
        frame.saturating_sub(self.base)
    }

    fn open_voice(&mut self, spec: &VoiceSpec) -> Result<sm_audio::VoiceId, String> {
        let sr = self.sample_rate();
        let path = resolve_media(self.show_dir.as_deref(), &spec.file);
        let mut opts = FileSourceOpts::new(sr);
        opts.start_seconds = spec.start;
        opts.end_seconds = spec.end;
        opts.loop_count = spec.loop_count;
        if spec.offset_frames > 0 {
            let mut offset = spec.offset_frames as f64 / f64::from(sr);
            if spec.loop_count != 1 {
                // The offset counts from the first pass: skip whole passes.
                let info = sm_audio::probe(&path).map_err(|e| e.to_string())?;
                let end = spec.end.or(info.duration_seconds).unwrap_or(0.0);
                let region = end - spec.start;
                if region > 0.0 {
                    let passes = (offset / region).floor();
                    offset -= passes * region;
                    if spec.loop_count > 1 {
                        opts.loop_count = spec.loop_count.saturating_sub(passes as u32).max(1);
                    }
                }
            }
            opts.offset_seconds = offset;
        }
        let source = FileSource::open(&path, opts).map_err(|e| format!("{}: {e}", spec.file))?;
        let params = VoiceParams {
            gain: spec.gain,
            matrix: spec.matrix.clone(),
        };
        self.audio
            .controller
            .prepare_voice(Box::new(source), params)
            .map_err(|e| e.to_string())
    }

    /// Execute engine actions. Returns voices that failed (to be reported
    /// back to the engine) with a message.
    pub fn apply(&mut self, actions: Vec<Action>) -> Vec<(VoiceId, String)> {
        let mut failed = Vec::new();
        for action in actions {
            let result: Result<(), String> = match action {
                Action::PrepareVoice { voice, spec, .. } => match self.open_voice(&spec) {
                    Ok(v) => {
                        self.voices.insert(voice, v);
                        Ok(())
                    }
                    Err(e) => {
                        failed.push((voice, e));
                        Ok(())
                    }
                },
                Action::StartVoice {
                    voice,
                    at,
                    fade_in_frames,
                    curve: c,
                } => match self.voices.get(&voice) {
                    Some(v) => {
                        self.started.insert(voice);
                        self.audio
                            .controller
                            .start(*v, self.local(at), fade_in_frames, curve(c))
                            .map_err(|e| e.to_string())
                    }
                    None => Ok(()), // prepare failed; already reported
                },
                Action::StopVoice {
                    voice,
                    at,
                    fade_frames,
                    curve: c,
                    hard,
                } => match self.voices.remove(&voice) {
                    Some(v) => self
                        .audio
                        .controller
                        .stop(v, self.local(at), fade_frames, curve(c), hard)
                        .map_err(|e| e.to_string()),
                    None => Ok(()),
                },
                Action::PauseVoice { voice, at } => match self.voices.get(&voice) {
                    Some(v) => self
                        .audio
                        .controller
                        .pause(*v, self.local(at))
                        .map_err(|e| e.to_string()),
                    None => Ok(()),
                },
                Action::ResumeVoice { voice, at } => match self.voices.get(&voice) {
                    Some(v) => self
                        .audio
                        .controller
                        .resume(*v, self.local(at))
                        .map_err(|e| e.to_string()),
                    None => Ok(()),
                },
                Action::FadeVoice {
                    voice,
                    at,
                    frames,
                    curve: c,
                    master,
                    crosspoints,
                } => match self.voices.get(&voice) {
                    Some(v) => self
                        .audio
                        .controller
                        .fade(*v, self.local(at), frames, curve(c), master, &crosspoints)
                        .map_err(|e| e.to_string()),
                    None => Ok(()),
                },
                Action::DiscardVoice { voice } => match self.voices.remove(&voice) {
                    Some(v) => self.audio.controller.discard(v).map_err(|e| e.to_string()),
                    None => Ok(()),
                },
                Action::StopAllAudio {
                    at,
                    fade_frames,
                    hard,
                } => {
                    let at = self.local(at);
                    let mut result = Ok(());
                    for id in std::mem::take(&mut self.started) {
                        if let Some(v) = self.voices.remove(&id) {
                            if let Err(e) = self.audio.controller.stop(
                                v,
                                at,
                                fade_frames,
                                FadeCurve::EqualPower,
                                hard,
                            ) {
                                result = Err(e.to_string());
                            }
                        }
                    }
                    result
                }
                Action::Midi { at, port, bytes } => {
                    let due = self.clock.instant_of(self.local(at));
                    self.midi.send_at(due, &port, bytes);
                    Ok(())
                }
                Action::MidiPanic => {
                    self.midi.panic();
                    Ok(())
                }
                Action::Timecode(mut t) => {
                    t.anchor_sample = self.local(t.anchor_sample);
                    self.audio.set_transport(t);
                    self.mtc.set_transport(t);
                    Ok(())
                }
                Action::MasterGain { gain, ramp_frames } => self
                    .audio
                    .controller
                    .set_master_gain(gain, ramp_frames.min(u64::from(u32::MAX)) as u32)
                    .map_err(|e| e.to_string()),
            };
            if let Err(e) = result {
                tracing::error!("audio command failed: {e}");
            }
        }
        failed
    }

    /// Housekeeping, called every engine tick with the engine's logical time.
    /// Returns (failed voices, log messages).
    pub fn tick(&mut self, logical_now: u64) -> (Vec<(VoiceId, String)>, Vec<String>) {
        let mut failed = Vec::new();
        let mut log = Vec::new();
        for ev in self.audio.controller.poll_events() {
            match ev {
                sm_audio::AudioEvent::VoiceEnded { voice } => {
                    self.voices.retain(|_, v| *v != voice);
                    let voices = &self.voices;
                    self.started.retain(|id| voices.contains_key(id));
                }
                sm_audio::AudioEvent::Underrun { .. } => {}
                sm_audio::AudioEvent::SourceError { voice, message } => {
                    if let Some((id, _)) = self.voices.iter().find(|(_, v)| **v == voice) {
                        failed.push((*id, message));
                    }
                }
            }
        }
        let underruns = self.audio.controller.underruns();
        if underruns > self.underruns_before {
            log.push(format!(
                "AUDIO UNDERRUN ({} new, {underruns} total)",
                underruns - self.underruns_before
            ));
            self.underruns_before = underruns;
        }
        log.extend(self.midi.take_errors());
        // MTC runs on the same clock as everything else.
        let until = self.local(logical_now);
        let (mtc, midi, clock, port) = (&mut self.mtc, &mut self.midi, &self.clock, &self.mtc_port);
        mtc.advance(until, &mut |sample, bytes| {
            if let Some(port) = port {
                midi.send_at(clock.instant_of(sample), port, bytes.to_vec());
            }
        });
        (failed, log)
    }
}
