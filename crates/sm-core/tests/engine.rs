//! Cue engine tests on simulated time: no audio, no threads, no sleeping.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sm_core::engine::{Action, Command, Engine, EventKind, Phase, VoiceId};
use sm_core::midi::MidiMessage;
use sm_core::model::*;
use sm_core::timecode::{FrameRate, Timecode};

const SR: u32 = 48_000;

fn audio(dur: f64) -> CueKind {
    CueKind::Audio(AudioCue {
        file: "a.wav".into(),
        file_duration: Some(dur),
        ..AudioCue::default()
    })
}

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

struct B(Show);

impl B {
    fn new() -> Self {
        let mut show = Show::default();
        show.settings.go_debounce_ms = 0;
        B(show)
    }
    fn cue(&mut self, kind: CueKind) -> CueId {
        self.with(kind, |_| {})
    }
    fn with(&mut self, kind: CueKind, f: impl FnOnce(&mut Cue)) -> CueId {
        let id = self.0.alloc_id();
        let mut cue = Cue {
            id,
            number: id.to_string(),
            kind,
            ..Cue::default()
        };
        f(&mut cue);
        self.0.cue_lists[0].cues.push(cue);
        id
    }
    fn child(&mut self, kind: CueKind) -> Cue {
        let id = self.0.alloc_id();
        Cue {
            id,
            number: id.to_string(),
            kind,
            ..Cue::default()
        }
    }
    fn engine(self) -> Engine {
        Engine::new(self.0, SR)
    }
}

fn starts(actions: &[Action]) -> Vec<(VoiceId, u64)> {
    actions
        .iter()
        .filter_map(|a| {
            if let Action::StartVoice { voice, at, .. } = a {
                Some((*voice, *at))
            } else {
                None
            }
        })
        .collect()
}

fn midis(actions: &[Action]) -> Vec<(u64, u8)> {
    actions
        .iter()
        .filter_map(|a| {
            if let Action::Midi { at, bytes, .. } = a {
                Some((*at, bytes[1]))
            } else {
                None
            }
        })
        .collect()
}

fn playhead(e: &Engine) -> Option<CueId> {
    e.snapshot().lists[0].playhead
}

