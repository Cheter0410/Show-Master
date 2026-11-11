//! Offline tests of the real-time renderer and the controller.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{channel, render, wait_primed};
use sm_audio::{
    engine, engine_with_generators, AudioError, AudioEvent, EngineConfig, FadeCurve, MemorySource,
    RtGenerator, SampleSource, VoiceParams,
};

const ODD_BLOCKS: &[usize] = &[127, 333, 64, 1, 511, 7];

fn mono(v: Vec<f32>) -> Box<MemorySource> {
    Box::new(MemorySource::new(v, 1))
}

fn route(matrix: &[(u16, u16, f32)]) -> VoiceParams {
    VoiceParams {
        gain: 1.0,
        matrix: matrix.to_vec(),
    }
}

fn dc(frames: usize) -> Box<MemorySource> {
    mono(vec![1.0; frames])
}

// ---------------------------------------------------------------- test 1

#[test]
fn simultaneous_start_is_sample_accurate() {
    let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 2));
    let a: Vec<f32> = (0..1000).map(|i| (i + 1) as f32 / 1000.0).collect();
    let mut b = vec![0.0f32; 1000];
    b[0] = 1.0;
    b[1] = -0.5;
    b[999] = 0.25;
    let c: Vec<f32> = (0..500).map(|i| 0.5 - i as f32 / 1000.0).collect();

    let va = ctl
        .prepare_voice(mono(a.clone()), route(&[(0, 0, 1.0)]))
        .unwrap();
    let vb = ctl
        .prepare_voice(mono(b.clone()), route(&[(0, 1, 1.0)]))
        .unwrap();
    let vc = ctl
        .prepare_voice(mono(c.clone()), route(&[(0, 0, 1.0)]))
        .unwrap();
    wait_primed(&ctl, &[va, vb, vc]);

    const T: u64 = 1000; // inside the 4th block of ODD_BLOCKS (525..526)... mid-block
    ctl.start(va, T, 0, FadeCurve::Linear).unwrap();
    ctl.start(vb, T, 0, FadeCurve::Linear).unwrap();
    ctl.start(vc, T + 12_345, 0, FadeCurve::Linear).unwrap();

    let out = render(&ctl, &mut r, 2, 16_000, ODD_BLOCKS);
    let l = channel(&out, 2, 0);
    let rch = channel(&out, 2, 1);
    let t = T as usize;
    assert!(l[..t].iter().all(|s| *s == 0.0), "left is silent before T");
    assert!(
        rch[..t].iter().all(|s| *s == 0.0),
        "right is silent before T"
    );
    assert_eq!(&l[t..t + 1000], &a[..], "voice A starts exactly at T");
    assert_eq!(&rch[t..t + 1000], &b[..], "voice B starts exactly at T");
    // offset between the two voices is 0 samples
    let first_l = l.iter().position(|s| *s != 0.0).unwrap();
    let first_r = rch.iter().position(|s| *s != 0.0).unwrap();
    assert_eq!(first_l, first_r);
    assert_eq!(first_l, t);

    let t2 = t + 12_345;
    assert!(l[t + 1000..t2].iter().all(|s| *s == 0.0));
    assert_eq!(&l[t2..t2 + 500], &c[..], "third voice lands at T + 12345");
    assert!(l[t2 + 500..].iter().all(|s| *s == 0.0));
    assert_eq!(ctl.clock(), 16_000);

    let ended = ctl
        .poll_events()
        .iter()
        .filter(|e| matches!(e, AudioEvent::VoiceEnded { .. }))
        .count();
    assert_eq!(ended, 3);
    assert_eq!(ctl.active_voices(), 0);
    assert_eq!(ctl.voice_count(), 0);
    assert_eq!(ctl.underruns(), 0);
}

#[test]
fn start_is_exact_for_every_block_size() {
    for block in [1usize, 2, 3, 63, 64, 65, 100, 128, 480, 1024, 4096] {
        let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 1));
        let sig: Vec<f32> = (0..300).map(|i| 0.1 + i as f32 * 0.001).collect();
        let v = ctl
            .prepare_voice(mono(sig.clone()), route(&[(0, 0, 1.0)]))
            .unwrap();
        wait_primed(&ctl, &[v]);
        ctl.start(v, 4097, 0, FadeCurve::Linear).unwrap();
        let out = render(&ctl, &mut r, 1, 8192, &[block]);
        assert!(out[..4097].iter().all(|s| *s == 0.0), "block {block}");
        assert_eq!(&out[4097..4397], &sig[..], "block {block}");
    }
}

