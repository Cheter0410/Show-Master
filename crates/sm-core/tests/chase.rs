//! Chasing external timecode on simulated time: LTC from the encoder is
//! decoded and fed to the engine the way the I/O layer does it, synthetic
//! frame streams cover drift, jumps, dropouts and stops.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sm_core::chase::{TcObservation, TcStatus};
use sm_core::engine::{Action, Command, Engine, EventKind};
use sm_core::midi::MidiMessage;
use sm_core::model::*;
use sm_core::timecode::{FrameRate, LtcDecoder, LtcEncoder, TcTransport, Timecode};

const SR: u32 = 48_000;
/// Engine look-ahead over the input clock, as in the runtime (12.5 ms).
const LOOKAHEAD: u64 = 600;

fn midi(note: u8) -> CueKind {
    CueKind::Midi(MidiCue {
        port: "out".into(),
        message: MidiMessage::NoteOn {
            channel: 1,
            note,
            velocity: 100,
        },
    })
}

struct Fixture {
    engine: Engine,
    audio: CueId,
    midi2: CueId,
    midi5: CueId,
}

/// 25 fps external timecode; audio cue (10 s) at 01:00:01:00, MIDI cues at
/// 01:00:02:00 and 01:00:05:00.
fn fixture(policy: TcJumpPolicy, rate: FrameRate) -> Fixture {
    let mut show = Show::default();
    show.timecode.rate = rate;
    show.timecode.source = TcSource::External;
    show.timecode.input.lock_frames = 4;
    show.timecode.input.freewheel_ms = 1000;
    show.cue_lists[0].tc_jump = policy;
    let add = |show: &mut Show, kind: CueKind, tc: Timecode| {
        let id = show.alloc_id();
        show.cue_lists[0].cues.push(Cue {
            id,
            number: id.to_string(),
            tc_trigger: Some(tc),
            kind,
            ..Cue::default()
        });
        id
    };
    let audio = add(
        &mut show,
        CueKind::Audio(AudioCue {
            file: "a.wav".into(),
            file_duration: Some(10.0),
            ..AudioCue::default()
        }),
        Timecode::new(1, 0, 1, 0),
    );
    let midi2 = add(&mut show, midi(2), Timecode::new(1, 0, 2, 0));
    let midi5 = add(&mut show, midi(5), Timecode::new(1, 0, 5, 0));
    Fixture {
        engine: Engine::new(show, SR),
        audio,
        midi2,
        midi5,
    }
}

fn midi_at(actions: &[Action]) -> Vec<(u64, u8)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Midi { at, bytes, .. } => Some((*at, bytes[1])),
            _ => None,
        })
        .collect()
}

fn started(events: &[sm_core::engine::Event]) -> Vec<(u64, CueId)> {
    events
        .iter()
        .filter_map(|e| match e.kind {
            EventKind::CueStarted { cue } => Some((e.frame, cue)),
            _ => None,
        })
        .collect()
}

fn tc_events(events: &[sm_core::engine::Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::Timecode { action, .. } => Some(action.clone()),
            _ => None,
        })
        .collect()
}

/// Run LTC from `source` through the decoder into the engine, block by
/// block, until input sample `until`. Returns all actions and events.
fn run_ltc(
    e: &mut Engine,
    enc: &mut LtcEncoder,
    dec: &mut LtcDecoder,
    from: u64,
    until: u64,
    rate: FrameRate,
) -> (Vec<Action>, Vec<sm_core::engine::Event>) {
    let mut actions = Vec::new();
    let mut events = Vec::new();
    let mut buf = vec![0.0f32; 256];
    let mut frames = Vec::new();
    let mut pos = from;
    while pos < until {
        let n = 256.min((until - pos) as usize);
        enc.render(pos, &mut buf[..n], 0.5);
        frames.clear();
        dec.push(&buf[..n], &mut frames);
        pos += n as u64;
        e.advance(pos + LOOKAHEAD);
        for f in &frames {
            // The decoder reports the end of a frame = the start of the next one.
            let frame = (f.timecode.to_frame_number(rate) + 1) % rate.frames_per_day();
            e.external_tc(TcObservation {
                frame,
                at: f.end_sample,
            });
        }
        actions.extend(e.drain_actions());
        events.extend(e.drain_events());
    }
    (actions, events)
}

