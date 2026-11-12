use super::*;

const BLOCKS: [usize; 9] = [64, 441, 1, 1024, 7, 333, 2048, 100, 3];

fn running(rate: FrameRate, anchor_sample: u64, anchor_frame: u64) -> TcTransport {
    TcTransport {
        rate,
        running: true,
        anchor_sample,
        anchor_frame,
    }
}

fn stopped(rate: FrameRate, anchor_sample: u64) -> TcTransport {
    TcTransport {
        rate,
        running: false,
        anchor_sample,
        anchor_frame: 0,
    }
}

/// Render samples `0..total` in irregular blocks into one vector.
fn render_all(enc: &mut LtcEncoder, total: usize, blocks: &[usize], gain: f32) -> Vec<f32> {
    let mut out = vec![9.9f32; total];
    let mut pos = 0usize;
    let mut b = 0usize;
    while pos < total {
        let n = blocks[b % blocks.len()].min(total - pos);
        b += 1;
        enc.render(pos as u64, &mut out[pos..pos + n], gain);
        pos += n;
    }
    out
}

fn decode_all(sample_rate: u32, signal: &[f32]) -> Vec<DecodedLtcFrame> {
    let mut dec = LtcDecoder::new(sample_rate);
    let mut frames = Vec::new();
    for chunk in signal.chunks(977) {
        dec.push(chunk, &mut frames);
    }
    frames
}

/// Number of frames of `t` whose terminating edge lies before `total`.
fn complete_frames(t: &TcTransport, sample_rate: u32, total: u64) -> u64 {
    let mut k = t.frames_elapsed(total, sample_rate);
    while k > 0 && t.sample_after_frames(k, sample_rate).unwrap() >= total {
        k -= 1;
    }
    k
}

/// Assert `frames` are exactly frames `0..` of transport `t`.
fn assert_sequence(frames: &[DecodedLtcFrame], t: &TcTransport, sample_rate: u32) {
    for (i, f) in frames.iter().enumerate() {
        let want = Timecode::from_frame_number(t.anchor_frame + i as u64, t.rate);
        assert_eq!(f.timecode, want, "{:?} @{sample_rate} frame {i}", t.rate);
        assert_eq!(f.drop_frame, t.rate.drop_frame());
        assert_eq!(
            f.end_sample,
            t.sample_after_frames(i as u64 + 1, sample_rate).unwrap(),
            "{:?} @{sample_rate} frame {i} position",
            t.rate
        );
    }
}

#[test]
fn bit_layout_matches_hand_computed_frames() {
    // 01:23:45:12 @ 30: frames 2/1 -> bits 1, 8; seconds 5/4 -> bits 16, 18,
    // 26; minutes 3/2 -> bits 32, 33, 41; hours 1/0 -> bit 48. 9 data ones
    // + 13 sync ones = 22 (even) -> no polarity correction.
    assert_eq!(
        encode_frame_bits(Timecode::new(1, 23, 45, 12), FrameRate::F30),
        [0x02, 0x01, 0x05, 0x04, 0x03, 0x02, 0x01, 0x00, 0xFC, 0xBF]
    );
    // All-zero time: 13 ones -> correction bit 27 (30/24 fps) or 59 (25 fps).
    let zero = Timecode::new(0, 0, 0, 0);
    for rate in [
        FrameRate::F30,
        FrameRate::F2997Ndf,
        FrameRate::F24,
        FrameRate::F23976,
    ] {
        assert_eq!(
            encode_frame_bits(zero, rate),
            [0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0xFC, 0xBF]
        );
    }
    assert_eq!(
        encode_frame_bits(zero, FrameRate::F25),
        [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0xFC, 0xBF]
    );
    // 00:01:00;02 drop frame: bit 1, drop flag bit 10, bit 32 -> 16 ones.
    assert_eq!(
        encode_frame_bits(Timecode::new(0, 1, 0, 2), FrameRate::F2997Df),
        [0x02, 0x04, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xFC, 0xBF]
    );
    // 23:59:59:24 @ 25: 4/2 -> bits 2, 9; 9/5 -> 16, 19, 24, 26;
    // 9/5 -> 32, 35, 40, 42; 3/2 -> 48, 49, 57. 13 + 13 = 26 -> none.
    assert_eq!(
        encode_frame_bits(Timecode::new(23, 59, 59, 24), FrameRate::F25),
        [0x04, 0x02, 0x09, 0x05, 0x09, 0x05, 0x03, 0x02, 0xFC, 0xBF]
    );
    // Sync word in transmission order: 0011 1111 1111 1101.
    let bits = encode_frame_bits(zero, FrameRate::F25);
    let sync: String = (64..80)
        .map(|i| {
            if (bits[i / 8] >> (i % 8)) & 1 == 1 {
                '1'
            } else {
                '0'
            }
        })
        .collect();
    assert_eq!(sync, "0011111111111101");
}

