//! Phase 0 prototype (a): multi-channel streaming benchmark.
//!
//! Generates WAV files, then
//!  1. renders N file voices offline and prints the mixer cost in ns/frame,
//!  2. streams them in real time through the null backend (default) or a
//!     real device and prints underruns and CPU load once per second.
//!
//! ```text
//! cargo run --release -p sm-audio --example audio_bench -- \
//!     [--voices 32] [--outs 16] [--file-channels 2] [--seconds 5] \
//!     [--rate 48000] [--file-rate 48000] [--buffer 256] \
//!     [--device null|default|<name>] [--list] [--skip-offline]
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sm_audio::{
    default_backends, engine, AudioBackend, Controller, EngineConfig, FadeCurve, FileSource,
    FileSourceOpts, NullBackend, StreamRequest, VoiceId, VoiceParams,
};

struct Args {
    voices: usize,
    outs: usize,
    file_channels: u16,
    seconds: u64,
    rate: u32,
    file_rate: u32,
    buffer: u32,
    device: String,
    list: bool,
    skip_offline: bool,
}

fn parse_args() -> Args {
    let mut a = Args {
        voices: 32,
        outs: 16,
        file_channels: 2,
        seconds: 5,
        rate: 48_000,
        file_rate: 48_000,
        buffer: 256,
        device: "null".into(),
        list: false,
        skip_offline: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = |name: &str| it.next().unwrap_or_else(|| panic!("{name} needs a value"));
        match flag.as_str() {
            "--voices" => a.voices = value("--voices").parse().expect("number"),
            "--outs" => a.outs = value("--outs").parse().expect("number"),
            "--file-channels" => {
                a.file_channels = value("--file-channels").parse().expect("number")
            }
            "--seconds" => a.seconds = value("--seconds").parse().expect("number"),
            "--rate" => a.rate = value("--rate").parse().expect("number"),
            "--file-rate" => a.file_rate = value("--file-rate").parse().expect("number"),
            "--buffer" => a.buffer = value("--buffer").parse().expect("number"),
            "--device" => a.device = value("--device"),
            "--list" => a.list = true,
            "--skip-offline" => a.skip_offline = true,
            other => panic!("unknown argument {other}"),
        }
    }
    a
}

/// A few seconds of detuned sines per channel, 16-bit.
fn write_wav(path: &Path, rate: u32, channels: u16, seconds: u32, seed: usize) {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).expect("create wav");
    for i in 0..(rate * seconds) as usize {
        for c in 0..channels as usize {
            let f = 110.0 * (1 + (seed * 7 + c * 3) % 16) as f64;
            let s = (2.0 * std::f64::consts::PI * f * i as f64 / rate as f64).sin();
            w.write_sample((s * 6000.0) as i16).expect("write");
        }
    }
    w.finalize().expect("finalize");
}

fn prepare(ctl: &mut Controller, files: &[PathBuf], a: &Args, n: usize) -> Vec<VoiceId> {
    let mut voices = Vec::new();
    for i in 0..n {
        let mut o = FileSourceOpts::new(a.rate);
        o.loop_count = 0;
        let src = FileSource::open(&files[i % files.len()], o).expect("open file");
        let matrix = (0..a.file_channels)
            .map(|c| {
                (
                    c,
                    ((i * a.file_channels as usize + c as usize) % a.outs) as u16,
                    1.0,
                )
            })
            .collect();
        let params = VoiceParams {
            gain: 0.5 / (n as f32).sqrt(),
            matrix,
        };
        voices.push(ctl.prepare_voice(Box::new(src), params).expect("prepare"));
    }
    while !voices.iter().all(|v| ctl.is_primed(*v)) {
        std::thread::sleep(Duration::from_millis(1));
    }
    voices
}

