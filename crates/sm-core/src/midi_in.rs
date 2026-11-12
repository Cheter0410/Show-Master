//! Incoming MIDI: message classification, remote-control mapping and MIDI
//! Show Control receive. Pure; the runtime feeds messages and executes the
//! resulting [`Command`]s on the command bus (origin `midi`).

use crate::engine::Command;
use crate::midi::MscCommand;
use crate::model::{
    MidiInputMode, MidiInputSettings, MidiMapAction, MidiMapping, MidiTriggerKind,
    MscInputSettings, Show,
};
use serde::Serialize;
use std::collections::HashMap;

/// A classified incoming message (only what the input side cares about).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Incoming {
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
    },
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
    ProgramChange {
        channel: u8,
        program: u8,
    },
    Msc(MscIn),
    /// MTC quarter frame or full frame.
    Mtc,
    /// Clock, active sensing and other real-time bytes.
    Realtime,
    Other,
}

/// A received MIDI Show Control message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MscIn {
    pub device_id: u8,
    pub command_format: u8,
    /// Raw command code.
    pub code: u8,
    pub command: Option<MscCommand>,
    pub q_number: String,
    pub q_list: String,
    pub q_path: String,
}

fn msc_command(code: u8) -> Option<MscCommand> {
    Some(match code {
        0x01 => MscCommand::Go,
        0x02 => MscCommand::Stop,
        0x03 => MscCommand::Resume,
        0x04 => MscCommand::TimedGo,
        0x05 => MscCommand::Load,
        0x06 => MscCommand::Set,
        0x07 => MscCommand::Fire,
        0x08 => MscCommand::AllOff,
        0x09 => MscCommand::Restore,
        0x0A => MscCommand::Reset,
        0x0B => MscCommand::GoOff,
        _ => return None,
    })
}

fn msc_name(code: u8) -> String {
    match code {
        0x01 => "GO".into(),
        0x02 => "STOP".into(),
        0x03 => "RESUME".into(),
        0x04 => "TIMED_GO".into(),
        0x05 => "LOAD".into(),
        0x06 => "SET".into(),
        0x07 => "FIRE".into(),
        0x08 => "ALL_OFF".into(),
        0x09 => "RESTORE".into(),
        0x0A => "RESET".into(),
        0x0B => "GO_OFF".into(),
        c => format!("command {c:02X}"),
    }
}

/// Parse `F0 7F <device> 02 <format> <command> [data] F7`.
pub fn parse_msc(bytes: &[u8]) -> Option<MscIn> {
    let [0xF0, 0x7F, device_id, 0x02, command_format, code, rest @ ..] = bytes else {
        return None;
    };
    let data = rest.strip_suffix(&[0xF7])?;
    // TIMED_GO carries a 5-byte time before the cue fields.
    let fields = if *code == 0x04 {
        data.get(5..).unwrap_or_default()
    } else {
        data
    };
    let mut parts = fields.split(|b| *b == 0x00).map(|f| {
        f.iter()
            .filter(|b| b.is_ascii_digit() || **b == b'.')
            .map(|b| char::from(*b))
            .collect::<String>()
    });
    Some(MscIn {
        device_id: *device_id,
        command_format: *command_format,
        code: *code,
        command: msc_command(*code),
        q_number: parts.next().unwrap_or_default(),
        q_list: parts.next().unwrap_or_default(),
        q_path: parts.next().unwrap_or_default(),
    })
}

pub fn classify(bytes: &[u8]) -> Incoming {
    let Some(&status) = bytes.first() else {
        return Incoming::Other;
    };
    let channel = (status & 0x0F) + 1;
    let d = |i: usize| bytes.get(i).copied().unwrap_or(0) & 0x7F;
    match status {
        0x80..=0x8F if bytes.len() >= 3 => Incoming::NoteOff {
            channel,
            note: d(1),
        },
        0x90..=0x9F if bytes.len() >= 3 => {
            if d(2) == 0 {
                Incoming::NoteOff {
                    channel,
                    note: d(1),
                }
            } else {
                Incoming::NoteOn {
                    channel,
                    note: d(1),
                    velocity: d(2),
                }
            }
        }
        0xB0..=0xBF if bytes.len() >= 3 => Incoming::ControlChange {
            channel,
            controller: d(1),
            value: d(2),
        },
        0xC0..=0xCF if bytes.len() >= 2 => Incoming::ProgramChange {
            channel,
            program: d(1),
        },
        0xF1 => Incoming::Mtc,
        0xF0 => {
            if sm_timecode::mtc_in::parse_full_frame(bytes).is_some() {
                Incoming::Mtc
            } else if let Some(m) = parse_msc(bytes) {
                Incoming::Msc(m)
            } else {
                Incoming::Other
            }
        }
        0xF8..=0xFF => Incoming::Realtime,
        _ => Incoming::Other,
    }
}