#[test]
fn commands_in_the_past_apply_immediately() {
    let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 1));
    let _ = render(&ctl, &mut r, 1, 1000, &[256]);
    let v = ctl.prepare_voice(dc(100), route(&[(0, 0, 0.5)])).unwrap();
    wait_primed(&ctl, &[v]);
    ctl.start(v, 10, 0, FadeCurve::Linear).unwrap(); // long gone
    let out = render(&ctl, &mut r, 1, 256, &[256]);
    assert!(out[..100].iter().all(|s| *s == 0.5));
    assert!(out[100..].iter().all(|s| *s == 0.0));
}

// ---------------------------------------------------------------- test 2

#[test]
fn routing_mono_to_several_outputs() {
    let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 6));
    let sig: Vec<f32> = (0..256)
        .map(|i| ((i * 37) % 100) as f32 / 100.0 - 0.5)
        .collect();
    let v = ctl
        .prepare_voice(
            mono(sig.clone()),
            VoiceParams {
                gain: 0.5,
                matrix: vec![(0, 0, 1.0), (0, 2, 0.25), (0, 5, -0.5)],
            },
        )
        .unwrap();
    wait_primed(&ctl, &[v]);
    ctl.start(v, 0, 0, FadeCurve::Linear).unwrap();
    let out = render(&ctl, &mut r, 6, 256, &[100]);
    for (i, s) in sig.iter().enumerate() {
        let f = &out[i * 6..i * 6 + 6];
        assert_eq!(f[0], s * 0.5);
        assert_eq!(f[1], 0.0);
        assert_eq!(f[2], s * 0.25 * 0.5);
        assert_eq!(f[3], 0.0);
        assert_eq!(f[4], 0.0);
        assert_eq!(f[5], s * -0.5 * 0.5);
    }
}

#[test]
fn routing_stereo_swapped_and_default_identity() {
    let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 2));
    let st: Vec<f32> = (0..200)
        .flat_map(|i| [i as f32 / 400.0, -(i as f32) / 800.0])
        .collect();
    let swapped = ctl
        .prepare_voice(
            Box::new(MemorySource::new(st.clone(), 2)),
            route(&[(0, 1, 1.0), (1, 0, 1.0)]),
        )
        .unwrap();
    wait_primed(&ctl, &[swapped]);
    ctl.start(swapped, 0, 0, FadeCurve::Linear).unwrap();
    let out = render(&ctl, &mut r, 2, 200, &[33]);
    for i in 0..200 {
        assert_eq!(out[i * 2], st[i * 2 + 1]);
        assert_eq!(out[i * 2 + 1], st[i * 2]);
    }

    // empty matrix = identity
    let ident = ctl
        .prepare_voice(
            Box::new(MemorySource::new(st.clone(), 2)),
            VoiceParams::default(),
        )
        .unwrap();
    wait_primed(&ctl, &[ident]);
    ctl.start(ident, 0, 0, FadeCurve::Linear).unwrap();
    let out = render(&ctl, &mut r, 2, 200, &[33]);
    assert_eq!(out, st);
}

#[test]
fn routing_8ch_source_to_64_outputs() {
    let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 64));
    let frames = 300;
    let src: Vec<f32> = (0..frames)
        .flat_map(|i| (0..8).map(move |c| (c + 1) as f32 * 0.01 + i as f32 * 0.0001))
        .collect();
    // source channel s → output s*7+3 with gain 0.1*(s+1); plus s → 63 summed
    let mut matrix: Vec<(u16, u16, f32)> = (0..8u16)
        .map(|s| (s, s * 7 + 3, 0.1 * (s + 1) as f32))
        .collect();
    matrix.extend((0..8u16).map(|s| (s, 63, 0.125)));
    let v = ctl
        .prepare_voice(Box::new(MemorySource::new(src.clone(), 8)), route(&matrix))
        .unwrap();
    wait_primed(&ctl, &[v]);
    ctl.start(v, 5, 0, FadeCurve::Linear).unwrap();
    let out = render(&ctl, &mut r, 64, frames + 5, &[77]);
    for i in 0..frames {
        let f = &out[(i + 5) * 64..(i + 6) * 64];
        let mut expected = [0.0f32; 64];
        let mut sum = 0.0f32;
        for s in 0..8 {
            let x = src[i * 8 + s];
            expected[s * 7 + 3] = x * (0.1 * (s + 1) as f32);
            sum += x * 0.125;
        }
        for o in 0..63 {
            assert_eq!(f[o], expected[o], "frame {i} out {o}");
        }
        assert!((f[63] - sum).abs() < 1e-6);
    }
}