fn offline(files: &[PathBuf], a: &Args) {
    println!(
        "\n== offline mixer cost ({} outs, {}-ch files, blocks of {}) ==",
        a.outs, a.file_channels, a.buffer
    );
    println!(
        "{:>7} {:>12} {:>12} {:>14} {:>10}",
        "voices", "ns/frame", "us/block", "% of realtime", "underruns"
    );
    let mut counts = vec![1usize, 8, 16, 32, 64, 128];
    if !counts.contains(&a.voices) {
        counts.push(a.voices);
    }
    for n in counts {
        let mut cfg = EngineConfig::new(a.rate, a.outs);
        cfg.max_voices = n.max(128);
        let (mut ctl, mut r) = engine(cfg);
        let voices = prepare(&mut ctl, files, a, n);
        for v in &voices {
            ctl.start(*v, 0, 0, FadeCurve::Linear).expect("start");
        }
        let block = a.buffer as usize;
        let mut buf = vec![0.0f32; block * a.outs];
        let total_blocks = (a.rate as usize * 4) / block;
        let mut spent = Duration::ZERO;
        for _ in 0..total_blocks {
            while !ctl.all_buffered(block as u64) {
                std::thread::yield_now();
            }
            let t0 = Instant::now();
            r.render(&mut buf, a.outs);
            spent += t0.elapsed();
        }
        let per_block = spent.as_secs_f64() / total_blocks as f64;
        println!(
            "{:>7} {:>12.1} {:>12.1} {:>13.2}% {:>10}",
            n,
            per_block * 1e9 / block as f64,
            per_block * 1e6,
            100.0 * per_block / (block as f64 / a.rate as f64),
            ctl.underruns()
        );
    }
}

fn realtime(files: &[PathBuf], a: &Args) {
    let mut cfg = EngineConfig::new(a.rate, a.outs);
    cfg.max_voices = a.voices.max(128);
    let (mut ctl, renderer) = engine(cfg);
    let voices = prepare(&mut ctl, files, a, a.voices);

    let req = StreamRequest {
        device_id: None,
        sample_rate: a.rate,
        channels: a.outs,
        buffer_frames: Some(a.buffer),
    };
    let stream = if a.device == "null" {
        NullBackend::new().open(req, renderer)
    } else {
        let backends = default_backends();
        let backend = backends.first().expect("a backend");
        let req = StreamRequest {
            device_id: (a.device != "default").then(|| a.device.clone()),
            ..req
        };
        backend.open(req, renderer)
    };
    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "cannot open device '{}': {e} (try --device null or --list)",
                a.device
            );
            std::process::exit(2);
        }
    };
    let info = stream.info();
    println!(
        "\n== real time: {} voices -> {} [{}], {} Hz, {} ch, buffer {:?} ==",
        a.voices,
        info.device_name,
        info.backend,
        info.sample_rate,
        info.channels,
        info.buffer_frames
    );
    let at = ctl.clock() + a.rate as u64 / 10;
    for v in &voices {
        ctl.start(*v, at, 0, FadeCurve::Linear).expect("start");
    }
    let mut max_load = 0.0f32;
    for s in 1..=a.seconds {
        let until = Instant::now() + Duration::from_secs(1);
        while Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
            max_load = max_load.max(stream.cpu_load());
            ctl.poll_events();
        }
        let m = ctl.meters();
        let peak = m.peak.iter().fold(0.0f32, |a, b| a.max(*b));
        println!(
            "t={s:>3}s clock={:>9} active={:>3} underruns={} cpu={:5.2}% (max {:5.2}%) peak={:.3}{}",
            ctl.clock(),
            ctl.active_voices(),
            ctl.underruns(),
            stream.cpu_load() * 100.0,
            max_load * 100.0,
            peak,
            stream.error().map(|e| format!(" ERROR: {e}")).unwrap_or_default()
        );
    }
    ctl.stop_all(0, a.rate as u64 / 10, false)
        .expect("stop all");
    std::thread::sleep(Duration::from_millis(250));
    ctl.poll_events();
    println!(
        "total underruns: {}, max cpu load: {:.2}%",
        ctl.underruns(),
        max_load * 100.0
    );
}

fn main() {
    let a = parse_args();
    if a.list {
        for b in default_backends() {
            println!("[{}]", b.name());
            for d in b.devices() {
                println!(
                    "  {}{} - {} ch, default {} Hz, rates {:?}",
                    d.name,
                    if d.is_default { " (default)" } else { "" },
                    d.max_output_channels,
                    d.default_sample_rate,
                    d.sample_rates
                );
            }
        }
        return;
    }
    assert!(a.voices > 0 && a.outs > 0 && a.file_channels > 0 && a.buffer > 0);

    let dir = std::env::temp_dir().join(format!("sm-audio-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let n_files = a.voices.min(8);
    let files: Vec<PathBuf> = (0..n_files)
        .map(|i| {
            let p = dir.join(format!("bench{i}.wav"));
            write_wav(&p, a.file_rate, a.file_channels, 4, i);
            p
        })
        .collect();
    println!(
        "generated {n_files} WAV files ({} ch, {} Hz, 4 s, looped) in {}",
        a.file_channels,
        a.file_rate,
        dir.display()
    );

    if !a.skip_offline {
        offline(&files, &a);
    }
    realtime(&files, &a);
    let _ = std::fs::remove_dir_all(&dir);
}
