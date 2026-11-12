//! FileSource, probe and waveform tests on WAV files generated on the fly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::io::Write;
use std::path::Path;

use common::{
    channel, i16_to_f32, noise_i16, render, wait_primed, write_wav_f32, write_wav_i16,
    write_wav_i24, TempDir,
};
use sm_audio::{
    engine, probe, waveform_peaks, AudioError, AudioEvent, EngineConfig, FadeCurve, FileSource,
    FileSourceOpts, SampleSource, VoiceParams,
};

/// Drain a source with awkward read sizes.
fn read_all(src: &mut dyn SampleSource) -> Vec<f32> {
    let ch = src.channels();
    let mut out = Vec::new();
    let sizes = [1usize, 7, 1024, 333, 4096, 2];
    let mut i = 0;
    loop {
        let mut buf = vec![0.0f32; sizes[i % sizes.len()] * ch];
        i += 1;
        let n = src.read(&mut buf);
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n * ch]);
        assert!(out.len() < 50_000_000, "runaway source");
    }
    assert_eq!(src.take_error(), None);
    out
}

fn opts() -> FileSourceOpts {
    FileSourceOpts::new(48_000)
}

#[test]
fn decodes_wav_bit_exact() {
    let dir = TempDir::new("decode");
    // 16-bit stereo
    let s16 = noise_i16(2 * 10_000, 1);
    let p = dir.path("s16.wav");
    write_wav_i16(&p, 48_000, 2, &s16);
    let mut src = FileSource::open(&p, opts()).unwrap();
    assert_eq!(src.channels(), 2);
    assert_eq!(src.total_frames(), Some(10_000));
    assert_eq!(src.info().codec, "pcm_s16le");
    assert_eq!(read_all(&mut src), i16_to_f32(&s16));

    // 24-bit mono
    let s24: Vec<i32> = (0..5000)
        .map(|i| (i * 1677 % 16_000_000) - 8_000_000)
        .collect();
    let p = dir.path("s24.wav");
    write_wav_i24(&p, 48_000, 1, &s24);
    let mut src = FileSource::open(&p, opts()).unwrap();
    let expect: Vec<f32> = s24.iter().map(|v| *v as f32 / 8_388_608.0).collect();
    assert_eq!(read_all(&mut src), expect);

    // 32-bit float, 3 channels
    let f: Vec<f32> = (0..3 * 4000)
        .map(|i| ((i * 31) % 2000) as f32 / 1000.0 - 1.0)
        .collect();
    let p = dir.path("f32.wav");
    write_wav_f32(&p, 48_000, 3, &f);
    let mut src = FileSource::open(&p, opts()).unwrap();
    assert_eq!(src.channels(), 3);
    assert_eq!(read_all(&mut src), f);
}

#[test]
fn start_and_end_points_are_sample_accurate() {
    let dir = TempDir::new("region");
    let s = noise_i16(2 * 20_000, 2);
    let f = i16_to_f32(&s);
    let p = dir.path("a.wav");
    write_wav_i16(&p, 48_000, 2, &s);

    let mut o = opts();
    o.start_frame = Some(101);
    o.end_frame = Some(1102);
    let mut src = FileSource::open(&p, o).unwrap();
    assert_eq!(src.total_frames(), Some(1001));
    assert_eq!(read_all(&mut src), &f[101 * 2..1102 * 2]);

    // the same through seconds
    let mut o = opts();
    o.start_seconds = 4800.0 / 48_000.0;
    o.end_seconds = Some(0.25);
    let mut src = FileSource::open(&p, o).unwrap();
    assert_eq!(read_all(&mut src), &f[4800 * 2..12_000 * 2]);

    // end beyond the file is clamped, start beyond the end gives nothing
    let mut o = opts();
    o.start_frame = Some(19_990);
    o.end_seconds = Some(100.0);
    let mut src = FileSource::open(&p, o).unwrap();
    assert_eq!(read_all(&mut src), &f[19_990 * 2..]);
    let mut o = opts();
    o.start_seconds = 50.0;
    let mut src = FileSource::open(&p, o).unwrap();
    assert_eq!(src.total_frames(), Some(0));
    assert!(read_all(&mut src).is_empty());
}