#[test]
fn every_frame_has_even_parity() {
    for rate in FrameRate::ALL {
        let mut n = 0;
        while n < rate.frames_per_day() {
            let bits = encode_frame_bits(Timecode::from_frame_number(n, rate), rate);
            let ones: u32 = bits.iter().map(|b| b.count_ones()).sum();
            assert_eq!(ones % 2, 0);
            n += 101;
        }
    }
}

#[test]
fn roundtrip_all_rates_and_sample_rates() {
    for sample_rate in [44_100u32, 48_000, 96_000] {
        for rate in FrameRate::ALL {
            let starts = [
                // 8 frames before 00:00:59:28: crosses the drop-frame minute.
                Timecode::new(0, 0, 59, 28).to_frame_number(rate) - 8,
                // Crosses the 24 h wrap.
                rate.frames_per_day() - 12,
            ];
            for start in starts {
                let t = running(rate, 777, start);
                let total = 777 + 2 * sample_rate as usize + 13;
                let mut enc = LtcEncoder::new(sample_rate);
                enc.set_transport(t);
                let signal = render_all(&mut enc, total, &BLOCKS, 0.7);
                assert!(signal[..777].iter().all(|&x| x == 0.0));
                assert!(signal[777..].iter().all(|&x| x == 0.7 || x == -0.7));
                let frames = decode_all(sample_rate, &signal);
                assert_eq!(
                    frames.len() as u64,
                    complete_frames(&t, sample_rate, total as u64),
                    "{rate:?} @{sample_rate}"
                );
                assert!(frames.len() >= 46);
                assert_sequence(&frames, &t, sample_rate);
            }
        }
    }
}

#[test]
fn drop_frame_minute_boundary_and_day_wrap_labels() {
    let rate = FrameRate::F2997Df;
    let start = Timecode::new(0, 0, 59, 28).to_frame_number(rate);
    let mut enc = LtcEncoder::new(48_000);
    enc.set_transport(running(rate, 0, start));
    let frames = decode_all(48_000, &render_all(&mut enc, 12_000, &BLOCKS, 1.0));
    let labels: Vec<String> = frames
        .iter()
        .take(4)
        .map(|f| f.timecode.to_string())
        .collect();
    assert_eq!(
        labels,
        ["00:00:59:28", "00:00:59:29", "00:01:00:02", "00:01:00:03"]
    );
    assert!(frames.iter().all(|f| f.drop_frame));

    let rate = FrameRate::F25;
    let mut enc = LtcEncoder::new(48_000);
    enc.set_transport(running(rate, 0, rate.frames_per_day() - 2));
    let frames = decode_all(48_000, &render_all(&mut enc, 12_000, &BLOCKS, 1.0));
    let labels: Vec<String> = frames
        .iter()
        .take(4)
        .map(|f| f.timecode.to_string())
        .collect();
    assert_eq!(
        labels,
        ["23:59:59:23", "23:59:59:24", "00:00:00:00", "00:00:00:01"]
    );
    assert!(frames.iter().all(|f| !f.drop_frame));
}

#[test]
fn bit_cells_follow_exact_formula() {
    // Check the raw signal against an independent biphase model.
    let sample_rate = 44_100u32;
    let rate = FrameRate::F2997Df;
    let (num, den) = rate.rational();
    let t = running(rate, 100, 1795);
    let mut enc = LtcEncoder::new(sample_rate);
    enc.set_transport(t);
    let signal = render_all(&mut enc, 20_000, &BLOCKS, 1.0);
    let mut level = -1.0f32;
    let mut prev_cell = None;
    for (s, &x) in signal.iter().enumerate().skip(100) {
        let d = (s - 100) as u64;
        let cell = d * num * 160 / (u64::from(sample_rate) * den);
        if prev_cell != Some(cell) {
            assert!(prev_cell.is_none() || prev_cell == Some(cell - 1));
            prev_cell = Some(cell);
            let frame = cell / 160;
            let bit = ((cell % 160) / 2) as usize;
            let bits = encode_frame_bits(Timecode::from_frame_number(1795 + frame, rate), rate);
            let one = (bits[bit / 8] >> (bit % 8)) & 1 == 1;
            if cell % 2 == 0 || one {
                level = -level;
            }
        }
        assert_eq!(x, level, "sample {s}");
    }
}

