//! System-level timing proof: cue engine -> I/O hub -> real mixer, rendered
//! offline. Verifies on the actual output samples that
//!   * cues fired by one GO start on the same sample,
//!   * an auto-follow cue starts exactly when its predecessor ends,
//!   * pre-waits are exact to the sample,
//!   * LTC on a spare channel is decodable and locked to the same clock.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sm_core::engine::{Command, Engine};
use sm_core::model::*;
use sm_io::audio::AudioSystem;
use sm_io::midi::NoMidi;
use sm_io::Io;
use sm_timecode::{LtcDecoder, Timecode};

const SR: u32 = 48_000;

/// Mono 16-bit WAV: full-scale impulse at frame 0, then `value` until the end.
fn write_wav(path: &std::path::Path, frames: u32, value: i16) {
    let mut out = Vec::new();
    let data_len = frames * 2;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&SR.to_le_bytes());
    out.extend_from_slice(&(SR * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        let s: i16 = if i == 0 { 32_000 } else { value };
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, out).unwrap();
}

fn audio_cue(show: &mut Show, file: &str, frames: u32, out: u16) -> Cue {
    let a = AudioCue {
        file: file.into(),
        file_duration: Some(f64::from(frames) / f64::from(SR)),
        channels: 1,
        routing: vec![Crosspoint {
            src: 0,
            out,
            db: 0.0,
        }],
        ..AudioCue::default()
    };
    Cue {
        id: show.alloc_id(),
        kind: CueKind::Audio(a),
        ..Cue::default()
    }
}

/// First frame on `channel` whose absolute value exceeds 0.9 (the impulse).
fn impulse(out: &[f32], channels: usize, channel: usize) -> Option<usize> {
    out.chunks_exact(channels)
        .position(|f| f[channel].abs() > 0.9)
}

#[test]
fn engine_to_speaker_is_sample_accurate() {
    let dir = tempfile::tempdir().unwrap();
    const LEN_A: u32 = 24_000; // 0.5 s
    write_wav(&dir.path().join("a.wav"), LEN_A, 3_000);
    write_wav(&dir.path().join("b.wav"), 12_000, 3_000);
    write_wav(&dir.path().join("c.wav"), 12_000, 3_000);
    write_wav(&dir.path().join("d.wav"), 12_000, 3_000);

    let mut show = Show::default();
    show.settings.audio.channels = 6;
    show.settings.go_debounce_ms = 0;
    show.patch.outputs = (0..6)
        .map(|c| OutputDef {
            name: format!("{c}"),
            channel: c,
        })
        .collect();
    show.timecode.ltc_enabled = true;
    show.timecode.ltc_channel = 5;
    show.timecode.ltc_level_db = -6.0;
    show.timecode.start = Timecode::new(10, 0, 0, 0);
    // Cue 1: parallel group of a (out 0) and b (out 1), auto-follow ->
    // Cue 2: c on out 2 with 0.25 s pre-wait, auto-continue (post-wait 0.1 s) ->
    // Cue 3: d on out 3.
    let a = audio_cue(&mut show, "a.wav", LEN_A, 0);
    let b = audio_cue(&mut show, "b.wav", 12_000, 1);
    let group = Cue {
        id: show.alloc_id(),
        continue_mode: ContinueMode::AutoFollow,
        kind: CueKind::Group(GroupCue {
            mode: GroupMode::Parallel,
            children: vec![a, b],
        }),
        ..Cue::default()
    };
    let mut c = audio_cue(&mut show, "c.wav", 12_000, 2);
    c.pre_wait = 0.25;
    c.post_wait = 0.1;
    c.continue_mode = ContinueMode::AutoContinue;
    let d = audio_cue(&mut show, "d.wav", 12_000, 3);
    show.cue_lists[0].cues = vec![group, c, d];

    let (audio, mut renderer) = AudioSystem::offline(&show.settings.audio);
    let mut io = Io::with_audio(
        &show,
        Some(dir.path().to_path_buf()),
        Box::new(NoMidi),
        audio,
    );
    let mut engine = Engine::new(show, SR);

    const CH: usize = 6;
    const BLOCK: usize = 480; // deliberately not a divisor of the cue lengths
    const LOOKAHEAD: u64 = 1024;
    let mut out: Vec<f32> = Vec::new();
    let mut block = vec![0.0f32; BLOCK * CH];
    let mut step = |engine: &mut Engine, io: &mut Io, out: &mut Vec<f32>| {
        let now = io.now() + LOOKAHEAD;
        engine.advance(now);
        for (voice, msg) in io.apply(engine.drain_actions()) {
            panic!("voice {voice} failed: {msg}");
        }
        let (failed, _log) = io.tick(engine.now());
        assert!(failed.is_empty());
        // Offline: wait for the disk streamer instead of racing it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !io.all_primed() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        renderer.render(&mut block, CH);
        out.extend_from_slice(&block);
    };

    for _ in 0..5 {
        step(&mut engine, &mut io, &mut out);
    }
    engine.command(Command::TcStart { from: None }).unwrap();
    let tc_anchor = engine.now() as usize;
    engine.command(Command::Go { list: None }).unwrap();
    let go = engine.now() as usize;
    while out.len() / CH < go + 2 * SR as usize {
        step(&mut engine, &mut io, &mut out);
    }
    assert_eq!(io.status().underruns, 0);

    let at = |ch| impulse(&out, CH, ch).unwrap_or_else(|| panic!("no impulse on channel {ch}"));
    assert_eq!(at(0), go, "first cue starts exactly at the GO frame");
    assert_eq!(
        at(1),
        at(0),
        "cues fired by one GO start on the same sample"
    );
    let follow = go + LEN_A as usize; // group ends when its longest child ends
    assert_eq!(
        at(2),
        follow + 12_000,
        "auto-follow + 0.25 s pre-wait, exact"
    );
    assert_eq!(
        at(3),
        follow + 12_000 + 4_800,
        "auto-continue after 0.1 s post-wait, exact"
    );
    // The sample right after cue a's last frame is silent again: it ended on time.
    assert!(out[(go + LEN_A as usize - 1) * CH].abs() > 0.05);
    assert_eq!(out[(go + LEN_A as usize + 400) * CH], 0.0);

    // LTC on channel 5 decodes, and frame k ends exactly (k+1) * 1920 samples after the anchor.
    let ltc: Vec<f32> = out.chunks_exact(CH).map(|f| f[5]).collect();
    assert!(
        ltc[..tc_anchor].iter().all(|s| *s == 0.0),
        "silent before the generator starts"
    );
    let mut decoder = LtcDecoder::new(SR);
    let mut frames = Vec::new();
    decoder.push(&ltc, &mut frames);
    assert!(frames.len() > 40, "decoded {} LTC frames", frames.len());
    assert_eq!(frames[0].timecode, Timecode::new(10, 0, 0, 0));
    for (k, f) in frames.iter().enumerate() {
        assert_eq!(
            f.timecode.to_frame_number(sm_timecode::FrameRate::F25),
            900_000 + k as u64
        );
        assert_eq!(
            f.end_sample as usize,
            tc_anchor + (k + 1) * 1920,
            "LTC frame {k} is sample-locked"
        );
    }
}