#[test]
fn loops_are_seamless_and_counted() {
    let dir = TempDir::new("loop");
    let s = noise_i16(8000, 3);
    let f = i16_to_f32(&s);
    let p = dir.path("a.wav");
    write_wav_i16(&p, 48_000, 1, &s);

    let mut o = opts();
    o.start_frame = Some(500);
    o.end_frame = Some(1500);
    o.loop_count = 3;
    let mut src = FileSource::open(&p, o.clone()).unwrap();
    assert_eq!(src.total_frames(), Some(3000));
    let got = read_all(&mut src);
    assert_eq!(got.len(), 3000, "exactly 3x the region");
    for pass in 0..3 {
        assert_eq!(
            &got[pass * 1000..(pass + 1) * 1000],
            &f[500..1500],
            "pass {pass}"
        );
    }

    // offset inside the first pass
    o.offset_seconds = 250.0 / 48_000.0;
    let mut src = FileSource::open(&p, o.clone()).unwrap();
    assert_eq!(src.total_frames(), Some(2750));
    let got = read_all(&mut src);
    assert_eq!(got.len(), 2750);
    assert_eq!(&got[..750], &f[750..1500]);
    assert_eq!(&got[750..1750], &f[500..1500]);

    // offset that skips a whole pass
    o.offset_seconds = 1250.0 / 48_000.0;
    let mut src = FileSource::open(&p, o.clone()).unwrap();
    let got = read_all(&mut src);
    assert_eq!(got.len(), 1750);
    assert_eq!(&got[..750], &f[750..1500]);

    // offset past everything
    o.offset_seconds = 1.0;
    let mut src = FileSource::open(&p, o.clone()).unwrap();
    assert!(read_all(&mut src).is_empty());

    // infinite loop: no total, keeps going
    o.offset_seconds = 0.0;
    o.loop_count = 0;
    let mut src = FileSource::open(&p, o).unwrap();
    assert_eq!(src.total_frames(), None);
    let mut buf = vec![0.0f32; 10_500];
    let mut filled = 0;
    while filled < buf.len() {
        let n = src.read(&mut buf[filled..]);
        assert!(n > 0);
        filled += n;
    }
    for (i, v) in buf.iter().enumerate() {
        assert_eq!(*v, f[500 + i % 1000]);
    }
}

#[test]
fn vamp_plays_the_intro_then_loops_the_vamp_region() {
    let dir = TempDir::new("vamp");
    let s = noise_i16(8000, 5);
    let f = i16_to_f32(&s);
    let p = dir.path("a.wav");
    write_wav_i16(&p, 48_000, 1, &s);
    let mut o = opts();
    o.start_frame = Some(100);
    o.end_frame = Some(1500);
    o.loop_start_frame = Some(1000);
    o.loop_count = 0;
    let mut src = FileSource::open(&p, o.clone()).unwrap();
    assert_eq!(src.total_frames(), None);
    let mut buf = vec![0.0f32; 1400 + 5 * 500];
    let mut filled = 0;
    while filled < buf.len() {
        let to = (filled + 333).min(buf.len());
        let n = src.read(&mut buf[filled..to]);
        assert!(n > 0);
        filled += n;
    }
    assert_eq!(&buf[..1400], &f[100..1500], "intro");
    for pass in 0..5 {
        let at = 1400 + pass * 500;
        assert_eq!(&buf[at..at + 500], &f[1000..1500], "loop pass {pass}");
    }
    // An offset past the intro lands inside the loop.
    o.offset_seconds = (1400.0 + 2.0 * 500.0 + 120.0) / 48_000.0;
    let mut src = FileSource::open(&p, o).unwrap();
    let mut buf = vec![0.0f32; 380];
    let n = src.read(&mut buf);
    assert_eq!(n, 380);
    assert_eq!(&buf[..], &f[1120..1500]);
}

