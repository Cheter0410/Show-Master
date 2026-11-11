//! Phase 0 prototype (b): generate LTC, decode it again and verify labels
//! and sample positions.
//!
//! ```text
//! cargo run -p sm-timecode --example ltc_roundtrip -- [seconds] [rate] [sample_rate] [start]
//! cargo run -p sm-timecode --example ltc_roundtrip -- 60 29.97df 48000 00:59:50:00
//! ```
//! Rates: 23.976, 24, 25, 29.97df, 29.97, 30.

use sm_timecode::{FrameRate, LtcDecoder, LtcEncoder, TcTransport, Timecode};
use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seconds: u64 = args.first().and_then(|s| s.parse().ok()).unwrap_or(10);
    let rate = match args.get(1) {
        None => FrameRate::F2997Df,
        Some(s) => match FrameRate::ALL.into_iter().find(|r| r.label() == s) {
            Some(r) => r,
            None => {
                eprintln!("unknown frame rate '{s}'");
                return ExitCode::FAILURE;
            }
        },
    };
    let sample_rate: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(48_000);
    let start: Timecode = args
        .get(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(Timecode::new(0, 59, 50, 0));

    let transport = TcTransport {
        rate,
        running: true,
        anchor_sample: 1_000,
        anchor_frame: start.to_frame_number(rate),
    };
    let total = transport.anchor_sample + seconds * u64::from(sample_rate);

    let mut encoder = LtcEncoder::new(sample_rate);
    encoder.set_transport(transport);
    let mut decoder = LtcDecoder::new(sample_rate);

    let mut block = vec![0.0f32; 512];
    let mut decoded = Vec::new();
    let mut frames = 0u64;
    let mut label_errors = 0u64;
    let mut max_offset = 0u64;
    let mut first = None;
    let mut last = None;
    let mut pos = 0u64;
    let started = Instant::now();
    while pos < total {
        let n = (block.len() as u64).min(total - pos) as usize;
        let Some(buf) = block.get_mut(..n) else { break };
        encoder.render(pos, buf, 0.5);
        decoder.push(buf, &mut decoded);
        pos += n as u64;
        for f in decoded.drain(..) {
            let want = Timecode::from_frame_number(transport.anchor_frame + frames, rate);
            if f.timecode != want || f.drop_frame != rate.drop_frame() {
                label_errors += 1;
            }
            if let Some(ideal) = transport.sample_after_frames(frames + 1, sample_rate) {
                max_offset = max_offset.max(f.end_sample.abs_diff(ideal));
            }
            first.get_or_insert(f);
            last = Some(f);
            frames += 1;
        }
    }
    let elapsed = started.elapsed();

    let expected = transport.frames_elapsed(total - 1, sample_rate);
    println!(
        "LTC roundtrip: {seconds} s of {} fps at {sample_rate} Hz",
        rate.label()
    );
    println!("  samples rendered      : {total}");
    println!("  frames decoded        : {frames} (expected {expected})");
    if let (Some(a), Some(b)) = (first, last) {
        println!(
            "  first / last label    : {} / {}",
            a.timecode.format(rate),
            b.timecode.format(rate)
        );
        println!("  last frame ends at    : sample {}", b.end_sample);
    }
    println!("  label errors          : {label_errors}");
    println!("  max position error    : {max_offset} samples");
    println!(
        "  encode+decode time    : {:.1} ms ({:.0}x real time)",
        elapsed.as_secs_f64() * 1e3,
        seconds as f64 / elapsed.as_secs_f64().max(1e-9)
    );
    let ok = label_errors == 0 && max_offset == 0 && frames == expected;
    println!(
        "  result                : {}",
        if ok { "OK" } else { "FAILED" }
    );
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