#[test]
fn block_size_does_not_change_output() {
    let setup = |enc: &mut LtcEncoder| {
        enc.set_transport(running(FrameRate::F2997Df, 1_000, 1790));
        enc.set_transport(running(FrameRate::F24, 9_001, 5));
        enc.set_transport(stopped(FrameRate::F24, 15_000));
        enc.set_transport(running(FrameRate::F30, 15_500, 123_456));
        enc.set_transport(running(FrameRate::F25, 15_501, 99));
    };
    let total = 40_000;
    let mut a = LtcEncoder::new(48_000);
    setup(&mut a);
    let whole = render_all(&mut a, total, &[total], 0.5);
    for blocks in [&BLOCKS[..], &[1], &[17], &[4096]] {
        let mut b = LtcEncoder::new(48_000);
        setup(&mut b);
        assert!(
            whole == render_all(&mut b, total, blocks, 0.5),
            "blocks {blocks:?}"
        );
    }
    assert_eq!(a.transport(), running(FrameRate::F25, 15_501, 99));
}

#[test]
fn start_at_future_anchor_mid_block() {
    let t = running(FrameRate::F25, 1_000, 0);
    let mut enc = LtcEncoder::new(48_000);
    enc.set_transport(t);
    let signal = render_all(&mut enc, 8_192, &[4096], 0.5);
    assert!(signal[..1_000].iter().all(|&x| x == 0.0));
    // 00:00:00:00 starts with zero bits: one full 24-sample cell per bit.
    assert!(signal[1_000..1_024].iter().all(|&x| x == 0.5));
    assert!(signal[1_024..1_048].iter().all(|&x| x == -0.5));
    let frames = decode_all(48_000, &signal);
    assert_eq!(frames.len(), 3);
    assert_sequence(&frames, &t, 48_000);
    assert_eq!(frames[0].end_sample, 1_000 + 1_920);
}

#[test]
fn stop_silences_exactly_at_anchor() {
    // Mid-frame stop and stop exactly on a frame boundary.
    for (stop_at, want_frames) in [(10_000usize, 5usize), (9_600, 5)] {
        let t = running(FrameRate::F25, 0, 250);
        let mut enc = LtcEncoder::new(48_000);
        enc.set_transport(t);
        enc.set_transport(stopped(FrameRate::F25, stop_at as u64));
        let signal = render_all(&mut enc, 20_000, &BLOCKS, 0.5);
        assert!(signal[..stop_at].iter().all(|&x| x.abs() == 0.5));
        assert!(signal[stop_at..].iter().all(|&x| x == 0.0));
        let frames = decode_all(48_000, &signal);
        assert_eq!(frames.len(), want_frames, "stop at {stop_at}");
        assert_sequence(&frames, &t, 48_000);
    }
}

#[test]
fn relocation_while_running_is_sample_accurate() {
    let sample_rate = 44_100u32;
    let rate = FrameRate::F2997Df;
    let first = running(rate, 0, 0);
    let hour = Timecode::new(1, 0, 0, 0).to_frame_number(rate);
    // Try many relocation points so both line levels and all bit phases
    // occur at the anchor.
    for reloc in (30_000u64..30_060).chain([29_430, 29_431]) {
        let second = running(rate, reloc, hour);
        let total = 80_000usize;
        let mut enc = LtcEncoder::new(sample_rate);
        enc.set_transport(first);
        enc.set_transport(second);
        let signal = render_all(&mut enc, total, &BLOCKS, 0.5);
        let r = reloc as usize;
        // There is always an edge exactly at the new anchor.
        assert_eq!(signal[r], -signal[r - 1], "reloc {reloc}");
        // From the anchor on the signal is that of a fresh generator
        // (up to polarity, which biphase mark does not care about).
        let mut fresh = LtcEncoder::new(sample_rate);
        fresh.set_transport(second);
        let mut reference = vec![0.0f32; total - r];
        fresh.render(reloc, &mut reference, 0.5);
        let sign = signal[r] / reference[0];
        assert!(signal[r..]
            .iter()
            .zip(&reference)
            .all(|(a, b)| *a == sign * *b));

        let frames = decode_all(sample_rate, &signal);
        let (before, after): (Vec<_>, Vec<_>) = frames.iter().partition(|f| f.end_sample <= reloc);
        assert_eq!(
            before.len() as u64,
            first.frames_elapsed(reloc, sample_rate)
        );
        assert_sequence(&before, &first, sample_rate);
        // The new sequence is decoded from its very first frame.
        assert_eq!(
            after.len() as u64,
            complete_frames(&second, sample_rate, total as u64)
        );
        assert_sequence(&after, &second, sample_rate);
        assert_eq!(after[0].timecode, Timecode::new(1, 0, 0, 0));
    }
}