#[test]
fn resamples_44k1_to_48k_with_correct_length_and_pitch() {
    let dir = TempDir::new("resample");
    let rate_in = 44_100u32;
    let n_in = 44_100usize; // 1 s
    let freq = 1000.0f64;
    let sine: Vec<f32> = (0..n_in)
        .map(|i| {
            (0.5 * (2.0 * std::f64::consts::PI * freq * i as f64 / rate_in as f64).sin()) as f32
        })
        .collect();
    let p = dir.path("sine44.wav");
    write_wav_f32(&p, rate_in, 1, &sine);

    let mut src = FileSource::open(&p, opts()).unwrap();
    assert_eq!(src.info().sample_rate, 44_100);
    assert_eq!(src.total_frames(), Some(48_000));
    let out = read_all(&mut src);
    assert_eq!(out.len(), 48_000, "1 s at 44.1 kHz is 1 s at 48 kHz");

    // pitch: count rising zero crossings (1000 per second)
    let crossings = out.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    assert!((999..=1001).contains(&crossings), "{crossings} cycles");

    // phase / delay compensation: matches the ideal 48 kHz sine sample by sample
    let mut worst = 0.0f32;
    for (i, v) in out.iter().enumerate().take(47_000).skip(1000) {
        let ideal = (0.5 * (2.0 * std::f64::consts::PI * freq * i as f64 / 48_000.0).sin()) as f32;
        worst = worst.max((v - ideal).abs());
    }
    assert!(worst < 0.01, "worst deviation from ideal {worst}");

    // a looped region stays the right length after resampling: 3 x 0.1 s
    let mut o = opts();
    o.end_frame = Some(4410);
    o.loop_count = 3;
    let mut src = FileSource::open(&p, o).unwrap();
    assert_eq!(src.total_frames(), Some(14_400));
    assert_eq!(read_all(&mut src).len(), 14_400);

    // stereo downsampling 96k → 48k keeps channels apart
    let st: Vec<f32> = (0..9600).flat_map(|_| [0.25f32, -0.5]).collect();
    let p = dir.path("dc96.wav");
    write_wav_f32(&p, 96_000, 2, &st);
    let mut src = FileSource::open(&p, opts()).unwrap();
    let out = read_all(&mut src);
    assert_eq!(out.len(), 4800 * 2);
    for f in out.chunks_exact(2).skip(500).take(3800) {
        assert!((f[0] - 0.25).abs() < 1e-3 && (f[1] + 0.5).abs() < 1e-3);
    }
}

/// A plain (non-extensible) PCM header, as written by many DAWs for
/// multichannel "direct out" files; stricter decoders reject it.
fn write_plain_pcm16(path: &Path, rate: u32, channels: u16, samples: &[i16]) {
    let mut f = std::fs::File::create(path).unwrap();
    let data_len = (samples.len() * 2) as u32;
    f.write_all(b"RIFF").unwrap();
    f.write_all(&(36 + data_len + 12).to_le_bytes()).unwrap();
    f.write_all(b"WAVE").unwrap();
    // an unknown chunk with odd size before fmt, to exercise chunk skipping
    f.write_all(b"junk").unwrap();
    f.write_all(&3u32.to_le_bytes()).unwrap();
    f.write_all(&[1, 2, 3, 0]).unwrap();
    f.write_all(b"fmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap();
    f.write_all(&channels.to_le_bytes()).unwrap();
    f.write_all(&rate.to_le_bytes()).unwrap();
    f.write_all(&(rate * channels as u32 * 2).to_le_bytes())
        .unwrap();
    f.write_all(&(channels * 2).to_le_bytes()).unwrap();
    f.write_all(&16u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&data_len.to_le_bytes()).unwrap();
    for s in samples {
        f.write_all(&s.to_le_bytes()).unwrap();
    }
}

#[test]
fn multichannel_wav_files() {
    let dir = TempDir::new("multi");
    for (name, channels, extensible) in [
        ("8ch.wav", 8u16, true),
        ("12ch.wav", 12, true),
        ("32ch_plain.wav", 32, false),
    ] {
        let frames = 3000usize;
        let s: Vec<i16> = (0..frames)
            .flat_map(|i| (0..channels).map(move |c| (c as i16 + 1) * 500 + (i % 400) as i16))
            .collect();
        let p = dir.path(name);
        if extensible {
            write_wav_i16(&p, 48_000, channels, &s);
        } else {
            write_plain_pcm16(&p, 48_000, channels, &s);
        }
        let info = probe(&p).unwrap();
        assert_eq!(info.channels, channels as usize, "{name}");
        assert_eq!(info.frames, Some(frames as u64));

        let mut o = opts();
        o.start_frame = Some(10);
        o.loop_count = 2;
        let mut src = FileSource::open(&p, o).unwrap();
        assert_eq!(src.channels(), channels as usize);
        let got = read_all(&mut src);
        let f = i16_to_f32(&s);
        let region = &f[10 * channels as usize..];
        assert_eq!(got.len(), region.len() * 2);
        assert_eq!(&got[..region.len()], region);
        assert_eq!(&got[region.len()..], region);
    }
}

#[test]
fn probe_and_waveform() {
    let dir = TempDir::new("probe");
    // stereo: left ramps up to +0.5, right is a constant -0.25
    let frames = 48_000usize;
    let s: Vec<i16> = (0..frames)
        .flat_map(|i| [(i as f32 / frames as f32 * 16_384.0) as i16, -8192])
        .collect();
    let p = dir.path("a.wav");
    write_wav_i16(&p, 44_100, 2, &s);

    let info = probe(&p).unwrap();
    assert_eq!(info.sample_rate, 44_100);
    assert_eq!(info.channels, 2);
    assert_eq!(info.frames, Some(48_000));
    assert!((info.duration_seconds.unwrap() - 48_000.0 / 44_100.0).abs() < 1e-9);
    assert_eq!(info.codec, "pcm_s16le");

    let peaks = waveform_peaks(&p, 100).unwrap();
    assert_eq!(peaks.len(), 100);
    for (i, (lo, hi)) in peaks.iter().enumerate() {
        assert!((lo + 0.25).abs() < 1e-4, "min is the right channel");
        let expect_hi = (i + 1) as f32 / 100.0 * 0.5;
        assert!(
            (hi - expect_hi).abs() < 0.01,
            "bucket {i}: {hi} vs {expect_hi}"
        );
    }
    assert!(waveform_peaks(&p, 0).unwrap().is_empty());
    assert_eq!(waveform_peaks(&p, 100_000).unwrap().len(), 100_000);

    // errors
    assert!(matches!(
        probe(dir.path("missing.wav")),
        Err(AudioError::Io(_))
    ));
    let junk = dir.path("junk.wav");
    std::fs::write(&junk, b"this is not audio at all, sorry........").unwrap();
    assert!(probe(&junk).is_err());
    assert!(FileSource::open(&junk, opts()).is_err());
}

// ---------------------------------------------------------------- test 7

#[test]
fn streaming_end_to_end_matches_file_content() {
    let dir = TempDir::new("e2e");
    // 3 s stereo: longer than the 2 s ring, so the streamer must keep
    // refilling while we render.
    let frames = 144_000usize;
    let s = noise_i16(frames * 2, 7);
    let f = i16_to_f32(&s);
    let p = dir.path("long.wav");
    write_wav_i16(&p, 48_000, 2, &s);

    let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 2));
    let src = FileSource::open(&p, opts()).unwrap();
    let v = ctl
        .prepare_voice(Box::new(src), VoiceParams::default())
        .unwrap();
    wait_primed(&ctl, &[v]);
    assert!(ctl.buffered_frames(v).unwrap() >= 24_000);
    const T: usize = 777;
    ctl.start(v, T as u64, 0, FadeCurve::Linear).unwrap();
    let out = render(&ctl, &mut r, 2, T + frames + 1000, &[512, 441, 64, 1000]);
    assert!(out[..T * 2].iter().all(|x| *x == 0.0));
    assert!(
        out[T * 2..(T + frames) * 2] == f[..],
        "output equals the file at the start frame"
    );
    assert!(out[(T + frames) * 2..].iter().all(|x| *x == 0.0));
    assert_eq!(ctl.underruns(), 0);
    assert_eq!(ctl.poll_events(), vec![AudioEvent::VoiceEnded { voice: v }]);
}

