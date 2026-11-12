//! Outgoing OSC messages (OSC cues, extra PANIC messages): model, argument
//! text format for the editors and the OSC 1.0 wire encoding.
//!
//! Only plain messages over UDP are sent (no bundles, no TCP/SLIP): that is
//! what mixing desks, QLab, Resolume, TouchDesigner, Companion etc. accept.

use serde::{Deserialize, Serialize};

/// One argument. In the showfile: a JSON number (integer -> `i`, with a
/// fraction -> `f`), `true`/`false` (`T`/`F`) or a string (`s`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum OscArg {
    Int(i32),
    Float(f32),
    Bool(bool),
    String(String),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct OscMessage {
    /// IP address or host name of the receiver.
    pub host: String,
    /// UDP port of the receiver.
    pub port: u16,
    /// OSC address pattern, e.g. `/cue/12/start`.
    pub address: String,
    pub args: Vec<OscArg>,
}

impl Default for OscMessage {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8000,
            address: "/go".into(),
            args: Vec::new(),
        }
    }
}

/// Lantern (the presentation player) listens for OSC on this UDP port.
pub const LANTERN_PORT: u16 = 53_100;

/// What a Lantern cue tells the presentation to do. A Lantern cue is an
/// OSC cue with one of these messages (address `/lantern/...`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanternAction {
    /// Next slide.
    Go,
    Back,
    /// Slide number (from 1).
    Slide(i32),
    First,
    BlackoutOn,
    BlackoutOff,
    BlackoutToggle,
    /// Back to black before the first slide.
    Clear,
}

impl LanternAction {
    /// The choices of the editor (the slide number is edited separately).
    pub const ALL: [LanternAction; 8] = [
        LanternAction::Go,
        LanternAction::Back,
        LanternAction::Slide(1),
        LanternAction::First,
        LanternAction::BlackoutOn,
        LanternAction::BlackoutOff,
        LanternAction::BlackoutToggle,
        LanternAction::Clear,
    ];

    pub fn label(self) -> String {
        match self {
            LanternAction::Go => "GO (next slide)".into(),
            LanternAction::Back => "Back".into(),
            LanternAction::Slide(n) => format!("Slide {n}"),
            LanternAction::First => "First slide".into(),
            LanternAction::BlackoutOn => "Blackout on".into(),
            LanternAction::BlackoutOff => "Blackout off".into(),
            LanternAction::BlackoutToggle => "Blackout toggle".into(),
            LanternAction::Clear => "Clear (black)".into(),
        }
    }

    /// Same choice, ignoring the slide number.
    pub fn same_kind(self, other: LanternAction) -> bool {
        std::mem::discriminant(&self) == std::mem::discriminant(&other)
    }

    pub fn address_args(self) -> (&'static str, Vec<OscArg>) {
        match self {
            LanternAction::Go => ("/lantern/go", vec![]),
            LanternAction::Back => ("/lantern/back", vec![]),
            LanternAction::Slide(n) => ("/lantern/slide", vec![OscArg::Int(n.max(1))]),
            LanternAction::First => ("/lantern/first", vec![]),
            LanternAction::BlackoutOn => ("/lantern/blackout", vec![OscArg::Int(1)]),
            LanternAction::BlackoutOff => ("/lantern/blackout", vec![OscArg::Int(0)]),
            LanternAction::BlackoutToggle => ("/lantern/blackout", vec![]),
            LanternAction::Clear => ("/lantern/clear", vec![]),
        }
    }
}

impl OscMessage {
    /// A message for Lantern on this computer.
    pub fn lantern(action: LanternAction) -> Self {
        let (address, args) = action.address_args();
        Self {
            host: "127.0.0.1".into(),
            port: LANTERN_PORT,
            address: address.into(),
            args,
        }
    }

