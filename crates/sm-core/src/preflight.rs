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
            CueKind::Group(g) => {
                if g.children.is_empty() {
                    push(Severity::Warning, Some(cue.id), format!("{who}: group is empty"));
                }
            }
            CueKind::Timecode(_) | CueKind::Memo => {}
        }
    });
    issues.sort_by_key(|i| match i.severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
    });
    issues
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
}