#[test]
fn streaming_resampled_looped_file_voice() {
    let dir = TempDir::new("e2e-rs");
    let sine: Vec<f32> = (0..4410)
        .map(|i| (0.5 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 44_100.0).sin()) as f32)
        .collect();
    let p = dir.path("sine.wav");
    write_wav_f32(&p, 44_100, 1, &sine);
    let (mut ctl, mut r) = engine(EngineConfig::new(48_000, 1));
    let mut o = opts();
    o.loop_count = 20; // 2 s of seamless 1 kHz (4410 frames = 100 cycles)
    let v = ctl
        .prepare_voice(
            Box::new(FileSource::open(&p, o).unwrap()),
            VoiceParams::default(),
        )
        .unwrap();
    wait_primed(&ctl, &[v]);
    ctl.start(v, 0, 0, FadeCurve::Linear).unwrap();
    let out = render(&ctl, &mut r, 1, 97_000, &[480]);
    let l = channel(&out, 1, 0);
    assert!(
        l[96_000..].iter().all(|x| *x == 0.0),
        "ends after exactly 2 s"
    );
    let mut worst = 0.0f32;
    for (i, v) in l.iter().enumerate().take(95_000).skip(1000) {
        let ideal =
            (0.5 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48_000.0).sin()) as f32;
        worst = worst.max((v - ideal).abs());
    }
    assert!(
        worst < 0.01,
        "loop seams are inaudible after resampling: {worst}"
    );
}

// ------------------------------------------------- compressed formats