#[test]
fn device_channel_count_mismatch_maps_common_channels() {
    let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 2));
    let v = ctl
        .prepare_voice(dc(64), route(&[(0, 0, 0.25), (0, 1, 0.5)]))
        .unwrap();
    wait_primed(&ctl, &[v]);
    ctl.start(v, 0, 0, FadeCurve::Linear).unwrap();
    let mut wide = vec![9.0f32; 32 * 4];
    r.render(&mut wide, 4);
    for f in wide.chunks_exact(4) {
        assert_eq!(f, [0.25, 0.5, 0.0, 0.0]);
    }
    let mut narrow = vec![9.0f32; 32];
    r.render(&mut narrow, 1);
    assert!(narrow.iter().all(|s| *s == 0.25));
}

// ---------------------------------------------------------------- test 3

fn max_step(x: &[f32]) -> f32 {
    x.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn linear_fade_in_endpoints_and_monotonicity() {
    let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 1));
    let v = ctl
        .prepare_voice(dc(10_000), route(&[(0, 0, 1.0)]))
        .unwrap();
    wait_primed(&ctl, &[v]);
    ctl.start(v, 500, 1000, FadeCurve::Linear).unwrap();
    let out = render(&ctl, &mut r, 1, 3000, ODD_BLOCKS);
    assert!(
        out[..=500].iter().all(|s| *s == 0.0),
        "gain 0 at the first frame"
    );
    for k in 0..=1000 {
        let expect = k as f32 / 1000.0;
        assert!(
            (out[500 + k] - expect).abs() < 1e-5,
            "k={k}: {}",
            out[500 + k]
        );
    }
    assert!(
        out[1500..].iter().all(|s| *s == 1.0),
        "exactly unity after the fade"
    );
    assert!(out[500..1501].windows(2).all(|w| w[1] >= w[0]), "monotonic");
    assert!(max_step(&out) <= 1.0 / 1000.0 + 1e-5, "no zipper steps");
}

#[test]
fn equal_power_and_s_curve_shapes() {
    for (curve, mid) in [
        (FadeCurve::EqualPower, std::f32::consts::FRAC_1_SQRT_2),
        (FadeCurve::SCurve, 0.5),
    ] {
        let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 1));
        let v = ctl
            .prepare_voice(dc(20_000), route(&[(0, 0, 1.0)]))
            .unwrap();
        wait_primed(&ctl, &[v]);
        ctl.start(v, 0, 4800, curve).unwrap();
        ctl.stop(v, 9600, 4800, curve, false).unwrap();
        let out = render(&ctl, &mut r, 1, 16_000, ODD_BLOCKS);
        assert_eq!(out[0], 0.0);
        assert!(
            (out[2400] - mid).abs() < 2e-3,
            "{curve:?} fade-in midpoint {}",
            out[2400]
        );
        assert_eq!(out[4800], 1.0);
        assert_eq!(out[9600], 1.0);
        assert!(
            (out[12_000] - mid).abs() < 2e-3,
            "{curve:?} fade-out midpoint {}",
            out[12_000]
        );
        assert!(out[14_400..].iter().all(|s| *s == 0.0));
        assert!(out[..4801].windows(2).all(|w| w[1] >= w[0]));
        assert!(out[9600..14_401].windows(2).all(|w| w[1] <= w[0]));
        // steepest slope of both curves is (pi/2)/len
        assert!(
            max_step(&out) <= 1.6 / 4800.0,
            "{curve:?} step {}",
            max_step(&out)
        );
    }
}

#[test]
fn fade_command_on_master_and_crosspoint() {
    let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 2));
    let v = ctl
        .prepare_voice(dc(20_000), route(&[(0, 0, 1.0), (0, 1, 0.0)]))
        .unwrap();
    wait_primed(&ctl, &[v]);
    ctl.start(v, 0, 0, FadeCurve::Linear).unwrap();
    // crosspoint (0,1): 0 → 0.5 over 480 frames at 1000
    ctl.fade(v, 1000, 480, FadeCurve::Linear, None, &[(0, 1, 0.5)])
        .unwrap();
    // master: 1 → 0.25 over 960 frames at 3000
    ctl.fade(v, 3000, 960, FadeCurve::Linear, Some(0.25), &[])
        .unwrap();
    // both at once at 6000: master → 1.0, crosspoint (0,0) → 0
    ctl.fade(v, 6000, 100, FadeCurve::Linear, Some(1.0), &[(0, 0, 0.0)])
        .unwrap();
    let out = render(&ctl, &mut r, 2, 8000, ODD_BLOCKS);
    let l = channel(&out, 2, 0);
    let rr = channel(&out, 2, 1);

    assert!(rr[..=1000].iter().all(|s| *s == 0.0));
    for k in 0..=480 {
        assert!((rr[1000 + k] - 0.5 * k as f32 / 480.0).abs() < 1e-5);
    }
    assert!(rr[1480..3000].iter().all(|s| *s == 0.5));
    assert!(l[..=3000].iter().all(|s| *s == 1.0));
    for k in 0..=960 {
        let g = 1.0 - 0.75 * k as f32 / 960.0;
        assert!((l[3000 + k] - g).abs() < 1e-5);
        assert!((rr[3000 + k] - 0.5 * g).abs() < 1e-5);
    }
    assert!(l[3960..=6000].iter().all(|s| *s == 0.25));
    assert_eq!(l[6100], 0.0);
    assert!((rr[6100] - 0.5).abs() < 1e-6);
    // steepest part: d/dt (0.25 + 0.75t)(1 - t) = -1 at t = 1, over 100 frames
    assert!(max_step(&l) <= 1.0 / 100.0 + 1e-4);
    assert!(max_step(&rr) <= 0.5 * 0.75 / 100.0 + 1e-4);
}

