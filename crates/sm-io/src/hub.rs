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
        Curve::Smooth => FadeCurve::Smooth,
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
    devices: crate::audio::DeviceWatcher,
    last_reconnect: Option<std::time::Instant>,
    /// Grows while reconnect attempts keep landing on the silent fallback.
    reconnect_backoff: std::time::Duration,
    pub midi: MidiHub,
    http: crate::http_out::HttpOut,
    /// Backup computer in standby: no MIDI / OSC / HTTP / timecode output.
    external_muted: bool,
    ltc_enabled: bool,
    mtc: MtcGenerator,
    mtc_port: Option<String>,
    clock: ClockMap,
    /// Engine time = `base` + device clock (the device clock restarts at 0
    /// whenever the stream is reopened; engine time never goes backwards).
    base: u64,
    voices: HashMap<VoiceId, sm_audio::VoiceId>,
    /// Voices whose file is being opened in the background, with the
    /// commands that arrived meanwhile (replayed once the file is open).
    opening: HashMap<VoiceId, Opening>,
    opener: Opener,
    /// Lateness (ms) of cues that started after their file finished opening.
    late_starts: Vec<u64>,
    /// Bumped when the stream is reopened; results of older jobs are dropped.
    generation: u64,
    /// Voices that have been started (stop-all only touches these, so the
    /// pre-opened standby voices stay ready for the next GO).
    started: std::collections::HashSet<VoiceId>,
    show_dir: Option<PathBuf>,
    underruns_before: u64,
    /// MIDI input, MTC and LTC input.
    inputs: crate::inputs::Inputs,
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
            devices: crate::audio::DeviceWatcher::new(std::time::Duration::from_secs(3)),
            last_reconnect: None,
            reconnect_backoff: std::time::Duration::from_secs(5),
            midi: MidiHub::new(midi),
            http: crate::http_out::HttpOut::new(),
            external_muted: false,
            ltc_enabled: false,
            mtc: MtcGenerator::new(sr),
            mtc_port: None,
            clock: ClockMap::new(sr),
            base: 0,
            voices: HashMap::new(),
            opening: HashMap::new(),
            opener: Opener::new(),
            late_starts: Vec::new(),
            generation: 0,
            started: Default::default(),
            show_dir,
            underruns_before: 0,
            inputs: crate::inputs::Inputs::new(
                crate::midi_in::default_input_backend(),
                sm_audio::input::default_input_backend(),
            ),
        };
        io.configure(show);
        io
    }

    /// True once every prepared voice has its pre-roll buffered.
    pub fn all_primed(&mut self) -> bool {
        let _ = self.collect_opened();
        self.opening.is_empty()
            && self
                .voices
                .values()
                .all(|v| self.audio.controller.is_primed(*v))
    }

    /// Number of files still being opened in the background.
    pub fn opening_count(&self) -> usize {
        self.opening.len()
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
        self.ltc_enabled = tc.ltc_enabled;
        self.audio
            .ltc
            .enabled
            .store(tc.ltc_enabled && !self.external_muted, Ordering::Relaxed);
        self.mtc_port = (tc.mtc_enabled && !tc.mtc_port.is_empty())
            .then(|| show.resolve_midi_port(&tc.mtc_port).to_string());
        if let Err(e) = self.midi.configure_rtp(&show.rtp_midi) {
            warnings.push(e);
        }
        self.inputs.configure(show, self.midi.rtp_session());
        warnings
    }

    /// Backup computer: silence everything that leaves this machine except
    /// the audio itself (MIDI incl. MTC and RTP-MIDI, OSC, HTTP, LTC), so a
    /// device wired to both computers does not get every command twice.
    pub fn set_external_muted(&mut self, muted: bool) {
        self.external_muted = muted;
        self.midi.set_muted(muted);
        self.http.set_muted(muted);
        self.audio
            .ltc
            .enabled
            .store(self.ltc_enabled && !muted, Ordering::Relaxed);
    }

    /// Replace the input backends (tests, virtual devices) and apply the show.
    pub fn set_input_backends(
        &mut self,
        show: &Show,
        midi: Box<dyn crate::midi_in::MidiInputBackend>,
        audio: Box<dyn sm_audio::input::AudioInputBackend>,
    ) {
        self.inputs = crate::inputs::Inputs::new(midi, audio);
        self.inputs.configure(show, self.midi.rtp_session());
    }

    /// Everything received on the inputs since the last call, stamped in
    /// engine time. Never blocks.
    pub fn poll_inputs(&mut self) -> crate::inputs::InputBatch {
        self.inputs.poll(&self.clock, self.base)
    }

    pub fn input_status(&self) -> crate::inputs::InputStatus {
        self.inputs.status()
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
        self.opening.clear();
        self.generation += 1;
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
        let wanted_null = show
            .settings
            .audio
            .backend
            .as_deref()
            .is_some_and(|b| b.eq_ignore_ascii_case("null"));
        self.reconnect_backoff = if self.audio.status().is_null && !wanted_null {
            (self.reconnect_backoff * 2).min(std::time::Duration::from_secs(60))
        } else {
            std::time::Duration::from_secs(5)
        };
        sr != old_sr
    }

    pub fn audio_status(&self) -> AudioStatus {
        let mut status = self.audio.status();
        if let Some(devices) = self.devices.devices() {
            status.devices = devices;
        }
        status
    }

    /// Why the audio output should be reopened now, if it should (device came
    /// back after being lost, or became available while on the silent
    /// fallback). Rate limited; never blocks (uses the watcher's last scan).
    pub fn reconnect_reason(&mut self, cues_running: bool) -> Option<&'static str> {
        if self
            .last_reconnect
            .is_some_and(|t| t.elapsed() < self.reconnect_backoff)
        {
            return None;
        }
        let devices = self.devices.devices()?;
        let status = self.audio.status();
        let failed = self.audio.stream_failed();
        if !crate::audio::should_reconnect(
            &self.audio_settings,
            &status,
            failed,
            &devices,
            cues_running,
        ) {
            return None;
        }
        self.last_reconnect = Some(std::time::Instant::now());
        Some(if failed {
            "the audio device failed"
        } else {
            "an audio device is available again"
        })
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

    /// Queue opening a voice's file on a background thread.
    fn request_open(&mut self, voice: VoiceId, spec: VoiceSpec) {
        let job = OpenJob {
            voice,
            generation: self.generation,
            path: resolve_media(self.show_dir.as_deref(), &spec.file),
            sample_rate: self.sample_rate(),
            spec,
        };
        self.opening.insert(voice, Opening::default());
        if let Err(job) = self.opener.submit(job) {
            // Opener threads gone (should not happen): open right here.
            let result = open_source(&job);
            self.opener.results_push(job.voice, job.generation, result);
        }
    }

    /// Take finished background opens: register the voices with the mixer and
    /// replay the commands that arrived meanwhile. Returns failed voices.
    fn collect_opened(&mut self) -> Vec<(VoiceId, String)> {
        let mut failed = Vec::new();
        for (voice, generation, result) in self.opener.poll() {
            if generation != self.generation {
                continue;
            }
            let Some(pending) = self.opening.remove(&voice) else {
                continue;
            };
            if pending.cancelled {
                continue; // stopped or discarded before the file was open
            }
            match result.and_then(|(source, params)| {
                self.audio
                    .controller
                    .prepare_voice(source, params)
                    .map_err(|e| e.to_string())
            }) {
                Ok(v) => {
                    self.voices.insert(voice, v);
                    let clock = self.audio.controller.clock();
                    for op in &pending.ops {
                        if let Action::StartVoice { at, .. } = op {
                            let late = clock.saturating_sub(self.local(*at));
                            let ms = late * 1000 / u64::from(self.sample_rate().max(1));
                            if ms > 10 {
                                self.late_starts.push(ms);
                            }
                        }
                    }
                    failed.extend(self.apply(pending.ops));
                }
                Err(e) => failed.push((voice, e)),
            }
        }
        failed
    }

    fn queue(&mut self, voice: VoiceId, action: Action) {
        if let Some(p) = self.opening.get_mut(&voice) {
            p.ops.push(action);
        }
    }

    /// Execute engine actions. Returns voices that failed (to be reported
    /// back to the engine) with a message.
    pub fn apply(&mut self, actions: Vec<Action>) -> Vec<(VoiceId, String)> {
        let failed = Vec::new();
        for action in actions {
            let result: Result<(), String> = match action {
                Action::PrepareVoice { voice, spec, .. } => {
                    self.request_open(voice, spec);
                    Ok(())
                }
                // Commands for a voice whose file is still opening wait for it.
                Action::StartVoice { voice, .. } if self.opening.contains_key(&voice) => {
                    self.started.insert(voice);
                    self.queue(voice, action);
                    Ok(())
                }
                Action::StopVoice { voice, .. } | Action::DiscardVoice { voice }
                    if self.opening.contains_key(&voice) =>
                {
                    if let Some(p) = self.opening.get_mut(&voice) {
                        p.cancelled = true;
                    }
                    self.started.remove(&voice);
                    Ok(())
                }
                Action::PauseVoice { voice, .. }
                | Action::ResumeVoice { voice, .. }
                | Action::FadeVoice { voice, .. }
                    if self.opening.contains_key(&voice) =>
                {
                    self.queue(voice, action);
                    Ok(())
                }
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
                        if let Some(p) = self.opening.get_mut(&id) {
                            p.cancelled = true;
                        }
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
                Action::Http {
                    at,
                    addr,
                    bytes,
                    summary,
                } => {
                    let due = self.clock.instant_of(self.local(at));
                    self.http.send_at(due, addr, bytes, summary);
                    Ok(())
                }
                Action::Osc { at, target, bytes } => {
                    let due = self.clock.instant_of(self.local(at));
                    self.midi
                        .send_at(due, &format!("{}{target}", crate::midi::OSC_PREFIX), bytes);
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
        let mut failed = self.collect_opened();
        let mut log = Vec::new();
        for ms in std::mem::take(&mut self.late_starts) {
            log.push(format!("Cue started {ms} ms late: its audio file opened slowly (preload by putting the cue on the playhead, or use a faster disk)"));
        }
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
        log.extend(self.http.take_errors());
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

// ---------------------------------------------------------------------------
// Background file opening
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Opening {
    ops: Vec<Action>,
    cancelled: bool,
}

struct OpenJob {
    voice: VoiceId,
    generation: u64,
    path: PathBuf,
    sample_rate: u32,
    spec: VoiceSpec,
}

type Opened = Result<(Box<dyn sm_audio::SampleSource>, VoiceParams), String>;
type OpenResult = (VoiceId, u64, Opened);

/// Open and position a file source (disk I/O and decoder set-up). Runs on an
/// opener thread, never on the engine thread.
fn open_source(job: &OpenJob) -> Opened {
    let spec = &job.spec;
    let sr = job.sample_rate;
    let mut opts = FileSourceOpts::new(sr);
    opts.start_seconds = spec.start;
    opts.start_frame = spec.start_frame;
    opts.end_seconds = spec.end;
    opts.loop_count = spec.loop_count;
    opts.loop_start_seconds = spec.loop_from;
    if spec.loop_from.is_some() {
        // Vamp: the source maps an offset past the intro into the loop.
        opts.offset_seconds = spec.offset_frames as f64 / f64::from(sr.max(1));
    } else if spec.offset_frames > 0 {
        let mut offset = spec.offset_frames as f64 / f64::from(sr.max(1));
        if spec.loop_count != 1 {
            // The offset counts from the first pass: skip whole passes.
            let info = sm_audio::probe(&job.path).map_err(|e| e.to_string())?;
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
    let source = FileSource::open(&job.path, opts).map_err(|e| format!("{}: {e}", spec.file))?;
    Ok((
        Box::new(source),
        VoiceParams {
            gain: spec.gain,
            matrix: spec.matrix.clone(),
        },
    ))
}

/// Two worker threads that open files; results are collected by the engine.
struct Opener {
    jobs: Option<std::sync::mpsc::Sender<OpenJob>>,
    results_tx: std::sync::mpsc::Sender<OpenResult>,
    results: std::sync::mpsc::Receiver<OpenResult>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

const OPENER_THREADS: usize = 2;

impl Opener {
    fn new() -> Self {
        let (jobs_tx, jobs_rx) = std::sync::mpsc::channel::<OpenJob>();
        let (results_tx, results) = std::sync::mpsc::channel::<OpenResult>();
        let jobs_rx = std::sync::Arc::new(std::sync::Mutex::new(jobs_rx));
        let mut threads = Vec::new();
        for n in 0..OPENER_THREADS {
            let rx = jobs_rx.clone();
            let tx = results_tx.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("sm-open-{n}"))
                .spawn(move || loop {
                    let job = match rx.lock() {
                        Ok(guard) => guard.recv(),
                        Err(_) => return,
                    };
                    let Ok(job) = job else { return };
                    let result = open_source(&job);
                    if tx.send((job.voice, job.generation, result)).is_err() {
                        return;
                    }
                });
            match spawned {
                Ok(t) => threads.push(t),
                Err(e) => tracing::error!("cannot start file opener thread: {e}"),
            }
        }
        let jobs = (!threads.is_empty()).then_some(jobs_tx);
        Self {
            jobs,
            results_tx,
            results,
            threads,
        }
    }

    fn submit(&self, job: OpenJob) -> Result<(), Box<OpenJob>> {
        match &self.jobs {
            Some(tx) => tx.send(job).map_err(|e| Box::new(e.0)),
            None => Err(Box::new(job)),
        }
    }

    fn results_push(&self, voice: VoiceId, generation: u64, result: Opened) {
        let _ = self.results_tx.send((voice, generation, result));
    }

    fn poll(&self) -> Vec<OpenResult> {
        self.results.try_iter().collect()
    }
}

impl Drop for Opener {
    fn drop(&mut self) {
        self.jobs = None; // workers see the closed channel and exit
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}
