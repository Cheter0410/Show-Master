//! External inputs through the whole runtime: fake MIDI input ports, an
//! in-process RTP-MIDI peer, MTC played in real time from the generator and
//! LTC through a virtual audio input. No hardware.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{json, Value};
use sm_audio::input::ManualInputBackend;
use sm_core::midi::{MidiMessage, MscCommand};
use sm_core::model::*;
use sm_core::timecode::{FrameRate, LtcEncoder, MtcGenerator, TcTransport, Timecode};
use sm_io::midi_in::{FakeMidiInput, FakeMidiInputFeed};
use sm_server::runtime::{self, Config, Handle};
use std::time::{Duration, Instant};

fn wait(h: &Handle, what: &str, secs: u64, pred: impl Fn(&Value, &Value) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let (state, tick) = (h.state(), h.tick());
        if pred(&state, &tick) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}\ntick: {tick}\ninputs: {}\nlog: {:?}",
            state["inputs"],
            log(h)
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn log(h: &Handle) -> Vec<String> {
    h.published
        .read()
        .map(|p| p.log_tail.iter().map(|e| e.message.clone()).collect())
        .unwrap_or_default()
}

fn cue(show: &mut Show, number: &str, kind: CueKind) -> CueId {
    let id = show.alloc_id();
    show.cue_lists[0].cues.push(Cue {
        id,
        number: number.into(),
        kind,
        ..Cue::default()
    });
    id
}

fn note_cue(note: u8) -> CueKind {
    CueKind::Midi(MidiCue {
        port: "nowhere".into(),
        message: MidiMessage::NoteOn {
            channel: 1,
            note,
            velocity: 1,
        },
    })
}

/// Cues 1-4 (MIDI cues to a port that does not exist: they start and end at
/// once, so the tests look at the log and the playhead).
fn base_show() -> (Show, Vec<CueId>) {
    let mut show = Show::default();
    show.settings.audio.backend = Some("null".into());
    show.settings.go_debounce_ms = 0;
    let ids = (1..=4)
        .map(|n| cue(&mut show, &n.to_string(), note_cue(n as u8)))
        .collect();
    (show, ids)
}

fn start(
    show: Show,
    midi: Option<FakeMidiInput>,
    audio: Option<ManualInputBackend>,
) -> (Handle, std::thread::JoinHandle<()>) {
    runtime::spawn(Config {
        show: Some(show),
        midi_backend: Some(Box::new(sm_io::midi::NoMidi)),
        midi_input_backend: midi.map(|m| Box::new(m) as Box<_>),
        audio_input_backend: audio.map(|a| Box::new(a) as Box<_>),
        ..Config::default()
    })
}

fn playhead(state: &Value) -> Option<u64> {
    let list = state["show"]["cue_lists"][0]["id"].as_u64()?;
    state["playheads"][list.to_string()].as_u64()
}

fn started(h: &Handle, number: &str) -> bool {
    log(h)
        .iter()
        .any(|l| l.starts_with(&format!("Cue {number} started")))
}