/// Short human-readable description for the log and the activity display.
pub fn describe(bytes: &[u8]) -> String {
    match classify(bytes) {
        Incoming::NoteOn {
            channel,
            note,
            velocity,
        } => format!("Note on ch {channel} note {note} vel {velocity}"),
        Incoming::NoteOff { channel, note } => format!("Note off ch {channel} note {note}"),
        Incoming::ControlChange {
            channel,
            controller,
            value,
        } => format!("CC ch {channel} #{controller} = {value}"),
        Incoming::ProgramChange { channel, program } => {
            format!("Program change ch {channel} #{program}")
        }
        Incoming::Msc(m) => {
            let mut s = format!(
                "MSC {} (device {}, format {:02X})",
                msc_name(m.code),
                m.device_id,
                m.command_format
            );
            if !m.q_number.is_empty() {
                s.push_str(&format!(" Q {}", m.q_number));
            }
            if !m.q_list.is_empty() {
                s.push_str(&format!(" list {}", m.q_list));
            }
            s
        }
        Incoming::Mtc => "MTC".into(),
        Incoming::Realtime | Incoming::Other => {
            let hex: Vec<String> = bytes.iter().take(12).map(|b| format!("{b:02X}")).collect();
            let more = if bytes.len() > 12 { " ..." } else { "" };
            format!("{}{more}", hex.join(" "))
        }
    }
}

/// The last incoming message in a form the UI can use for "learn".
#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct MidiActivity {
    /// Increases with every shown message.
    pub seq: u64,
    pub port: String,
    pub text: String,
    /// `note_on` | `note_off` | `control_change` | `program_change` | `msc` | `other`
    pub kind: String,
    pub channel: u8,
    pub number: u8,
    pub value: u8,
}

impl MidiActivity {
    /// `None` for MTC and real-time bytes (too frequent to show).
    pub fn of(port: &str, bytes: &[u8], seq: u64) -> Option<Self> {
        let (kind, channel, number, value) = match classify(bytes) {
            Incoming::NoteOn {
                channel,
                note,
                velocity,
            } => ("note_on", channel, note, velocity),
            Incoming::NoteOff { channel, note } => ("note_off", channel, note, 0),
            Incoming::ControlChange {
                channel,
                controller,
                value,
            } => ("control_change", channel, controller, value),
            Incoming::ProgramChange { channel, program } => ("program_change", channel, program, 0),
            Incoming::Msc(_) => ("msc", 0, 0, 0),
            Incoming::Mtc | Incoming::Realtime => return None,
            Incoming::Other => ("other", 0, 0, 0),
        };
        Some(Self {
            seq,
            port: port.to_string(),
            text: describe(bytes),
            kind: kind.into(),
            channel,
            number,
            value,
        })
    }
}

/// Is `port` (a device name or `rtp:` session) one the settings listen to?
pub fn listened(show: &Show, settings: &MidiInputSettings, port: &str) -> bool {
    match settings.mode {
        MidiInputMode::None => false,
        MidiInputMode::All => true,
        MidiInputMode::Selected => settings
            .ports
            .iter()
            .any(|p| p == port || show.resolve_midi_port(p) == port),
    }
}

/// Does a mapping's port field accept messages from `port`?
pub fn port_matches(show: &Show, wanted: &str, port: &str) -> bool {
    wanted.is_empty() || wanted == port || show.resolve_midi_port(wanted) == port
}

/// Controller repeats at or above the threshold count as a new press after
/// this much silence on that controller (buttons that only send 127).
const CC_REPRESS_MS: u64 = 250;

