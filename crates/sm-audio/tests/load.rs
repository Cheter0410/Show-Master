//! Load tests: many streamed voices into a wide output, offline and in
//! real time through the null backend, plus backend smoke tests.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::{Duration, Instant};

use common::{noise_i16, wait_buffered, wait_primed, write_wav_i16, TempDir};
use sm_audio::{
    default_backends, engine, AudioBackend, AudioEvent, EngineConfig, FadeCurve, FileSource,
    FileSourceOpts, MemorySource, NullBackend, StreamRequest, VoiceParams,
};

fn stereo_route(i: usize, outs: usize) -> VoiceParams {
    VoiceParams {
        gain: 0.02,
        matrix: vec![
            (0, ((2 * i) % outs) as u16, 1.0),
            (1, ((2 * i + 1) % outs) as u16, 1.0),
        ],
    }
}

/// Test 8a: 48 stereo voices (24 looped files, 24 looped memory buffers)
/// into 64 outputs, 30 s rendered offline in 512-frame blocks.
#[test]
fn offline_48_stereo_voices_to_64_outputs_30s() {
    const RATE: u32 = 48_000;
    const OUTS: usize = 64;
    const VOICES: usize = 48;
    const SECONDS: usize = 30;
    const BLOCK: usize = 512;

    let dir = TempDir::new("load-offline");
    let (mut ctl, mut r) = engine(EngineConfig::new(RATE, OUTS));
    let mut voices = Vec::new();
    for i in 0..VOICES {
        let v = if i % 2 == 0 {
            // 5 s stereo file, looped 6 times = 30 s
            let p = dir.path(&format!("v{i}.wav"));
            write_wav_i16(&p, RATE, 2, &noise_i16(2 * 5 * RATE as usize, i as u32 + 1));
            let mut o = FileSourceOpts::new(RATE);
            o.loop_count = 6;
            let src = FileSource::open(&p, o).unwrap();
            ctl.prepare_voice(Box::new(src), stereo_route(i, OUTS))
                .unwrap()
        } else {
            let data: Vec<f32> = noise_i16(2 * RATE as usize, i as u32 + 1)
                .iter()
                .map(|s| *s as f32 / 32768.0)
                .collect();
            let src = MemorySource::new(data, 2).with_loops(SECONDS as u32);
            ctl.prepare_voice(Box::new(src), stereo_route(i, OUTS))
                .unwrap()
        };
        voices.push(v);
    }
    wait_primed(&ctl, &voices);
    for v in &voices {
        ctl.start(*v, 1000, 0, FadeCurve::Linear).unwrap();
    }

    let total = SECONDS * RATE as usize + 1000;
    let mut buf = vec![0.0f32; BLOCK * OUTS];
    let mut done = 0;
    let mut render_time = Duration::ZERO;
    let mut worst = Duration::ZERO;
    let mut blocks = 0u32;
    let mut peak = 0.0f32;
    let mut ended = 0;
    while done < total {
        let n = BLOCK.min(total - done);
        wait_buffered(&ctl, n as u64);
        let t0 = Instant::now();
        r.render(&mut buf[..n * OUTS], OUTS);
        let dt = t0.elapsed();
        render_time += dt;
        worst = worst.max(dt);
        blocks += 1;
        done += n;
        if blocks % 64 == 0 {
            peak = buf.iter().fold(peak, |m, s| m.max(s.abs()));
            assert_eq!(
                ctl.active_voices(),
                VOICES,
                "all voices playing at frame {done}"
            );
            ended += ctl.poll_events().len();
        }
    }
    r.render(&mut buf, OUTS);
    ended += ctl
        .poll_events()
        .iter()
        .filter(|e| matches!(e, AudioEvent::VoiceEnded { .. }))
        .count();

    let avg = render_time / blocks;
    let block_dur = Duration::from_secs_f64(BLOCK as f64 / RATE as f64);
    println!(
        "offline load: {VOICES} stereo voices -> {OUTS} outs, {SECONDS} s, {blocks} blocks of {BLOCK}: \
         avg {:.1} us/block ({:.1} ns/frame, {:.2}% of real time), worst {:.1} us",
        avg.as_secs_f64() * 1e6,
        avg.as_secs_f64() * 1e9 / BLOCK as f64,
        100.0 * avg.as_secs_f64() / block_dur.as_secs_f64(),
        worst.as_secs_f64() * 1e6,
    );
    assert_eq!(
        ctl.underruns(),
        0,
        "no underruns when the streamer keeps up"
    );
    assert!(peak > 0.001, "there was audio");
    assert_eq!(ended, VOICES, "every voice played its 30 s and ended");
    assert_eq!(ctl.active_voices(), 0);
}