#[test]
fn fade_with_many_crosspoints_is_split_transparently() {
    let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 40));
    let v = ctl.prepare_voice(dc(4000), route(&[(0, 0, 1.0)])).unwrap();
    wait_primed(&ctl, &[v]);
    ctl.start(v, 0, 0, FadeCurve::Linear).unwrap();
    let xps: Vec<(u16, u16, f32)> = (0..40u16).map(|o| (0, o, (o + 1) as f32 / 40.0)).collect();
    ctl.fade(v, 100, 200, FadeCurve::Linear, None, &xps)
        .unwrap();
    let out = render(&ctl, &mut r, 40, 400, &[64]);
    let last = &out[399 * 40..];
    for (o, got) in last.iter().enumerate() {
        assert!((got - (o + 1) as f32 / 40.0).abs() < 1e-6, "out {o}");
    }
}

#[test]
fn stop_with_fade_ends_voice_and_emits_event() {
    let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 1));
    let v = ctl
        .prepare_voice(dc(100_000), route(&[(0, 0, 1.0)]))
        .unwrap();
    wait_primed(&ctl, &[v]);
    ctl.start(v, 0, 0, FadeCurve::Linear).unwrap();
    ctl.stop(v, 2000, 960, FadeCurve::Linear, false).unwrap();
    let out = render(&ctl, &mut r, 1, 2500, ODD_BLOCKS);
    assert_eq!(ctl.active_voices(), 1);
    assert!(ctl.poll_events().is_empty(), "still fading");
    let out2 = render(&ctl, &mut r, 1, 1500, ODD_BLOCKS);
    let out: Vec<f32> = out.into_iter().chain(out2).collect();

    assert!(out[..=2000].iter().all(|s| *s == 1.0));
    for k in 0..=960 {
        assert!((out[2000 + k] - (1.0 - k as f32 / 960.0)).abs() < 1e-5);
    }
    assert!(out[2960..].iter().all(|s| *s == 0.0));
    assert!(out[2000..2961].windows(2).all(|w| w[1] <= w[0]));
    assert!(max_step(&out) <= 1.0 / 960.0 + 1e-5);

    assert_eq!(ctl.poll_events(), vec![AudioEvent::VoiceEnded { voice: v }]);
    assert_eq!(ctl.active_voices(), 0);
    assert!(matches!(
        ctl.stop(v, 0, 0, FadeCurve::Linear, false),
        Err(AudioError::UnknownVoice(_))
    ));
}

#[test]
fn stop_without_fade_declicks_and_hard_stop_cuts() {
    let cfg = EngineConfig::new(48_000, 1);
    let d = cfg.declick_frames as usize;
    assert_eq!(d, 144);
    let (mut ctl, mut r) = engine(cfg);
    let a = ctl
        .prepare_voice(dc(100_000), route(&[(0, 0, 1.0)]))
        .unwrap();
    wait_primed(&ctl, &[a]);
    ctl.start(a, 0, 0, FadeCurve::Linear).unwrap();
    ctl.stop(a, 1000, 0, FadeCurve::Linear, false).unwrap();
    let out = render(&ctl, &mut r, 1, 2000, ODD_BLOCKS);
    assert_eq!(out[1000], 1.0);
    assert!(out[1000 + d / 2] > 0.4 && out[1000 + d / 2] < 0.6);
    assert!(out[1000 + d..].iter().all(|s| *s == 0.0));
    assert!(
        max_step(&out) <= 1.0 / d as f32 + 1e-5,
        "declick ramp has no step"
    );

    let b = ctl
        .prepare_voice(dc(100_000), route(&[(0, 0, 1.0)]))
        .unwrap();
    wait_primed(&ctl, &[b]);
    ctl.start(b, 2000, 0, FadeCurve::Linear).unwrap();
    ctl.stop(b, 3000, 0, FadeCurve::Linear, true).unwrap();
    let out = render(&ctl, &mut r, 1, 2000, ODD_BLOCKS);
    assert_eq!(out[999], 1.0);
    assert_eq!(out[1000], 0.0, "hard stop is a cut at the exact frame");
}

