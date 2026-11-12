//! Pre-flight check run when switching into live mode.

use crate::model::{CueId, CueKind, Show};
use crate::showfile::resolve_media;
use serde::Serialize;
use std::path::Path;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Issue {
    pub severity: Severity,
    pub cue: Option<CueId>,
    pub message: String,
}

/// What the machine currently offers; supplied by the I/O layer.
#[derive(Clone, Debug, Default)]
pub struct Environment<'a> {
    pub show_dir: Option<&'a Path>,
    /// `None` = no real audio device is open (null output).
    pub audio_device: Option<String>,
    pub audio_channels: usize,
    /// Names of available MIDI destinations (system ports and `rtp:` sessions).
    pub midi_ports: Vec<String>,
    /// Names of available MIDI inputs (system ports and `rtp:` sessions).
    pub midi_inputs: Vec<String>,
    /// Ids of available audio input devices; `None` = not known (not scanned).
    pub audio_inputs: Option<Vec<String>>,
}

fn label(number: &str, name: &str) -> String {
    match (number.is_empty(), name.is_empty()) {
        (false, false) => format!("Cue {number} \"{name}\""),
        (false, true) => format!("Cue {number}"),
        (true, false) => format!("Cue \"{name}\""),
        (true, true) => "Unnumbered cue".to_string(),
    }
}

pub fn check(show: &Show, env: &Environment<'_>) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut push = |severity, cue, message: String| {
        issues.push(Issue {
            severity,
            cue,
            message,
        })
    };

    if env.audio_device.is_none() {
        push(
            Severity::Error,
            None,
            "No audio device is open - running on the silent null output".into(),
        );
    } else if env.audio_channels < usize::from(show.settings.audio.channels) {
        push(
            Severity::Error,
            None,
            format!(
                "Audio device offers {} channels, show is set up for {}",
                env.audio_channels, show.settings.audio.channels
            ),
        );
    }
    for (i, out) in show.patch.outputs.iter().enumerate() {
        if usize::from(out.channel) >= env.audio_channels {
            push(
                Severity::Error,
                None,
                format!(
                    "Output {} \"{}\" is patched to device channel {} which does not exist",
                    i + 1,
                    out.name,
                    out.channel + 1
                ),
            );
        }
    }
    let tc = &show.timecode;
    if tc.ltc_enabled && usize::from(tc.ltc_channel) >= env.audio_channels {
        push(
            Severity::Error,
            None,
            format!(
                "LTC output channel {} does not exist on the audio device",
                tc.ltc_channel + 1
            ),
        );
    }
    let port_ok = |name: &str| {
        let device = show.resolve_midi_port(name);
        !device.is_empty() && env.midi_ports.iter().any(|p| p == device)
    };
    if tc.mtc_enabled && !port_ok(&tc.mtc_port) {
        push(
            Severity::Error,
            None,
            format!("MTC port \"{}\" not found", tc.mtc_port),
        );
    }
    for alias in &show.midi_ports {
        if !env.midi_ports.contains(&alias.device) {
            push(
                Severity::Warning,
                None,
                format!("MIDI port \"{}\" ({}) not found", alias.name, alias.device),
            );
        }
    }

    check_inputs(show, env, &mut push);

    let n_out = show.patch.outputs.len();
    let mut numbers = std::collections::HashMap::new();
    show.for_each_cue(|_, cue| {
        let who = label(&cue.number, &cue.name);
        if !cue.number.is_empty() {
            if let Some(_prev) = numbers.insert(cue.number.clone(), cue.id) {
                push(Severity::Warning, Some(cue.id), format!("{who}: duplicate cue number"));
            }
        }
        if !cue.armed {
            push(Severity::Info, Some(cue.id), format!("{who} is disarmed"));
        }
        match &cue.kind {
            CueKind::Audio(a) => {
                if a.file.is_empty() {
                    push(Severity::Error, Some(cue.id), format!("{who}: no audio file assigned"));
                } else if !resolve_media(env.show_dir, &a.file).is_file() {
                    push(Severity::Error, Some(cue.id), format!("{who}: file not found: {}", a.file));
                }
                for x in &a.routing {
                    if usize::from(x.out) >= n_out {
                        push(Severity::Error, Some(cue.id), format!("{who}: routed to output {} which is not in the patch", x.out + 1));
                        break;
                    }
                }
                if a.routing.is_empty() && usize::from(a.channels) > n_out {
                    push(
                        Severity::Warning,
                        Some(cue.id),
                        format!("{who}: file has {} channels but only {n_out} outputs are patched (extra channels are dropped)", a.channels),
                    );
                }
            }
            CueKind::Fade(f) => match f.target {
                Some(t) if show.cue(t).is_some() => {}
                _ => push(Severity::Error, Some(cue.id), format!("{who}: fade has no valid target")),
            },
            CueKind::Control(c) => match c.target {
                Some(t) if show.cue(t).is_some() => {}
                _ => push(Severity::Error, Some(cue.id), format!("{who}: control cue has no valid target")),
            },
            CueKind::Midi(m) => {
                if !port_ok(&m.port) {
                    push(Severity::Error, Some(cue.id), format!("{who}: MIDI port \"{}\" not found", m.port));
                }
                if let Err(e) = m.message.to_bytes() {
                    push(Severity::Error, Some(cue.id), format!("{who}: {e}"));
                }
            }
            CueKind::Osc(m) => {
                if let Err(e) = m.validate() {
                    push(Severity::Error, Some(cue.id), format!("{who}: {e}"));
                }
            }
            CueKind::Group(g) => {
                if g.children.is_empty() {
                    push(Severity::Warning, Some(cue.id), format!("{who}: group is empty"));
                }
            }
            CueKind::Timecode(_) | CueKind::Memo => {}
        }
    });
    for (n, m) in show.settings.panic_send.iter().enumerate() {
        let who = format!("PANIC message {}", n + 1);
        match m {
            crate::model::PanicMessage::Midi { port, message } => {
                if !port_ok(port) {
                    push(
                        Severity::Error,
                        None,
                        format!("{who}: MIDI port \"{port}\" not found"),
                    );
                }
                if let Err(e) = message.to_bytes() {
                    push(Severity::Error, None, format!("{who}: {e}"));
                }
            }
            crate::model::PanicMessage::Osc(o) => {
                if let Err(e) = o.validate() {
                    push(Severity::Error, None, format!("{who}: {e}"));
                }
            }
        }
    }
    issues.sort_by_key(|i| match i.severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
    });
    issues
}

