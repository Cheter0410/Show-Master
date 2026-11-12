//! Minimal OSC 1.0 listener mapping `/showmaster/<cmd> [args]` onto the
//! command bus (transport commands only).

use crate::runtime::{parse_command, Handle, Role};
use serde_json::{json, Map, Value};
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

/// Short addresses (QLab-style) onto a command document. `cue_id` turns a
/// cue number into its id.
///
/// `/go`, `/stop` (stop all), `/panic`, `/pause`, `/resume`, `/toggle_pause`,
/// `/reset`, `/next`, `/previous`, `/master <dB>`, `/playhead/<number>`,
/// `/cue/<number>/start|stop|pause|resume|load|arm|disarm` (`load` takes the
/// time in seconds), `/tc/start|stop|pause|resume`, `/tc/locate <HH:MM:SS:FF>`.
pub fn short_command(
    addr: &str,
    args: &[Value],
    cue_id: &dyn Fn(&str) -> Option<u64>,
) -> Option<Value> {
    let parts: Vec<&str> = addr.trim_start_matches('/').split('/').collect();
    let first = args.first();
    let cmd = match parts.as_slice() {
        ["go"] => json!({ "cmd": "go" }),
        ["stop"] | ["stop_all"] => json!({ "cmd": "stop_all" }),
        ["panic"] => json!({ "cmd": "panic" }),
        ["pause"] => json!({ "cmd": "pause_all" }),
        ["resume"] => json!({ "cmd": "resume_all" }),
        ["toggle_pause"] => json!({ "cmd": "toggle_pause" }),
        ["reset"] => json!({ "cmd": "reset" }),
        ["next"] => json!({ "cmd": "next" }),
        ["previous"] => json!({ "cmd": "previous" }),
        ["master"] => json!({ "cmd": "set_master", "db": first?.as_f64()? }),
        ["playhead", number] => json!({ "cmd": "set_playhead", "cue": cue_id(number)? }),
        ["cue", number, action] => {
            let cue = cue_id(number)?;
            match *action {
                "start" | "go" => json!({ "cmd": "start_cue", "cue": cue }),
                "stop" => json!({ "cmd": "stop_cue", "cue": cue }),
                "pause" => json!({ "cmd": "pause_cue", "cue": cue }),
                "resume" => json!({ "cmd": "resume_cue", "cue": cue }),
                "load" => json!({
                    "cmd": "load_cue", "cue": cue,
                    "time": first.and_then(Value::as_f64).unwrap_or(0.0),
                }),
                "arm" => json!({ "cmd": "arm_cue", "cue": cue, "armed": true }),
                "disarm" => json!({ "cmd": "arm_cue", "cue": cue, "armed": false }),
                _ => return None,
            }
        }
        ["tc", "start"] => json!({ "cmd": "tc_start" }),
        ["tc", "stop"] => json!({ "cmd": "tc_stop" }),
        ["tc", "pause"] => json!({ "cmd": "tc_pause" }),
        ["tc", "resume"] => json!({ "cmd": "tc_resume" }),
        ["tc", "locate"] => json!({ "cmd": "tc_relocate", "to": first?.as_str()? }),
        _ => return None,
    };
    Some(cmd)
}

/// Id of the cue with this number (any list) in the published state.
fn cue_by_number(state: &Value, number: &str) -> Option<u64> {
    fn walk(cues: &[Value], number: &str) -> Option<u64> {
        for c in cues {
            if c["number"].as_str() == Some(number) {
                return c["id"].as_u64();
            }
            if let Some(kids) = c["kind"]["children"].as_array() {
                if let Some(id) = walk(kids, number) {
                    return Some(id);
                }
            }
        }
        None
    }
    state["show"]["cue_lists"]
        .as_array()?
        .iter()
        .find_map(|l| walk(l["cues"].as_array()?, number))
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
        let cmd = to_command(&addr, &args).or_else(|| {
            let state = handle.state();
            short_command(&addr, &args, &|n| cue_by_number(&state, n))
        });
        let Some(cmd) = cmd else {
            tracing::debug!("OSC {addr}: unknown address");
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

    #[test]
    fn short_addresses() {
        let state = json!({ "show": { "cue_lists": [ { "cues": [
            { "id": 7, "number": "12", "kind": { "type": "memo" } },
            { "id": 9, "number": "13", "kind": { "type": "group", "children": [
                { "id": 11, "number": "13.1", "kind": { "type": "memo" } } ] } } ] } ] } });
        let ids = |n: &str| cue_by_number(&state, n);
        let c = |a: &str, args: &[Value]| short_command(a, args, &ids);
        assert_eq!(c("/go", &[]), Some(json!({ "cmd": "go" })));
        assert_eq!(c("/stop", &[]), Some(json!({ "cmd": "stop_all" })));
        assert_eq!(c("/cue/12/start", &[]), Some(json!({ "cmd": "start_cue", "cue": 7 })));
        assert_eq!(c("/cue/13.1/stop", &[]), Some(json!({ "cmd": "stop_cue", "cue": 11 })));
        assert_eq!(
            c("/cue/12/load", &[json!(4.5)]),
            Some(json!({ "cmd": "load_cue", "cue": 7, "time": 4.5 }))
        );
        assert_eq!(c("/playhead/13", &[]), Some(json!({ "cmd": "set_playhead", "cue": 9 })));
        assert_eq!(c("/master", &[json!(-6.0)]), Some(json!({ "cmd": "set_master", "db": -6.0 })));
        assert_eq!(
            c("/tc/locate", &[json!("01:00:00:00")]),
            Some(json!({ "cmd": "tc_relocate", "to": "01:00:00:00" }))
        );
        assert_eq!(c("/cue/99/start", &[]), None);
        assert_eq!(c("/cue/12/explode", &[]), None);
        assert_eq!(c("/master", &[]), None);
        // Every produced command parses.
        for a in ["/go", "/stop", "/panic", "/pause", "/resume", "/reset", "/next", "/previous", "/cue/12/arm", "/tc/start"] {
            parse_command(c(a, &[]).unwrap()).unwrap();
        }
    }
}
