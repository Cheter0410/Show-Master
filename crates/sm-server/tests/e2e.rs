//! End-to-end: real runtime (null audio output, recording MIDI port), real
//! HTTP + WebSocket server on an ephemeral loopback port.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sm_core::midi::MidiMessage;
use sm_core::model::*;
use sm_io::midi::RecordingBackend;
use sm_server::runtime::{self, Config, Handle};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type MidiLog = Arc<Mutex<Vec<(Instant, Vec<u8>)>>>;

fn write_wav(path: &std::path::Path, seconds: f32) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..(48_000.0 * seconds) as u32 {
        let s = ((i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 12_000.0) as i16;
        w.write_sample(s).unwrap();
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

struct Fixture {
    port: u16,
    handle: Handle,
    midi: MidiLog,
    dir: tempfile::TempDir,
    audio_cue: CueId,
}

async fn start() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    write_wav(&dir.path().join("tone.wav"), 0.6);
    let mut show = Show::default();
    show.settings.audio.backend = Some("null".into());
    show.settings.go_debounce_ms = 0;
    show.settings.remote.control_pin = "4711".into();
    show.settings.remote.view_pin = "".into();
    let a = show.alloc_id();
    let m = show.alloc_id();
    let audio = AudioCue {
        file: "tone.wav".into(),
        file_duration: Some(0.6),
        ..AudioCue::default()
    };
    show.cue_lists[0].cues.push(Cue {
        id: a,
        number: "1".into(),
        name: "Tone".into(),
        continue_mode: ContinueMode::AutoFollow,
        kind: CueKind::Audio(audio),
        ..Cue::default()
    });
    let midi_cue = MidiCue {
        port: "test".into(),
        message: MidiMessage::NoteOn {
            channel: 1,
            note: 64,
            velocity: 99,
        },
    };
    show.cue_lists[0].cues.push(Cue {
        id: m,
        number: "2".into(),
        kind: CueKind::Midi(midi_cue),
        ..Cue::default()
    });
    let path = dir.path().join("e2e.smshow");
    sm_core::showfile::save(&show, &path).unwrap();
    let midi: MidiLog = Arc::new(Mutex::new(Vec::new()));
    let backend = RecordingBackend {
        port: "test".into(),
        log: midi.clone(),
    };
    let (handle, _thread) = runtime::spawn(Config {
        show_path: Some(path),
        midi_backend: Some(Box::new(backend)),
        ..Config::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(sm_server::http::serve(handle.clone(), listener));
    Fixture {
        port,
        handle,
        midi,
        dir,
        audio_cue: a,
    }
}

async fn http(port: u16, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let req = format!("{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf).to_string();
    let status: u16 = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let payload = text.split_once("\r\n\r\n").map(|x| x.1).unwrap_or("");
    (
        status,
        serde_json::from_str(payload).unwrap_or(Value::String(payload.to_string())),
    )
}

/// A WebSocket client that maintains the state document from patches.
struct Client {
    ws: Ws,
    state: Value,
    rev: u64,
    role: String,
    next_id: u64,
    last_tick: Value,
    logs: Vec<String>,
    kicked: bool,
}

impl Client {
    async fn connect(port: u16, token: Option<&str>) -> Client {
        let url = match token {
            Some(t) => format!("ws://127.0.0.1:{port}/ws?token={t}"),
            None => format!("ws://127.0.0.1:{port}/ws"),
        };
        let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let mut c = Client {
            ws,
            state: Value::Null,
            rev: 0,
            role: String::new(),
            next_id: 1,
            last_tick: Value::Null,
            logs: vec![],
            kicked: false,
        };
        while c.state.is_null() {
            c.pump_one(None).await;
        }
        c
    }

    async fn pump_one(&mut self, want_ack: Option<u64>) -> Option<Value> {
        let msg = tokio::time::timeout(Duration::from_secs(5), self.ws.next())
            .await
            .expect("ws timeout")?
            .ok()?;
        let Message::Text(text) = msg else {
            return None;
        };
        let v: Value = serde_json::from_str(&text).unwrap();
        match v["t"].as_str().unwrap() {
            "hello" => self.role = v["role"].as_str().unwrap().to_string(),
            "state" => {
                self.state = v["state"].clone();
                self.rev = v["rev"].as_u64().unwrap();
            }
            "patch" => {
                assert_eq!(v["rev"].as_u64().unwrap(), self.rev + 1, "no revision gaps");
                self.rev += 1;
                sm_server::diff::apply(&mut self.state, v["ops"].as_array().unwrap());
            }
            "tick" => self.last_tick = v,
            "log" => self.logs.extend(
                v["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|e| e["message"].as_str().unwrap().to_string()),
            ),
            "kicked" => self.kicked = true,
            "ack" if Some(v["id"].as_u64().unwrap()) == want_ack => return Some(v),
            _ => {}
        }
        None
    }

    async fn cmd(&mut self, cmd: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.ws
            .send(Message::Text(
                json!({ "t": "cmd", "id": id, "cmd": cmd })
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        loop {
            if let Some(ack) = self.pump_one(Some(id)).await {
                return if ack["ok"].as_bool().unwrap() {
                    Ok(ack["result"].clone())
                } else {
                    Err(ack["error"].as_str().unwrap().to_string())
                };
            }
        }
    }

    async fn until(&mut self, what: &str, mut pred: impl FnMut(&Client) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(6);
        while !pred(self) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for: {what}\nstate: {}\ntick: {}",
                self.state,
                self.last_tick
            );
            self.pump_one(None).await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_show_flow_over_websocket() {
    let fx = start().await;
    let mut admin = Client::connect(fx.port, None).await;
    assert_eq!(admin.role, "admin");
    assert_eq!(
        admin.state["show"]["cue_lists"][0]["cues"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(admin.state["mode"], "edit");
    assert_eq!(admin.state["audio"]["is_null"], true);
    assert_eq!(
        admin.state["show"]["settings"]["remote"]["control_pin"],
        "4711"
    );

    // --- roles -----------------------------------------------------------
    let (status, _) = http(
        fx.port,
        "POST",
        "/api/login",
        Some(json!({ "pin": "0000", "role": "control" })),
    )
    .await;
    assert_eq!(status, 401);
    let (_, login) = http(
        fx.port,
        "POST",
        "/api/login",
        Some(json!({ "pin": "4711", "role": "control" })),
    )
    .await;
    let mut control = Client::connect(fx.port, login["token"].as_str()).await;
    assert_eq!(control.role, "control");
    assert_eq!(
        control.state["show"]["settings"]["remote"]["control_pin"], "****",
        "PINs are never sent to remotes"
    );
    let (_, login) = http(
        fx.port,
        "POST",
        "/api/login",
        Some(json!({ "pin": "", "role": "view" })),
    )
    .await;
    let mut view = Client::connect(fx.port, login["token"].as_str()).await;
    assert_eq!(view.role, "view");
    assert_eq!(
        view.cmd(json!({ "cmd": "go" })).await.unwrap_err(),
        "forbidden"
    );
    assert_eq!(
        view.cmd(json!({ "cmd": "set_notes", "text": "x" }))
            .await
            .unwrap_err(),
        "forbidden"
    );
    assert_eq!(
        control
            .cmd(json!({ "cmd": "update_show", "patch": { "name": "hacked" } }))
            .await
            .unwrap_err(),
        "forbidden"
    );
    assert_eq!(
        control
            .cmd(json!({ "cmd": "save_show" }))
            .await
            .unwrap_err(),
        "forbidden"
    );
    let (status, _) = http(fx.port, "GET", "/api/v1/cmd/go?token=bogus", None).await;
    assert_eq!(status, 401);
    let (status, _) = http(fx.port, "GET", "/api/fs?token=pin:4711", None).await;
    assert_eq!(status, 401, "file browser is admin only");
    admin
        .until("three clients listed", |c| {
            c.state["remote"]["clients"]
                .as_array()
                .is_some_and(|a| a.len() == 3)
        })
        .await;

    // --- editing ---------------------------------------------------------
    let r = admin
        .cmd(json!({ "cmd": "add_cue", "kind": "audio", "file": fx.dir.path().join("tone.wav") }))
        .await
        .unwrap();
    let new_cue = r["cue"].as_u64().unwrap();
    admin
        .until("new cue with probed media", |c| {
            let cues = c.state["show"]["cue_lists"][0]["cues"].as_array().unwrap();
            cues.len() == 3
                && cues[2]["kind"]["file"] == "tone.wav"
                && (cues[2]["kind"]["file_duration"].as_f64().unwrap_or(0.0) - 0.6).abs() < 0.01
                && cues[2]["kind"]["channels"] == 2
        })
        .await;
    admin
        .cmd(json!({ "cmd": "update_cue", "cue": new_cue, "patch": { "name": "Third" } }))
        .await
        .unwrap();
    admin.cmd(json!({ "cmd": "undo" })).await.unwrap();
    admin
        .until("undo visible", |c| {
            c.state["show"]["cue_lists"][0]["cues"][2]["name"] == "tone"
                && c.state["can_redo"] == true
        })
        .await;
    admin
        .cmd(json!({ "cmd": "delete_cues", "cues": [new_cue] }))
        .await
        .unwrap();
    assert!(admin
        .cmd(json!({ "cmd": "update_cue", "cue": 1, "patch": { "pre_wait": "x" } }))
        .await
        .is_err());
    assert!(admin
        .cmd(json!({ "cmd": "nonsense" }))
        .await
        .unwrap_err()
        .contains("unknown command"));
    let (_, wave) = http(
        fx.port,
        "GET",
        "/api/waveform?file=tone.wav&buckets=64",
        None,
    )
    .await;
    assert_eq!(wave["peaks"].as_array().unwrap().len(), 64);
    assert!(wave["peaks"][10][1].as_f64().unwrap() > 0.2);
    let (_, fs) = http(fx.port, "GET", "/api/fs", None).await;
    assert!(fs["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["name"] == "tone.wav"));

    // --- live mode is write protected --------------------------------------
    admin
        .cmd(json!({ "cmd": "set_mode", "mode": "live" }))
        .await
        .unwrap();
    admin
        .until("live + preflight", |c| {
            c.state["mode"] == "live"
                && c.state["preflight"].as_array().is_some_and(|p| {
                    p.iter()
                        .any(|i| i["message"].as_str().unwrap().contains("No audio device"))
                })
        })
        .await;
    assert_eq!(
        admin
            .cmd(json!({ "cmd": "delete_cues", "cues": [fx.audio_cue] }))
            .await
            .unwrap_err(),
        "live_mode"
    );
    assert_eq!(
        admin
            .cmd(json!({ "cmd": "new_show", "force": true }))
            .await
            .unwrap_err(),
        "live_mode"
    );

    // --- GO from the remote: audio cue runs, auto-follows into the MIDI cue ---
    let t_go = Instant::now();
    control.cmd(json!({ "cmd": "go" })).await.unwrap();
    control
        .until("audio cue running", |c| {
            c.last_tick["active"].as_array().is_some_and(|a| {
                a.iter()
                    .any(|x| x["cue"] == fx.audio_cue && x["phase"] == "running")
            })
        })
        .await;
    assert!(t_go.elapsed() < Duration::from_millis(500));
    assert_eq!(
        admin
            .cmd(json!({ "cmd": "set_mode", "mode": "edit" }))
            .await
            .unwrap_err(),
        "cues_running"
    );
    view.until("view sees end of list", |c| {
        c.state["playheads"]
            .as_object()
            .is_some_and(|p| p.values().all(Value::is_null))
    })
    .await;
    control
        .until("cue finished", |c| {
            c.last_tick["active"].as_array().is_some_and(Vec::is_empty)
                && c.last_tick["show_elapsed"].as_f64().unwrap_or(0.0) > 0.6
        })
        .await;
    let deadline = Instant::now() + Duration::from_secs(2);
    while fx.midi.lock().unwrap().is_empty() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let midi = fx.midi.lock().unwrap().clone();
    assert_eq!(midi.len(), 1, "auto-follow fired the MIDI cue");
    assert_eq!(midi[0].1, vec![0x90, 64, 99]);
    let after = midi[0].0.duration_since(t_go);
    assert!(
        after > Duration::from_millis(550) && after < Duration::from_millis(900),
        "MIDI follows the 0.6 s cue on the audio clock, got {after:?}"
    );
    assert_eq!(control.last_tick["underruns"], 0);
    control
        .until("log arrives", |c| {
            c.logs.iter().any(|l| l.contains("GO cue 1 \"Tone\""))
        })
        .await;

    // --- HTTP API (Stream Deck / Companion) --------------------------------
    let (status, r) = http(
        fx.port,
        "GET",
        "/api/v1/cmd/goto_number?number=2&token=pin:4711",
        None,
    )
    .await;
    assert_eq!((status, r["ok"].as_bool()), (200, Some(true)));
    // State is published every 50 ms: poll until the move is visible.
    let mut state = Value::Null;
    for _ in 0..40 {
        state = http(fx.port, "GET", "/api/v1/state?token=pin:4711", None)
            .await
            .1;
        if state["playheads"]
            .as_object()
            .unwrap()
            .values()
            .all(|v| !v.is_null())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let playhead = state["playheads"]
        .as_object()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .as_u64()
        .unwrap();
    assert_eq!(state["show"]["cue_lists"][0]["cues"][1]["id"], playhead);
    assert!(state["tick"]["wall_ms"].as_u64().unwrap() > 0);
    assert_eq!(state["show"]["settings"]["remote"]["control_pin"], "****");
    let (status, r) = http(
        fx.port,
        "POST",
        "/api/v1/command?token=pin:4711",
        Some(json!({ "cmd": "save_show" })),
    )
    .await;
    assert_eq!((status, r["error"].as_str()), (403, Some("forbidden")));
    let (status, _) = http(fx.port, "GET", "/api/qr.svg?role=control", None).await;
    assert!(
        status == 200 || status == 404,
        "404 only when the machine has no LAN address"
    );

    // --- every client converged on the same document ------------------------
    admin
        .cmd(json!({ "cmd": "set_mode", "mode": "edit", "force": true }))
        .await
        .unwrap();
    admin
        .cmd(json!({ "cmd": "set_notes", "text": "Hold for clearance" }))
        .await
        .unwrap();
    admin
        .until("admin converged", |c| {
            c.state["show"]["notes"] == "Hold for clearance" && c.state == fx.handle.state()
        })
        .await;
    let redacted = runtime::redact_state(fx.handle.state());
    view.until("view converged", |c| c.state == redacted).await;

    // --- kick ---------------------------------------------------------------
    let id = admin.state["remote"]["clients"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["role"] == "view")
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    admin
        .cmd(json!({ "cmd": "kick_client", "id": id }))
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !view.kicked && Instant::now() < deadline {
        view.pump_one(None).await;
    }
    assert!(view.kicked);

    // --- save / dirty / static UI -----------------------------------------
    assert_eq!(admin.state["dirty"], true);
    admin.cmd(json!({ "cmd": "save_show" })).await.unwrap();
    admin.until("clean", |c| c.state["dirty"] == false).await;
    let saved = sm_core::showfile::load(&fx.dir.path().join("e2e.smshow")).unwrap();
    assert_eq!(saved.notes, "Hold for clearance");
    assert!(fx.dir.path().join("backups").is_dir());
    for path in ["/", "/remote", "/view"] {
        let (status, body) = http(fx.port, "GET", path, None).await;
        assert_eq!(status, 200);
        assert!(body
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("<!doctype html"));
    }
    let (status, _) = http(fx.port, "GET", "/assets/nope.js", None).await;
    assert_eq!(status, 404);
}
