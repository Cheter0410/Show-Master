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

impl OscMessage {
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
}