/// Matches incoming messages against the mapping (keeps controller state
/// for edge detection).
#[derive(Default)]
pub struct MidiMatcher {
    /// (port, channel, controller) -> (last value, time ms)
    cc: HashMap<(String, u8, u8), (u8, u64)>,
}

impl MidiMatcher {
    /// Mappings triggered by `bytes` from `port` at `now_ms` (any monotonic ms).
    pub fn matches<'a>(
        &mut self,
        show: &'a Show,
        port: &str,
        bytes: &[u8],
        now_ms: u64,
    ) -> Vec<&'a MidiMapping> {
        let msg = classify(bytes);
        // Controller edge: did the value just rise to / above a threshold?
        let mut cc_prev = None;
        if let Incoming::ControlChange {
            channel,
            controller,
            value,
        } = msg
        {
            let key = (port.to_string(), channel, controller);
            cc_prev = Some(self.cc.get(&key).copied());
            self.cc.insert(key, (value, now_ms));
        }
        show.midi_input
            .mappings
            .iter()
            .filter(|m| port_matches(show, &m.port, port))
            .filter(|m| match (&msg, m.kind) {
                (
                    Incoming::NoteOn {
                        channel,
                        note,
                        velocity,
                    },
                    MidiTriggerKind::NoteOn,
                ) => {
                    (m.channel == 0 || m.channel == *channel)
                        && m.number == *note
                        && *velocity >= m.min_value.max(1)
                }
                (
                    Incoming::ControlChange {
                        channel,
                        controller,
                        value,
                    },
                    MidiTriggerKind::ControlChange,
                ) => {
                    let threshold = m.min_value.max(1);
                    (m.channel == 0 || m.channel == *channel)
                        && m.number == *controller
                        && *value >= threshold
                        && match cc_prev.flatten() {
                            None => true,
                            Some((prev, t)) => {
                                prev < threshold || now_ms.saturating_sub(t) >= CC_REPRESS_MS
                            }
                        }
                }
                (Incoming::ProgramChange { channel, program }, MidiTriggerKind::ProgramChange) => {
                    (m.channel == 0 || m.channel == *channel) && m.number == *program
                }
                _ => false,
            })
            .collect()
    }
}

/// Commands for a mapping's action.
pub fn mapping_commands(show: &Show, m: &MidiMapping) -> Result<Vec<Command>, String> {
    let cue = |number: &str| -> Result<u64, String> {
        show.find_by_number(number)
            .map(|(_, c)| c.id)
            .ok_or_else(|| format!("no cue with number {number}"))
    };
    Ok(vec![match m.action {
        MidiMapAction::Go => Command::Go { list: None },
        MidiMapAction::StopAll => Command::StopAll,
        MidiMapAction::Panic => Command::Panic,
        MidiMapAction::TogglePause => Command::TogglePause,
        MidiMapAction::PauseAll => Command::PauseAll,
        MidiMapAction::ResumeAll => Command::ResumeAll,
        MidiMapAction::Next => Command::Next { list: None },
        MidiMapAction::Previous => Command::Previous { list: None },
        MidiMapAction::Reset => Command::Reset,
        MidiMapAction::GotoNumber => Command::GotoNumber {
            number: m.cue_number.trim().to_string(),
        },
        MidiMapAction::StartCue => Command::StartCue {
            cue: cue(m.cue_number.trim())?,
        },
    }])
}

/// Human-readable name of a mapping action (for the log and the UI).
pub fn action_label(a: MidiMapAction) -> &'static str {
    match a {
        MidiMapAction::Go => "GO",
        MidiMapAction::StopAll => "Stop all",
        MidiMapAction::Panic => "Panic",
        MidiMapAction::TogglePause => "Pause / resume all",
        MidiMapAction::PauseAll => "Pause all",
        MidiMapAction::ResumeAll => "Resume all",
        MidiMapAction::Next => "Next (playhead down)",
        MidiMapAction::Previous => "Previous (playhead up)",
        MidiMapAction::Reset => "Reset",
        MidiMapAction::GotoNumber => "Go to cue (playhead)",
        MidiMapAction::StartCue => "Start cue",
    }
}

/// Result of an incoming MSC message.
#[derive(Clone, Debug, PartialEq)]
pub enum MscOutcome {
    /// Not addressed to us (device id / command format): silently dropped.
    NotForUs,
    /// For us but not executed (unsupported command, unknown cue).
    Ignored(String),
    /// Execute these commands; the text describes what happens.
    Run(Vec<Command>, String),
}