#[test]
fn go_fires_playhead_and_advances() {
    let mut b = B::new();
    let c1 = b.cue(audio(10.0));
    let c2 = b.cue(audio(10.0));
    let mut e = b.engine();
    assert_eq!(playhead(&e), Some(c1));
    // The playhead cue is pre-opened (standby) before GO.
    let pre = e.drain_actions();
    let standby: Vec<VoiceId> = pre
        .iter()
        .filter_map(|a| {
            if let Action::PrepareVoice { voice, cue, .. } = a {
                (*cue == c1).then_some(*voice)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(standby.len(), 1);

    e.advance(1000);
    e.command(Command::Go { list: None }).unwrap();
    let acts = e.drain_actions();
    assert_eq!(
        starts(&acts),
        vec![(standby[0], 1000)],
        "GO starts the standby voice at the GO frame"
    );
    assert_eq!(playhead(&e), Some(c2));
    let snap = e.snapshot();
    assert_eq!(snap.active.len(), 1);
    assert_eq!(snap.active[0].phase, Phase::Running);
    assert_eq!(snap.show_elapsed, Some(0.0));
    // cue ends by itself after exactly 10 s
    e.advance(1000 + 10 * SR as u64 - 1);
    assert_eq!(e.snapshot().active.len(), 1);
    e.advance(1000 + 10 * SR as u64);
    assert!(e.snapshot().active.is_empty());
    e.command(Command::Go { list: None }).unwrap();
    assert_eq!(playhead(&e), None);
    e.command(Command::Go { list: None }).unwrap();
    assert!(e
        .drain_events()
        .iter()
        .any(|ev| matches!(&ev.kind, EventKind::GoIgnored { reason } if reason == "end of list")));
}

#[test]
fn go_is_debounced() {
    let mut b = B::new();
    b.0.settings.go_debounce_ms = 250;
    b.cue(midi(1));
    b.cue(midi(2));
    b.cue(midi(3));
    let mut e = b.engine();
    e.advance(100);
    e.command(Command::Go { list: None }).unwrap();
    e.advance(100 + 11_999);
    e.command(Command::Go { list: None }).unwrap(); // double click: ignored
    e.advance(100 + 12_000);
    e.command(Command::Go { list: None }).unwrap();
    assert_eq!(midis(&e.drain_actions()), vec![(100, 1), (12_100, 2)]);
}

#[test]
fn parallel_group_starts_children_on_the_same_sample() {
    let mut b = B::new();
    let kids = vec![b.child(audio(5.0)), b.child(audio(3.0)), b.child(midi(9))];
    let g = b.cue(CueKind::Group(GroupCue {
        mode: GroupMode::Parallel,
        children: kids,
    }));
    let after = b.cue(midi(1));
    b.0.cue_mut(g).unwrap().continue_mode = ContinueMode::AutoFollow;
    let mut e = b.engine();
    e.advance(777);
    e.command(Command::Go { list: None }).unwrap();
    let acts = e.drain_actions();
    let s = starts(&acts);
    assert_eq!(s.len(), 2);
    assert!(
        s.iter().all(|(_, at)| *at == 777),
        "all children share one start frame: {s:?}"
    );
    assert_eq!(midis(&acts), vec![(777, 9)]);
    assert_eq!(playhead(&e), None, "playhead moves past the follow chain");
    // group ends when the longest child ends -> auto-follow fires exactly then
    e.advance(777 + 5 * SR as u64);
    assert_eq!(midis(&e.drain_actions()), vec![(777 + 5 * SR as u64, 1)]);
    let _ = after;
}

#[test]
fn waits_and_continues_are_sample_exact() {
    let mut b = B::new();
    b.with(midi(1), |c| {
        c.pre_wait = 1.0;
        c.post_wait = 2.0;
        c.continue_mode = ContinueMode::AutoContinue;
    });
    b.with(audio(2.5), |c| {
        c.pre_wait = 0.25;
        c.continue_mode = ContinueMode::AutoFollow;
    });
    b.cue(midi(3));
    let mut e = b.engine();
    let t0 = 12_345u64;
    e.advance(t0);
    e.command(Command::Go { list: None }).unwrap();
    assert!(
        midis(&e.drain_actions()).is_empty(),
        "nothing before pre-wait"
    );
    assert_eq!(e.snapshot().active[0].phase, Phase::PreWait);
    assert!((e.snapshot().next_auto.unwrap() - 3.0).abs() < 1e-9);
    // Advance in one huge step: timestamps must still be exact.
    e.advance(t0 + 20 * SR as u64);
    let acts = e.drain_actions();
    let s = SR as u64;
    assert_eq!(
        midis(&acts),
        vec![(t0 + s, 1), (t0 + 3 * s + s / 4 + 5 * s / 2, 3)]
    );
    assert_eq!(
        starts(&acts).iter().map(|x| x.1).collect::<Vec<_>>(),
        vec![t0 + 3 * s + s / 4]
    );
}

#[test]
fn stepping_in_small_or_large_blocks_gives_identical_results() {
    let build = || {
        let mut b = B::new();
        b.with(audio(1.0), |c| c.continue_mode = ContinueMode::AutoFollow);
        b.with(midi(2), |c| {
            c.post_wait = 0.3333;
            c.continue_mode = ContinueMode::AutoContinue;
        });
        b.with(audio(0.7), |c| c.pre_wait = 0.111);
        b.engine()
    };
    let run = |step: u64| {
        let mut e = build();
        e.advance(5);
        e.command(Command::Go { list: None }).unwrap();
        let mut t = 5;
        let mut acts = e.drain_actions();
        while t < 3 * SR as u64 {
            t += step;
            e.advance(t);
            acts.extend(e.drain_actions());
        }
        (starts(&acts), midis(&acts))
    };
    assert_eq!(run(1), run(64));
    assert_eq!(run(64), run(100_000));
}

#[test]
fn pause_shifts_everything() {
    let mut b = B::new();
    let c1 = b.with(audio(2.0), |c| c.continue_mode = ContinueMode::AutoFollow);
    b.cue(midi(7));
    let mut e = b.engine();
    e.command(Command::Go { list: None }).unwrap();
    e.advance(SR as u64);
    e.command(Command::PauseAll).unwrap();
    assert!(e.snapshot().paused_all && e.snapshot().active[0].paused);
    e.advance(11 * SR as u64); // 10 s pause
    assert!(midis(&e.drain_actions()).is_empty());
    assert!((e.snapshot().active[0].elapsed - 1.0).abs() < 1e-9);
    e.command(Command::TogglePause).unwrap();
    e.advance(13 * SR as u64);
    let acts = e.drain_actions();
    assert!(acts
        .iter()
        .any(|a| matches!(a, Action::ResumeVoice { at, .. } if *at == 11 * SR as u64)));
    assert_eq!(midis(&acts), vec![(12 * SR as u64, 7)]);
    // per-cue pause
    e.command(Command::StartCue { cue: c1 }).unwrap();
    e.command(Command::PauseCue { cue: c1 }).unwrap();
    assert!(e.snapshot().active[0].paused);
    e.command(Command::ResumeCue { cue: c1 }).unwrap();
    assert!(!e.snapshot().active[0].paused);
}

#[test]
fn fade_cue_fades_and_stops_target() {
    let mut b = B::new();
    let a = b.cue(audio(60.0));
    b.cue(CueKind::Fade(FadeCue {
        target: Some(a),
        duration: 2.0,
        gain_db: Some(-120.0),
        stop_target: true,
        ..FadeCue::default()
    }));
    let mut e = b.engine();
    e.command(Command::Go { list: None }).unwrap();
    let voice = starts(&e.drain_actions())[0].0;
    e.advance(SR as u64);
    e.command(Command::Go { list: None }).unwrap();
    let acts = e.drain_actions();
    assert!(acts.iter().any(|x| matches!(x, Action::FadeVoice { voice: v, at, frames, master: Some(m), .. } if *v == voice && *at == SR as u64 && *frames == 2 * SR as u64 && *m == 0.0)));
    assert_eq!(e.snapshot().active.len(), 2);
    e.advance(3 * SR as u64);
    let acts = e.drain_actions();
    assert!(acts.iter().any(|x| matches!(x, Action::StopVoice { voice: v, at, .. } if *v == voice && *at == 3 * SR as u64)));
    assert!(e.snapshot().active.is_empty());
}

#[test]
fn audio_fade_out_is_scheduled_before_the_end() {
    let mut b = B::new();
    b.cue(CueKind::Audio(AudioCue {
        file: "a.wav".into(),
        file_duration: Some(10.0),
        start: 2.0,
        end: Some(6.0),
        fade_out: 1.0,
        loop_count: 2,
        ..AudioCue::default()
    }));
    let mut e = b.engine();
    e.command(Command::Go { list: None }).unwrap();
    e.advance(20 * SR as u64);
    let acts = e.drain_actions();
    // region 4 s x 2 loops = 8 s, fade starts at 7 s
    assert!(acts.iter().any(|x| matches!(x, Action::FadeVoice { at, frames, master: Some(m), .. } if *at == 7 * SR as u64 && *frames == SR as u64 && *m == 0.0)));
    assert!(e
        .drain_events()
        .iter()
        .any(|ev| ev.frame == 8 * SR as u64 && matches!(ev.kind, EventKind::CueEnded { .. })));
}

#[test]
fn disarmed_cues_do_nothing_but_keep_the_chain() {
    let mut b = B::new();
    b.with(midi(1), |c| {
        c.armed = false;
        c.continue_mode = ContinueMode::AutoContinue;
        c.post_wait = 1.0;
    });
    b.cue(midi(2));
    let mut e = b.engine();
    e.command(Command::Go { list: None }).unwrap();
    e.advance(2 * SR as u64);
    assert_eq!(midis(&e.drain_actions()), vec![(SR as u64, 2)]);
}

#[test]
fn control_cues() {
    let mut b = B::new();
    let a = b.cue(audio(60.0));
    let m = b.cue(midi(5));
    let start = b.cue(CueKind::Control(ControlCue {
        action: ControlAction::Start,
        target: Some(a),
        time: 0.0,
    }));
    let pause = b.cue(CueKind::Control(ControlCue {
        action: ControlAction::Pause,
        target: Some(a),
        time: 0.0,
    }));
    let resume = b.cue(CueKind::Control(ControlCue {
        action: ControlAction::Resume,
        target: Some(a),
        time: 0.0,
    }));
    let disarm = b.cue(CueKind::Control(ControlCue {
        action: ControlAction::Disarm,
        target: Some(m),
        time: 0.0,
    }));
    let goto = b.cue(CueKind::Control(ControlCue {
        action: ControlAction::GoTo,
        target: Some(m),
        time: 0.0,
    }));
    let stop = b.cue(CueKind::Control(ControlCue {
        action: ControlAction::Stop,
        target: Some(a),
        time: 1.5,
    }));
    let load = b.cue(CueKind::Control(ControlCue {
        action: ControlAction::LoadToTime,
        target: Some(a),
        time: 30.0,
    }));
    let broken = b.cue(CueKind::Control(ControlCue {
        action: ControlAction::Start,
        target: None,
        time: 0.0,
    }));
    let mut e = b.engine();
    let fire = |e: &mut Engine, c| e.command(Command::StartCue { cue: c }).unwrap();
    fire(&mut e, start);
    assert_eq!(
        e.snapshot()
            .active
            .iter()
            .map(|x| x.cue)
            .collect::<Vec<_>>(),
        vec![a]
    );
    fire(&mut e, pause);
    assert!(e.snapshot().active[0].paused);
    fire(&mut e, resume);
    assert!(!e.snapshot().active[0].paused);
    fire(&mut e, disarm);
    assert!(!e.show().cue(m).unwrap().armed);
    fire(&mut e, goto);
    assert_eq!(playhead(&e), Some(m));
    e.drain_actions();
    fire(&mut e, stop);
    assert!(e.drain_actions().iter().any(|x| matches!(
        x,
        Action::StopVoice {
            fade_frames: 72_000,
            ..
        }
    )));
    assert!(e.snapshot().active.is_empty());
    fire(&mut e, load);
    assert_eq!(e.snapshot().loaded.get(&a), Some(&30.0));
    e.drain_actions();
    fire(&mut e, start);
    let acts = e.drain_actions();
    assert!(acts.iter().any(
        |x| matches!(x, Action::PrepareVoice { spec, .. } if spec.offset_frames == 30 * SR as u64)
    ));
    assert!((e.snapshot().active[0].elapsed - 30.0).abs() < 1e-9);
    fire(&mut e, broken);
    assert!(e.snapshot().errors.contains_key(&broken));
}

#[test]
fn playlist_and_random_groups() {
    let mut b = B::new();
    let kids = vec![b.child(midi(1)), b.child(audio(1.0)), b.child(midi(3))];
    let pl = b.cue(CueKind::Group(GroupCue {
        mode: GroupMode::Playlist,
        children: kids,
    }));
    let kids = vec![
        b.child(midi(10)),
        b.child(midi(11)),
        b.child(midi(12)),
        b.child(midi(13)),
    ];
    let rnd = b.cue(CueKind::Group(GroupCue {
        mode: GroupMode::Random,
        children: kids,
    }));
    let mut e = b.engine();
    e.command(Command::StartCue { cue: pl }).unwrap();
    e.advance(2 * SR as u64);
    let acts = e.drain_actions();
    assert_eq!(midis(&acts), vec![(0, 1), (SR as u64, 3)]);
    assert_eq!(starts(&acts).len(), 1);
    assert!(e.snapshot().active.is_empty());
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..64 {
        e.command(Command::StartCue { cue: rnd }).unwrap();
        let m = midis(&e.drain_actions());
        assert_eq!(m.len(), 1, "random plays exactly one child");
        seen.insert(m[0].1);
    }
    assert_eq!(seen.len(), 4, "all children get picked eventually");
}

#[test]
fn stop_all_and_panic() {
    let mut b = B::new();
    b.0.settings.stop_fade_ms = 500;
    b.0.settings.panic_fade_ms = 0;
    let a = b.cue(audio(60.0));
    let mut e = b.engine();
    e.command(Command::Go { list: None }).unwrap();
    e.drain_actions();
    e.command(Command::StopAll).unwrap();
    assert!(e.drain_actions().contains(&Action::StopAllAudio {
        at: 0,
        fade_frames: 24_000,
        hard: false
    }));
    assert!(e.snapshot().active.is_empty());
    e.command(Command::StartCue { cue: a }).unwrap();
    e.command(Command::Panic).unwrap();
    let acts = e.drain_actions();
    assert!(acts.contains(&Action::StopAllAudio {
        at: 0,
        fade_frames: 0,
        hard: true
    }));
    assert!(acts.contains(&Action::MidiPanic));
    assert!(e.snapshot().active.is_empty());
    e.command(Command::Reset).unwrap();
    assert_eq!(playhead(&e), Some(a));
    assert_eq!(e.snapshot().show_elapsed, None);
}

#[test]
fn playhead_navigation() {
    let mut b = B::new();
    let c1 = b.cue(midi(1));
    let c2 = b.cue(midi(2));
    let c3 = b.with(midi(3), |c| c.number = "14.5".into());
    let mut e = b.engine();
    e.command(Command::Next { list: None }).unwrap();
    assert_eq!(playhead(&e), Some(c2));
    e.command(Command::Next { list: None }).unwrap();
    e.command(Command::Next { list: None }).unwrap();
    assert_eq!(playhead(&e), None);
    e.command(Command::Previous { list: None }).unwrap();
    assert_eq!(playhead(&e), Some(c3));
    e.command(Command::SetPlayhead {
        list: None,
        cue: Some(c1),
    })
    .unwrap();
    e.command(Command::Previous { list: None }).unwrap();
    assert_eq!(playhead(&e), Some(c1));
    e.command(Command::GotoNumber {
        number: "14.5".into(),
    })
    .unwrap();
    assert_eq!(playhead(&e), Some(c3));
    assert!(e
        .command(Command::GotoNumber {
            number: "99".into()
        })
        .is_err());
    assert!(e.command(Command::StartCue { cue: 4242 }).is_err());
    assert!(
        e.drain_actions()
            .iter()
            .all(|a| !matches!(a, Action::Midi { .. })),
        "moving the playhead never fires"
    );
}

fn tc_show(policy: TcJumpPolicy) -> (Engine, CueId, CueId, CueId) {
    let mut b = B::new();
    b.0.timecode.rate = FrameRate::F25;
    b.0.timecode.start = Timecode::new(1, 0, 0, 0);
    b.0.cue_lists[0].tc_jump = policy;
    let a = b.with(audio(10.0), |c| {
        c.tc_trigger = Some(Timecode::new(1, 0, 2, 0))
    });
    let m = b.with(midi(5), |c| c.tc_trigger = Some(Timecode::new(1, 0, 4, 12)));
    let late = b.with(audio(10.0), |c| {
        c.tc_trigger = Some(Timecode::new(1, 0, 30, 0))
    });
    (b.engine(), a, m, late)
}

#[test]
fn timecode_triggers_fire_on_the_exact_sample() {
    let (mut e, _, _, _) = tc_show(TcJumpPolicy::Skip);
    e.advance(1000);
    e.command(Command::TcStart { from: None }).unwrap();
    assert!(e
        .drain_actions()
        .iter()
        .any(|a| matches!(a, Action::Timecode(t) if t.running && t.anchor_sample == 1000)));
    e.advance(1000 + 6 * SR as u64);
    let acts = e.drain_actions();
    assert_eq!(
        starts(&acts).iter().map(|s| s.1).collect::<Vec<_>>(),
        vec![1000 + 2 * SR as u64]
    );
    // 4 s 12 frames at 25 fps = 4.48 s
    assert_eq!(midis(&acts), vec![(1000 + 4 * SR as u64 + 12 * 1920, 5)]);
    assert_eq!(e.snapshot().tc_position, "01:00:06:00");
    assert!(e.snapshot().active[0].from_tc);
    // pause / resume does not re-fire and keeps the position
    e.command(Command::TcPause).unwrap();
    e.advance(1000 + 60 * SR as u64);
    assert_eq!(e.snapshot().tc_position, "01:00:06:00");
    e.command(Command::TcResume).unwrap();
    e.advance(1000 + 61 * SR as u64);
    assert_eq!(e.snapshot().tc_position, "01:00:07:00");
    assert!(starts(&e.drain_actions()).is_empty());
}

#[test]
fn timecode_jump_skip_fire_chase() {
    // Skip: relocating past triggers fires nothing.
    let (mut e, _, _, _) = tc_show(TcJumpPolicy::Skip);
    e.command(Command::TcStart { from: None }).unwrap();
    e.advance(100);
    e.command(Command::TcRelocate {
        to: Timecode::new(1, 0, 5, 0),
    })
    .unwrap();
    e.advance(SR as u64);
    let acts = e.drain_actions();
    assert!(starts(&acts).is_empty() && midis(&acts).is_empty());

    // Fire: jumped-over cues fire at the jump.
    let (mut e, _, _, _) = tc_show(TcJumpPolicy::Fire);
    e.command(Command::TcStart { from: None }).unwrap();
    e.advance(100);
    e.command(Command::TcRelocate {
        to: Timecode::new(1, 0, 5, 0),
    })
    .unwrap();
    let acts = e.drain_actions();
    assert_eq!(
        starts(&acts).iter().map(|s| s.1).collect::<Vec<_>>(),
        vec![100]
    );
    assert_eq!(midis(&acts), vec![(100, 5)]);

    // Chase: the 10 s audio cue triggered at :02 is 3 s in at :05; the MIDI cue is in the past.
    let (mut e, a, _, late) = tc_show(TcJumpPolicy::Chase);
    e.command(Command::TcStart { from: None }).unwrap();
    e.advance(100);
    e.drain_actions();
    e.command(Command::TcRelocate {
        to: Timecode::new(1, 0, 5, 0),
    })
    .unwrap();
    let acts = e.drain_actions();
    assert!(acts.iter().any(|x| matches!(x, Action::PrepareVoice { cue, spec, .. } if *cue == a && spec.offset_frames == 3 * SR as u64)));
    assert_eq!(starts(&acts).len(), 1);
    assert!(midis(&acts).is_empty());
    assert!((e.snapshot().active[0].elapsed - 3.0).abs() < 1e-9);
    // Jump forward to :31 -> first cue is over (stays as is until its own end), late cue chases in at 1 s.
    e.command(Command::TcRelocate {
        to: Timecode::new(1, 0, 31, 0),
    })
    .unwrap();
    assert!(e
        .snapshot()
        .active
        .iter()
        .any(|x| x.cue == late && (x.elapsed - 1.0).abs() < 1e-9));
    // Jump back to :03 -> the late cue (triggered in the future) is stopped.
    e.command(Command::TcRelocate {
        to: Timecode::new(1, 0, 3, 0),
    })
    .unwrap();
    assert!(e.snapshot().active.iter().all(|x| x.cue != late));
}

#[test]
fn transport_coupled_timecode_follows_the_show() {
    let mut b = B::new();
    b.0.timecode.mode = TcMode::Transport;
    b.0.timecode.start = Timecode::new(0, 0, 10, 0);
    b.cue(audio(60.0));
    let mut e = b.engine();
    assert!(!e.snapshot().tc.running);
    e.advance(500);
    e.command(Command::Go { list: None }).unwrap();
    assert!(e.snapshot().tc.running);
    e.advance(500 + SR as u64);
    assert_eq!(e.snapshot().tc_position, "00:00:11:00");
    e.command(Command::PauseAll).unwrap();
    e.advance(500 + 5 * SR as u64);
    assert_eq!(e.snapshot().tc_position, "00:00:11:00");
    e.command(Command::ResumeAll).unwrap();
    e.advance(500 + 6 * SR as u64);
    assert_eq!(e.snapshot().tc_position, "00:00:12:00");
    e.command(Command::StopAll).unwrap();
    assert!(!e.snapshot().tc.running);
    assert_eq!(e.snapshot().tc_position, "00:00:10:00");
}

#[test]
fn editing_the_show_while_running() {
    let mut b = B::new();
    let a = b.cue(audio(60.0));
    let m = b.cue(midi(1));
    let mut e = b.engine();
    e.command(Command::Go { list: None }).unwrap();
    e.drain_actions();
    let mut show = e.show().clone();
    show.cue_lists[0].cues.retain(|c| c.id != a);
    e.set_show(show);
    assert!(e.snapshot().active.is_empty(), "deleted cue is stopped");
    assert!(e
        .drain_actions()
        .iter()
        .any(|x| matches!(x, Action::StopVoice { .. })));
    assert_eq!(playhead(&e), Some(m));
    // voice errors mark the cue as broken
    let mut show = e.show().clone();
    let id = show.alloc_id();
    show.cue_lists[0].cues.insert(
        0,
        Cue {
            id,
            kind: audio(5.0),
            ..Cue::default()
        },
    );
    e.set_show(show);
    e.command(Command::StartCue { cue: id }).unwrap();
    let voice = starts(&e.drain_actions())[0].0;
    e.voice_error(voice, "file not found".into());
    assert_eq!(
        e.snapshot().errors.get(&id).map(String::as_str),
        Some("file not found")
    );
    assert!(e.snapshot().active.is_empty());
}

#[test]
fn commands_roundtrip_as_json() {
    let cmd: Command = serde_json::from_str(r#"{"cmd":"go"}"#).unwrap();
    assert_eq!(cmd, Command::Go { list: None });
    let cmd: Command = serde_json::from_str(r#"{"cmd":"tc_relocate","to":"01:00:00:00"}"#).unwrap();
    assert_eq!(
        cmd,
        Command::TcRelocate {
            to: Timecode::new(1, 0, 0, 0)
        }
    );
    let cmd: Command = serde_json::from_str(r#"{"cmd":"load_cue","cue":3,"time":1.5}"#).unwrap();
    assert_eq!(cmd, Command::LoadCue { cue: 3, time: 1.5 });
}

#[test]
fn panic_holds_a_free_running_timecode_and_no_trigger_fires_afterwards() {
    let (mut e, _, _, _) = tc_show(TcJumpPolicy::Skip);
    e.command(Command::TcStart { from: None }).unwrap();
    e.advance(SR as u64); // 01:00:01:00
    e.command(Command::Panic).unwrap();
    let snap = e.snapshot();
    assert!(!snap.tc.running, "generator paused by PANIC");
    assert_eq!(snap.tc_position, "01:00:01:00");
    e.drain_actions();
    // The triggers at :02 and :04 must not fire while the generator is held.
    e.advance(10 * SR as u64);
    let acts = e.drain_actions();
    assert!(starts(&acts).is_empty() && midis(&acts).is_empty());
    // Resume continues from the held position.
    e.command(Command::TcResume).unwrap();
    e.advance(11 * SR as u64);
    assert_eq!(e.snapshot().tc_position, "01:00:02:00");
}

#[test]
fn stop_all_leaves_a_free_running_timecode_alone() {
    let (mut e, _, _, _) = tc_show(TcJumpPolicy::Skip);
    e.command(Command::TcStart { from: None }).unwrap();
    e.advance(SR as u64);
    e.command(Command::StopAll).unwrap();
    assert!(e.snapshot().tc.running);
}

#[test]
fn osc_cue_sends_its_message_and_bad_ones_report() {
    use sm_core::osc::{OscArg, OscMessage};
    let mut b = B::new();
    let ok = b.cue(CueKind::Osc(OscMessage {
        host: "10.0.0.5".into(),
        port: 53000,
        address: "/cue/1/start".into(),
        args: vec![OscArg::Int(1)],
    }));
    let bad = b.cue(CueKind::Osc(OscMessage {
        address: "no-slash".into(),
        ..OscMessage::default()
    }));
    let mut e = b.engine();
    e.drain_actions();
    e.command(Command::Go { list: None }).unwrap();
    let a = e.drain_actions();
    let sent: Vec<_> = a
        .iter()
        .filter_map(|a| match a {
            Action::Osc { target, bytes, .. } => Some((target.clone(), bytes.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, "10.0.0.5:53000");
    assert!(sent[0].1.starts_with(b"/cue/1/start\0"));
    let _ = ok;
    e.command(Command::Go { list: None }).unwrap();
    assert!(e
        .drain_actions()
        .iter()
        .all(|a| !matches!(a, Action::Osc { .. })));
    let errors: Vec<_> = e
        .drain_events()
        .into_iter()
        .filter(|ev| matches!(&ev.kind, EventKind::CueError { cue, .. } if *cue == bad))
        .collect();
    assert_eq!(errors.len(), 1);
}

#[test]
fn panic_sends_the_extra_messages_after_all_notes_off() {
    use sm_core::osc::OscMessage;
    let mut b = B::new();
    b.cue(audio(10.0));
    b.0.settings.panic_send = vec![
        PanicMessage::Osc(OscMessage {
            host: "10.0.0.9".into(),
            port: 7000,
            address: "/blackout".into(),
            args: vec![],
        }),
        PanicMessage::Midi {
            port: "desk".into(),
            message: MidiMessage::ProgramChange {
                channel: 1,
                program: 99,
            },
        },
    ];
    let mut e = b.engine();
    e.drain_actions();
    e.command(Command::Panic).unwrap();
    let a = e.drain_actions();
    let pos = |f: &dyn Fn(&Action) -> bool| a.iter().position(f);
    let notes_off = pos(&|a| matches!(a, Action::MidiPanic)).unwrap();
    let osc =
        pos(&|a| matches!(a, Action::Osc { target, .. } if target == "10.0.0.9:7000")).unwrap();
    let midi = pos(&|a| matches!(a, Action::Midi { port, bytes, .. } if port == "desk" && bytes == &vec![0xC0, 99])).unwrap();
    assert!(notes_off < osc && osc < midi);
}

#[test]
fn http_cue_sends_its_request() {
    use sm_core::http_cue::HttpRequest;
    let mut b = B::new();
    b.cue(CueKind::Http(HttpRequest {
        method: "POST".into(),
        url: "http://10.0.0.7:8080/scene".into(),
        body: "3".into(),
        content_type: "text/plain".into(),
    }));
    let bad = b.cue(CueKind::Http(HttpRequest {
        url: "https://secure/".into(),
        ..HttpRequest::default()
    }));
    let mut e = b.engine();
    e.drain_actions();
    e.command(Command::Go { list: None }).unwrap();
    let sent: Vec<_> = e
        .drain_actions()
        .into_iter()
        .filter_map(|a| match a {
            Action::Http { addr, bytes, .. } => Some((addr, bytes)),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, "10.0.0.7:8080");
    assert!(sent[0].1.starts_with(b"POST /scene HTTP/1.1\r\n"));
    e.command(Command::Go { list: None }).unwrap();
    assert!(e
        .drain_events()
        .iter()
        .any(|ev| matches!(&ev.kind, EventKind::CueError { cue, .. } if *cue == bad)));
}

#[test]
fn gain_and_routing_edits_reach_a_running_cue() {
    let mut b = B::new();
    let c = b.cue(audio(10.0));
    let mut e = b.engine();
    e.drain_actions();
    e.command(Command::Go { list: None }).unwrap();
    e.advance(48_000);
    e.drain_actions();
    // Gain knob while it plays.
    let mut show = e.show().clone();
    if let CueKind::Audio(a) = &mut show.cue_mut(c).unwrap().kind {
        a.gain_db = -6.0;
    }
    e.set_show(show.clone());
    let fades: Vec<_> = e
        .drain_actions()
        .into_iter()
        .filter_map(|a| match a {
            Action::FadeVoice {
                master,
                crosspoints,
                ..
            } => Some((master, crosspoints)),
            _ => None,
        })
        .collect();
    assert_eq!(fades.len(), 1);
    assert!((fades[0].0.unwrap() - 0.501).abs() < 0.01);
    // Routing: send the left channel to output 2 instead; the old crosspoint fades out.
    if let CueKind::Audio(a) = &mut show.cue_mut(c).unwrap().kind {
        a.routing = vec![Crosspoint {
            src: 0,
            out: 1,
            db: 0.0,
        }];
    }
    e.set_show(show.clone());
    let fade = e
        .drain_actions()
        .into_iter()
        .find_map(|a| match a {
            Action::FadeVoice { crosspoints, .. } => Some(crosspoints),
            _ => None,
        })
        .expect("routing change sent");
    assert!(fade.iter().any(|&(s, d, g)| s == 0 && d == 1 && g > 0.9));
    assert!(
        fade.iter().any(|&(_, d, g)| d == 0 && g == 0.0),
        "old crosspoint silenced: {fade:?}"
    );
    // Nothing changed: nothing sent.
    e.set_show(show);
    assert!(!e
        .drain_actions()
        .iter()
        .any(|a| matches!(a, Action::FadeVoice { .. })));
}