#[test]
fn ltc_locks_and_fires_triggers_on_the_exact_sample() {
    for rate in [FrameRate::F25, FrameRate::F2997Df, FrameRate::F24] {
        let mut f = fixture(TcJumpPolicy::Skip, rate);
        assert_eq!(f.engine.tc_status(), TcStatus::NoSignal);
        let start = Timecode::new(1, 0, 0, 10).to_frame_number(rate);
        let source = TcTransport {
            rate,
            running: true,
            anchor_sample: 1000,
            anchor_frame: start,
        };
        let mut enc = LtcEncoder::new(SR);
        enc.set_transport(source);
        let mut dec = LtcDecoder::new(SR);
        let (actions, events) = run_ltc(
            &mut f.engine,
            &mut enc,
            &mut dec,
            0,
            u64::from(SR) * 4,
            rate,
        );
        assert_eq!(f.engine.tc_status(), TcStatus::Locked, "{rate:?}");
        assert!(tc_events(&events).iter().any(|a| a == "ext locked"));
        // Exact sample of a trigger frame in the source signal.
        let src_sample = |tc: Timecode| {
            let k = tc.to_frame_number(rate) - start;
            source.sample_after_frames(k, SR).unwrap()
        };
        let m = midi_at(&actions);
        assert_eq!(m.len(), 1, "{rate:?} {m:?}");
        let want = src_sample(Timecode::new(1, 0, 2, 0));
        assert!(m[0].0.abs_diff(want) <= 2, "{rate:?}: {} vs {want}", m[0].0);
        let s = started(&events);
        let audio = s.iter().find(|(_, c)| *c == f.audio).unwrap();
        let want = src_sample(Timecode::new(1, 0, 1, 0));
        assert!(
            audio.0.abs_diff(want) <= 2,
            "{rate:?}: {} vs {want}",
            audio.0
        );
        assert!(s.iter().any(|(_, c)| *c == f.midi2));
        assert!(!s.iter().any(|(_, c)| *c == f.midi5));
        // The engine clock shows the source position.
        let snap = f.engine.snapshot();
        let shown = Timecode::from_frame_number(snap.tc.frame_at(snap.frame, SR), rate);
        assert_eq!(shown, source.timecode_at(snap.frame, SR));
        assert!(snap.tc.running);
        // An LTC output following the engine transport regenerates the input.
        assert!(actions
            .iter()
            .any(|a| matches!(a, Action::Timecode(t) if t.running)));
    }
}

/// Feed synthetic frames: frame `first + i` starts at `t0 + i * len` (len in
/// samples, fractional to model a source clock that runs fast or slow).
fn feed(
    e: &mut Engine,
    first: u64,
    t0: f64,
    len: f64,
    count: u64,
) -> (Vec<Action>, Vec<sm_core::engine::Event>) {
    let mut actions = Vec::new();
    let mut events = Vec::new();
    for i in 0..count {
        let at = (t0 + i as f64 * len).round() as u64;
        e.advance(at + LOOKAHEAD);
        e.external_tc(TcObservation {
            frame: first + i,
            at,
        });
        actions.extend(e.drain_actions());
        events.extend(e.drain_events());
    }
    (actions, events)
}

#[test]
fn drift_is_corrected_without_double_or_missed_triggers() {
    let rate = FrameRate::F25;
    let start = Timecode::new(1, 0, 0, 0).to_frame_number(rate);
    for factor in [0.99, 1.01] {
        let mut f = fixture(TcJumpPolicy::Fire, rate);
        // 1 % off: the source drifts a frame against the audio clock every 4 s.
        let len = 1920.0 * factor;
        let (actions, events) = feed(&mut f.engine, start, 0.0, len, 25 * 8);
        let tcs = tc_events(&events);
        assert!(
            tcs.iter().any(|a| a == "ext re-anchored"),
            "{factor}: {tcs:?}"
        );
        assert!(!tcs.iter().any(|a| a == "ext jump"), "{tcs:?}");
        let m = midi_at(&actions);
        assert_eq!(
            m.iter().map(|x| x.1).collect::<Vec<_>>(),
            vec![2, 5],
            "{factor}"
        );
        // Each fires within one frame of the source position.
        for (at, note) in m {
            let want = (f64::from(note) * 25.0 * len).round() as u64;
            assert!(
                at.abs_diff(want) <= 1920,
                "{factor} note {note}: {at} vs {want}"
            );
        }
        assert_eq!(f.engine.tc_status(), TcStatus::Locked);
    }
}

