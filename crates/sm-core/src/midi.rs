//! MIDI message model and wire encoding (incl. SysEx and MIDI Show Control).

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "msg", rename_all = "snake_case")]
pub enum MidiMessage {
    /// `channel` is 1-16 everywhere.
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
        velocity: u8,
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
    /// -8192..=8191
    PitchBend {
        channel: u8,
        value: i16,
    },
    /// Raw SysEx as hex text (`F0 ... F7`; the framing bytes are added if missing).
    Sysex {
        hex: String,
    },
    /// MIDI Show Control.
    Msc {
        /// 0-111 device, 112-126 group, 127 all-call.
        device_id: u8,
        /// e.g. 0x01 lighting, 0x10 sound, 0x7F all-types.
        command_format: u8,
        command: MscCommand,
        q_number: String,
        q_list: String,
        q_path: String,
    },
}

impl Default for MidiMessage {
    fn default() -> Self {
        MidiMessage::NoteOn {
            channel: 1,
            note: 60,
            velocity: 100,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MscCommand {
    #[default]
    Go,
    Stop,
    Resume,
    TimedGo,
    Load,
    Set,
    Fire,
    AllOff,
    Restore,
    Reset,
    GoOff,
}

impl MscCommand {
    pub fn code(self) -> u8 {
        match self {
            MscCommand::Go => 0x01,
            MscCommand::Stop => 0x02,
            MscCommand::Resume => 0x03,
            MscCommand::TimedGo => 0x04,
            MscCommand::Load => 0x05,
            MscCommand::Set => 0x06,
            MscCommand::Fire => 0x07,
            MscCommand::AllOff => 0x08,
            MscCommand::Restore => 0x09,
            MscCommand::Reset => 0x0A,
            MscCommand::GoOff => 0x0B,
        }
    }
}

fn status(base: u8, channel: u8) -> Result<u8, String> {
    if (1..=16).contains(&channel) {
        Ok(base | (channel - 1))
    } else {
        Err(format!("MIDI channel {channel} out of range 1-16"))
    }
}

fn data(name: &str, v: u8) -> Result<u8, String> {
    if v < 128 {
        Ok(v)
    } else {
        Err(format!("{name} {v} out of range 0-127"))
    }
}

/// Parse hex text such as `F0 7E 7F 06 01 F7` (spaces, commas, `0x` allowed).
pub fn parse_hex(text: &str) -> Result<Vec<u8>, String> {
    let cleaned: String = text.replace("0x", " ").replace("0X", " ").replace(',', " ");
    let mut out = Vec::new();
    for tok in cleaned.split_whitespace() {
        if tok.len() > 2 && tok.len() % 2 == 0 {
            for i in (0..tok.len()).step_by(2) {
                let pair = tok
                    .get(i..i + 2)
                    .ok_or_else(|| format!("invalid hex '{tok}'"))?;
                out.push(u8::from_str_radix(pair, 16).map_err(|_| format!("invalid hex '{tok}'"))?);
            }
        } else {
            out.push(u8::from_str_radix(tok, 16).map_err(|_| format!("invalid hex '{tok}'"))?);
        }
    }
    Ok(out)
}

fn msc_ascii(name: &str, s: &str) -> Result<Vec<u8>, String> {
    if s.chars().all(|c| c.is_ascii_digit() || c == '.') {
        Ok(s.as_bytes().to_vec())
    } else {
        Err(format!("MSC {name} '{s}' may only contain digits and '.'"))
    }
}

impl MidiMessage {
    /// Encode to wire bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        Ok(match self {
            MidiMessage::NoteOn {
                channel,
                note,
                velocity,
            } => {
                vec![
                    status(0x90, *channel)?,
                    data("note", *note)?,
                    data("velocity", *velocity)?,
                ]
            }
            MidiMessage::NoteOff {
                channel,
                note,
                velocity,
            } => {
                vec![
                    status(0x80, *channel)?,
                    data("note", *note)?,
                    data("velocity", *velocity)?,
                ]
            }
            MidiMessage::ControlChange {
                channel,
                controller,
                value,
            } => {
                vec![
                    status(0xB0, *channel)?,
                    data("controller", *controller)?,
                    data("value", *value)?,
                ]
            }
            MidiMessage::ProgramChange { channel, program } => {
                vec![status(0xC0, *channel)?, data("program", *program)?]
            }
            MidiMessage::PitchBend { channel, value } => {
                let v = (i32::from(*value) + 8192).clamp(0, 16383) as u16;
                vec![status(0xE0, *channel)?, (v & 0x7F) as u8, (v >> 7) as u8]
            }
            MidiMessage::Sysex { hex } => {
                let mut bytes = parse_hex(hex)?;
                if bytes.first() != Some(&0xF0) {
                    bytes.insert(0, 0xF0);
                }
                if bytes.last() != Some(&0xF7) {
                    bytes.push(0xF7);
                }
                if bytes.len() < 3 {
                    return Err("SysEx is empty".into());
                }
                if bytes[1..bytes.len() - 1].iter().any(|b| *b > 0x7F) {
                    return Err("SysEx data bytes must be 00-7F".into());
                }
                bytes
            }
            MidiMessage::Msc {
                device_id,
                command_format,
                command,
                q_number,
                q_list,
                q_path,
            } => {
                let mut b = vec![
                    0xF0,
                    0x7F,
                    data("device id", *device_id)?,
                    0x02,
                    data("command format", *command_format)?,
                    command.code(),
                ];
                let fields = [
                    msc_ascii("cue number", q_number)?,
                    msc_ascii("cue list", q_list)?,
                    msc_ascii("cue path", q_path)?,
                ];
                let last = fields.iter().rposition(|f| !f.is_empty());
                if let Some(last) = last {
                    for (i, f) in fields.iter().enumerate().take(last + 1) {
                        if i > 0 {
                            b.push(0x00);
                        }
                        b.extend_from_slice(f);
                    }
                }
                b.push(0xF7);
                b
            }
        })
    }
}

