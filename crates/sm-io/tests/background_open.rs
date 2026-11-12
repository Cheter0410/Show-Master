//! Audio files are opened off the engine thread: commands that arrive while
//! a file is opening are kept and replayed; stop/discard cancel the open;
//! broken files are reported through `tick`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sm_core::engine::{Action, VoiceSpec};
use sm_core::model::{AudioSettings, Curve, Show};
use sm_io::audio::AudioSystem;
use sm_io::midi::NoMidi;
use sm_io::Io;
use std::time::{Duration, Instant};

fn wav(path: &std::path::Path, frames: u32) {
    let mut out = Vec::new();
    let data_len = frames * 2;
    for (tag, v) in [(b"RIFF", 36 + data_len)] {
        out.extend_from_slice(tag);
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&48_000u32.to_le_bytes());
    out.extend_from_slice(&96_000u32.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for _ in 0..frames {
        out.extend_from_slice(&20_000i16.to_le_bytes());
    }
    std::fs::write(path, out).unwrap();
}

fn spec(file: &str) -> VoiceSpec {
    VoiceSpec {
        file: file.into(),
        start: 0.0,
        end: None,
        loop_count: 1,
        offset_frames: 0,
        gain: 1.0,
        matrix: vec![(0, 0, 1.0)],
    }
}

fn setup() -> (tempfile::TempDir, Io, sm_audio::Renderer) {
    let dir = tempfile::tempdir().unwrap();
    wav(&dir.path().join("a.wav"), 48_000);
    let settings = AudioSettings {
        channels: 2,
        ..AudioSettings::default()
    };
    let (audio, renderer) = AudioSystem::offline(&settings);
    let io = Io::with_audio(
        &Show::default(),
        Some(dir.path().to_path_buf()),
        Box::new(NoMidi),
        audio,
    );
    (dir, io, renderer)
}

fn wait_opened(io: &mut Io) -> Vec<(u64, String)> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut failed = Vec::new();
    while io.opening_count() > 0 {
        assert!(Instant::now() < deadline, "files never finished opening");
        failed.extend(io.tick(0).0);
        std::thread::sleep(Duration::from_millis(2));
    }
    failed.extend(io.tick(0).0);
    failed
}

#[test]
fn apply_returns_at_once_and_queued_start_plays() {
    let (_dir, mut io, mut renderer) = setup();
    let t = Instant::now();
    io.apply(vec![
        Action::PrepareVoice {
            voice: 1,
            cue: 1,
            spec: spec("a.wav"),
        },
        Action::StartVoice {
            voice: 1,
            at: 256,
            fade_in_frames: 0,
            curve: Curve::Linear,
        },
    ]);
    assert!(
        t.elapsed() < Duration::from_millis(50),
        "apply must not do disk I/O"
    );
    assert_eq!(io.opening_count(), 1);
    assert!(wait_opened(&mut io).is_empty());
    while !io.all_primed() {
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut out = vec![0.0f32; 1024 * 2];
    renderer.render(&mut out, 2);
    // Silent before frame 256, the file's level from frame 256 on: the queued start kept its time.
    assert_eq!(out[255 * 2], 0.0);
    assert!(
        out[256 * 2] > 0.5,
        "voice started at its frame: {}",
        out[256 * 2]
    );
}

#[test]
fn stop_before_the_file_is_open_cancels_it() {
    let (_dir, mut io, mut renderer) = setup();
    io.apply(vec![
        Action::PrepareVoice {
            voice: 7,
            cue: 1,
            spec: spec("a.wav"),
        },
        Action::StartVoice {
            voice: 7,
            at: 0,
            fade_in_frames: 0,
            curve: Curve::Linear,
        },
        Action::StopVoice {
            voice: 7,
            at: 0,
            fade_frames: 0,
            curve: Curve::Linear,
            hard: true,
        },
    ]);
    assert!(wait_opened(&mut io).is_empty());
    let mut out = vec![0.0f32; 512 * 2];
    renderer.render(&mut out, 2);
    assert!(
        out.iter().all(|s| *s == 0.0),
        "cancelled voice must stay silent"
    );
    assert_eq!(io.status().voices, 0);
}

#[test]
fn missing_file_is_reported_through_tick() {
    let (_dir, mut io, _renderer) = setup();
    io.apply(vec![Action::PrepareVoice {
        voice: 3,
        cue: 1,
        spec: spec("nope.wav"),
    }]);
    let failed = wait_opened(&mut io);
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, 3);
    assert!(failed[0].1.contains("nope.wav"), "{}", failed[0].1);
}
