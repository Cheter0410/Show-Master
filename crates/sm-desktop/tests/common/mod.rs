//! Test harness: a real runtime (engine, null audio output, no MIDI) plus the
//! desktop `App`, driven headless by egui_kittest. No display needed.

#![allow(dead_code)]

use egui_kittest::Harness;
use sm_core::model::Show;
use sm_desktop::client::Client;
use sm_desktop::App;
use sm_server::runtime::{self, Config, Handle};
use std::time::{Duration, Instant};

pub struct Env {
    pub rt: tokio::runtime::Runtime,
    pub handle: Handle,
    pub dir: tempfile::TempDir,
}

impl Env {
    /// Start a runtime with `show` (audio forced to the silent null output).
    pub fn start(mut show: Show) -> Env {
        show.settings.audio.backend = Some("null".into());
        let dir = tempfile::tempdir().expect("tempdir");
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime");
        let (handle, _thread) = runtime::spawn(Config {
            show: Some(show),
            midi_backend: Some(Box::new(sm_io::midi::NoMidi)),
            ..Config::default()
        });
        Env { rt, handle, dir }
    }

    /// The demo show (generated audio files, every cue type) in a temp folder.
    pub fn demo() -> Env {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = demo_show(dir.path());
        let mut show = sm_core::showfile::load(&path).expect("demo show");
        show.settings.audio.backend = Some("null".into());
        sm_core::showfile::save(&show, &path).expect("save");
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime");
        let (handle, _thread) = runtime::spawn(Config {
            show_path: Some(path),
            midi_backend: Some(Box::new(sm_io::midi::NoMidi)),
            ..Config::default()
        });
        Env { rt, handle, dir }
    }

    pub fn harness(&self) -> Harness<'static, App> {
        self.harness_sized(egui::vec2(1440.0, 900.0))
    }

    pub fn harness_sized(&self, size: egui::Vec2) -> Harness<'static, App> {
        let client = Client::new(self.handle.clone(), self.rt.handle().clone());
        let app = App::new(&egui::Context::default(), client, None);
        let mut h = Harness::builder().with_size(size).build_ui_state(
            |ui, app: &mut App| {
                app.logic(ui.ctx());
                app.ui_root(ui);
            },
            app,
        );
        settle(&mut h);
        h
    }

    /// The runtime's published state document.
    pub fn state(&self) -> serde_json::Value {
        self.handle.state()
    }

    /// Wait (stepping the UI) until `pred` holds for the published state.
    pub fn wait(
        &self,
        h: &mut Harness<'static, App>,
        what: &str,
        pred: impl Fn(&serde_json::Value) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !pred(&self.state()) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for: {what}\nstate: {}",
                self.state()
            );
            h.step();
            std::thread::sleep(Duration::from_millis(10));
        }
        settle(h);
    }
}

/// Let command replies and state updates arrive and be drawn.
pub fn settle(h: &mut Harness<'static, App>) {
    for _ in 0..8 {
        h.step();
        std::thread::sleep(Duration::from_millis(15));
    }
}

/// Write the demo show; same content as `showmaster --make-demo`.
fn demo_show(dir: &std::path::Path) -> std::path::PathBuf {
    // The demo generator lives in the binary crate; recreate a compact
    // equivalent here: three audio files and cues of every type.
    use sm_core::model::*;
    let media = dir.join("media");
    std::fs::create_dir_all(&media).expect("media dir");
    for (name, secs) in [("a.wav", 2.0f32), ("b.wav", 1.0), ("c.wav", 3.0)] {
        write_wav(&media.join(name), secs);
    }
    let mut show = Show {
        name: "Test Show".into(),
        ..Show::default()
    };
    let mk = |show: &mut Show, number: &str, name: &str, kind: CueKind| {
        let id = show.alloc_id();
        Cue {
            id,
            number: number.into(),
            name: name.into(),
            kind,
            ..Cue::default()
        }
    };
    let audio = |f: &str, d: f64| {
        CueKind::Audio(AudioCue {
            file: format!("media/{f}"),
            file_duration: Some(d),
            channels: 2,
            ..AudioCue::default()
        })
    };
    let c1 = mk(&mut show, "1", "Intro", audio("a.wav", 2.0));
    let target = c1.id;
    let c2 = mk(
        &mut show,
        "2",
        "Fade intro",
        CueKind::Fade(FadeCue {
            target: Some(target),
            duration: 1.0,
            ..FadeCue::default()
        }),
    );
    let child1 = mk(&mut show, "", "Bed", audio("b.wav", 1.0));
    let child2 = mk(
        &mut show,
        "",
        "Desk note",
        CueKind::Midi(MidiCue::default()),
    );
    let g = mk(
        &mut show,
        "3",
        "Scene",
        CueKind::Group(GroupCue {
            mode: GroupMode::Parallel,
            children: vec![child1, child2],
        }),
    );
    let c4 = mk(&mut show, "4", "Long", audio("c.wav", 3.0));
    let c5 = mk(
        &mut show,
        "5",
        "Start TC",
        CueKind::Timecode(TimecodeCue::default()),
    );
    let c6 = mk(
        &mut show,
        "6",
        "Stop Long",
        CueKind::Control(ControlCue {
            action: ControlAction::Stop,
            target: Some(c4.id),
            time: 0.5,
        }),
    );
    let c7 = mk(&mut show, "7", "Note", CueKind::Memo);
    show.cue_lists[0].cues = vec![c1, c2, g, c4, c5, c6, c7];
    show.settings.go_debounce_ms = 0;
    let path = dir.join("test.smshow");
    sm_core::showfile::save(&show, &path).expect("save show");
    path
}

fn write_wav(path: &std::path::Path, secs: f32) {
    let frames = (48_000.0 * secs) as u32;
    let mut out = Vec::new();
    let data_len = frames * 4;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&48_000u32.to_le_bytes());
    out.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        let s = ((i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 8000.0) as i16;
        out.extend_from_slice(&s.to_le_bytes());
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, out).expect("write wav");
}