#[test]
fn stop_all_fades_started_voices_and_keeps_standby_voices() {
    let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 2));
    let a = ctl
        .prepare_voice(dc(100_000), route(&[(0, 0, 1.0)]))
        .unwrap();
    let b = ctl
        .prepare_voice(dc(100_000), route(&[(0, 1, 1.0)]))
        .unwrap();
    let scheduled = ctl
        .prepare_voice(dc(100_000), route(&[(0, 1, 1.0)]))
        .unwrap();
    let standby = ctl.prepare_voice(dc(100), route(&[(0, 0, 0.5)])).unwrap();
    wait_primed(&ctl, &[a, b, scheduled, standby]);
    ctl.start(a, 0, 0, FadeCurve::Linear).unwrap();
    ctl.start(b, 0, 0, FadeCurve::Linear).unwrap();
    ctl.start(scheduled, 50_000, 0, FadeCurve::Linear).unwrap();
    ctl.stop_all(1000, 480, false).unwrap();
    let out = render(&ctl, &mut r, 2, 3000, ODD_BLOCKS);
    assert_eq!(&out[1000 * 2..1001 * 2], &[1.0, 1.0]);
    assert!((out[1240 * 2] - 0.5).abs() < 1e-5);
    assert!(out[1480 * 2..].iter().all(|s| *s == 0.0));
    let ev = ctl.poll_events();
    assert_eq!(ev.len(), 3, "{ev:?}");
    assert_eq!(ctl.voice_count(), 1);

    // the standby voice is untouched and still playable
    ctl.start(standby, 3000, 0, FadeCurve::Linear).unwrap();
    let out = render(&ctl, &mut r, 2, 200, &[64]);
    assert!(channel(&out, 2, 0)[..100].iter().all(|s| *s == 0.5));

    // hard stop-all = panic
    let c = ctl
        .prepare_voice(dc(100_000), route(&[(0, 0, 1.0)]))
        .unwrap();
    wait_primed(&ctl, &[c]);
    ctl.start(c, 0, 0, FadeCurve::Linear).unwrap();
    let _ = render(&ctl, &mut r, 2, 100, &[64]);
    ctl.stop_all(0, 0, true).unwrap();
    let out = render(&ctl, &mut r, 2, 100, &[64]);
    assert!(out.iter().all(|s| *s == 0.0));
}

#[test]
fn master_and_output_gain_ramps() {
    let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 2));
    let v = ctl
        .prepare_voice(dc(100_000), route(&[(0, 0, 1.0), (0, 1, 1.0)]))
        .unwrap();
    wait_primed(&ctl, &[v]);
    ctl.start(v, 0, 0, FadeCurve::Linear).unwrap();
    ctl.set_master_gain_at(1000, 0.5, 500, FadeCurve::Linear)
        .unwrap();
    let out = render(&ctl, &mut r, 2, 2000, ODD_BLOCKS);
    let l = channel(&out, 2, 0);
    assert!(l[..=1000].iter().all(|s| *s == 1.0));
    for k in 0..=500 {
        assert!((l[1000 + k] - (1.0 - 0.5 * k as f32 / 500.0)).abs() < 1e-5);
    }
    assert!(l[1500..].iter().all(|s| *s == 0.5));
    assert!(max_step(&l) <= 0.001 + 1e-5);

    ctl.set_output_gain(1, 0.0, 100).unwrap(); // mute right
    ctl.set_master_gain(1.0, 0).unwrap();
    let out = render(&ctl, &mut r, 2, 300, &[50]);
    assert_eq!(&out[299 * 2..], &[1.0, 0.0]);
    assert!((out[50 * 2 + 1] - 0.5).abs() < 1e-5);
    assert!(max_step(&channel(&out, 2, 1)) <= 0.01 + 1e-5);
}

// ---------------------------------------------------------------- test 4