    /// The Lantern action this message is, if it is one.
    pub fn lantern_action(&self) -> Option<LanternAction> {
        let int = |a: &OscArg| match a {
            OscArg::Int(i) => Some(*i),
            OscArg::Float(f) => Some(f.round() as i32),
            OscArg::Bool(b) => Some(i32::from(*b)),
            OscArg::String(s) => s.trim().parse().ok(),
        };
        let first = self.args.first().and_then(int);
        Some(match self.address.trim_end_matches('/') {
            "/lantern/go" | "/lantern/next" => LanternAction::Go,
            "/lantern/back" | "/lantern/prev" => LanternAction::Back,
            "/lantern/slide" => LanternAction::Slide(first.unwrap_or(1)),
            "/lantern/first" => LanternAction::First,
            "/lantern/clear" => LanternAction::Clear,
            "/lantern/blackout" => match first {
                None => LanternAction::BlackoutToggle,
                Some(0) => LanternAction::BlackoutOff,
                Some(_) => LanternAction::BlackoutOn,
            },
            _ => return None,
        })
    }

    /// `host:port` (IPv6 literals in brackets), as handed to the sender.
    pub fn target(&self) -> String {
        let host = self.host.trim();
        if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]:{}", self.port)
        } else {
            format!("{host}:{}", self.port)
        }
    }

    /// Problems that make the message unsendable.
    pub fn validate(&self) -> Result<(), String> {
        if self.host.trim().is_empty() {
            return Err("OSC: no receiver (host) set".into());
        }
        if self.port == 0 {
            return Err("OSC: no port set".into());
        }
        if !self.address.starts_with('/') {
            return Err(format!(
                "OSC address \"{}\" must start with /",
                self.address
            ));
        }
        if self.address.chars().any(|c| c.is_whitespace() || c == '#') {
            return Err(format!(
                "OSC address \"{}\" must not contain spaces or #",
                self.address
            ));
        }
        Ok(())
    }

    /// The UDP payload (OSC 1.0 message).
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut out = Vec::with_capacity(32 + self.address.len());
        push_str(&mut out, &self.address);
        let mut tags = String::from(",");
        for a in &self.args {
            tags.push(match a {
                OscArg::Int(_) => 'i',
                OscArg::Float(_) => 'f',
                OscArg::Bool(true) => 'T',
                OscArg::Bool(false) => 'F',
                OscArg::String(_) => 's',
            });
        }
        push_str(&mut out, &tags);
        for a in &self.args {
            match a {
                OscArg::Int(i) => out.extend_from_slice(&i.to_be_bytes()),
                OscArg::Float(f) => out.extend_from_slice(&f.to_be_bytes()),
                OscArg::Bool(_) => {}
                OscArg::String(s) => push_str(&mut out, s),
            }
        }
        Ok(out)
    }

    /// Short description for lists: `/cue/1/start 1 "x" -> 10.0.0.5:53000`.
    pub fn summary(&self) -> String {
        if let Some(a) = self.lantern_action() {
            return format!("Lantern · {} -> {}", a.label(), self.target());
        }
        let args = format_args(&self.args);
        if args.is_empty() {
            format!("{} -> {}", self.address, self.target())
        } else {
            format!("{} {args} -> {}", self.address, self.target())
        }
    }
}

/// OSC string: UTF-8, NUL-terminated, padded to a multiple of 4 bytes.
fn push_str(out: &mut Vec<u8>, s: &str) {
    out.extend(s.bytes().filter(|b| *b != 0));
    out.push(0);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

/// Arguments as typed in the editors: separated by spaces; `12` is an int,
/// `0.5` a float, `true`/`false` booleans, everything else a string. Use
/// double quotes for strings with spaces or that look like numbers
/// (`"12"`); `\"` and `\\` escape inside quotes.
pub fn parse_args(text: &str) -> Vec<OscArg> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let Some(&first) = chars.peek() else { break };
        if first == '"' {
            chars.next();
            let mut s = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => {
                        if let Some(n) = chars.next() {
                            s.push(n);
                        }
                    }
                    c => s.push(c),
                }
            }
            out.push(OscArg::String(s));
            continue;
        }
        let mut word = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                break;
            }
            word.push(c);
            chars.next();
        }
        out.push(if word == "true" {
            OscArg::Bool(true)
        } else if word == "false" {
            OscArg::Bool(false)
        } else if let Ok(i) = word.parse::<i32>() {
            OscArg::Int(i)
        } else if let Some(f) = word
            .parse::<f32>()
            .ok()
            .filter(|f| f.is_finite() && word.chars().any(|c| c.is_ascii_digit()))
        {
            OscArg::Float(f)
        } else {
            OscArg::String(word)
        });
    }
    out
}