/// All Notes Off + All Sound Off + Reset All Controllers on all 16 channels.
pub fn panic_messages() -> Vec<[u8; 3]> {
    let mut v = Vec::with_capacity(48);
    for ch in 0..16u8 {
        v.push([0xB0 | ch, 123, 0]);
        v.push([0xB0 | ch, 120, 0]);
        v.push([0xB0 | ch, 121, 0]);
    }
    v
}

/// Short text form of a message, used where a full editor would be too much
/// (extra PANIC messages):
///
/// `note_on CH NOTE VEL`, `note_off CH NOTE VEL`, `cc CH CONTROLLER VALUE`,
/// `pc CH PROGRAM`, `bend CH VALUE`, `sysex F0 .. F7`,
/// `msc COMMAND DEVICE FORMAT [Q_NUMBER [Q_LIST [Q_PATH]]]`
/// (COMMAND e.g. `go`, `all_off`, `stop`; DEVICE 127 = all; FORMAT 1 = lighting, 127 = all).
impl MidiMessage {
    pub fn to_text(&self) -> String {
        match self {
            MidiMessage::NoteOn {
                channel,
                note,
                velocity,
            } => format!("note_on {channel} {note} {velocity}"),
            MidiMessage::NoteOff {
                channel,
                note,
                velocity,
            } => format!("note_off {channel} {note} {velocity}"),
            MidiMessage::ControlChange {
                channel,
                controller,
                value,
            } => format!("cc {channel} {controller} {value}"),
            MidiMessage::ProgramChange { channel, program } => format!("pc {channel} {program}"),
            MidiMessage::PitchBend { channel, value } => format!("bend {channel} {value}"),
            MidiMessage::Sysex { hex } => format!("sysex {hex}"),
            MidiMessage::Msc {
                device_id,
                command_format,
                command,
                q_number,
                q_list,
                q_path,
            } => {
                let cmd = serde_json::to_value(command)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                let mut s = format!("msc {cmd} {device_id} {command_format}");
                for q in [q_number, q_list, q_path] {
                    if q.is_empty() {
                        break;
                    }
                    s.push(' ');
                    s.push_str(q);
                }
                s
            }
        }
    }