#[test]
fn pause_resume_holds_position_exactly() {
    let cfg = EngineConfig::new(48_000, 1);
    let d = cfg.declick_frames as usize;
    let (mut ctl, mut r) = engine(cfg);
    let sig: Vec<f32> = (0..20_000)
        .map(|i| 0.2 + (i % 5000) as f32 * 1e-4)
        .collect();
    let v = ctl
        .prepare_voice(mono(sig.clone()), route(&[(0, 0, 1.0)]))
        .unwrap();
    wait_primed(&ctl, &[v]);
    ctl.start(v, 0, 0, FadeCurve::Linear).unwrap();
    ctl.pause(v, 1000).unwrap();
    ctl.resume(v, 5000).unwrap();
    let out = render(&ctl, &mut r, 1, 9000, ODD_BLOCKS);

    assert_eq!(&out[..=1000], &sig[..=1000]);
    for k in 0..d {
        let g = 1.0 - k as f32 / d as f32;
        assert!(
            (out[1000 + k] - sig[1000 + k] * g).abs() < 1e-5,
            "pause declick k={k}"
        );
    }
    assert!(
        out[1000 + d..=5000].iter().all(|s| *s == 0.0),
        "silent while paused"
    );
    assert_eq!(ctl.voice_position(v), Some((9000 - 5000 + 1000 + d) as u64));
    // After resume the signal continues with the very next source sample.
    let held = 1000 + d;
    for k in 0..d {
        let g = k as f32 / d as f32;
        assert!(
            (out[5000 + k] - sig[held + k] * g).abs() < 1e-5,
            "resume declick k={k}"
        );
    }
    assert_eq!(
        &out[5000 + d..],
        &sig[held + d..held + 4000],
        "no sample lost or repeated"
    );
    assert_eq!(ctl.active_voices(), 1, "paused voices stay active");

    // stop while paused ends the voice at once
    ctl.pause(v, 9000).unwrap();
    let _ = render(&ctl, &mut r, 1, 500, &[64]);
    ctl.stop(v, 0, 48_000, FadeCurve::Linear, false).unwrap();
    let _ = render(&ctl, &mut r, 1, 64, &[64]);
    assert_eq!(ctl.poll_events(), vec![AudioEvent::VoiceEnded { voice: v }]);
}

// ---------------------------------------------------------------- test 5

/// Delivers `gate_at` frames, then stalls until the gate opens.
struct StallingSource {
    data: Vec<f32>,
    pos: usize,
    gate_at: usize,
    gate: Arc<AtomicBool>,
}

impl SampleSource for StallingSource {
    fn channels(&self) -> usize {
        1
    }
    fn total_frames(&self) -> Option<u64> {
        Some(self.data.len() as u64)
    }
    fn read(&mut self, buf: &mut [f32]) -> usize {
        if self.pos >= self.gate_at {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !self.gate.load(Ordering::Acquire) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let limit = if self.pos < self.gate_at {
            self.gate_at
        } else {
            self.data.len()
        };
        let n = buf.len().min(limit - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        n
    }
}

#[test]
fn underrun_outputs_silence_then_recovers_without_losing_samples() {
    let mut cfg = EngineConfig::new(48_000, 1);
    cfg.preroll_frames = 1000;
    let (mut ctl, mut r) = engine(cfg);
    let data: Vec<f32> = (0..40_000)
        .map(|i| 0.1 + (i % 1000) as f32 * 5e-4)
        .collect();
    let gate = Arc::new(AtomicBool::new(false));
    let v = ctl
        .prepare_voice(
            Box::new(StallingSource {
                data: data.clone(),
                pos: 0,
                gate_at: 20_000,
                gate: Arc::clone(&gate),
            }),
            route(&[(0, 0, 1.0)]),
        )
        .unwrap();
    wait_primed(&ctl, &[v]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while ctl.buffered_frames(v) != Some(20_000) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    ctl.start(v, 0, 0, FadeCurve::Linear).unwrap();

    // Plain render calls: the stall must not block the audio thread.
    let mut out = vec![0.0f32; 30_000];
    for block in out.chunks_mut(441) {
        r.render(block, 1);
    }
    assert_eq!(&out[..20_000], &data[..20_000]);
    assert!(
        out[20_000..].iter().all(|s| *s == 0.0),
        "silence during the underrun"
    );
    assert_eq!(ctl.underruns(), 1, "one episode");
    assert_eq!(
        ctl.poll_events(),
        vec![AudioEvent::Underrun { voice: v }],
        "one event per episode"
    );
    assert_eq!(
        ctl.voice_position(v),
        Some(20_000),
        "position did not advance"
    );
    assert_eq!(ctl.active_voices(), 1);

    gate.store(true, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(10);
    while ctl.buffered_frames(v) != Some(20_000) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    let mut out2 = vec![0.0f32; 25_000];
    for block in out2.chunks_mut(441) {
        r.render(block, 1);
    }
    assert_eq!(
        &out2[..20_000],
        &data[20_000..],
        "continues with the next sample"
    );
    assert!(out2[20_000..].iter().all(|s| *s == 0.0));
    assert_eq!(ctl.underruns(), 1);
    assert_eq!(ctl.poll_events(), vec![AudioEvent::VoiceEnded { voice: v }]);
}

struct FailingSource {
    left: usize,
}

impl SampleSource for FailingSource {
    fn channels(&self) -> usize {
        1
    }
    fn total_frames(&self) -> Option<u64> {
        None
    }
    fn read(&mut self, buf: &mut [f32]) -> usize {
        let n = buf.len().min(self.left);
        buf[..n].fill(0.5);
        self.left -= n;
        n
    }
    fn take_error(&mut self) -> Option<String> {
        Some("disk on fire".to_string())
    }
}

#[test]
fn source_error_ends_stream_and_is_reported() {
    let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 1));
    let v = ctl
        .prepare_voice(
            Box::new(FailingSource { left: 3000 }),
            route(&[(0, 0, 1.0)]),
        )
        .unwrap();
    wait_primed(&ctl, &[v]);
    ctl.start(v, 0, 0, FadeCurve::Linear).unwrap();
    let out = render(&ctl, &mut r, 1, 4000, &[256]);
    assert!(out[..3000].iter().all(|s| *s == 0.5));
    assert!(out[3000..].iter().all(|s| *s == 0.0));
    let ev = ctl.poll_events();
    assert!(ev.contains(&AudioEvent::SourceError {
        voice: v,
        message: "disk on fire".into()
    }));
    assert!(ev.contains(&AudioEvent::VoiceEnded { voice: v }));
    assert_eq!(ctl.underruns(), 0);
}

// ---------------------------------------------------------------- test 10

#[test]
fn meters_and_clip_flags() {
    let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 3));
    let n = 96_000;
    let sine: Vec<f32> = (0..n)
        .map(|i| 0.5 * (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 48_000.0).sin())
        .collect();
    let a = ctl
        .prepare_voice(mono(sine), route(&[(0, 0, 1.0)]))
        .unwrap();
    wait_primed(&ctl, &[a]);
    ctl.start(a, 0, 0, FadeCurve::Linear).unwrap();
    let _ = render(&ctl, &mut r, 3, 72_000, &[480]);
    let m = ctl.meters();
    assert_eq!(m.peak.len(), 3);
    assert!((m.peak[0] - 0.5).abs() < 0.01, "peak {}", m.peak[0]);
    assert!((m.rms[0] - 0.3536).abs() < 0.01, "rms {}", m.rms[0]);
    assert_eq!(m.peak[1], 0.0);
    assert_eq!(m.rms[2], 0.0);
    assert_eq!(m.clip, vec![false, false, false]);

