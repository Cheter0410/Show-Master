//! Resampling quality (the 44.1 kHz files most music comes as, on a 48 kHz engine).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod common;
use common::{write_wav_f32, TempDir};
use sm_audio::{FileSource, FileSourceOpts, SampleSource};

fn read_all(src: &mut dyn SampleSource) -> Vec<f32> {
    let mut out = Vec::new();
    let mut buf = vec![0.0f32; 1000];
    loop {
        let n = src.read(&mut buf);
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }
    out
}

/// 44.1 -> 48 kHz must be transparent: error below -100 dBFS (relative to
/// the signal) at every frequency, no clicks across resampler blocks.
#[test]
fn resampling_44k1_to_48k_is_transparent() {
    let dir = TempDir::new("rsq");
    for freq in [1000.0f64, 5000.0, 12000.0, 18000.0] {
        let sine: Vec<f32> = (0..44_100)
            .map(|i| (0.5 * (2.0 * std::f64::consts::PI * freq * i as f64 / 44_100.0).sin()) as f32)
            .collect();
        let p = dir.path("s.wav");
        write_wav_f32(&p, 44_100, 1, &sine);
        let out = read_all(&mut FileSource::open(&p, FileSourceOpts::new(48_000)).unwrap());
        let mut worst = 0.0f32;
        for (i, v) in out.iter().enumerate().take(47_000).skip(1000) {
            let ideal =
                (0.5 * (2.0 * std::f64::consts::PI * freq * i as f64 / 48_000.0).sin()) as f32;
            worst = worst.max((v - ideal).abs());
        }
        // largest sample-to-sample step relative to the ideal maximum step (clicks)
        let ideal_step = (0.5 * 2.0 * std::f64::consts::PI * freq / 48_000.0) as f32;
        let step = out
            .windows(2)
            .skip(1000)
            .take(46_000)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        let db = 20.0 * (worst / 0.5).log10();
        println!(
            "{freq:>6} Hz: worst error {db:.1} dB, max step / ideal {:.3}",
            step / ideal_step
        );
        assert!(db < -100.0, "{freq} Hz: error {db:.1} dB");
        assert!(
            step <= ideal_step * 1.01,
            "{freq} Hz: a click (step {step} > {ideal_step})"
        );
    }
}