    pub fn parse_text(text: &str) -> Result<Self, String> {
        let words: Vec<&str> = text.split_whitespace().collect();
        let Some((&kind, rest)) = words.split_first() else {
            return Err("Enter a MIDI message, e.g. \"pc 1 99\"".into());
        };
        let num = |i: usize, name: &str, lo: i64, hi: i64| -> Result<i64, String> {
            let w = rest
                .get(i)
                .ok_or_else(|| format!("{kind}: {name} missing"))?;
            let v: i64 = w
                .parse()
                .map_err(|_| format!("{kind}: {name} \"{w}\" is not a number"))?;
            if v < lo || v > hi {
                return Err(format!("{kind}: {name} must be {lo}-{hi}"));
            }
            Ok(v)
        };
        let count = |n: usize| -> Result<(), String> {
            if rest.len() > n {
                Err(format!("{kind}: too many values"))
            } else {
                Ok(())
            }
        };
        let ch = || num(0, "channel", 1, 16).map(|v| v as u8);
        let msg = match kind.to_ascii_lowercase().as_str() {
            "note_on" | "note" | "on" => {
                count(3)?;
                MidiMessage::NoteOn {
                    channel: ch()?,
                    note: num(1, "note", 0, 127)? as u8,
                    velocity: num(2, "velocity", 0, 127)? as u8,
                }
            }
            "note_off" | "off" => {
                count(3)?;
                MidiMessage::NoteOff {
                    channel: ch()?,
                    note: num(1, "note", 0, 127)? as u8,
                    velocity: if rest.len() > 2 {
                        num(2, "velocity", 0, 127)? as u8
                    } else {
                        0
                    },
                }
            }
            "cc" => {
                count(3)?;
                MidiMessage::ControlChange {
                    channel: ch()?,
                    controller: num(1, "controller", 0, 127)? as u8,
                    value: num(2, "value", 0, 127)? as u8,
                }
            }
            "pc" | "program" => {
                count(2)?;
                MidiMessage::ProgramChange {
                    channel: ch()?,
                    program: num(1, "program", 0, 127)? as u8,
                }
            }
            "bend" => {
                count(2)?;
                MidiMessage::PitchBend {
                    channel: ch()?,
                    value: num(1, "value", -8192, 8191)? as i16,
                }
            }
            "sysex" => MidiMessage::Sysex {
                hex: rest.join(" "),
            },
            "msc" => {
                count(6)?;
                let c = rest
                    .first()
                    .ok_or("msc: command missing (go, stop, all_off, …)")?;
                let command: MscCommand =
                    serde_json::from_value(serde_json::Value::String(c.to_ascii_lowercase()))
                        .map_err(|_| format!("msc: unknown command \"{c}\""))?;
                let q = |i: usize| rest.get(i).map_or(String::new(), |s| (*s).to_string());
                MidiMessage::Msc {
                    device_id: num(1, "device", 0, 127)? as u8,
                    command_format: num(2, "format", 0, 127)? as u8,
                    command,
                    q_number: q(3),
                    q_list: q(4),
                    q_path: q(5),
                }
            }
            other => return Err(format!(
                "unknown MIDI message \"{other}\" (note_on, note_off, cc, pc, bend, sysex, msc)"
            )),
        };
        msg.to_bytes()?;
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_form_round_trips() {
        for t in [
            "note_on 1 60 100",
            "note_off 2 61 0",
            "cc 16 7 127",
            "pc 1 99",
            "bend 3 -8192",
            "sysex F0 7E 7F 06 01 F7",
            "msc all_off 127 127",
            "msc go 1 1 12.5 3",
        ] {
            let m = MidiMessage::parse_text(t).unwrap();
            assert_eq!(m.to_text(), t);
        }
        assert!(MidiMessage::parse_text("pc 17 1").is_err());
        assert!(MidiMessage::parse_text("pc 1").is_err());
        assert!(MidiMessage::parse_text("cc 1 2 3 4").is_err());
        assert!(MidiMessage::parse_text("msc jump 1 1").is_err());
        assert!(MidiMessage::parse_text("").is_err());
        assert!(MidiMessage::parse_text("sysex zz").is_err());
    }

    #[test]
    fn channel_messages() {
        assert_eq!(
            MidiMessage::NoteOn {
                channel: 1,
                note: 60,
                velocity: 100
            }
            .to_bytes()
            .unwrap(),
            [0x90, 60, 100]
        );
        assert_eq!(
            MidiMessage::ProgramChange {
                channel: 16,
                program: 5
            }
            .to_bytes()
            .unwrap(),
            [0xCF, 5]
        );
        assert_eq!(
            MidiMessage::PitchBend {
                channel: 2,
                value: 0
            }
            .to_bytes()
            .unwrap(),
            [0xE1, 0x00, 0x40]
        );
        assert!(MidiMessage::NoteOn {
            channel: 0,
            note: 60,
            velocity: 1
        }
        .to_bytes()
        .is_err());
        assert!(MidiMessage::ControlChange {
            channel: 1,
            controller: 200,
            value: 1
        }
        .to_bytes()
        .is_err());
    }

    #[test]
    fn sysex_and_msc() {
        assert_eq!(
            MidiMessage::Sysex {
                hex: "7E 7F 06 01".into()
            }
            .to_bytes()
            .unwrap(),
            [0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7]
        );
        assert_eq!(
            MidiMessage::Sysex {
                hex: "F07E7F0601F7".into()
            }
            .to_bytes()
            .unwrap()
            .len(),
            6
        );
        assert!(MidiMessage::Sysex {
            hex: "F0 80 F7".into()
        }
        .to_bytes()
        .is_err());
        let msc = MidiMessage::Msc {
            device_id: 1,
            command_format: 0x01,
            command: MscCommand::Go,
            q_number: "12.5".into(),
            q_list: "2".into(),
            q_path: String::new(),
        };
        assert_eq!(
            msc.to_bytes().unwrap(),
            [0xF0, 0x7F, 0x01, 0x02, 0x01, 0x01, b'1', b'2', b'.', b'5', 0x00, b'2', 0xF7]
        );
        let bare = MidiMessage::Msc {
            device_id: 127,
            command_format: 0x7F,
            command: MscCommand::AllOff,
            q_number: String::new(),
            q_list: String::new(),
            q_path: String::new(),
        };
        assert_eq!(
            bare.to_bytes().unwrap(),
            [0xF0, 0x7F, 0x7F, 0x02, 0x7F, 0x08, 0xF7]
        );
    }
}