    // a hot voice on output 1: clip flag is sticky, output is clamped
    let hot = ctl
        .prepare_voice(
            dc(480),
            VoiceParams {
                gain: 1.5,
                matrix: vec![(0, 1, 1.0)],
            },
        )
        .unwrap();
    wait_primed(&ctl, &[hot]);
    ctl.start(hot, 0, 0, FadeCurve::Linear).unwrap();
    let out = render(&ctl, &mut r, 3, 960, &[480]);
    assert_eq!(out[1], 1.0, "safety clamp");
    let m = ctl.meters();
    assert_eq!(m.clip, vec![false, true, false]);
    assert!(
        m.peak[1] > 1.0 && m.peak[1] <= 1.5,
        "meter shows the unclamped level"
    );
    let _ = render(&ctl, &mut r, 3, 4800, &[480]);
    assert!(ctl.meters().clip[1], "clip flag is sticky");
    ctl.reset_clip();
    let _ = render(&ctl, &mut r, 3, 480, &[480]);
    assert_eq!(ctl.meters().clip, vec![false, false, false]);

    // exactly full scale counts as clipping; master gain is applied first
    let full = ctl.prepare_voice(dc(4800), route(&[(0, 2, 1.0)])).unwrap();
    wait_primed(&ctl, &[full]);
    ctl.start(full, 0, 0, FadeCurve::Linear).unwrap();
    let _ = render(&ctl, &mut r, 3, 480, &[480]);
    assert!(ctl.meters().clip[2]);
    ctl.set_master_gain(0.5, 0).unwrap();
    ctl.reset_clip();
    let _ = render(&ctl, &mut r, 3, 480, &[480]);
    assert!(
        !ctl.meters().clip[2],
        "clip is detected after the master gain"
    );

    // peak decays after the signal is gone (~20 dB/s)
    let before = ctl.meters().peak[2];
    let _ = render(&ctl, &mut r, 3, 48_000, &[480]);
    let after = ctl.meters().peak[2];
    assert!(
        after < before * 0.2 && after > before * 0.05,
        "{before} -> {after}"
    );
}

// ------------------------------------------------------------ generators

struct Marker {
    channel: usize,
    next: u64,
    discontinuity: Arc<AtomicBool>,
    frames: Arc<AtomicU64>,
}

