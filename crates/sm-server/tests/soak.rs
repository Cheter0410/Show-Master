//! Soak / load test of the complete system on the null audio output.
//!
//! Every few seconds a group of 32 parallel stereo file cues is fired while
//! LTC + MTC run, MIDI cues fire, remote clients are connected and the show
//! is being edited. Asserts: no underruns, no cue errors, no clock stall,
//! bounded memory growth.
//!
//! Ignored by default. Run with:
//!   SM_SOAK_SECONDS=7200 cargo test --release -p sm-server --test soak -- --ignored --nocapture

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use futures_util::StreamExt;
use serde_json::json;
use sm_core::midi::MidiMessage;
use sm_core::model::*;
use sm_io::midi::RecordingBackend;
use sm_server::runtime::{self, parse_command, Config, Role};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn write_wav(path: &std::path::Path, seconds: f32, freq: f32) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..(48_000.0 * seconds) as u32 {
        let s = ((i as f32 * freq * std::f32::consts::TAU / 48_000.0).sin() * 600.0) as i16;
        w.write_sample(s).unwrap();
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

fn rss_kb() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "long-running; see module docs"]
async fn soak() {
    let seconds: u64 = std::env::var("SM_SOAK_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let dir = tempfile::tempdir().unwrap();
    const STREAMS: usize = 32;
    for i in 0..STREAMS {
        write_wav(
            &dir.path().join(format!("s{i}.wav")),
            4.0,
            200.0 + 20.0 * i as f32,
        );
    }
    let mut show = Show::default();
    show.settings.audio.backend = Some("null".into());
    show.settings.audio.channels = 16;
    show.settings.go_debounce_ms = 0;
    show.patch.outputs = (0..16)
        .map(|c| OutputDef {
            name: format!("Out {}", c + 1),
            channel: c,
        })
        .collect();
    show.timecode.ltc_enabled = true;
    show.timecode.ltc_channel = 15;
    show.timecode.mtc_enabled = true;
    show.timecode.mtc_port = "test".into();
    let mut children = Vec::new();
    for i in 0..STREAMS {
        let id = show.alloc_id();
        let routing = vec![
            Crosspoint {
                src: 0,
                out: (i % 14) as u16,
                db: -30.0,
            },
            Crosspoint {
                src: 1,
                out: (i % 14) as u16 + 1,
                db: -30.0,
            },
        ];
        let a = AudioCue {
            file: format!("s{i}.wav"),
            file_duration: Some(4.0),
            fade_in: 0.2,
            fade_out: 0.5,
            routing,
            ..AudioCue::default()
        };
        children.push(Cue {
            id,
            kind: CueKind::Audio(a),
            ..Cue::default()
        });
    }
    let midi_id = show.alloc_id();
    let m = MidiCue {
        port: "test".into(),
        message: MidiMessage::ControlChange {
            channel: 1,
            controller: 7,
            value: 100,
        },
    };
    children.push(Cue {
        id: midi_id,
        kind: CueKind::Midi(m),
        ..Cue::default()
    });
    let group = show.alloc_id();
    show.cue_lists[0].cues.push(Cue {
        id: group,
        number: "1".into(),
        kind: CueKind::Group(GroupCue {
            mode: GroupMode::Parallel,
            children,
        }),
        ..Cue::default()
    });
    let path = dir.path().join("soak.smshow");
    sm_core::showfile::save(&show, &path).unwrap();

    let midi = Arc::new(Mutex::new(Vec::new()));
    let backend = RecordingBackend {
        port: "test".into(),
        log: midi.clone(),
    };
    let (handle, _t) = runtime::spawn(Config {
        show_path: Some(path),
        midi_backend: Some(Box::new(backend)),
        ..Config::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(sm_server::http::serve(handle.clone(), listener));

    // Three remote clients that just consume the stream (like idle tablets).
    let received = Arc::new(std::sync::atomic::AtomicU64::new(0));
    for _ in 0..3 {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
            .await
            .unwrap();
        let received = received.clone();
        tokio::spawn(async move {
            let (_tx, mut rx) = ws.split();
            while let Some(Ok(_)) = rx.next().await {
                received.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        });
    }
    let cmd = |v| parse_command(v).unwrap();
    handle
        .command(cmd(json!({ "cmd": "tc_start" })), Role::Admin, "soak")
        .await
        .unwrap();

    let start = Instant::now();
    let frame0 = handle.tick()["frame"].as_u64().unwrap();
    let rss0 = rss_kb();
    let (mut fired, mut max_voices, mut max_cpu, mut edits) = (0u64, 0u64, 0f64, 0u64);
    let mut rss_mid = None;
    while start.elapsed() < Duration::from_secs(seconds) {
        handle
            .command(
                cmd(json!({ "cmd": "start_cue", "cue": group })),
                Role::Control,
                "soak",
            )
            .await
            .unwrap();
        fired += 1;
        for step in 0..25 {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let tick = handle.tick();
            max_voices = max_voices.max(tick["voices"].as_u64().unwrap_or(0));
            max_cpu = max_cpu.max(tick["cpu"].as_f64().unwrap_or(0.0));
            if step % 5 == 0 {
                // UI activity: edits and undo while cues are running.
                handle.command(cmd(json!({ "cmd": "update_cue", "cue": group, "patch": { "notes": format!("edit {edits}") } })), Role::Admin, "soak").await.unwrap();
                handle
                    .command(
                        cmd(json!({ "cmd": "set_master", "db": -(edits as f64 % 6.0) })),
                        Role::Control,
                        "soak",
                    )
                    .await
                    .unwrap();
                edits += 1;
            }
        }
        if rss_mid.is_none() && start.elapsed() > Duration::from_secs(seconds / 2) {
            rss_mid = rss_kb();
        }
    }
    let tick = handle.tick();
    let state = handle.state();
    let elapsed = start.elapsed().as_secs_f64();
    let frames = tick["frame"].as_u64().unwrap() - frame0;
    let rate = frames as f64 / elapsed;
    let mtc = midi
        .lock()
        .unwrap()
        .iter()
        .filter(|m| m.1[0] == 0xF1)
        .count() as f64;
    println!(
        "soak: {elapsed:.0} s, {fired} group starts ({} voice starts), {edits} edit rounds",
        fired * STREAMS as u64
    );
    println!(
        "soak: max voices {max_voices}, max cpu {:.1} %, underruns {}",
        max_cpu * 100.0,
        tick["underruns"]
    );
    println!(
        "soak: clock {rate:.1} frames/s, MTC quarter frames {:.2}/s, ws messages {}",
        mtc / elapsed,
        received.load(std::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "soak: rss start {rss0:?} kB, mid {rss_mid:?} kB, end {:?} kB",
        rss_kb()
    );
    assert_eq!(tick["underruns"], 0, "audio underruns under load");
    assert!(
        state["errors"].as_object().unwrap().is_empty(),
        "cue errors: {}",
        state["errors"]
    );
    assert!(
        max_voices >= STREAMS as u64,
        "all {STREAMS} streams played simultaneously (saw {max_voices})"
    );
    assert!(
        (rate - 48_000.0).abs() < 480.0,
        "audio clock ran at {rate} frames/s"
    );
    assert!(
        (mtc / elapsed - 100.0).abs() < 2.0,
        "MTC at 25 fps is 100 quarter frames/s, got {}",
        mtc / elapsed
    );
    assert_eq!(state["show"]["timecode"]["ltc_enabled"], true);
    if let (Some(mid), Some(end)) = (rss_mid, rss_kb()) {
        assert!(
            end < mid + mid / 5 + 20_000,
            "memory keeps growing: {mid} kB -> {end} kB"
        );
    }
}