/// MIDI input ports, remote-control mapping, MSC and timecode input.
fn check_inputs(
    show: &Show,
    env: &Environment<'_>,
    push: &mut impl FnMut(Severity, Option<CueId>, String),
) {
    use crate::model::{MidiInputMode, TcInputKind, TcSource};
    let input_ok = |name: &str| {
        let device = show.resolve_midi_port(name);
        env.midi_inputs.iter().any(|p| p == device || p == name)
    };
    let mi = &show.midi_input;
    if mi.mode == MidiInputMode::Selected {
        if mi.ports.is_empty() {
            push(
                Severity::Warning,
                None,
                "MIDI input is set to selected ports, but no port is selected".into(),
            );
        }
        for p in &mi.ports {
            if !input_ok(p) {
                push(
                    Severity::Warning,
                    None,
                    format!("MIDI input port \"{p}\" not found"),
                );
            }
        }
    }
    let listening = mi.mode != MidiInputMode::None;
    if !listening && (!mi.mappings.is_empty() || mi.msc.enabled) {
        push(
            Severity::Warning,
            None,
            "MIDI remote control / MSC is set up, but no MIDI input port is listened to".into(),
        );
    }
    for (i, m) in mi.mappings.iter().enumerate() {
        if m.action.needs_cue() && show.find_by_number(m.cue_number.trim()).is_none() {
            push(
                Severity::Warning,
                None,
                format!(
                    "MIDI mapping {}: no cue with number \"{}\"",
                    i + 1,
                    m.cue_number
                ),
            );
        }
        if !m.port.is_empty() && !input_ok(&m.port) {
            push(
                Severity::Warning,
                None,
                format!(
                    "MIDI mapping {}: input port \"{}\" not found",
                    i + 1,
                    m.port
                ),
            );
        }
    }
    let tc = &show.timecode;
    if tc.source != TcSource::External {
        return;
    }
    match tc.input.kind {
        TcInputKind::Mtc => {
            if tc.input.mtc_port.is_empty() {
                if !listening {
                    push(
                        Severity::Error,
                        None,
                        "Timecode input: no MTC input port chosen and no MIDI input open".into(),
                    );
                }
            } else if !input_ok(&tc.input.mtc_port) {
                push(
                    Severity::Error,
                    None,
                    format!(
                        "Timecode input: MTC port \"{}\" not found",
                        tc.input.mtc_port
                    ),
                );
            }
            let out = show.resolve_midi_port(&tc.mtc_port);
            if tc.mtc_enabled
                && !tc.mtc_port.is_empty()
                && (tc.input.mtc_port.is_empty()
                    || show.resolve_midi_port(&tc.input.mtc_port) == out)
            {
                push(
                    Severity::Warning,
                    None,
                    format!("MTC is sent to \"{}\" and received from the same port: check for a feedback loop", tc.mtc_port),
                );
            }
        }
        TcInputKind::Ltc => {
            if let Some(devices) = &env.audio_inputs {
                match &tc.input.ltc_device {
                    Some(d) if !devices.contains(d) => push(
                        Severity::Error,
                        None,
                        format!("Timecode input: audio input \"{d}\" not found"),
                    ),
                    None if devices.is_empty() => push(
                        Severity::Error,
                        None,
                        "Timecode input: no audio input device available for LTC".into(),
                    ),
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AudioCue, Crosspoint, Cue, FadeCue, MidiCue};

    #[test]
    fn finds_problems() {
        let mut show = Show::default();
        let mk = |show: &mut Show, number: &str, armed: bool, kind| {
            let id = show.alloc_id();
            show.cue_lists[0].cues.push(Cue {
                id,
                number: number.into(),
                armed,
                kind,
                ..Cue::default()
            });
        };
        mk(
            &mut show,
            "1",
            true,
            CueKind::Audio(AudioCue {
                file: "missing.wav".into(),
                routing: vec![Crosspoint {
                    src: 0,
                    out: 7,
                    db: 0.0,
                }],
                ..AudioCue::default()
            }),
        );
        mk(
            &mut show,
            "1",
            false,
            CueKind::Fade(FadeCue {
                target: Some(999),
                ..FadeCue::default()
            }),
        );
        mk(
            &mut show,
            "3",
            true,
            CueKind::Midi(MidiCue {
                port: "Desk".into(),
                ..MidiCue::default()
            }),
        );
        let env = Environment {
            show_dir: None,
            audio_device: Some("dev".into()),
            audio_channels: 2,
            midi_ports: vec![],
            ..Environment::default()
        };
        let issues = check(&show, &env);
        let has = |s: &str| issues.iter().any(|i| i.message.contains(s));
        assert!(has("file not found"));
        assert!(has("not in the patch"));
        assert!(has("duplicate cue number"));
        assert!(has("is disarmed"));
        assert!(has("no valid target"));
        assert!(has("MIDI port \"Desk\" not found"));
        assert_eq!(issues[0].severity, Severity::Error);
        let none = check(&show, &Environment::default());
        assert!(none.iter().any(|i| i.message.contains("No audio device")));
    }

    #[test]
    fn input_problems() {
        use crate::model::{MidiInputMode, MidiMapAction, MidiMapping, TcInputKind, TcSource};
        let mut show = Show::default();
        show.midi_input.mappings.push(MidiMapping {
            action: MidiMapAction::GotoNumber,
            cue_number: "9".into(),
            ..MidiMapping::default()
        });
        show.timecode.source = TcSource::External;
        show.timecode.input.mtc_port = "DAW".into();
        let env = Environment {
            audio_device: Some("dev".into()),
            audio_channels: 2,
            midi_inputs: vec!["Keys".into()],
            ..Environment::default()
        };
        let has = |issues: &[Issue], s: &str| issues.iter().any(|i| i.message.contains(s));
        let issues = check(&show, &env);
        assert!(has(&issues, "no MIDI input port is listened to"));
        assert!(has(&issues, "no cue with number \"9\""));
        assert!(has(&issues, "MTC port \"DAW\" not found"));
        show.midi_input.mode = MidiInputMode::Selected;
        show.midi_input.ports = vec!["Keys".into(), "Gone".into()];
        show.timecode.input.mtc_port = "Keys".into();
        let issues = check(&show, &env);
        assert!(has(&issues, "MIDI input port \"Gone\" not found"));
        assert!(!has(&issues, "\"Keys\" not found"));
        assert!(!has(&issues, "Timecode input"));
        show.timecode.input.kind = TcInputKind::Ltc;
        show.timecode.input.ltc_device = Some("Interface".into());
        let issues = check(
            &show,
            &Environment {
                audio_inputs: Some(vec!["Mic".into()]),
                ..env.clone()
            },
        );
        assert!(has(&issues, "audio input \"Interface\" not found"));
        // Unknown device list: no claim either way.
        assert!(!has(&check(&show, &env), "Timecode input"));
    }
}