/// The inverse of [`parse_args`] (round-trips).
pub fn format_args(args: &[OscArg]) -> String {
    args.iter()
        .map(|a| match a {
            OscArg::Int(i) => i.to_string(),
            OscArg::Float(f) => {
                let s = f.to_string();
                if s.contains('.') || s.contains('e') {
                    s
                } else {
                    format!("{s}.0")
                }
            }
            OscArg::Bool(b) => b.to_string(),
            OscArg::String(s) => {
                let plain = !s.is_empty()
                    && !s
                        .chars()
                        .any(|c| c.is_whitespace() || c == '"' || c == '\\')
                    && parse_args(s) == [OscArg::String(s.clone())];
                if plain {
                    s.clone()
                } else {
                    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn encodes_osc_1_0() {
        let m = OscMessage {
            host: "10.0.0.5".into(),
            port: 53000,
            address: "/cue/1/start".into(),
            args: vec![
                OscArg::Int(1),
                OscArg::Float(0.5),
                OscArg::String("hi".into()),
                OscArg::Bool(true),
            ],
        };
        let b = m.encode().unwrap();
        let mut want = b"/cue/1/start\0\0\0\0,ifsT\0\0\0".to_vec();
        want.extend_from_slice(&1i32.to_be_bytes());
        want.extend_from_slice(&0.5f32.to_be_bytes());
        want.extend_from_slice(b"hi\0\0");
        assert_eq!(b, want);
        assert_eq!(b.len() % 4, 0);
        // No arguments: still a type tag string.
        let m = OscMessage {
            address: "/go".into(),
            ..Default::default()
        };
        assert_eq!(m.encode().unwrap(), b"/go\0,\0\0\0".to_vec());
    }

    #[test]
    fn invalid_messages() {
        let bad = |f: fn(&mut OscMessage)| {
            let mut m = OscMessage::default();
            f(&mut m);
            m.encode().is_err()
        };
        assert!(bad(|m| m.address = "go".into()));
        assert!(bad(|m| m.address = "/a b".into()));
        assert!(bad(|m| m.host = " ".into()));
        assert!(bad(|m| m.port = 0));
    }

    #[test]
    fn args_text_round_trip() {
        let a = parse_args(r#"1 -2 0.5 true false hello "two words" "12" "q\"x""#);
        assert_eq!(
            a,
            vec![
                OscArg::Int(1),
                OscArg::Int(-2),
                OscArg::Float(0.5),
                OscArg::Bool(true),
                OscArg::Bool(false),
                OscArg::String("hello".into()),
                OscArg::String("two words".into()),
                OscArg::String("12".into()),
                OscArg::String("q\"x".into()),
            ]
        );
        assert_eq!(parse_args(&format_args(&a)), a);
        assert_eq!(format_args(&[OscArg::Float(1.0)]), "1.0");
        assert_eq!(parse_args("1.0"), vec![OscArg::Float(1.0)]);
        assert_eq!(
            parse_args("inf nan"),
            vec![OscArg::String("inf".into()), OscArg::String("nan".into())]
        );
    }

    #[test]
    fn json_shape() {
        let m = OscMessage {
            args: vec![
                OscArg::Int(3),
                OscArg::Float(0.25),
                OscArg::Bool(false),
                OscArg::String("x".into()),
            ],
            ..Default::default()
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["args"], serde_json::json!([3, 0.25, false, "x"]));
        let back: OscMessage = serde_json::from_value(v).unwrap();
        assert_eq!(back, m);
        assert_eq!(
            OscMessage {
                host: "::1".into(),
                port: 9,
                ..Default::default()
            }
            .target(),
            "[::1]:9"
        );
    }

    #[test]
    fn lantern_messages_round_trip() {
        for a in LanternAction::ALL
            .into_iter()
            .chain([LanternAction::Slide(12)])
        {
            let m = OscMessage::lantern(a);
            assert_eq!(m.lantern_action(), Some(a), "{a:?}");
            assert!(m.encode().is_ok());
        }
        assert_eq!(OscMessage::default().lantern_action(), None);
        assert!(OscMessage::lantern(LanternAction::Slide(3))
            .summary()
            .starts_with("Lantern · Slide 3"));
    }
}