/// Test 8b: 32 stereo file streams in real time through the null backend.
#[test]
fn realtime_null_backend_32_file_streams() {
    const RATE: u32 = 48_000;
    const OUTS: usize = 16;
    const VOICES: usize = 32;

    let dir = TempDir::new("load-rt");
    let (mut ctl, renderer) = engine(EngineConfig::new(RATE, OUTS));
    let mut voices = Vec::new();
    for i in 0..VOICES {
        let p = dir.path(&format!("v{i}.wav"));
        write_wav_i16(&p, RATE, 2, &noise_i16(2 * RATE as usize, i as u32 + 100));
        let mut o = FileSourceOpts::new(RATE);
        o.loop_count = 0; // endless
        let src = FileSource::open(&p, o).unwrap();
        voices.push(
            ctl.prepare_voice(Box::new(src), stereo_route(i, OUTS))
                .unwrap(),
        );
    }
    wait_primed(&ctl, &voices);

    let backend = NullBackend::new();
    let stream = backend
        .open(
            StreamRequest {
                device_id: None,
                sample_rate: RATE,
                channels: OUTS,
                buffer_frames: Some(256),
            },
            renderer,
        )
        .unwrap();
    let info = stream.info();
    assert_eq!(info.channels, OUTS);
    assert_eq!(info.buffer_frames, Some(256));

    let t0 = ctl.clock();
    let start_at = t0 + 4800;
    for v in &voices {
        ctl.start(*v, start_at, 0, FadeCurve::Linear).unwrap();
    }
    let began = Instant::now();
    let mut max_load = 0.0f32;
    while began.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(50));
        max_load = max_load.max(stream.cpu_load());
        assert!(
            ctl.poll_events().is_empty(),
            "no events expected while playing"
        );
    }
    let played = ctl.clock() - t0;
    assert!(
        (played as i64 - 3 * RATE as i64).abs() < RATE as i64 / 4,
        "null backend runs at real-time pace: {played} frames in 3 s"
    );
    assert_eq!(ctl.active_voices(), VOICES);
    // all voices are in lock step: same position
    let positions: Vec<u64> = voices
        .iter()
        .map(|v| ctl.voice_position(*v).unwrap())
        .collect();
    let spread = positions.iter().max().unwrap() - positions.iter().min().unwrap();
    assert!(spread <= 256, "positions {positions:?}");
    assert_eq!(ctl.underruns(), 0, "zero underruns");
    assert!(ctl.meters().peak.iter().all(|p| *p > 0.0));
    assert!(stream.error().is_none());

    ctl.stop_all(0, 2400, false).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut ended = 0;
    while ended < VOICES {
        assert!(Instant::now() < deadline, "voices did not end");
        ended += ctl.poll_events().len();
        std::thread::sleep(Duration::from_millis(5));
    }
    println!(
        "realtime null backend: {VOICES} stereo file streams -> {OUTS} outs, buffer 256: \
         cpu load now {:.2}%, max {:.2}%, underruns {}",
        stream.cpu_load() * 100.0,
        max_load * 100.0,
        ctl.underruns()
    );
    drop(stream); // joins the audio thread
    assert_eq!(ctl.underruns(), 0);
}

#[test]
fn backends_enumerate_without_hardware() {
    let backends = default_backends();
    assert!(!backends.is_empty());
    let null = backends.last().unwrap();
    assert_eq!(null.name(), "Null");
    let devs = null.devices();
    assert_eq!(devs.len(), 1);
    assert_eq!(devs[0].name, "Null output (no sound)");
    assert_eq!(devs[0].max_output_channels, 64);
    assert!(devs[0].is_default);
    for b in &backends {
        // must not panic or hang without a sound card
        let devices = b.devices();
        println!("backend {}: {} device(s)", b.name(), devices.len());
    }

    // request validation
    let (_ctl, renderer) = engine(EngineConfig::new(48_000, 2));
    let err = NullBackend::new().open(
        StreamRequest {
            device_id: None,
            sample_rate: 44_100,
            channels: 2,
            buffer_frames: None,
        },
        renderer,
    );
    assert!(err.is_err(), "rate mismatch is rejected");
    let (_ctl, renderer) = engine(EngineConfig::new(48_000, 2));
    let err = NullBackend::new().open(
        StreamRequest {
            device_id: Some("nope".into()),
            sample_rate: 48_000,
            channels: 2,
            buffer_frames: None,
        },
        renderer,
    );
    assert!(err.is_err(), "unknown device is rejected");
}
