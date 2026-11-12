//! Incoming MIDI on the runtime thread: activity display, remote-control
//! mapping and MIDI Show Control. Timecode input goes straight to the engine
//! (see `Runtime::process_inputs`).

use serde_json::{json, Value};
use sm_core::chase::TcStatus;
use sm_core::engine::Command;
use sm_core::midi_in::{
    action_label, classify, describe, listened, mapping_commands, msc_commands, Incoming,
    MidiActivity, MidiMatcher, MscOutcome,
};
use sm_core::model::{Show, TcInputKind, TcSource, TimecodeSettings};
use sm_io::inputs::{InputStatus, MidiIn};
use std::time::{Duration, Instant};

/// `generator`, `mtc` or `ltc` (tick `tc.source`).
pub fn tc_source_name(tc: &TimecodeSettings) -> &'static str {
    match (tc.source, tc.input.kind) {
        (TcSource::Generator, _) => "generator",
        (TcSource::External, TcInputKind::Mtc) => "mtc",
        (TcSource::External, TcInputKind::Ltc) => "ltc",
    }
}

/// The state document is republished at most this often for MIDI activity.
const UI_INTERVAL: Duration = Duration::from_millis(200);

pub struct InputState {
    matcher: MidiMatcher,
    started: Instant,
    /// Messages received (everything except MTC / real-time bytes).
    count: u64,
    last: Option<MidiActivity>,
    changed: bool,
    shown: Option<Instant>,
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            matcher: MidiMatcher::default(),
            started: Instant::now(),
            count: 0,
            last: None,
            changed: false,
            shown: None,
        }
    }
}

impl InputState {
    /// Handle received messages: returns log lines and commands to execute.
    pub fn handle(
        &mut self,
        show: &Show,
        midi: Vec<MidiIn>,
    ) -> (Vec<(&'static str, String)>, Vec<Command>) {
        let mut notes = Vec::new();
        let mut commands = Vec::new();
        let now_ms = self.started.elapsed().as_millis() as u64;
        for m in midi {
            if let Some(a) = MidiActivity::of(&m.port, &m.bytes, self.count + 1) {
                self.count += 1;
                self.last = Some(a);
                self.changed = true;
            }
            if !listened(show, &show.midi_input, &m.port) {
                continue;
            }
            if let Incoming::Msc(msc) = classify(&m.bytes) {
                match msc_commands(show, &show.midi_input.msc, &msc) {
                    MscOutcome::NotForUs => {}
                    MscOutcome::Ignored(why) => notes.push((
                        "warn",
                        format!(
                            "MSC received from {}: {} - ignored: {why}",
                            m.port,
                            describe(&m.bytes)
                        ),
                    )),
                    MscOutcome::Run(cmds, what) => {
                        notes.push((
                            "info",
                            format!(
                                "MSC received from {}: {} -> {what}",
                                m.port,
                                describe(&m.bytes)
                            ),
                        ));
                        commands.extend(cmds);
                    }
                }
                continue;
            }
            let mapped: Vec<_> = self
                .matcher
                .matches(show, &m.port, &m.bytes, now_ms)
                .into_iter()
                .cloned()
                .collect();
            for map in mapped {
                let mut what = action_label(map.action).to_string();
                if map.action.needs_cue() {
                    what.push_str(&format!(" {}", map.cue_number));
                }
                match mapping_commands(show, &map) {
                    Ok(cmds) => {
                        notes.push((
                            "info",
                            format!("MIDI {} ({}) -> {what}", describe(&m.bytes), m.port),
                        ));
                        commands.extend(cmds);
                    }
                    Err(e) => notes.push((
                        "warn",
                        format!("MIDI {} ({}) -> {what}: {e}", describe(&m.bytes), m.port),
                    )),
                }
            }
        }
        (notes, commands)
    }

    /// True when the UI should get a fresh state document (rate limited).
    pub fn take_ui_update(&mut self) -> bool {
        if self.changed && self.shown.is_none_or(|t| t.elapsed() >= UI_INTERVAL) {
            self.changed = false;
            self.shown = Some(Instant::now());
            return true;
        }
        false
    }

    /// `state.inputs`.
    pub fn json(&self, status: InputStatus, tc_status: TcStatus) -> Value {
        json!({
            "tc_status": tc_status,
            "midi_ports": status.midi_ports,
            "midi_open": status.midi_open,
            "midi_count": self.count,
            "midi_last": self.last,
            "audio_inputs": status.audio_inputs,
            "ltc_open": status.ltc_open,
            "ltc_level_db": status.ltc_level_db,
            "mtc_rate": status.mtc_rate,
        })
    }
}