#[test]
fn transport_with_past_anchor_takes_effect_immediately() {
    let t = running(FrameRate::F24, 2_000, 48);
    let mut reference = LtcEncoder::new(48_000);
    reference.set_transport(t);
    let want = render_all(&mut reference, 30_000, &[512], 1.0);

    let mut enc = LtcEncoder::new(48_000);
    let mut got = vec![0.0f32; 30_000];
    enc.render(0, &mut got[..5_000], 1.0);
    assert!(got[..5_000].iter().all(|&x| x == 0.0));
    enc.set_transport(t);
    enc.render(5_000, &mut got[5_000..], 1.0);
    assert!(got[5_000..] == want[5_000..]);
}

#[test]
fn non_contiguous_render_is_stateless() {
    let t = running(FrameRate::F2997Ndf, 123, 1000);
    let mut reference = LtcEncoder::new(96_000);
    reference.set_transport(t);
    let want = render_all(&mut reference, 100_000, &[100_000], 1.0);
    let mut enc = LtcEncoder::new(96_000);
    enc.set_transport(t);
    let mut buf = [0.0f32; 1_000];
    for start in [0usize, 50_000, 7, 99_000, 3_333] {
        enc.render(start as u64, &mut buf, 1.0);
        assert!(buf[..] == want[start..start + 1_000], "start {start}");
    }
    // Very large sample positions do not overflow.
    let far = running(FrameRate::F23976, u64::MAX - 500_000, 0);
    enc.set_transport(far);
    enc.render(u64::MAX - 1_000, &mut buf, 1.0);
    assert!(buf.iter().all(|&x| x.abs() == 1.0));
}

#[test]
fn degenerate_inputs_do_not_panic() {
    let mut buf = [1.0f32; 256];
    let mut enc = LtcEncoder::new(0);
    enc.set_transport(running(FrameRate::F25, 0, 0));
    enc.render(0, &mut buf, 1.0);
    assert!(buf.iter().all(|&x| x == 0.0));
    // Sample rate far too low to carry LTC: still well defined.
    let mut enc = LtcEncoder::new(1_000);
    enc.set_transport(running(FrameRate::F30, 10, u64::MAX));
    enc.render(0, &mut buf, 1.0);
    enc.render(0, &mut [], 1.0);
    assert!(buf[10..].iter().all(|&x| x.abs() == 1.0));
    let mut dec = LtcDecoder::new(0);
    let mut frames = Vec::new();
    dec.push(&buf, &mut frames);
    dec.push(&[f32::NAN, f32::INFINITY, -f32::INFINITY, 0.0], &mut frames);
    assert_eq!(dec.samples_pushed(), 260);
}