fn open(feed: &FakeMidiInputFeed, port: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !feed.is_open(port) {
        assert!(Instant::now() < deadline, "port {port} not opened");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn midi_mapping_drives_the_command_bus() {
    let (mut show, ids) = base_show();
    show.midi_ports.push(MidiPortAlias {
        name: "Desk".into(),
        device: "USB Desk".into(),
    });
    show.midi_input.mode = MidiInputMode::Selected;
    show.midi_input.ports = vec!["Desk".into()];
    show.midi_input.mappings = vec![
        MidiMapping {
            port: "Desk".into(),
            number: 60,
            ..MidiMapping::default()
        },
        MidiMapping {
            kind: MidiTriggerKind::ProgramChange,
            channel: 1,
            number: 5,
            action: MidiMapAction::GotoNumber,
            cue_number: "4".into(),
            ..MidiMapping::default()
        },
        MidiMapping {
            kind: MidiTriggerKind::ControlChange,
            number: 7,
            min_value: 64,
            action: MidiMapAction::StartCue,
            cue_number: "2".into(),
            ..MidiMapping::default()
        },
    ];
    let (backend, feed) = FakeMidiInput::new(&["USB Desk", "Other"]);
    let (h, _t) = start(show, Some(backend), None);
    open(&feed, "USB Desk");
    // Not listened to.
    assert!(!feed.is_open("Other"));
    assert_eq!(playhead(&h.state()), Some(ids[0]));

    // Unmapped messages do nothing (but show up as activity).
    feed.send("USB Desk", &[0x90, 61, 100]);
    feed.send("USB Desk", &[0x90, 60, 0]); // note-off
    wait(&h, "activity", 5, |s, _| s["inputs"]["midi_count"] == 2);
    assert_eq!(
        h.state()["inputs"]["midi_last"]["text"],
        "Note off ch 1 note 60"
    );
    assert_eq!(playhead(&h.state()), Some(ids[0]));

    // Note 60 -> GO: cue 1 fires, playhead to 2.
    feed.send("USB Desk", &[0x90, 60, 100]);
    wait(&h, "GO", 5, |s, _| playhead(s) == Some(ids[1]));
    wait(&h, "cue 1", 5, |_, _| started(&h, "1"));
    assert!(log(&h)
        .iter()
        .any(|l| l.contains("MIDI Note on ch 1 note 60 vel 100 (USB Desk) -> GO")));

    // Program change 5 on channel 1 -> playhead to cue 4; on channel 2 nothing.
    feed.send("USB Desk", &[0xC1, 5]);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(playhead(&h.state()), Some(ids[1]));
    feed.send("USB Desk", &[0xC0, 5]);
    wait(&h, "goto 4", 5, |s, _| playhead(s) == Some(ids[3]));

    // CC 7 rising through 64 -> start cue 2 (playhead stays).
    feed.send("USB Desk", &[0xB0, 7, 30]);
    feed.send("USB Desk", &[0xB0, 7, 90]);
    wait(&h, "cue 2", 5, |_, _| started(&h, "2"));
    assert_eq!(playhead(&h.state()), Some(ids[3]));

    // Learn support: the last message is structured.
    let last = &h.state()["inputs"]["midi_last"];
    assert_eq!(last["kind"], "control_change");
    assert_eq!(last["number"], 7);
    assert_eq!(last["port"], "USB Desk");
    let ports = h.state()["inputs"]["midi_ports"].clone();
    assert!(ports.as_array().unwrap().iter().any(|p| p == "Other"));
    assert_eq!(h.state()["inputs"]["midi_open"], json!(["USB Desk"]));
}

fn msc(device: u8, format: u8, command: MscCommand, q: &str) -> Vec<u8> {
    MidiMessage::Msc {
        device_id: device,
        command_format: format,
        command,
        q_number: q.into(),
        q_list: String::new(),
        q_path: String::new(),
    }
    .to_bytes()
    .unwrap()
}

#[test]
fn msc_receive() {
    let (mut show, ids) = base_show();
    show.midi_input.mode = MidiInputMode::All;
    show.midi_input.msc = MscInputSettings {
        enabled: true,
        device_id: 3,
        command_format: 0x10,
    };
    let (backend, feed) = FakeMidiInput::new(&["Lighting Desk"]);
    let (h, _t) = start(show, Some(backend), None);
    open(&feed, "Lighting Desk");
    // Wrong device, wrong format: not for us.
    feed.send("Lighting Desk", &msc(4, 0x10, MscCommand::Go, "3"));
    feed.send("Lighting Desk", &msc(3, 0x01, MscCommand::Go, "3"));
    // LOAD 3: playhead only.
    feed.send("Lighting Desk", &msc(3, 0x10, MscCommand::Load, "3"));
    wait(&h, "load", 5, |s, _| playhead(s) == Some(ids[2]));
    assert!(!started(&h, "3"));
    // GO 2 (all-call, all-types): goes to 2 and fires it.
    feed.send("Lighting Desk", &msc(127, 0x7F, MscCommand::Go, "2"));
    wait(&h, "go 2", 5, |s, _| playhead(s) == Some(ids[2]));
    wait(&h, "cue 2", 5, |_, _| started(&h, "2"));
    // GO without number: the playhead cue.
    feed.send("Lighting Desk", &msc(3, 0x10, MscCommand::Go, ""));
    wait(&h, "cue 3", 5, |_, _| started(&h, "3"));
    // Unsupported and unknown: logged, nothing happens.
    feed.send("Lighting Desk", &msc(3, 0x10, MscCommand::Fire, ""));
    feed.send("Lighting Desk", &msc(3, 0x10, MscCommand::Go, "77"));
    wait(&h, "ignored", 5, |_, _| {
        log(&h).iter().any(|l| l.contains("no cue with number 77"))
    });
    // RESET: playhead back to the top.
    feed.send("Lighting Desk", &msc(3, 0x10, MscCommand::Reset, ""));
    wait(&h, "reset", 5, |s, _| playhead(s) == Some(ids[0]));
    let l = log(&h);
    assert!(l
        .iter()
        .any(|x| x.contains("MSC received from Lighting Desk: MSC LOAD")));
    assert!(l.iter().any(|x| x.contains("FIRE is not supported")));
    assert!(!l.iter().any(|x| x.contains("device 4")));
}

#[test]
fn rtp_midi_peers_are_an_input() {
    let (mut show, ids) = base_show();
    show.rtp_midi = RtpMidiSettings {
        enabled: true,
        session_name: "SM Test".into(),
        port: 0,
        ..RtpMidiSettings::default()
    };
    show.midi_input.mode = MidiInputMode::Selected;
    show.midi_input.ports = vec!["rtp:SM Test".into()];
    show.midi_input.mappings = vec![MidiMapping {
        port: "rtp:SM Test".into(),
        kind: MidiTriggerKind::NoteOn,
        channel: 10,
        number: 36,
        action: MidiMapAction::Next,
        ..MidiMapping::default()
    }];
    let (h, _t) = start(show, Some(FakeMidiInput::new(&[]).0), None);
    wait(&h, "rtp port", 5, |s, _| {
        s["midi"]["rtp"]["port"].as_u64().is_some_and(|p| p > 0)
    });
    let port = h.state()["midi"]["rtp"]["port"].as_u64().unwrap() as u16;
    let mut cfg = sm_rtpmidi::SessionConfig::new("Peer");
    cfg.port = 0;
    cfg.bind_addr = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
    let peer = sm_rtpmidi::Session::start(cfg).unwrap();
    peer.connect(std::net::SocketAddr::from(([127, 0, 0, 1], port)))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !peer
        .peers()
        .iter()
        .any(|p| p.state == sm_rtpmidi::PeerState::Connected)
    {
        assert!(Instant::now() < deadline, "peer not connected");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(h.state()["inputs"]["midi_ports"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == "rtp:SM Test"));
    peer.send(&[0x99, 36, 120]).unwrap();
    wait(&h, "next", 5, |s, _| playhead(s) == Some(ids[1]));
    peer.send(&[0x99, 36, 120]).unwrap();
    wait(&h, "next again", 5, |s, _| playhead(s) == Some(ids[2]));
    assert!(log(&h).iter().any(|l| l.contains("(rtp:SM Test) -> Next")));
    peer.close();
}

/// External timecode show: a cue triggered at 01:00:02:00.
fn chase_show(kind: TcInputKind) -> Show {
    let (mut show, _) = base_show();
    show.timecode.rate = FrameRate::F25;
    show.timecode.source = TcSource::External;
    show.timecode.input = TcInputSettings {
        kind,
        mtc_port: "DAW".into(),
        ltc_device: Some("Line In".into()),
        ltc_channel: 1,
        lock_frames: 4,
        freewheel_ms: 500,
    };
    show.midi_ports.push(MidiPortAlias {
        name: "DAW".into(),
        device: "DAW MTC".into(),
    });
    show.cue_lists[0].cues[1].tc_trigger = Some(Timecode::new(1, 0, 2, 0));
    show
}

fn tc_frame(tick: &Value) -> Option<u64> {
    let tc: Timecode = tick["tc"]["position"].as_str()?.parse().ok()?;
    Some(tc.to_frame_number(FrameRate::F25))
}

#[test]
fn mtc_input_locks_follows_fires_and_is_lost() {
    let show = chase_show(TcInputKind::Mtc);
    let (backend, feed) = FakeMidiInput::new(&["DAW MTC"]);
    let (h, _t) = start(show, Some(backend), None);
    wait(&h, "status", 5, |_, t| {
        t["tc"]["status"] == "no_signal" && t["tc"]["source"] == "mtc"
    });
    open(&feed, "DAW MTC");
    let rate = FrameRate::F25;
    let start = Timecode::new(1, 0, 0, 0).to_frame_number(rate);
    let sr = 48_000u32;
    let mut g = MtcGenerator::new(sr);
    g.set_transport(TcTransport {
        rate,
        running: true,
        anchor_sample: 0,
        anchor_frame: start,
    });
    // Play 3 s of MTC in real time.
    let t0 = Instant::now();
    let mut msgs = Vec::new();
    g.advance(u64::from(sr) * 3, &mut |s, m| msgs.push((s, m.to_vec())));
    let mut locked_at = None;
    for (s, m) in msgs {
        let due = t0 + Duration::from_secs_f64(s as f64 / f64::from(sr));
        if let Some(d) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
        feed.send_at("DAW MTC", &m, due);
        if locked_at.is_none() && h.tick()["tc"]["status"] == "locked" {
            locked_at = Some(t0.elapsed());
        }
    }
    let locked_at = locked_at.expect("never locked");
    assert!(locked_at < Duration::from_millis(800), "{locked_at:?}");
    // Following: the shown position matches the source within a few frames
    // (tick publishing and scheduling jitter).
    let tick = h.tick();
    let src = start + (t0.elapsed().as_secs_f64() * 25.0) as u64;
    let shown = tc_frame(&tick).unwrap();
    assert!(shown.abs_diff(src) <= 4, "shown {shown} source {src}");
    assert!(tick["tc"]["running"].as_bool().unwrap());
    assert!(started(&h, "2"), "{:?}", log(&h));
    // The input stops: freewheel, then lost and stopped.
    wait(&h, "lost", 5, |_, t| t["tc"]["status"] == "lost");
    assert_eq!(h.tick()["tc"]["running"], false);
    let l = log(&h);
    assert!(l.iter().any(|x| x.contains("Timecode ext locked")));
    assert!(l.iter().any(|x| x.contains("ext LOST")));
    // A Full Frame locates the stopped timecode.
    feed.send(
        "DAW MTC",
        &sm_core::timecode::full_frame_message(Timecode::new(1, 0, 30, 0), rate),
    );
    wait(&h, "located", 5, |_, t| {
        t["tc"]["status"] == "stopped" && t["tc"]["position"] == "01:00:30:00"
    });
}

#[test]
fn ltc_input_through_a_virtual_audio_device() {
    let show = chase_show(TcInputKind::Ltc);
    let (backend, feed) = ManualInputBackend::new("Line In", 2, 48_000);
    let (h, _t) = start(show, None, Some(backend));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !feed.is_open() {
        assert!(Instant::now() < deadline, "LTC input not opened");
        std::thread::sleep(Duration::from_millis(5));
    }
    wait(&h, "device list", 5, |s, _| {
        s["inputs"]["audio_inputs"][0]["id"] == "Line In"
    });
    let rate = FrameRate::F25;
    let start = Timecode::new(1, 0, 0, 5).to_frame_number(rate);
    let sr = 48_000u32;
    let mut enc = LtcEncoder::new(sr);
    enc.set_transport(TcTransport {
        rate,
        running: true,
        anchor_sample: 0,
        anchor_frame: start,
    });
    let t0 = Instant::now();
    let block = 480usize;
    let mut mono = vec![0.0f32; block];
    let mut pos = 0u64;
    while pos < u64::from(sr) * 3 {
        enc.render(pos, &mut mono, 0.3);
        pos += block as u64;
        let due = t0 + Duration::from_secs_f64(pos as f64 / f64::from(sr));
        if let Some(d) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        }
        let stereo: Vec<f32> = mono.iter().flat_map(|x| [0.0, *x]).collect();
        feed.push(&stereo, due);
    }
    let tick = h.tick();
    assert_eq!(tick["tc"]["status"], "locked", "{:?}", log(&h));
    assert_eq!(tick["tc"]["source"], "ltc");
    let src = start + (t0.elapsed().as_secs_f64() * 25.0) as u64;
    let shown = tc_frame(&tick).unwrap();
    assert!(shown.abs_diff(src) <= 4, "shown {shown} source {src}");
    assert!(started(&h, "2"), "{:?}", log(&h));
    let level = h.state()["inputs"]["ltc_level_db"].as_f64();
    assert!(level.is_some_and(|l| l > -20.0), "{level:?}");
    // Silence: lost after the freewheel time.
    wait(&h, "lost", 5, |_, t| t["tc"]["status"] == "lost");
}