fn ffmpeg(args: &[&str]) -> bool {
    std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .stdout(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// FLAC / AIFF / MP3 / Ogg Vorbis / AAC through symphonia. Needs an `ffmpeg` binary to
/// make the fixtures; skipped (with a note) when there is none.
#[test]
fn compressed_formats_decode_seek_and_loop() {
    let dir = TempDir::new("codecs");
    let frames = 96_000usize;
    // slow sweep: any seek error shows up as a waveform mismatch
    let sig: Vec<i16> = (0..frames)
        .map(|i| {
            let t = i as f64 / 48_000.0;
            (8000.0 * (2.0 * std::f64::consts::PI * (200.0 + 150.0 * t) * t).sin()) as i16
        })
        .collect();
    let wav = dir.path("src.wav");
    write_wav_i16(&wav, 48_000, 1, &sig);
    let f = i16_to_f32(&sig);
    if !ffmpeg(&["-version"]) {
        eprintln!("ffmpeg not found: skipping compressed format test");
        return;
    }
    let wav_s = wav.to_str().unwrap();

    for (ext, codec, lossless) in [
        ("flac", "flac", true),
        ("aiff", "pcm_s16be", true),
        ("mp3", "mp3", false),
        ("ogg", "vorbis", false),
        ("m4a", "aac", false),
    ] {
        let p = dir.path(&format!("a.{ext}"));
        let extra: &[&str] = if ext == "ogg" {
            &["-c:a", "libvorbis", "-q:a", "6"]
        } else {
            &[]
        };
        let mut args = vec!["-i", wav_s];
        args.extend_from_slice(extra);
        args.push(p.to_str().unwrap());
        if !ffmpeg(&args) {
            eprintln!("ffmpeg cannot encode {ext}: skipped");
            continue;
        }
        let info = probe(&p).unwrap();
        assert_eq!(info.codec, codec, "{ext}");
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(info.channels, 1);

        let tol = if lossless { 1e-6 } else { 0.08 };
        let close = |got: &[f32], want: &[f32], what: &str| {
            assert_eq!(got.len(), want.len(), "{ext} {what}: length");
            let worst = got
                .iter()
                .zip(want)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(worst <= tol, "{ext} {what}: worst error {worst}");
        };

        // whole file
        let mut src = FileSource::open(&p, opts()).unwrap();
        let all = read_all(&mut src);
        // AAC in MP4 keeps its encoder priming (symphonia does not apply
        // edit lists), so it is not aligned with the original; its seeks
        // are checked against its own linear decode instead.
        let aligned = ext != "m4a";
        if lossless {
            close(&all, &f, "full decode");
        } else {
            // lossy encoders may pad; the content must still line up
            assert!(
                all.len() >= frames && all.len() < frames + 4800,
                "{ext}: {} frames",
                all.len()
            );
            if aligned {
                close(
                    &all[2000..frames - 2000],
                    &f[2000..frames - 2000],
                    "full decode",
                );
            }
        }
        let f: &[f32] = if aligned { &f } else { &all };

        // region in the middle, looped twice, with an offset: exercises
        // seeking to a start point and seeking back at the loop point
        let mut o = opts();
        o.start_frame = Some(30_001);
        o.end_frame = Some(50_001);
        o.loop_count = 2;
        o.offset_seconds = 5000.0 / 48_000.0;
        let mut src = FileSource::open(&p, o).unwrap();
        assert_eq!(src.total_frames(), Some(35_000));
        let got = read_all(&mut src);
        close(&got[..15_000], &f[35_001..50_001], "first pass");
        close(&got[15_000..], &f[30_001..50_001], "second pass");

        let peaks = waveform_peaks(&p, 50).unwrap();
        assert!(
            peaks.iter().all(|(lo, hi)| *lo < -0.2 && *hi > 0.2),
            "{ext} waveform"
        );
    }
}

#[test]
fn aac_encoder_delay_and_padding_are_trimmed() {
    // A 0.5 s file whose tone starts exactly at frame 4800 (encoded by
    // ffmpeg's AAC encoder: 1024 frames of priming, length in the edit list).
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/onset_4800.m4a");
    let info = probe(&p).unwrap();
    assert_eq!(info.codec, "aac");
    assert_eq!(info.frames, Some(24_000));
    let mut src = FileSource::open(&p, opts()).unwrap();
    let pcm = read_all(&mut src);
    let left: Vec<f32> = pcm.chunks(2).map(|f| f[0]).collect();
    assert_eq!(left.len(), 24_000);
    let onset = left.iter().position(|s| s.abs() > 0.1).unwrap();
    assert!(
        (4790..=4830).contains(&onset),
        "tone starts at frame {onset} (untrimmed it would be ~5824)"
    );
    // Starting in the middle is frame-exact, too.
    let mut o = opts();
    o.start_seconds = 0.09; // 4320 frames: the tone begins 480 frames in
    let mut src = FileSource::open(&p, o).unwrap();
    let pcm = read_all(&mut src);
    let onset = pcm.chunks(2).position(|f| f[0].abs() > 0.1).unwrap();
    assert!((470..=510).contains(&onset), "onset after seek at {onset}");
}