#[test]
fn jumps_follow_the_list_policy() {
    let rate = FrameRate::F25;
    let start = Timecode::new(1, 0, 0, 0).to_frame_number(rate);
    // Chase: jump from 00:00 to 03:00 (source) starts the audio cue 2 s in.
    let mut f = fixture(TcJumpPolicy::Chase, rate);
    feed(&mut f.engine, start, 0.0, 1920.0, 10);
    assert_eq!(f.engine.tc_status(), TcStatus::Locked);
    let t0 = 10.0 * 1920.0;
    let (actions, events) = feed(&mut f.engine, start + 75, t0, 1920.0, 10);
    assert!(tc_events(&events).iter().any(|a| a == "ext jump"));
    let started: Vec<CueId> = started(&events).iter().map(|x| x.1).collect();
    assert!(started.contains(&f.audio));
    // MIDI cue at :02 is in the past: not fired by chase.
    assert!(midi_at(&actions).is_empty());
    let prepared = actions.iter().find_map(|a| match a {
        Action::PrepareVoice { spec, .. } => Some(spec.offset_frames),
        _ => None,
    });
    // Two seconds plus the confirmation frame.
    let off = prepared.unwrap();
    assert!((96_000..96_000 + 2 * 1920).contains(&off), "{off}");

    // Fire: skipped triggers fire when jumping forward while running.
    let mut f = fixture(TcJumpPolicy::Fire, rate);
    feed(&mut f.engine, start, 0.0, 1920.0, 10);
    let (actions, _) = feed(&mut f.engine, start + 150, 10.0 * 1920.0, 1920.0, 3);
    let notes: Vec<u8> = midi_at(&actions).iter().map(|x| x.1).collect();
    assert_eq!(notes, vec![2, 5]);

    // Skip: nothing.
    let mut f = fixture(TcJumpPolicy::Skip, rate);
    feed(&mut f.engine, start, 0.0, 1920.0, 10);
    let (actions, _) = feed(&mut f.engine, start + 150, 10.0 * 1920.0, 1920.0, 3);
    assert!(midi_at(&actions).is_empty());
}

#[test]
fn dropout_freewheels_then_stops_and_no_trigger_fires_after_loss() {
    let rate = FrameRate::F25;
    let start = Timecode::new(1, 0, 1, 10).to_frame_number(rate);
    let mut f = fixture(TcJumpPolicy::Skip, rate);
    // Signal for 10 frames, then nothing.
    feed(&mut f.engine, start, 0.0, 1920.0, 10);
    let last = 9 * 1920;
    f.engine.advance(last + 20_000);
    assert_eq!(f.engine.tc_status(), TcStatus::Freewheel);
    // The trigger at :02:00 (15 frames after the last one received) fires in freewheel.
    f.engine.advance(last + 47_000);
    let actions = f.engine.drain_actions();
    let m = midi_at(&actions);
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].0, (15 * 1920) as u64);
    assert_eq!(f.engine.tc_status(), TcStatus::Freewheel);
    // One second after the last frame: lost, stopped where it was.
    f.engine.advance(last + 48_000 + 10 * 48_000);
    assert_eq!(f.engine.tc_status(), TcStatus::Lost);
    let events = f.engine.drain_events();
    assert!(tc_events(&events).iter().any(|a| a == "ext LOST - stopped"));
    let snap = f.engine.snapshot();
    assert!(!snap.tc.running);
    let pos = snap.tc.frame_at(snap.frame, SR);
    assert_eq!(pos, start + 9 + 25);
    // The trigger at :05:00 never fired.
    assert!(midi_at(&f.engine.drain_actions()).is_empty());
}

#[test]
fn losing_the_input_is_exact_regardless_of_step_size() {
    let rate = FrameRate::F25;
    let start = Timecode::new(1, 0, 4, 0).to_frame_number(rate);
    let run = |step: u64| {
        let mut f = fixture(TcJumpPolicy::Skip, rate);
        feed(&mut f.engine, start, 0.0, 1920.0, 5);
        let mut t = 4 * 1920 + LOOKAHEAD;
        let mut midi = Vec::new();
        while t < 300_000 {
            t += step;
            f.engine.advance(t);
            midi.extend(midi_at(&f.engine.drain_actions()));
        }
        (midi, f.engine.tc_status())
    };
    // The 01:00:05 trigger lies 0.84 s after the last frame: fires (freewheel 1 s).
    let a = run(64);
    let b = run(100_000);
    assert_eq!(a, b);
    assert_eq!(a.0.len(), 1);
    assert_eq!(a.1, TcStatus::Lost);
}

