//! Main / backup link between two runtimes on this machine: commands and
//! show changes made on the main show up on the backup; a wrong key is refused.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{json, Value};
use sm_core::model::{Cue, CueKind, Show};
use sm_server::runtime::{self, parse_command, Config, Handle, Role};
use std::time::{Duration, Instant};

fn show() -> Show {
    let mut s = Show::default();
    s.settings.audio.backend = Some("null".into());
    s.settings.go_debounce_ms = 0;
    for n in 1..=3 {
        let id = s.alloc_id();
        s.cue_lists[0].cues.push(Cue {
            id,
            number: n.to_string(),
            name: format!("Memo {n}"),
            kind: CueKind::Memo,
            ..Cue::default()
        });
    }
    s
}

async fn start() -> (Handle, u16) {
    let (handle, _thread) = runtime::spawn(Config {
        show: Some(show()),
        ..Config::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(sm_server::http::serve(handle.clone(), listener));
    (handle, port)
}

async fn cmd(h: &Handle, v: Value) -> Result<Value, String> {
    h.command(parse_command(v).unwrap(), Role::Admin, "test")
        .await
}

async fn wait(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backup_follows_the_main() {
    let (main, _) = start().await;
    let (backup, backup_port) = start().await;
    cmd(
        &backup,
        json!({ "cmd": "set_link", "role": "backup", "key": "show-key" }),
    )
    .await
    .unwrap();
    cmd(
        &main,
        json!({ "cmd": "set_link", "role": "main", "peer": format!("127.0.0.1:{backup_port}"), "key": "show-key" }),
    )
    .await
    .unwrap();
    // Connected, in sync.
    wait("main connected", || {
        main.state()["remote"]["link"]["connected"] == true
    })
    .await;
    wait("backup hears the main", || {
        backup.state()["remote"]["link"]["connected"] == true
    })
    .await;
    // An edit on the main reaches the backup.
    let first = main.state()["show"]["cue_lists"][0]["cues"][0]["id"]
        .as_u64()
        .unwrap();
    cmd(
        &main,
        json!({ "cmd": "update_cue", "cue": first, "patch": { "name": "Opening" } }),
    )
    .await
    .unwrap();
    wait("show synced", || {
        backup.state()["show"]["cue_lists"][0]["cues"][0]["name"] == "Opening"
    })
    .await;
    // GO on the main moves the backup's playhead too.
    let before = backup.state()["playheads"].clone();
    cmd(&main, json!({ "cmd": "go" })).await.unwrap();
    wait("backup playhead moved", || {
        backup.state()["playheads"] != before
    })
    .await;
    assert_eq!(backup.state()["playheads"], main.state()["playheads"]);
    // Live mode follows.
    cmd(
        &main,
        json!({ "cmd": "set_mode", "mode": "live", "force": true }),
    )
    .await
    .unwrap();
    wait("backup live", || backup.state()["mode"] == "live").await;
    // The key is never published.
    assert!(!main.state().to_string().contains("show-key"));
    assert!(!backup.state().to_string().contains("show-key"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wrong_key_is_refused() {
    let (main, _) = start().await;
    let (backup, backup_port) = start().await;
    cmd(
        &backup,
        json!({ "cmd": "set_link", "role": "backup", "key": "right-key" }),
    )
    .await
    .unwrap();
    cmd(
        &main,
        json!({ "cmd": "set_link", "role": "main", "peer": format!("127.0.0.1:{backup_port}"), "key": "wrong-key" }),
    )
    .await
    .unwrap();
    wait("error reported", || {
        main.state()["remote"]["link"]["error"].is_string()
    })
    .await;
    assert_eq!(main.state()["remote"]["link"]["connected"], false);
    let before = backup.state()["playheads"].clone();
    cmd(&main, json!({ "cmd": "go" })).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(backup.state()["playheads"], before);
}

fn show_with_midi() -> Show {
    let mut s = show();
    let id = s.alloc_id();
    s.cue_lists[0].cues.insert(
        0,
        Cue {
            id,
            number: "0".into(),
            name: "Desk GO".into(),
            kind: CueKind::Midi(sm_core::model::MidiCue {
                port: "test".into(),
                message: sm_core::midi::MidiMessage::ProgramChange {
                    channel: 1,
                    program: 7,
                },
            }),
            ..Cue::default()
        },
    );
    s
}

async fn start_with_midi() -> (Handle, u16, sm_io::midi::MidiLog) {
    let log: sm_io::midi::MidiLog = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (handle, _thread) = runtime::spawn(Config {
        show: Some(show_with_midi()),
        midi_backend: Some(Box::new(sm_io::midi::RecordingBackend {
            port: "test".into(),
            log: log.clone(),
        })),
        ..Config::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(sm_server::http::serve(handle.clone(), listener));
    (handle, port, log)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standby_mutes_the_backup_and_it_takes_over_when_the_main_is_gone() {
    let (main, _, main_midi) = start_with_midi().await;
    let (backup, backup_port, backup_midi) = start_with_midi().await;
    cmd(
        &backup,
        json!({ "cmd": "set_link", "role": "backup", "key": "show-key" }),
    )
    .await
    .unwrap();
    cmd(
        &main,
        json!({ "cmd": "set_link", "role": "main", "peer": format!("127.0.0.1:{backup_port}"), "key": "show-key" }),
    )
    .await
    .unwrap();
    wait("in sync", || {
        backup.state()["remote"]["link"]["connected"] == true
    })
    .await;
    assert_eq!(backup.state()["remote"]["link"]["standby"], true);
    // Local commands on the following backup are refused (no double GO).
    assert_eq!(
        cmd(&backup, json!({ "cmd": "go" })).await,
        Err("backup_standby".into())
    );
    // GO on the main: its desk gets the MIDI message, the backup stays silent.
    cmd(&main, json!({ "cmd": "go" })).await.unwrap();
    wait("main sent MIDI", || !main_midi.lock().unwrap().is_empty()).await;
    let heads = main.state()["playheads"].clone();
    wait("backup followed", || backup.state()["playheads"] == heads).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        backup_midi.lock().unwrap().is_empty(),
        "backup must not send while following"
    );

    // A tablet logged in on the main is logged in on the backup too.
    let token = "tablet-session-token";
    main.sessions
        .lock()
        .unwrap()
        .insert(token.into(), Role::Control);
    // (the sessions travel with the next sync)
    let first = main.state()["show"]["cue_lists"][0]["cues"][1]["id"]
        .as_u64()
        .unwrap();
    cmd(
        &main,
        json!({ "cmd": "update_cue", "cue": first, "patch": { "name": "x" } }),
    )
    .await
    .unwrap();
    wait("session synced", || {
        backup.sessions.lock().unwrap().get(token) == Some(&Role::Control)
    })
    .await;
    assert_eq!(
        main.state()["remote"]["link"]["failover_url"],
        format!("http://127.0.0.1:{backup_port}")
    );

    // The main goes away (its link stops): the backup takes over by itself.
    cmd(&main, json!({ "cmd": "set_link", "role": "off" }))
        .await
        .unwrap();
    wait("backup took over", || {
        backup.state()["remote"]["link"]["taken_over"] == true
    })
    .await;
    assert_eq!(backup.state()["remote"]["link"]["standby"], false);
    // Now it runs on its own: GO works and its outputs are live.
    backup_midi.lock().unwrap().clear();
    cmd(&backup, json!({ "cmd": "reset" })).await.unwrap();
    cmd(&backup, json!({ "cmd": "go" })).await.unwrap();
    wait("backup sends MIDI", || {
        !backup_midi.lock().unwrap().is_empty()
    })
    .await;

    // A main that comes back is refused: the backup keeps running on its own.
    cmd(
        &main,
        json!({ "cmd": "set_link", "role": "main", "peer": format!("127.0.0.1:{backup_port}"), "key": "show-key" }),
    )
    .await
    .unwrap();
    wait("main told", || {
        main.state()["remote"]["link"]["backup_took_over"] == true
    })
    .await;
    // Until the backup is told to follow again.
    cmd(&backup, json!({ "cmd": "link_follow_main" }))
        .await
        .unwrap();
    wait("following again", || {
        backup.state()["remote"]["link"]["connected"] == true
    })
    .await;
    assert_eq!(backup.state()["remote"]["link"]["standby"], true);
}
