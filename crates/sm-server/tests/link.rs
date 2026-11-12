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
