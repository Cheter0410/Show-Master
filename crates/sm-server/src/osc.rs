//! Minimal OSC 1.0 listener mapping `/showmaster/<cmd> [args]` onto the
//! command bus (transport commands only).

use crate::runtime::{parse_command, Handle, Role};
use serde_json::{Map, Value};
use std::net::UdpSocket;

fn padded(len: usize) -> usize {
    (len + 4) & !3
}

fn read_str(buf: &[u8], pos: &mut usize) -> Option<String> {
    let rest = buf.get(*pos..)?;
    let end = rest.iter().position(|b| *b == 0)?;
    let s = std::str::from_utf8(rest.get(..end)?).ok()?.to_string();
    *pos += padded(end);
    Some(s)
}

/// Decode one OSC message into (address, arguments).
pub fn decode(buf: &[u8]) -> Option<(String, Vec<Value>)> {
    let mut pos = 0;
    let addr = read_str(buf, &mut pos)?;
    if !addr.starts_with('/') {
        return None;
    }
    let mut args = Vec::new();
    if pos >= buf.len() {
        return Some((addr, args));
    }
    let tags = read_str(buf, &mut pos)?;
    for tag in tags.strip_prefix(',')?.chars() {
        match tag {
            'i' => {
                let b: [u8; 4] = buf.get(pos..pos + 4)?.try_into().ok()?;
                args.push(Value::from(i32::from_be_bytes(b)));
                pos += 4;
            }
            'f' => {
                let b: [u8; 4] = buf.get(pos..pos + 4)?.try_into().ok()?;
                args.push(Value::from(f64::from(f32::from_be_bytes(b))));
                pos += 4;
            }
            's' => args.push(Value::from(read_str(buf, &mut pos)?)),
            'T' => args.push(Value::Bool(true)),
            'F' => args.push(Value::Bool(false)),
            'N' => args.push(Value::Null),
            _ => return None,
        }
    }
    Some((addr, args))
}

/// Map an OSC message onto a command document.
pub fn to_command(addr: &str, args: &[Value]) -> Option<Value> {
    let name = addr.strip_prefix("/showmaster/")?;
    let mut cmd = Map::new();
    cmd.insert("cmd".into(), Value::from(name));
    let field = match name {
        "goto_number" => Some("number"),
        "start_cue" | "stop_cue" | "pause_cue" | "resume_cue" | "reset_cue" | "load_cue"
        | "arm_cue" | "set_playhead" => Some("cue"),
        "set_master" => Some("db"),
        "tc_relocate" => Some("to"),
        "tc_start" => Some("from"),
        _ => None,
    };
    if let (Some(field), Some(first)) = (field, args.first()) {
        let v = if field == "number" && !first.is_string() {
            Value::from(first.to_string())
        } else {
            first.clone()
        };
        cmd.insert(field.into(), v);
    }
    match (name, args.get(1)) {
        ("load_cue", Some(t)) => {
            cmd.insert("time".into(), t.clone());
        }
        ("arm_cue", second) => {
            cmd.insert("armed".into(), second.cloned().unwrap_or(Value::Bool(true)));
        }
        _ => {}
    }
    Some(Value::Object(cmd))
}

/// Blocking receive loop; run on its own thread.
pub fn listen(socket: UdpSocket, handle: Handle) {
    let mut buf = [0u8; 2048];
    loop {
        let (n, from) = match socket.recv_from(&mut buf) {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!("OSC socket closed: {e}");
                return;
            }
        };
        let Some((addr, args)) = buf.get(..n).and_then(decode) else {
            continue;
        };
        let Some(cmd) = to_command(&addr, &args) else {
            continue;
        };
        match parse_command(cmd) {
            // OSC is unauthenticated: transport commands only.
            Ok(c) => {
                if let Err(e) = handle.post(c, Role::Control, &format!("osc {}", from.ip())) {
                    tracing::warn!("OSC {addr}: {e}");
                }
            }
            Err(e) => tracing::warn!("OSC {addr}: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decode_and_map() {
        assert_eq!(
            decode(b"/showmaster/go\0\0,\0\0\0").unwrap(),
            ("/showmaster/go".into(), vec![])
        );
        assert_eq!(
            to_command("/showmaster/go", &[]).unwrap(),
            json!({ "cmd": "go" })
        );
        let mut msg = b"/showmaster/start_cue\0\0\0,i\0\0".to_vec();
        msg.extend_from_slice(&5i32.to_be_bytes());
        let (addr, args) = decode(&msg).unwrap();
        assert_eq!(
            to_command(&addr, &args).unwrap(),
            json!({ "cmd": "start_cue", "cue": 5 })
        );
        let (addr, args) = decode(b"/showmaster/goto_number\0,s\0\x0014.5\0\0\0\0").unwrap();
        assert_eq!(
            to_command(&addr, &args).unwrap(),
            json!({ "cmd": "goto_number", "number": "14.5" })
        );
        assert!(decode(b"garbage").is_none());
        assert!(decode(b"/x\0\0,i\0\0\x00").is_none());
        assert!(to_command("/other/go", &[]).is_none());
    }
}