#[test]
fn decoder_tolerates_gain_changes_inversion_and_noise() {
    let sample_rate = 48_000u32;
    for rate in [FrameRate::F25, FrameRate::F2997Df, FrameRate::F23976] {
        let t = running(rate, 0, 1_000);
        let total = 4 * sample_rate as usize;
        let mut enc = LtcEncoder::new(sample_rate);
        enc.set_transport(t);
        // Gain changes from block to block, and the polarity is inverted.
        let mut signal = vec![0.0f32; total];
        let gains = [-0.8f32, -0.5, -0.9, -0.35, -0.6, -1.0, -0.45];
        for (b, chunk) in signal.chunks_mut(4_800).enumerate() {
            enc.render((b * 4_800) as u64, chunk, gains[b % gains.len()]);
        }
        // Deterministic white noise at about -20 dB relative to the signal.
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        for x in &mut signal {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let r = ((seed >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0;
            *x += 0.05 * r;
        }
        let frames = decode_all(sample_rate, &signal);
        assert_eq!(
            frames.len() as u64,
            complete_frames(&t, sample_rate, total as u64)
        );
        assert_sequence(&frames, &t, sample_rate);
    }
}

#[test]
fn decoder_reacquires_after_garbage_and_large_level_drop() {
    let sample_rate = 48_000u32;
    let t = running(FrameRate::F30, 0, 0);
    let mut enc = LtcEncoder::new(sample_rate);
    enc.set_transport(t);
    let mut signal = render_all(&mut enc, 96_000, &BLOCKS, 1.0);
    for x in &mut signal[48_000..] {
        *x *= 0.05; // -26 dB step
    }
    for x in &mut signal[20_000..20_400] {
        *x = 0.3; // dropout replaced by DC
    }
    let frames = decode_all(sample_rate, &signal);
    // Every decoded frame is correct and correctly placed ...
    for f in &frames {
        let k = f.timecode.to_frame_number(t.rate);
        assert_eq!(
            f.end_sample,
            t.sample_after_frames(k + 1, sample_rate).unwrap()
        );
    }
    // ... and only a few around the disturbances are missing.
    assert!(frames.len() >= 60 - 8, "decoded {}", frames.len());
    assert_eq!(frames.last().map(|f| f.end_sample), Some(96_000 - 1_600));
}

/// Stream `seconds` of LTC through encoder and decoder block by block (the
/// signal is never held in memory) and check that every frame is decoded,
/// consecutively labelled and positioned exactly on the ideal sample grid.
fn assert_no_drift(rate: FrameRate, seconds: u64) {
    let sample_rate = 48_000u32;
    let (num, den) = rate.rational();
    let anchor = 4_321u64;
    // Start late in the day so the long runs also cross midnight.
    let t = running(
        rate,
        anchor,
        Timecode::new(23, 57, 30, 0).to_frame_number(rate),
    );
    let total = anchor + seconds * u64::from(sample_rate);
    let mut enc = LtcEncoder::new(sample_rate);
    enc.set_transport(t);
    let mut dec = LtcDecoder::new(sample_rate);
    let blocks = [4096usize, 1000, 1, 4096, 333, 2048];
    let mut buf = vec![0.0f32; 4096];
    let mut frames = Vec::with_capacity(16);
    let mut pos = 0u64;
    let mut b = 0usize;
    let mut k = 0u64;
    let mut last_end = 0u64;
    while pos < total {
        let n = (blocks[b % blocks.len()] as u64).min(total - pos) as usize;
        b += 1;
        enc.render(pos, &mut buf[..n], 0.5);
        dec.push(&buf[..n], &mut frames);
        pos += n as u64;
        for f in frames.drain(..) {
            assert_eq!(
                f.timecode,
                Timecode::from_frame_number(t.anchor_frame + k, rate),
                "frame {k}"
            );
            assert_eq!(f.drop_frame, rate.drop_frame());
            // Ideal end of frame k: (k + 1) * sample_rate * den / num
            // samples after the anchor, rounded up to the next sample.
            let ideal = (u128::from(k + 1) * u128::from(sample_rate) * u128::from(den))
                .div_ceil(u128::from(num)) as u64;
            assert_eq!(f.end_sample, anchor + ideal, "frame {k}");
            last_end = f.end_sample;
            k += 1;
        }
    }
    assert_eq!(k, complete_frames(&t, sample_rate, total));
    // Total deviation from the real-valued ideal position is below 1 sample.
    let ideal = k as f64 * f64::from(sample_rate) * den as f64 / num as f64;
    let deviation = (last_end - anchor) as f64 - ideal;
    assert!((0.0..1.0).contains(&deviation), "deviation {deviation}");
    assert!(k >= seconds * num / den - 1);
}

#[test]
fn no_drift_5_minutes_2997df() {
    assert_no_drift(FrameRate::F2997Df, 300);
}

#[test]
fn no_drift_5_minutes_25() {
    assert_no_drift(FrameRate::F25, 300);
}

/// Full-hour drift tests (172.8 M samples each, streamed). They take about
/// 4 s each in the unoptimised test profile, so they are not `#[ignore]`d;
/// run them alone with `cargo test -p sm-timecode no_drift_1_hour`.
#[test]
fn no_drift_1_hour_2997df() {
    assert_no_drift(FrameRate::F2997Df, 3600);
}

#[test]
fn no_drift_1_hour_25() {
    assert_no_drift(FrameRate::F25, 3600);
}

#[test]
fn backwards_ltc_is_decoded_as_reverse() {
    let rate = FrameRate::F25;
    let t = running(rate, 0, Timecode::new(1, 0, 0, 0).to_frame_number(rate));
    let mut enc = LtcEncoder::new(48_000);
    enc.set_transport(t);
    let mut signal = render_all(&mut enc, 48_000, &BLOCKS, 0.7);
    // Forward decoding knows the direction too.
    let fwd = decode_all(48_000, &signal);
    assert!(fwd.len() > 20 && fwd.iter().all(|f| !f.reverse));
    // Played backwards: frames arrive in descending order, flagged reverse.
    signal.reverse();
    let back = decode_all(48_000, &signal);
    assert!(back.len() > 20, "decoded {} reverse frames", back.len());
    assert!(back.iter().all(|f| f.reverse));
    for w in back.windows(2) {
        assert_eq!(
            w[1].timecode.to_frame_number(rate) + 1,
            w[0].timecode.to_frame_number(rate),
            "{} then {}",
            w[0].timecode,
            w[1].timecode
        );
    }
}