/// Is an MSC message addressed to this show?
pub fn msc_for_us(settings: &MscInputSettings, msc: &MscIn) -> bool {
    let device =
        msc.device_id == 0x7F || settings.device_id == 0x7F || msc.device_id == settings.device_id;
    let format = settings.command_format == 0x7F
        || msc.command_format == 0x7F
        || msc.command_format == settings.command_format;
    settings.enabled && device && format
}

/// What to do with a received MSC message.
pub fn msc_commands(show: &Show, settings: &MscInputSettings, msc: &MscIn) -> MscOutcome {
    if !msc_for_us(settings, msc) {
        return MscOutcome::NotForUs;
    }
    let q = msc.q_number.trim();
    let find = |q: &str| show.find_by_number(q).map(|(l, c)| (l, c.id));
    let top_level = |list: u64, id: u64| show.list(list).and_then(|l| l.index_of(id)).is_some();
    match msc.command {
        Some(MscCommand::Go) if q.is_empty() => {
            MscOutcome::Run(vec![Command::Go { list: None }], "GO".into())
        }
        Some(MscCommand::Go) => match find(q) {
            Some((list, id)) if top_level(list, id) => MscOutcome::Run(
                vec![
                    Command::GotoNumber {
                        number: q.to_string(),
                    },
                    Command::Go { list: Some(list) },
                ],
                format!("GO cue {q}"),
            ),
            Some((_, id)) => MscOutcome::Run(
                vec![Command::StartCue { cue: id }],
                format!("start cue {q} (inside a group)"),
            ),
            None => MscOutcome::Ignored(format!("GO: no cue with number {q}")),
        },
        Some(MscCommand::Stop) if q.is_empty() => {
            MscOutcome::Run(vec![Command::StopAll], "stop all".into())
        }
        Some(MscCommand::Stop) => match find(q) {
            Some((_, id)) => {
                MscOutcome::Run(vec![Command::StopCue { cue: id }], format!("stop cue {q}"))
            }
            None => MscOutcome::Ignored(format!("STOP: no cue with number {q}")),
        },
        Some(MscCommand::Resume) if q.is_empty() => {
            MscOutcome::Run(vec![Command::ResumeAll], "resume all".into())
        }
        Some(MscCommand::Resume) => match find(q) {
            Some((_, id)) => MscOutcome::Run(
                vec![Command::ResumeCue { cue: id }],
                format!("resume cue {q}"),
            ),
            None => MscOutcome::Ignored(format!("RESUME: no cue with number {q}")),
        },
        Some(MscCommand::AllOff) => MscOutcome::Run(vec![Command::StopAll], "stop all".into()),
        Some(MscCommand::Reset) => MscOutcome::Run(vec![Command::Reset], "reset".into()),
        Some(MscCommand::Load) if q.is_empty() => {
            MscOutcome::Ignored("LOAD without a cue number".into())
        }
        Some(MscCommand::Load) => match find(q) {
            Some((list, id)) if top_level(list, id) => MscOutcome::Run(
                vec![Command::GotoNumber {
                    number: q.to_string(),
                }],
                format!("playhead to cue {q}"),
            ),
            Some(_) => MscOutcome::Ignored(format!("LOAD: cue {q} is inside a group")),
            None => MscOutcome::Ignored(format!("LOAD: no cue with number {q}")),
        },
        _ => MscOutcome::Ignored(format!("{} is not supported", msc_name(msc.code))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::MidiMessage;
    use crate::model::{Cue, CueKind, GroupCue};

    fn show() -> Show {
        let mut s = Show::default();
        for n in ["1", "2", "5"] {
            let id = s.alloc_id();
            s.cue_lists[0].cues.push(Cue {
                id,
                number: n.into(),
                ..Cue::default()
            });
        }
        let child = Cue {
            id: s.alloc_id(),
            number: "7.1".into(),
            ..Cue::default()
        };
        let id = s.alloc_id();
        s.cue_lists[0].cues.push(Cue {
            id,
            number: "7".into(),
            kind: CueKind::Group(GroupCue {
                children: vec![child],
                ..GroupCue::default()
            }),
            ..Cue::default()
        });
        s
    }

    fn msc(device: u8, fmt: u8, cmd: MscCommand, q: &str) -> Vec<u8> {
        MidiMessage::Msc {
            device_id: device,
            command_format: fmt,
            command: cmd,
            q_number: q.into(),
            q_list: String::new(),
            q_path: String::new(),
        }
        .to_bytes()
        .unwrap()
    }

    #[test]
    fn msc_roundtrip_with_the_encoder() {
        let m = parse_msc(&msc(1, 0x10, MscCommand::Go, "12.5")).unwrap();
        assert_eq!(m.command, Some(MscCommand::Go));
        assert_eq!((m.device_id, m.command_format), (1, 0x10));
        assert_eq!(m.q_number, "12.5");
        let all = MidiMessage::Msc {
            device_id: 127,
            command_format: 0x7F,
            command: MscCommand::Load,
            q_number: "3".into(),
            q_list: "2".into(),
            q_path: "1".into(),
        }
        .to_bytes()
        .unwrap();
        let m = parse_msc(&all).unwrap();
        assert_eq!(
            (m.q_number.as_str(), m.q_list.as_str(), m.q_path.as_str()),
            ("3", "2", "1")
        );
        assert_eq!(parse_msc(&[0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7]), None);
        assert!(describe(&all).contains("LOAD"));
    }

    #[test]
    fn msc_addressing_and_actions() {
        let s = show();
        let on = MscInputSettings {
            enabled: true,
            ..MscInputSettings::default()
        };
        let run = |bytes: &[u8]| msc_commands(&s, &on, &parse_msc(bytes).unwrap());
        assert!(matches!(
            run(&msc(1, 0x10, MscCommand::Go, "")),
            MscOutcome::Run(ref c, _) if c == &vec![Command::Go { list: None }]
        ));
        let MscOutcome::Run(c, _) = run(&msc(127, 0x7F, MscCommand::Go, "5")) else {
            panic!()
        };
        assert_eq!(
            c,
            vec![
                Command::GotoNumber { number: "5".into() },
                Command::Go {
                    list: Some(s.cue_lists[0].id)
                }
            ]
        );
        assert!(matches!(
            run(&msc(1, 0x10, MscCommand::Go, "7.1")),
            MscOutcome::Run(ref c, _) if matches!(c[0], Command::StartCue { .. })
        ));
        assert!(matches!(
            run(&msc(1, 0x10, MscCommand::Go, "99")),
            MscOutcome::Ignored(_)
        ));
        assert_eq!(run(&msc(2, 0x10, MscCommand::Go, "")), MscOutcome::NotForUs);
        assert_eq!(run(&msc(1, 0x01, MscCommand::Go, "")), MscOutcome::NotForUs);
        assert!(matches!(
            run(&msc(1, 0x10, MscCommand::AllOff, "")),
            MscOutcome::Run(ref c, _) if c == &vec![Command::StopAll]
        ));
        assert!(matches!(
            run(&msc(1, 0x10, MscCommand::Stop, "")),
            MscOutcome::Run(ref c, _) if c == &vec![Command::StopAll]
        ));
        assert!(matches!(
            run(&msc(1, 0x10, MscCommand::Resume, "")),
            MscOutcome::Run(ref c, _) if c == &vec![Command::ResumeAll]
        ));
        assert!(matches!(
            run(&msc(1, 0x10, MscCommand::Reset, "")),
            MscOutcome::Run(ref c, _) if c == &vec![Command::Reset]
        ));
        assert!(matches!(
            run(&msc(1, 0x10, MscCommand::Load, "2")),
            MscOutcome::Run(ref c, _) if c == &vec![Command::GotoNumber { number: "2".into() }]
        ));
        assert!(matches!(
            run(&msc(1, 0x10, MscCommand::Fire, "")),
            MscOutcome::Ignored(_)
        ));
        let off = MscInputSettings::default();
        assert_eq!(
            msc_commands(
                &s,
                &off,
                &parse_msc(&msc(1, 0x10, MscCommand::Go, "")).unwrap()
            ),
            MscOutcome::NotForUs
        );
    }

    #[test]
    fn mapping_matches_notes_cc_and_program_changes() {
        let mut s = show();
        s.midi_ports.push(crate::model::MidiPortAlias {
            name: "Pad".into(),
            device: "USB Pad".into(),
        });
        s.midi_input.mappings = vec![
            MidiMapping {
                port: "Pad".into(),
                number: 60,
                ..MidiMapping::default()
            },
            MidiMapping {
                kind: MidiTriggerKind::ControlChange,
                channel: 2,
                number: 20,
                min_value: 64,
                action: MidiMapAction::StopAll,
                ..MidiMapping::default()
            },
            MidiMapping {
                kind: MidiTriggerKind::ProgramChange,
                number: 4,
                action: MidiMapAction::GotoNumber,
                cue_number: "5".into(),
                ..MidiMapping::default()
            },
        ];
        let mut m = MidiMatcher::default();
        let hit = |m: &mut MidiMatcher, port: &str, b: &[u8], t: u64| -> Vec<MidiMapAction> {
            m.matches(&s, port, b, t).iter().map(|x| x.action).collect()
        };
        assert_eq!(
            hit(&mut m, "USB Pad", &[0x90, 60, 100], 0),
            vec![MidiMapAction::Go]
        );
        assert!(hit(&mut m, "Other", &[0x90, 60, 100], 0).is_empty());
        // Velocity 0 is a note-off.
        assert!(hit(&mut m, "USB Pad", &[0x90, 60, 0], 0).is_empty());
        assert!(hit(&mut m, "USB Pad", &[0x90, 61, 100], 0).is_empty());
        // CC: rising edge through the threshold only, right channel only.
        assert!(hit(&mut m, "X", &[0xB0, 20, 127], 0).is_empty());
        assert_eq!(
            hit(&mut m, "X", &[0xB1, 20, 100], 0),
            vec![MidiMapAction::StopAll]
        );
        assert!(hit(&mut m, "X", &[0xB1, 20, 110], 10).is_empty());
        assert!(hit(&mut m, "X", &[0xB1, 20, 10], 20).is_empty());
        assert_eq!(
            hit(&mut m, "X", &[0xB1, 20, 64], 30),
            vec![MidiMapAction::StopAll]
        );
        // A button that only sends 127: a new press after a pause counts.
        assert_eq!(
            hit(&mut m, "X", &[0xB1, 20, 127], 1000),
            vec![MidiMapAction::StopAll]
        );
        assert_eq!(
            hit(&mut m, "X", &[0xC5, 4], 0),
            vec![MidiMapAction::GotoNumber]
        );
        let cmds = mapping_commands(&s, &s.midi_input.mappings[2]).unwrap();
        assert_eq!(cmds, vec![Command::GotoNumber { number: "5".into() }]);
        let bad = MidiMapping {
            action: MidiMapAction::StartCue,
            cue_number: "42".into(),
            ..MidiMapping::default()
        };
        assert!(mapping_commands(&s, &bad).is_err());
    }

    #[test]
    fn listening_modes() {
        let mut s = show();
        s.midi_ports.push(crate::model::MidiPortAlias {
            name: "Desk".into(),
            device: "USB Desk".into(),
        });
        let mut settings = MidiInputSettings::default();
        assert!(!listened(&s, &settings, "USB Desk"));
        settings.mode = MidiInputMode::All;
        assert!(listened(&s, &settings, "anything"));
        settings.mode = MidiInputMode::Selected;
        settings.ports = vec!["Desk".into(), "rtp:Show Master".into()];
        assert!(listened(&s, &settings, "USB Desk"));
        assert!(listened(&s, &settings, "rtp:Show Master"));
        assert!(!listened(&s, &settings, "Other"));
    }

    #[test]
    fn classification_and_activity() {
        assert_eq!(classify(&[0xF8]), Incoming::Realtime);
        assert_eq!(classify(&[0xF1, 0x23]), Incoming::Mtc);
        assert!(MidiActivity::of("p", &[0xF1, 0x23], 1).is_none());
        let a = MidiActivity::of("p", &[0xB3, 7, 99], 4).unwrap();
        assert_eq!(
            (a.kind.as_str(), a.channel, a.number, a.value),
            ("control_change", 4, 7, 99)
        );
        assert_eq!(describe(&[0x90, 60, 100]), "Note on ch 1 note 60 vel 100");
    }
}