#[test]
fn full_frame_stops_and_relocks_from_there() {
    let rate = FrameRate::F30;
    let start = Timecode::new(1, 0, 0, 0).to_frame_number(rate);
    let mut f = fixture(TcJumpPolicy::Chase, rate);
    feed(&mut f.engine, start, 0.0, 1600.0, 8);
    assert!(f.engine.snapshot().tc.running);
    let target = Timecode::new(1, 0, 3, 0).to_frame_number(rate);
    f.engine.external_tc_stopped(target);
    assert_eq!(f.engine.tc_status(), TcStatus::Stopped);
    let snap = f.engine.snapshot();
    assert!(!snap.tc.running);
    assert_eq!(snap.tc.anchor_frame, target);
    f.engine.drain_events();
    // Playback resumes from the located position: chase starts the audio cue 2 s in.
    let (_, events) = feed(&mut f.engine, target, 20_000.0, 1600.0, 6);
    assert_eq!(f.engine.tc_status(), TcStatus::Locked);
    assert!(started(&events).iter().any(|(_, c)| *c == f.audio));
}

#[test]
fn panic_holds_following_until_resumed_and_generator_commands_are_ignored() {
    let rate = FrameRate::F25;
    let start = Timecode::new(1, 0, 1, 20).to_frame_number(rate);
    let mut f = fixture(TcJumpPolicy::Skip, rate);
    feed(&mut f.engine, start, 0.0, 1920.0, 6);
    f.engine.command(Command::Panic).unwrap();
    assert_eq!(f.engine.tc_status(), TcStatus::Held);
    f.engine.drain_actions();
    // The source runs on past the :02:00 trigger: nothing fires.
    let (actions, _) = feed(&mut f.engine, start + 6, 6.0 * 1920.0, 1920.0, 30);
    assert!(midi_at(&actions).is_empty());
    assert!(!f.engine.snapshot().tc.running);
    // Relocating is impossible while chasing.
    f.engine
        .command(Command::TcRelocate {
            to: Timecode::new(2, 0, 0, 0),
        })
        .unwrap();
    assert!(f.engine.drain_events().iter().any(
        |e| matches!(&e.kind, EventKind::Info { message } if message.contains("relocate ignored"))
    ));
    // Resume: follows again.
    f.engine.command(Command::TcResume).unwrap();
    let (_, events) = feed(&mut f.engine, start + 36, 36.0 * 1920.0, 1920.0, 6);
    assert_eq!(f.engine.tc_status(), TcStatus::Locked);
    assert!(tc_events(&events).iter().any(|a| a == "ext locked"));
    // A GO does not start a transport-coupled generator.
    let mut show = f.engine.show().clone();
    show.timecode.mode = TcMode::Transport;
    f.engine.set_show(show);
    assert_eq!(f.engine.tc_status(), TcStatus::Locked);
}

#[test]
fn switching_the_source_stops_the_timecode() {
    let mut show = Show::default();
    let mut e = Engine::new(show.clone(), SR);
    e.command(Command::TcStart { from: None }).unwrap();
    e.advance(48_000);
    assert!(e.snapshot().tc.running);
    assert_eq!(e.tc_status(), TcStatus::Generator);
    show.timecode.source = TcSource::External;
    e.set_show(show.clone());
    assert_eq!(e.tc_status(), TcStatus::NoSignal);
    let snap = e.snapshot();
    assert!(!snap.tc.running);
    // Generator commands do not start anything while chasing.
    e.command(Command::TcStart { from: None }).unwrap();
    assert!(!e.snapshot().tc.running);
    show.timecode.source = TcSource::Generator;
    e.set_show(show.clone());
    assert_eq!(e.tc_status(), TcStatus::Generator);
    assert_eq!(
        e.snapshot().tc.anchor_frame,
        show.timecode.start.to_frame_number(show.timecode.rate)
    );
}