impl RtGenerator for Marker {
    fn render(&mut self, start_frame: u64, out: &mut [f32], channels: usize) {
        if start_frame != self.next {
            self.discontinuity.store(true, Ordering::Relaxed);
        }
        let n = out.len() / channels;
        for (i, f) in out.chunks_exact_mut(channels).enumerate() {
            // overwrite: frame index modulo 100 as a recognisable pattern
            f[self.channel] = ((start_frame + i as u64) % 100) as f32 / 200.0;
        }
        self.next = start_frame + n as u64;
        self.frames.fetch_add(n as u64, Ordering::Relaxed);
    }
}

#[test]
fn generators_run_after_master_gain_with_a_continuous_clock() {
    let disc = Arc::new(AtomicBool::new(false));
    let frames = Arc::new(AtomicU64::new(0));
    let gen = Marker {
        channel: 1,
        next: 0,
        discontinuity: Arc::clone(&disc),
        frames: Arc::clone(&frames),
    };
    let (mut ctl, mut r) =
        engine_with_generators(EngineConfig::new(48_000, 2), vec![Box::new(gen)]);
    let v = ctl
        .prepare_voice(dc(100_000), route(&[(0, 0, 1.0), (0, 1, 1.0)]))
        .unwrap();
    wait_primed(&ctl, &[v]);
    ctl.start(v, 0, 0, FadeCurve::Linear).unwrap();
    ctl.set_master_gain(0.0, 0).unwrap();
    let out = render(&ctl, &mut r, 2, 5000, ODD_BLOCKS);
    for (i, f) in out.chunks_exact(2).enumerate() {
        assert_eq!(f[0], 0.0, "voice is muted by the master");
        assert_eq!(
            f[1],
            (i % 100) as f32 / 200.0,
            "generator ignores the master"
        );
    }
    assert!(!disc.load(Ordering::Relaxed));
    assert_eq!(frames.load(Ordering::Relaxed), 5000);
    assert!(
        (ctl.meters().peak[1] - 0.495).abs() < 1e-3,
        "generators are metered"
    );
}

// ------------------------------------------------------------ bookkeeping

#[test]
fn voice_slots_are_recycled_and_limits_are_reported() {
    let mut cfg = EngineConfig::new(48_000, 1);
    cfg.max_voices = 2;
    let (mut ctl, mut r) = engine(cfg);
    for round in 0..20 {
        let a = ctl.prepare_voice(dc(50), route(&[(0, 0, 0.25)])).unwrap();
        let b = ctl.prepare_voice(dc(70), route(&[(0, 0, 0.5)])).unwrap();
        assert!(matches!(
            ctl.prepare_voice(dc(10), VoiceParams::default()),
            Err(AudioError::NoFreeVoice)
        ));
        wait_primed(&ctl, &[a, b]);
        let t = ctl.clock();
        ctl.start(a, t, 0, FadeCurve::Linear).unwrap();
        ctl.start(b, t, 0, FadeCurve::Linear).unwrap();
        let out = render(&ctl, &mut r, 1, 128, &[37]);
        assert_eq!(out[49], 0.75, "round {round}");
        assert_eq!(out[50], 0.5);
        assert_eq!(out[70], 0.0);
        assert_eq!(ctl.poll_events().len(), 2);
    }

    // discard a prepared voice
    let a = ctl.prepare_voice(dc(50), VoiceParams::default()).unwrap();
    ctl.discard(a).unwrap();
    let _ = render(&ctl, &mut r, 1, 16, &[16]);
    assert_eq!(ctl.poll_events(), vec![AudioEvent::VoiceEnded { voice: a }]);
    assert_eq!(ctl.voice_count(), 0);

    // invalid parameters
    assert!(matches!(
        ctl.prepare_voice(dc(10), route(&[(0, 1, 1.0)])),
        Err(AudioError::InvalidParam(_))
    ));
    assert!(matches!(
        ctl.prepare_voice(dc(10), route(&[(1, 0, 1.0)])),
        Err(AudioError::InvalidParam(_))
    ));
}

#[test]
fn full_command_queue_is_an_error_not_a_block() {
    let mut cfg = EngineConfig::new(48_000, 1);
    cfg.command_capacity = 64;
    let (mut ctl, mut r) = engine(cfg);
    let v = ctl.prepare_voice(dc(10), VoiceParams::default()).unwrap();
    let mut sent = 1;
    let err = loop {
        match ctl.pause(v, 1_000_000) {
            Ok(()) => sent += 1,
            Err(e) => break e,
        }
        assert!(sent < 1000);
    };
    assert!(matches!(err, AudioError::QueueFull));
    assert_eq!(sent, 64);
    // rendering drains the queue again
    let _ = render(&ctl, &mut r, 1, 16, &[16]);
    ctl.set_master_gain(1.0, 0).unwrap();
}
