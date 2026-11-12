//! Vamp end to end: a 44.1 kHz file through the cue engine, the I/O hub and
//! the real mixer at 48 kHz, rendered offline. The file is a 440 Hz sine and
//! the loop is a whole number of cycles, so loop, release and the rest of
//! the file must come out as one unbroken sine - any glitch at a loop end or
//! at the release splice shows as an error against the ideal wave.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sm_core::engine::{Command, Engine};
use sm_core::model::*;
use sm_io::audio::AudioSystem;
use sm_io::midi::NoMidi;
use sm_io::Io;

const SR: u32 = 48_000;
const FILE_SR: u32 = 44_100;
const AMP: f64 = 0.5;
const F: f64 = 440.0;

fn write_sine(path: &std::path::Path, frames: u32) {
    let mut out = Vec::new();
    let data_len = frames * 2;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&FILE_SR.to_le_bytes());
    out.extend_from_slice(&(FILE_SR * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        let v = AMP * (2.0 * std::f64::consts::PI * F * f64::from(i) / f64::from(FILE_SR)).sin();
        out.extend_from_slice(&((v * 32767.0).round() as i16).to_le_bytes());
    }
    std::fs::write(path, out).unwrap();
}

#[test]
fn vamp_loops_and_releases_without_a_glitch() {
    let dir = tempfile::tempdir().unwrap();
    write_sine(&dir.path().join("s.wav"), 2 * FILE_SR); // 2 s
    let mut show = Show::default();
    show.settings.audio.channels = 1;
    show.settings.go_debounce_ms = 0;
    show.patch.outputs = vec![OutputDef {
        name: "1".into(),
        channel: 0,
    }];
    let id = show.alloc_id();
    show.cue_lists[0].cues = vec![Cue {
        id,
        kind: CueKind::Audio(AudioCue {
            file: "s.wav".into(),
            file_duration: Some(2.0),
            file_rate: Some(FILE_SR),
            channels: 1,
            routing: vec![Crosspoint {
                src: 0,
                out: 0,
                db: 0.0,
            }],
            // 220 whole cycles: the loop itself is seamless.
            vamp: Some(Vamp {
                start: 1.0,
                end: 1.5,
                timecode: true,
            }),
            ..AudioCue::default()
        }),
        ..Cue::default()
    }];

    let (audio, mut renderer) = AudioSystem::offline(&show.settings.audio);
    let mut io = Io::with_audio(
        &show,
        Some(dir.path().to_path_buf()),
        Box::new(NoMidi),
        audio,
    );
    let mut engine = Engine::new(show, SR);
    const BLOCK: usize = 480;
    const LOOKAHEAD: u64 = 1024;
    let mut out: Vec<f32> = Vec::new();
    let mut block = vec![0.0f32; BLOCK];
    let mut step = |engine: &mut Engine, io: &mut Io, out: &mut Vec<f32>| {
        let now = io.now() + LOOKAHEAD;
        engine.advance(now);
        let failed = io.apply(engine.drain_actions());
        assert!(failed.is_empty(), "voices failed: {failed:?}");
        let (failed, _) = io.tick(engine.now());
        assert!(failed.is_empty());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !io.all_primed() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        // An endless loop never sits in the ring completely: give the disk
        // streamer time, as a real device would (here 5x real time).
        std::thread::sleep(std::time::Duration::from_millis(2));
        renderer.render(&mut block, 1);
        out.extend_from_slice(&block);
    };
    for _ in 0..5 {
        step(&mut engine, &mut io, &mut out);
    }
    engine.command(Command::Go { list: None }).unwrap();
    let go = engine.now() as usize;
    // Intro 1.5 s + four loop passes, then release mid-pass.
    while out.len() < go + 3_700 * SR as usize / 1000 {
        step(&mut engine, &mut io, &mut out);
    }
    engine.command(Command::ReleaseLoop { cue: id }).unwrap();
    // Next loop end far enough ahead: 4.5 s; then 0.5 s of file remain.
    while out.len() < go + 5_300 * SR as usize / 1000 {
        step(&mut engine, &mut io, &mut out);
    }
    assert_eq!(io.status().underruns, 0);

    let end = go + 5 * SR as usize;
    let mut worst = (0.0f64, 0usize);
    for (n, x) in out.iter().enumerate().take(end - 64).skip(go + 64) {
        let t = (n - go) as f64 / f64::from(SR);
        let ideal = AMP * (2.0 * std::f64::consts::PI * F * t).sin();
        let err = (f64::from(*x) - ideal).abs();
        if err > worst.0 {
            worst = (err, n - go);
        }
    }
    eprintln!("worst error {:.6} at {} frames", worst.0, worst.1);
    // 16-bit file + resampler: well below -60 dB of the signal everywhere,
    // loop ends and the release splice included.
    assert!(
        worst.0 < AMP * 1e-3,
        "worst error {:.5} at {} ms",
        worst.0,
        worst.1 as f64 * 1000.0 / f64::from(SR)
    );
    // It ended with the file: 1.5 s intro + 3 passes + 0.5 s rest = 5 s.
    assert!(
        out[end + 2_000..].iter().all(|x| x.abs() < 1e-6),
        "silent after the end"
    );
    assert!(
        out[end - 2_000..end - 1_000].iter().any(|x| x.abs() > 0.3),
        "still playing just before"
    );
}
